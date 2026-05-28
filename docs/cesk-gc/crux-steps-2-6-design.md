# CRUX Steps 2–6 — Index-mode decode path (implementation design)

> Part of the GC clean-room migration (plan: `would-it-be-possible-resilient-gosling.md`,
> pgmcp work-item `inc-2-…-c6e73ab2`). Increment 2, sub-steps after CRUX Step 1
> (mode-aware `MettaValue::view()`, already landed). Source-verified design from a
> Plan agent, 2026-05-28. Each step: Slab arm byte-identical (Index behavior behind
> `if gc_mode_is_index()`), its own `set_gc_mode_index()` test (nextest is
> process-per-test → global flip isolated), green wall stays green.

## Hazard analysis (confirmed against source)
- `from_inner`/`from_inner_tagged`: all production callers are in `GcFactory`
  (gc_allocator.rs, Slab-only) or JIT (Inc 2b) or `gc_cron.rs:480` (Slab cron, Inc 6).
  Lone shared round-trip = `strip_spans` (metta_value.rs:1139) — and it has **0 callers**
  (dead code), so correctness-only, not green-wall.
- `inner_ptr()`-as-key shared sites needing a stable Index key: conformance_common.rs:151,
  eval/types.rs:1001, alpha_equiv.rs:47-48, eval_loop.rs:4124/4172-4174. (Slab-only: all
  gc_allocator/gc_thread; cfg(test): state.rs:380/386; JIT/2b: jit/**, eval_loop.rs:3899
  T1 tier key; Slab-nursery/Inc8: eval_loop.rs:3441.)
- `inner_raw()` has LIVE production `&MettaValueInner` consumers (eval/step, eval/types,
  wide_mork/encoding, varint_encoding) → needs Step 2c.

## Decisions
- **A** — reroute `is_X`/`as_X` accessors through `view()` (NO materialization, NO new
  unsafe; `view()` already `'static`-launders). Materialize a `MettaValueInner` only for
  residual raw walkers via `inner_ref/inner/inner_raw` (Step 2c), into a thread-local
  `RefCell<Vec<Box<MettaValueInner>>>` cleared at the trampoline safepoint
  (eval_loop.rs ~3378) + top-level call return. Materialization is **one node deep**
  (children stay handles); composite payloads reuse the stable side-arena Box launder.
- **B** — `inner_ptr()` Index arm returns `(INDEX_KEY_TAG | (tagged>>4))` with
  `INDEX_KEY_TAG = 1<<48` (avoids Addr(0)=null; never deref'd, key-only).
- **C** — `IndexFactory` must content-hash-cons `atom`/`sexpr_from_slice`/
  `conjunction_from_slice` (Step 4) so equal content ⇒ equal Addr ⇒ equal `inner_ptr`
  key, matching Slab's fixpoint/cycle/dedup identity sites (R9). Keyed by mode-aware
  `hash_value()` + structural `==`; `clear_hash_cons()` hook called from `sweep` (real
  clear wired Inc 6).
- **D** — `PartialEq` keep tagged/inline/PTR_MASK fast paths; gate only the
  `inner()==inner()` fallback to a `view()`-structural compare. `Hash`/`Display` decode
  via `view()` + key via the Step-3a `inner_ptr`.
- **E** — `span/spans/peel_span/strip_spans` follow `Node::Spanned` chain by Addr;
  `strip_spans` returns the bare **handle** (`from_addr`), never `from_inner`.
- **F** — mork_convert.rs: guard slot-epoch validity + gc_trace dangling check with
  `&& !gc_mode_is_index()`; walk body unchanged (its `inner_ref()` source is mode-aware
  via 2c). Full Addr-bitmap re-key = Inc 6 (safe now: no Index sweep runs yet).

## Ordered steps (2a→2b→2c→3a→3b→3c→3d→4→5→6)
- **2a** reroute `is_X`/`as_X`/`type_name`/`unwrap_lazy`/`as_quoted_ref`/`as_lazy_ref`
  (metta_value.rs:1185-1740) through `view()` behind the gate. Lazy-transparency for
  `as_long`/`as_float`. New `IndexHeap::quoted_inner_ref(addr)` for the `_ref` accessors.
  Test `accessors_are_mode_aware`.
- **2b** `span`/`spans`/`peel_span` (1098/1115/1147) via new `IndexHeap::span_ref_at`.
  Test `spans_are_mode_aware`.
- **2c** `inner_ref`/`inner`/`inner_raw` (930/1007/1089) materialize via new
  `IndexHeap::materialize_inner(addr)` + thread-local shadow + `clear_inner_shadow()` at
  safepoint. Test `inner_ref_materializes_in_index_mode` (+ `inner_shadow_len()` test-only).
- **3a** `inner_ptr` (1172) INDEX_KEY_TAG. Test `inner_ptr_is_stable_distinct_key_in_index`.
- **3b** `hash_value_cached_inner` (149) decode via `view()` (preserve Lazy-transparency),
  cache key via 3a. Test `hash_is_content_stable_and_mode_consistent`.
- **3c** `PartialEq` (2448) gate `:2474` fallback to `view_eq`. Test `partial_eq_is_structural_in_index`.
- **3d** `format_value_iterative` (2042) + `friendly_repr` (3045) decode via `view()`,
  key via 3a. Test `display_is_mode_aware`.
- **4** `IndexFactory` hash-cons (index_heap.rs:369/401/431) + `clear_hash_cons()` from
  `sweep`. Fixed lock order: hash-cons table BEFORE heap read. Test `index_factory_hash_conses_equal_content`.
- **5** `strip_spans` (1135) Index arm via `IndexHeap::strip_spanned_addr`. Test `strip_spans_returns_bare_handle_in_index`.
- **6** mork_convert.rs (548-635/838) three `&& !gc_mode_is_index()` guards. Test `mork_convert_roundtrip_in_index`.

## Out of scope (carried forward, nothing dropped)
- **Inc 2b (JIT)**: jit/runtime/helpers.rs:62/75/83/125, jit/types/value.rs, jit/hybrid/arena.rs,
  jit/types/context.rs value_mode→index; eval_loop.rs:3899 T1 tier key gates a bytecode
  decode that's 2b (disable bytecode tier under --gc=index until 2b, or confirm mode-aware).
  jit_comparison + tier tests gate it.
- **Inc 6 (collector)**: invoke IndexHeap::mark/sweep live; retire clear_inner_shadow when
  raw walkers migrate to view(); wire hash-cons clear to real sweep epoch; mork_convert
  ground-cache full Addr-bitmap re-key; watermark/backpressure rehome; delete slab epoch/ABA.
  Nursery (eval_loop.rs:3441) is Slab/Inc8 — `should_collect()` must be false in Index mode.

## Validation after Step 6 (Slab green wall — Index validated by per-step tests; cross-mode A/B is Inc 3)
- `cargo nextest run` (under systemd-run cgroup) == recorded N (~4308).
- `mtt-conformance --strict` → 483 / 221 / 40, PLN-main 7/7.
- HE-bisim 40/40.
