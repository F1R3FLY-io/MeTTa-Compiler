# A4.3 — Machine-equivalence oracle (implementation design)

CESK GC migration, Phase A4.3. A **debug-only** oracle asserting the STRUCTURAL reader
(`collect_machine_roots` + the deferred-drop transient) is a SUPERSET (sorted-deduped
`inner_ptr()` multiset) of the DISCOVERED root set the GC safepoint feeds the collector —
BEFORE A5 irreversibly deletes the discovery apparatus. Closes the completeness gaps so the
assertion actually holds. Permanent CI invariant. Designed by Plan agent 2026-05-29 + one
correction (oracle gated on `gc_mode_is_index()`) found by the parent.

## Source-verified corrections (the Plan agent caught two of my wrong assumptions)
1. **`collect_eval_memo_roots` is NOT a no-op** — it walks `EVAL_MEMO` (dispatch_hints.rs).
   Binding-capture frames are still excluded, but not through a live no-op collector: the
   empty shim was removed after source audit. Those frames store metadata only
   (`tracked_vars` and fork depth); value-bearing bindings travel with `BoundValue` and are
   walked by the work item / continuation root readers.
2. **`SAFEPOINT_ROOTS` is NOT empty at quiescence** — `refresh_thread_local_cache_roots`
   (eval/mod.rs:133) snapshots the 4 caches into `CACHE_ROOT_HANDLE` before EvalGuard drops.

## CRITICAL correction by parent: gate the oracle on `gc_mode_is_index()`
In SLAB mode the k_spine thread-locals are EMPTY (all k_spine push sites are
`gc_mode_is_index()`-gated, A4.2b) while `collect_frame_chain_roots` (OLD, safepoint:3527)
is populated (maybe_push_frame / with_vm_roots_frame push the EvalFrameGuard
unconditionally). So an ungated oracle would see `NEW(k_spine=∅) ⊉ OLD(frame_chain≠∅)` and
PANIC in slab. The structural reader is the INDEX-gc root source; the oracle only makes
sense in index mode. ⇒ oracle body = `#[cfg(debug_assertions)]` + `if gc_mode_is_index()`.
(Confirmed: under `--features index-gc` the conformance fires the collector ⇒
`gc_mode_is_index()` is true during eval, so the oracle is exercised there.)

## Gap classification (discovered source → CESK role → resolution)
| Source (safepoint) | Backing state | Role | Resolution |
|---|---|---|---|
| `collect_eval_memo_roots` (dispatch_hints) | EVAL_MEMO thread_local | global cache | ADD to collect_global_anchors |
| `collect_match_result_roots` (dispatch_hints) | MATCH_RESULT_CACHE thread_local | global cache | ADD to collect_global_anchors |
| `collect_subgoal_roots` (tabling.rs) | THREAD_TABLE thread_local | global cache | ADD to collect_global_anchors |
| `collect_thunk_roots` (thunk.rs) | THREAD_THUNKS thread_local | global cache | ADD to collect_global_anchors |
| binding-capture frames | metadata only (`tracked_vars`, fork depth) | ∅ | EXCLUDE (values live in `BoundValue` / WorkItem / Continuation readers) |
| `deferred_shared_drops` (eval_loop.rs:3417) | transient `Vec<Arc<EnvShared>>` local | transient register | APPEND at oracle / A4.4 flip site (NOT a reader param) |
| `MettaStateGcRoots` (metta_state.rs) | per-instance result vec | covered (index regime) | result appended explicitly by quiescence collectors; A5.3b re-homes |
| `CurrentIterRootProvider` | per-thread current-iter mirror | covered | = the current `work` item in C (collect_all) |
| `SAFEPOINT_ROOTS` | cache snapshot via CACHE_ROOT_HANDLE | covered transitively | = the 4 caches now in collect_global_anchors |

Single-threaded-regime justification (why thread-local caches read by-name is CESK-faithful):
the collector runs only under `!worker_ever_spawned() && active∈{0,1}` ⇒ the calling thread
is the SOLE owner of σ ⇒ its thread-locals are machine-global σ-value holders, exactly the
`collect_global_anchors` contract for the 5 OnceLock caches. No new unsafe, no cross-thread.

## Implementation
### 1. Extend `collect_global_anchors` (roots.rs) — 4 by-name cache reads
After the 5 existing anchors, append:
```rust
crate::backend::eval::trampoline::dispatch_hints::collect_eval_memo_roots(out);
crate::backend::eval::trampoline::dispatch_hints::collect_match_result_roots(out);
crate::backend::eval::cesk::tabling::collect_subgoal_roots(out);
crate::backend::eval::cesk::thunk::collect_thunk_roots(out);
```
Purely additive/append-only. `collect_machine_roots` gains them automatically (signature
unchanged). Release behavior byte-identical: collect_machine_roots is unused in the hot
path (only oracle + tests call it) until A4.4.

### 2. The oracle (eval_loop.rs, after the cache+deferred block closes ~line 3547, before clear_aba_sensitive_caches:3553)
`#[cfg(debug_assertions)] { if gc_mode_is_index() { ... } }`:
- OLD = `root_set.roots()` (collect_all ∪ frame_chain ∪ 4 caches ∪ binding_capture[∅] ∪
  deferred-env roots) ∪ `collect_all_roots()` (registry ∪ SAFEPOINT_ROOTS) — reuses the
  already-assembled set; mirrors the midloop set (3580) exactly.
- NEW = `collect_machine_roots(&mut new, &machine_operand_stack, &work, &work_stack,
  &continuations, env.shared.as_ref())` ∪ deferred-drop append (each
  `deferred_env.as_ref().collect_roots(&mut new)`) — exactly the A4.4 flip shape.
- Assert NEW ⊇ OLD (sorted-deduped inner_ptr multiset; safety direction = no protected root
  dropped). On failure panic with `|OLD| |NEW| |OLD∖NEW|` + ≤16-ptr sample + a 3-class
  checklist (missing cache / missing transient / missing K-spine push).
- Zero-cost in release (cfg'd out). Debug cost: 2 Vecs + 2 sorts + a collect_all_roots walk
  per safepoint (every 4096 iters) — acceptable.
Placement is sound: all 6 inputs in scope; root_set intact (read before drain@3639); the 4
caches unchanged between OLD-read (3533-3536) and oracle (clear_aba is AFTER, and doesn't
touch the 4 eval caches anyway).

A4.3 scopes ONLY the main eval_loop safepoint (C∪K live). The quiescence collectors
(eval/mod.rs:267, tier_forced.rs:285 — C∪K empty, result-vec transient) get their oracle in
A4.4 when those sites flip.

### 3. CI-invariant test
A `#[cfg(all(debug_assertions, feature = "index-gc"))]` must-not-panic test: drive a
recursive + nondeterministic-fork + cache-populating eval (`(cnt 20000)` > 4096 reductions →
≥1 safepoint; `(amb …)` → deferred env drops; recursive rule → EVAL_MEMO/MATCH/subgoal/
thunk) so the oracle fires with every gap source populated. Reaching the end without panic =
NEW ⊇ OLD held at every safepoint. (Ensure `gc_mode_is_index()` is true — verify it is under
`--features index-gc` during eval, else call the set fn at test start.)

## Green-wall
- **RELEASE** (oracle cfg'd out): slab nextest 4325, index nextest 4177, index conformance
  483/221/40 byte-identical, 0 new warnings — confirms byte-identical + zero release cost.
- **DEBUG index** (oracle fires): build debug `--features index-gc`, run a debug conformance
  SUBSET (mmverify + a few M11 — debug is slow, NOT full 483) + the CI test → oracle holds.
  Capped under systemd-run (MemoryMax, MemorySwapMax=0); debug RSS ≈ 2-3× release.

## A4.4 forward-link
The midloop flip (eval_loop.rs:3579) lifts the oracle's NEW computation verbatim:
`collect_machine_roots(...) ∪ deferred-drop append`. Quiescence sites flip to
`collect_machine_roots(…) ∪ result`. The oracle stays (compares against the still-computed
OLD) until A5 deletes the apparatus piece-by-piece.

## Risk / A5 blocker
Only `MettaStateGcRoots` source/output is non-structural in the SLAB path; covered in the
index regime A4.4 flips (result appended explicitly), so it does NOT block A4.3/A4.4, but
A5.3b must re-home it as an explicit anchor when ROOT_REGISTRY is deleted. No other
discovered source resists structural reading.
