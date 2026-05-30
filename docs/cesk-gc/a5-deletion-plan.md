# CESK GC Migration — Phase A5: Delete the Discovery Apparatus

**Branch**: `feature/petta-semantics` (HEAD `390b743`, A4.4 collector-flip committed)
**Date**: 2026-05-29
**Status**: DESIGN (read-only; no code edits in this document's authoring pass)
**Scope**: Retire the `ROOT_REGISTRY` / `RootProvider` / `frame_chain` / `SAFEPOINT_ROOTS`
discovery apparatus now that the **structural reader** (`collect_machine_roots` /
`collect_persistent_roots` / `collect_k_spine`, roots.rs + k_spine.rs) is the LIVE
index-gc collector source at all three sites, and the two debug machine-equivalence
oracles have proven `OLD ⊆ (NEW ∪ KEPT)` green across the full 744-fixture corpus
(base 483 / M11-pt 221 / M11-he 40, MIN_BYTES low ⇒ oracle fires every collection).

Companion docs: `a4-4-collector-flip-design.md` (the flip this builds on),
`a4-4-safepoint-roots-refinement.md` (the SAFEPOINT_ROOTS keep-narrow rationale),
`roots-apparatus-extent-a3a4a5.md` (the file:line inventory),
`a3-a4-a5-implementation-design.md` (the umbrella sequencing).

---

## 0. Executive summary (answers to the five design questions)

### C — slab-vs-index disposition (THE LINCHPIN, get this right)

**The apparatus is the LIVE root source for the SLAB build (the default build).** It is
NOT a vestigial parallel path there. Verified consumers in slab mode:

| Consumer | Site | What it feeds |
|----------|------|---------------|
| async GC pool (quiescent/session-release collection) | `gc_allocator.rs:4283, 4303` (`collect_all_roots()`) | the slab mark-sweep root set |
| `trace_surviving_set` (session-release UAF guard) | `gc_allocator.rs:5126` (`collect_all_roots_readonly()`) | survivor set across session boundary |
| slab old-gen safepoint | `eval_loop.rs:3756` (`ctx.perform_safepoint(root_set.drain_into_vec())`) → `register_temporary_roots` (context.rs:79/482, session_context.rs:247) | the trampoline's S∪C∪K + frame-chain + caches + deferred envs, published to `SAFEPOINT_ROOTS` for the async collector |
| parallel VM/JIT worker cooperative safepoint | `eval_loop.rs:184` (`worker_cooperative_safepoint` → `collect_frame_chain_roots` + `register_temporary_roots`) | each worker's frame-chain roots |
| `process_gc_response` live-set guard | `gc_allocator.rs:3900` (`trace_safepoint_live_set`) | prev-cycle-dead-but-now-live protection |

The structural reader's single-threaded K-spine (`SUSPENDED_ACTIVATIONS` /
`LIVE_VM_STACK` thread-locals) **does NOT cover the slab build's multi-threaded root
sources**: `ParallelDispatchRootProvider`, `ParallelCollapseRootProvider`, and
`CurrentIterRootProvider` (types.rs:298/373, current_iter_root.rs:88) publish roots
of *other* worker threads' live values through the registry — exactly the cross-thread
visibility the K-spine (thread-local, single-owner) cannot provide. So:

> **A5 does NOT physically delete the apparatus. A5 `#[cfg(not(feature = "index-gc"))]`-scopes
> it to the slab build, and makes the index-gc build stop referencing it entirely.** Physical
> deletion of the slab apparatus happens at **Phase F4** ("delete the bridge"), when the slab
> collector itself is removed. This is forced by the master plan's own gate: the slab guard
> remains `cargo nextest run --release → 4312/0` **until F3** (Cargo.toml flips the default).

The index build, after A5, has roots = `σ|_Reachable(⟨C,E,K⟩) ∪ reach(E₀)` read structurally,
with NO registry, NO frame_chain, NO `RootProvider` — the genuine CESK property the workstream
demands. The slab build keeps the discovery apparatus, cfg-walled, inert under `index-gc`.

**Why this is safe and not a cop-out.** The index collector's gate is
`gate_open() = gc_mode_is_index() && !worker_ever_spawned() && active_evaluator_count() == 0`
(index_heap.rs:828). It fires ONLY in the single-threaded regime. In that regime the three
parallel `RootProvider`s are provably empty (no worker ever spawned ⇒ no parallel dispatch ⇒
no `ParallelDispatch*` registered; `CurrentIterRootProvider` is only populated inside worker
closures). So the index build loses NOTHING by not consulting the registry: the registry's
*entire content* in the index regime is exactly what the structural reader already reads by
name (`collect_global_anchors`) or structurally (env, K-spine, VM-leaf). The A4.3/A4.4
oracles proved precisely this (`OLD ⊆ NEW ∪ KEPT`) on the full corpus.

### A — oracle transition

Use **option (b): delete each apparatus piece together with its corresponding OLD-term in the
SAME sub-commit, so the oracle's OLD shrinks consistently and the check stays green and
meaningful at every step.** The oracle is NOT retired during A5 — per RT-7 it graduates to a
**permanent CI invariant** at the end (A5.7), restated as "the structural reader covers the
slab discovery apparatus" in slab debug builds, and "the structural reader is internally
consistent" in index debug builds.

Concretely, OLD has exactly two registry-derived terms (roots.rs:384, eval_loop.rs:3568):
`collect_all_roots()` = (`ROOT_REGISTRY` providers) `∪` `collect_safepoint_roots()`. Across
A5 these terms are rewritten — never silently dropped — as each provider is re-homed or
cfg-walled. Because every individual provider's roots are ALREADY covered by NEW (proven by
A4.4), removing a provider from OLD can only *shrink* OLD; `OLD ⊆ NEW ∪ KEPT` is preserved by
monotonicity. The detailed per-sub-step oracle state is in §3.

### B — re-homing order (each prerequisite precedes its deletion)

1. **A5.0** (groundwork, no deletion): add the `#[cfg(feature = "index-gc")]` /
   `#[cfg(not(...))]` seams so the index build can drop apparatus references while slab keeps
   them. Green-wall both builds. This unblocks every subsequent index-side deletion.
2. **A5.1**: delete the K-spine ⊗ frame_chain DUALITY in the index build — collapse the bundled
   guards (`FrameAndKSpineGuard`, the `with_vm_roots_frame` tuple, the eval_loop sibling
   `_tramp_frame_guard`/`_tramp_kspine_guard` pair) so the index build pushes ONLY the typed
   K-spine guard; the slab build pushes ONLY the frame_chain guard. **Prereq met by A4.2b**: the
   K-spine `Spine`/`ExprVec`/`Vm`/`SavedBindings` arms already mirror every frame_chain
   collector term-by-term (proven by the k_spine.rs unit tests + the corpus oracle).
3. **A5.2**: re-home the module-import / assertion `Vec<MettaValue>` (the RT-7 completeness item)
   — the callers at modules.rs:132/331 and testing_ops.rs (9 sites) push a bare
   `SuspendedActivationGuard::ExprVec` directly (index build) instead of `maybe_push_frame`.
   **Prereq met by A4.2b**: the K-spine `ExprVec` arm already records these via
   `maybe_push_frame`'s bundled guard; A5.2 just removes the frame_chain half of the bundle.
4. **A5.3**: cfg-wall the parallel/per-iter providers (`ParallelDispatch*`,
   `ParallelCollapse*`, `CurrentIterRootProvider`) and `MettaStateGcRoots`' *registration* to
   slab. **Prereq for MettaState (A5.3b)**: confirm (§2.3) that MettaState's source/output are
   covered in the index live feed by `state.collect_driver_program_roots` at every collection
   site (they are — all 3 flips call it) AND at the non-trampoline index entry paths
   (they don't exist — see §2.3) BEFORE removing the index-side registration.
5. **A5.4**: narrow `SAFEPOINT_ROOTS` — KEEP `register_temporary_roots` /
   `collect_safepoint_roots` / `SafepointRootHandle` as the **driver-publication transport**
   (the conformance `all` accumulator, the REPL/CLI `filtered_results`, the
   `CACHE_ROOT_HANDLE` snapshot); DELETE only the `RootProvider`-registry half
   (`collect_all_roots`'s provider loop). Reconciliation with the A5-list "delete
   SAFEPOINT_ROOTS" is in §2.4 — the list meant the *broad provider registry*, not the narrow
   transport; the plan's own words are "SAFEPOINT_ROOTS = transport, KEEP narrow".
6. **A5.5**: with all consumers re-homed/cfg-walled, cfg-wall the registry CORE
   (`ROOT_REGISTRY`, `RootProvider`, `register_root_provider`, `collect_all_roots`,
   `collect_all_roots_readonly`, `collect_provider_roots_readonly`, `trace_safepoint_live_set`)
   to slab and drop the index oracle's `collect_all_roots()` OLD-term.
7. **A5.6**: delete `frame_chain.rs` outright in BOTH builds (the slab build now reaches its
   frame-chain roots through the cfg-walled providers + `worker_cooperative_safepoint`, which
   are re-pointed to the surviving slab paths). **Prereq**: A5.1+A5.2 removed all index pushes;
   the slab frame-chain pushes are re-homed to the providers (§2.6).
8. **A5.7**: graduate the oracle to the permanent CI invariant; finalize docs.

### D — per-sub-step green-wall

Every sub-step ends green on BOTH builds before commit:

```
slab:   systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 \
            cargo nextest run --release                      # → 4325/0 (4312 + A4.x adds)
index:  systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 \
            cargo nextest run --release --features index-gc  # → 4177/0
index conformance (release): FANOUT_DEPTH=0 INDEX_GC_REPORT=1 \
            ./target/release/mtt-conformance --strict        # → base 483 / M11-pt 221 / M11-he 40
oracle (debug, until A5.7): MIN_BYTES low so it fires every collection:
            cargo build --features index-gc                  # debug, oracle live
            FANOUT_DEPTH=0 ./target/debug/mtt-conformance --strict  # base 483 / M11-pt 221 / M11-he 40, 0 oracle panics
warnings: cargo clippy --all-targets {,--features index-gc}  # 0 NEW warnings
```

The actual nextest target counts (slab `4325`, index `4177`) and conformance counts come
from the A4.4 green-wall (`a4-4-safepoint-roots-refinement.md`). Each sub-step asserts the
delta is the symbols it deleted (dead-test removal only) and nothing else regresses.

**Biggest risk** (see §5): the slab build silently losing a root because a cfg seam mis-routes
(`#[cfg(feature="index-gc")]` vs `#[cfg(not(...))]` inverted, or a `register_root_provider`
call cfg-walled out of slab). This is a UAF that release tests may not surface (the value is
only freed if a later allocation reuses the slot). **Mitigation**: ASAN on the SLAB build at
A5.3, A5.5, A5.6 (not just index), plus the slab debug oracle as a standing check that the
*slab* discovery set is still fully assembled.

---

## 1. The apparatus, verified extent + current consumers

All citations verified against HEAD `390b743`. (The `roots-apparatus-extent-a3a4a5.md` doc
predates A4.4 and has some stale line numbers; the items below are re-verified.)

### 1.1 Registry core — `gc_allocator.rs`

| Symbol | Line | Disposition |
|--------|------|-------------|
| `trait RootProvider` | 3637 | cfg→slab (A5.5) |
| `static ROOT_REGISTRY` | 3653 | cfg→slab (A5.5) |
| `fn root_registry()` | 3655 | cfg→slab (A5.5) |
| `pub fn register_root_provider` | 3663 | cfg→slab (A5.5) |
| `pub fn collect_all_roots` | 3686 | cfg→slab (A5.5) |
| `fn collect_all_roots_readonly` | 3726 | cfg→slab (A5.5) |
| `fn collect_provider_roots_readonly` | 3854 | cfg→slab (A5.5) |
| `pub(crate) fn trace_safepoint_live_set` | 3900 | cfg→slab (A5.5) — env_roots branch (3926) calls `collect_provider_roots_readonly` |
| `static SAFEPOINT_ROOTS` | 3771 | **KEEP** (narrow transport, A5.4) |
| `fn safepoint_registry` | 3773 | **KEEP** (A5.4) |
| `struct SafepointRootHandle` + `Drop` | 3781 | **KEEP** (A5.4) |
| `pub fn register_temporary_roots` | 3807 | **KEEP** (A5.4) |
| `pub fn collect_safepoint_roots` | 3837 | **KEEP** (A5.4) — but its CALLER `collect_all_roots` is cfg-walled |

### 1.2 The 11 `RootProvider` impls + their KEEP-bodies

Each impl's *registration* is deleted/cfg-walled; each holder's *inherent structural collector*
(the body `collect_global_anchors` / the structural reader calls by name) is **KEPT**.

| Impl | File:line | Inherent KEEP-body (read by name, NOT via registry) | Registration disposition |
|------|-----------|------------------------------------------------------|--------------------------|
| `GenericEnvironmentShared<MettaValue>` | core.rs:2268 | `collect_roots_into` (core.rs:2168) — read by `collect_structural`/`collect_persistent_roots` | impl→slab; registration call(s) in core.rs ~2200/2250 cfg→slab (A5.3) |
| `MettaStateGcRoots` | metta_state.rs:25 | `collect_driver_program_roots` (metta_state.rs:233) — read by all 3 flips | impl + registration (metta_state.rs:124) cfg→slab (A5.3b) |
| `BytecodeCacheRoots` | cache.rs:242 | `collect_bytecode_cache_roots` (cache.rs:255) — in `collect_global_anchors` | impl + reg (cache.rs:303) cfg→slab (A5.3) |
| `SpaceRegistryRoots` | space_registry.rs:165 | `collect_all_gc_values` (space_registry.rs:146) — in anchors | impl + reg (space_registry.rs:182) cfg→slab (A5.3) |
| `MemoCacheRoots` | memo_cache.rs:247 | `collect_all_values` (memo_cache.rs:198) — in anchors | impl + reg (memo_cache.rs:264) cfg→slab (A5.3) |
| `TieredCacheRoots` | tiered_cache.rs:1929 | `collect_roots_into` (tiered_cache.rs:1104) — in anchors | impl + reg (tiered_cache.rs:1946) cfg→slab (A5.3) |
| `CompilerAtomRoots` | iterative.rs:49 | `collect_compiler_atom_roots` (iterative.rs:62) — in anchors | impl + reg (iterative.rs:85) cfg→slab (A5.3) |
| `ParallelDispatchRootProvider` | types.rs:298 | (none — parallel-only; not in index regime) | impl + reg (eval_loop.rs:2534) cfg→slab (A5.3) |
| `ParallelCollapseRootProvider` | types.rs:373 | (none — parallel-only) | impl + reg (eval_loop.rs:3002) cfg→slab (A5.3) |
| `CurrentIterRootProvider` | current_iter_root.rs:88 | (none — parallel-worker-only) | whole file/registration (current_iter_root.rs:128) cfg→slab (A5.3) |
| `(env-fork)` via `try_register_env_roots` | gc_allocator.rs:4236 | (same as GenericEnvironmentShared body) | call cfg→slab (A5.3) |

> NOTE: the prompt's "8" providers undercounts. There are **10 `impl RootProvider for`** plus the
> env-fork registration helper. The three parallel ones (`ParallelDispatch*`,
> `ParallelCollapse*`, `CurrentIter*`) are the ones with NO structural mirror — they are the
> exact reason A5 cannot delete the apparatus for slab; they are inert in the index regime.

### 1.3 `frame_chain.rs` (whole file, 515 lines, 9 tests) + the K-spine that supersedes it

| frame_chain symbol | Superseded by (k_spine) | Notes |
|--------------------|--------------------------|-------|
| `EvalFrame` / `FRAME_CHAIN_HEAD` / `EvalFrameGuard` | `SuspendedActivationGuard` / `VmLeafGuard` (k_spine.rs:83/109) | typed, structurally-walkable |
| `collect_vec_roots` | `SuspendedActivation::ExprVec` arm (k_spine.rs:161) | `extend_from_slice`, identical |
| `RootCollectorFn` / `push_custom` | (typed enums — no type erasure) | |
| `FrameLabel` (+ stack-trace) | (see §2.6 — stack-trace re-homing) | `capture_stack_trace`/`format_stack_trace` are SEPARATE concern |
| `maybe_push_frame` | bare `SuspendedActivationGuard::push` at callers | A5.2 |
| `FrameAndKSpineGuard` | (collapses to bare `SuspendedActivationGuard`) | A5.1 |
| `collect_frame_chain_roots` | `collect_k_spine` (k_spine.rs:144) | walked at safepoints |
| eval_loop `TrampolineFrameRoots` / `collect_trampoline_frame_roots` (3093/3108) | `SuspendedActivation::Spine` arm | A5.1 |
| eval_loop `_tramp_frame_guard` (3322) | `_tramp_kspine_guard` (3344) | A5.1 |
| vm/mod `with_vm_roots_frame` / `vm_roots_collector` (1181/144) | `VmLeaf::Vm` (k_spine.rs:64) | A5.1 |
| vm/mod `SavedBindings` push (4081/4230) | `VmLeaf::SavedBindings` (k_spine.rs:69) | already typed; drop frame_chain half |

### 1.4 frame_chain / K-spine PUSH-SITE pairs (the sibling-guard inventory)

Every index-gated K-spine push currently sits ALONGSIDE a frame_chain push. A5.1/A5.2 remove
the frame_chain half of each pair in the index build; A5.6 removes the slab half.

| Site | frame_chain push | K-spine sibling | Re-home action |
|------|------------------|-----------------|----------------|
| eval_trampoline_inner spine | `_tramp_frame_guard` (eval_loop.rs:3322, `EvalFrameGuard::push_custom` w/ `collect_trampoline_frame_roots`) | `_tramp_kspine_guard` Spine (3344) | A5.1: index keeps only K-spine; slab keeps only frame_chain |
| VM nested dispatch ×2 | `with_vm_roots_frame` → frame half (vm/mod.rs:1199) called at 8612/8708 | `with_vm_roots_frame` → `VmLeaf::Vm` half (1201+) | A5.1: split the tuple; index→VmLeafGuard only, slab→EvalFrameGuard only |
| VM saved-bindings ×2 | EvalFrameGuard (vm/mod.rs ~4044) | `VmLeaf::SavedBindings` (4081/4230) | A5.1 |
| module Include/Import ×2 | `maybe_push_frame` (modules.rs:132/331) | `ExprVec` inside `maybe_push_frame` | A5.2: callers push bare `ExprVec` (index) |
| assertions ×9 | `maybe_push_frame` (testing_ops.rs:146…554) | `ExprVec` inside `maybe_push_frame` | A5.2 |
| parallel dispatch | `ParallelDispatchHandle._root_guard` (EvalFrameGuard) | (none — parallel) | slab-only; cfg-walled A5.3, deleted A5.6 |
| parallel collapse | `ParallelCollapseDispatchHandle._root_guard` | (none) | slab-only; A5.3/A5.6 |

---

## 2. Re-homing details (the prerequisites)

### 2.1 The bundled-guard collapse (A5.1)

`FrameAndKSpineGuard` (frame_chain.rs:227) holds `{_frame: EvalFrameGuard, _kspine:
Option<SuspendedActivationGuard>}`. `with_vm_roots_frame` returns `(EvalFrameGuard,
Option<VmLeafGuard>)`. After A5.1 these become single-guard:

- **index build**: only the K-spine guard is constructed. `maybe_push_frame` returns
  `Option<SuspendedActivationGuard>` (the ExprVec guard); `with_vm_roots_frame` returns
  `Option<VmLeafGuard>`; the eval_loop spine pushes only `_tramp_kspine_guard`.
- **slab build**: only the frame_chain guard is constructed (the K-spine push sites are already
  `gc_mode_is_index()`-gated, so in slab they're `None` today — A5.1 makes that a compile-time
  `#[cfg]` so the slab build doesn't even reference k_spine).

This is mechanically a `#[cfg(feature="index-gc")]` / `#[cfg(not)]` split of each bundling
constructor. The K-spine arms are PROVEN equivalent to the frame_chain collectors (k_spine.rs
unit tests + corpus oracle), so the index build loses nothing.

**Subtle correctness note flagged for the implementer**: at eval_loop.rs:3819 the yield path
does `drop(_tramp_frame_guard)` before moving `work_stack`/`continuations` into `SuspendedEval`,
but there is **no matching `drop(_tramp_kspine_guard)`**. Today this is benign (the yield path is
the parallel-worker cooperative-yield, unreachable in the single-threaded index regime where the
K-spine guard is actually pushed — comment at 3812-3818). After A5.1 makes the K-spine the SOLE
index guard, the implementer MUST add `drop(_tramp_kspine_guard)` at 3819 (gated to index) so the
raw-pointer-into-moved-Vec contract stays sound unconditionally. This is a re-homing prerequisite,
not an afterthought.

### 2.2 Module-import / assertion ExprVec re-home (A5.2, the RT-7 completeness item)

RT-7 flagged: `frame_chain` protects a caller-held `Vec<MettaValue>` of compiled
module-import / assertion expressions across nested `eval_trampoline`
(modules.rs:132 Include, modules.rs:331 Import, testing_ops.rs:146/235/279/326/368/414/460/509/554).
These are NOT in any C/K — they're a Rust-local Vec the caller iterates AFTER the nested
trampoline returns.

**Coverage is already proven**: `maybe_push_frame` (frame_chain.rs:241) records BOTH the
frame_chain `collect_vec_roots` AND (index-gated) the K-spine `SuspendedActivation::ExprVec`
over the SAME `data` pointer (frame_chain.rs:250-258). `collect_k_spine`'s ExprVec arm does
`out.extend_from_slice(&*exprs)` (k_spine.rs:161) — byte-identical to `collect_vec_roots`
(frame_chain.rs:210). The corpus oracle confirms these are in NEW.

**A5.2 action**: replace each `maybe_push_frame::<C>(label, &vec)` call with, in the index build,
a bare `SuspendedActivationGuard::push(SuspendedActivation::ExprVec { exprs: &vec })`, and in the
slab build, a bare `EvalFrameGuard::push_vec(label, &vec)`. Wrap in a tiny shared helper (e.g.
`push_expr_vec_frame(label, &vec) -> impl Drop`) that `#[cfg]`-selects, so the 11 call sites stay
one-liners and drop order is preserved. The `label: FrameLabel` argument is only used by the
slab frame_chain (for stack traces, §2.6); the index ExprVec doesn't need it.

### 2.3 MettaState off the registry (A5.3b) — coverage at EVERY collection site

MettaState's roots = `source` + `output` (metta_state.rs:21-22). The live index feed reads them
via `state.collect_driver_program_roots` (metta_state.rs:233) at ALL THREE flips:
- `eval/mod.rs:292` (quiescence `eval()`),
- `tier_forced.rs:317` (quiescence `eval_with_tier`),
- `eval_loop.rs:3696` (midloop, via `ctx.collect_driver_roots` → SessionContext forwards to
  `state.collect_driver_program_roots`, session_context.rs:179).

**Non-trampoline index entry paths that still need MettaState** — MAPPED, and there are none in
the index regime:
- `collect_all_roots_readonly()` (→ `trace_surviving_set`, gc_allocator.rs:5126) is the SLAB
  session-release path. It is gated by the slab GC pool, which never runs in the index regime
  (index uses its own `run_collection_if_triggered`). After A5.5 it's cfg-walled to slab.
- The slab session-release worker (gc_allocator.rs:4283/4303 `collect_all_roots`) — slab only.
- The index collector NEVER calls `collect_all_roots*`; it only calls
  `collect_persistent_roots`/`collect_machine_roots` + `collect_driver_program_roots`.

**Conclusion**: in the index build, MettaState's roots are FULLY covered by
`collect_driver_program_roots` at every site where the index collector fires. Removing the
index-side `MettaStateGcRoots` registration loses nothing. The registration + impl are
cfg-walled to slab (the slab session-release still needs them).

### 2.4 SAFEPOINT_ROOTS — KEEP narrow, reconcile with the "delete" list (A5.4)

The A5 deletion list (prompt + `a3-a4-a5-implementation-design.md`) names `SAFEPOINT_ROOTS` /
`register_temporary_roots` / `SafepointRootHandle` as deletion targets. The A4.4 refinement
(`a4-4-safepoint-roots-refinement.md`) and the plan's standing words ("SAFEPOINT_ROOTS =
transport, KEEP narrow") say KEEP them. **Reconciliation**: the deletion list refers to the
*broad provider-registry coupling* — `SAFEPOINT_ROOTS` was historically read ONLY through
`collect_all_roots()` (which unions providers ∪ safepoint roots). What A5 deletes is that
COUPLING (the `collect_all_roots` provider loop). What A5 KEEPS is the **narrow driver-publication
transport**:

KEEP (the legitimate driver-C / cache publication channel):
- `register_temporary_roots` / `SafepointRootHandle` / `safepoint_registry` / `SAFEPOINT_ROOTS`
  / `collect_safepoint_roots` (gc_allocator.rs:3771-3844).
- Its driver consumers: `mtt_conformance.rs:189` (the `all` cross-directive result accumulator),
  `main.rs:775/1033` (REPL/CLI `filtered_results`), and `CACHE_ROOT_HANDLE` /
  `refresh_thread_local_cache_roots` (eval/mod.rs:104/133 — the thread-local cache snapshot).
- The index live feed already KEEPS `collect_safepoint_roots` in all 3 flips (eval/mod.rs:297,
  tier_forced.rs:320, eval_loop.rs:3699) — this is correct and stays.

DELETE/cfg-wall (the broad coupling):
- `collect_all_roots`'s call to `collect_safepoint_roots` (gc_allocator.rs:3712) and
  `collect_all_roots_readonly`'s (3747) — these go away WITH `collect_all_roots*` (A5.5,
  cfg→slab).
- The SLAB safepoint publication `ctx.perform_safepoint` → `register_temporary_roots`
  (context.rs:79/482, session_context.rs:247) is the slab old-gen path; it KEEPS using
  `register_temporary_roots` (the transport survives) but is only reached in slab mode.

So `register_temporary_roots` survives in BOTH builds (driver publication is build-agnostic);
only the provider-registry that *also* read `SAFEPOINT_ROOTS` is cfg-walled. A5.4's concrete
work is therefore small: it's mostly a DOC/comment correction + confirming `collect_safepoint_roots`
is no longer reachable from any index-live provider path (it isn't — only from `collect_all_roots*`,
which A5.5 cfg-walls, and from the kept driver flips). The "narrowing" is the removal of the
`RootProvider`-registry reader, achieved structurally by A5.5.

### 2.5 `CurrentIterRootProvider` (A5.3) — whole-file cfg-wall

current_iter_root.rs is entirely a parallel-worker root source (module docs: "Inputs and
outputs of every parallel dispatch are already covered by ParallelDispatch*; the remaining hole
is the current iteration's value"). It is populated only inside worker closures
(`CurrentIterScope::enter`). In the single-threaded index regime it is never populated. The
WHOLE FILE is `#[cfg(not(feature = "index-gc"))]`-walled (A5.3), and physically deleted at F4.
Its `store_current_iter` / `clear_current_iter` / `CurrentIterScope` call sites (worker closures
in eval_loop.rs) must be cfg-walled in lock-step — they're already on the parallel path so the
index build's worker-spawn path is unreachable, but the references must compile-out.

### 2.6 Stack-trace re-homing (A5.6 prerequisite, easy to miss)

`frame_chain.rs` is NOT only a GC root source — `capture_stack_trace` / `format_stack_trace`
(frame_chain.rs:299/315) + `FrameLabel` provide the `\n  in: #0 assertEqual …` error context.
Before deleting frame_chain.rs (A5.6), the implementer MUST verify whether any error path calls
`format_stack_trace`. Search: `rg -n 'format_stack_trace|capture_stack_trace' src/`. If consumed,
either (a) re-home the labels onto the K-spine (`SuspendedActivation` already has the Spine/ExprVec
distinction; add an optional `FrameLabel`), or (b) keep a minimal `FrameLabel`-only trace chain
(no root_data, no collector) as a separate tiny module. This is a re-homing PREREQ for A5.6 —
not a GC concern, but deleting the file loses it. If unused, delete with the file (one-line
confirmation in the A5.6 commit message).

---

## 3. Ordered sub-steps (each green on both builds before commit)

Legend per step: **DELETE/CFG** (symbols), **RE-HOME** (done-first), **ORACLE** (state after),
**SLAB/INDEX** (build disposition), **GREEN-WALL** (gates run before commit).

### A5.0 — cfg seams + helper scaffolding (NO deletion)

- **RE-HOME**: introduce the cfg-select helper(s) the later steps need — `push_expr_vec_frame`
  (§2.2), and confirm `gc_mode_is_index()` (runtime) vs `feature="index-gc"` (compile) are used
  consistently. No symbol deleted; no behavior change.
- **ORACLE**: unchanged (both oracles live, OLD = `collect_all_roots ∪ result`).
- **SLAB/INDEX**: both unchanged.
- **GREEN-WALL**: slab nextest 4325 / index nextest 4177 / index conf 483·221·40 / debug oracle
  green / 0 new warnings. (This step exists so A5.1+ are pure deletions on a known-green base.)
- **Commit**: `A5.0 — cfg seams + cfg-select frame helper (no deletion)`.

### A5.1 — collapse the K-spine ⊗ frame_chain bundled guards (index → K-spine only)

- **RE-HOME (first)**: add `drop(_tramp_kspine_guard)` at eval_loop.rs:3819 gated to index
  (§2.1 subtle note). Verify the K-spine arms cover each bundle (already proven; re-assert via
  the k_spine.rs tests).
- **DELETE/CFG**:
  - `FrameAndKSpineGuard` (frame_chain.rs:227) → split: index `maybe_push_frame` returns
    `Option<SuspendedActivationGuard>`; slab returns `Option<EvalFrameGuard>` (interim — A5.2
    removes `maybe_push_frame` itself).
  - `with_vm_roots_frame` (vm/mod.rs:1181) return type tuple → `#[cfg]`-split: index
    `Option<VmLeafGuard>`, slab `Option<EvalFrameGuard>`. Delete `vm_roots_collector`
    (vm/mod.rs:144) from the INDEX build (cfg→slab).
  - eval_loop spine: index keeps only `_tramp_kspine_guard`; the `_tramp_frame_guard` +
    `TrampolineFrameRoots` + `collect_trampoline_frame_roots` (3093/3108/3318/3322) become
    `#[cfg(not(feature="index-gc"))]`.
- **ORACLE**: index midloop oracle (eval_loop.rs:3560) — OLD's `root_set` no longer accrues
  `collect_frame_chain_roots` for the spine/VM (those frame pushes are cfg-walled out of index),
  but NEW still has them via the K-spine. The oracle's `collect_frame_chain_roots` call in the
  OLD assembly (eval_loop.rs:3527) must be `#[cfg(not(feature="index-gc"))]` so OLD shrinks
  consistently. `OLD ⊆ NEW ∪ KEPT` preserved (NEW unchanged, OLD shrank). Slab oracle path
  unaffected (frame_chain still feeds slab OLD).
- **SLAB/INDEX**: slab byte-identical (still frame_chain). Index now pushes ONLY typed K-spine.
- **GREEN-WALL**: both nextest + index conf + debug oracle (index, fires every collection) +
  **ASAN index** (`-Zsanitizer=address`, FANOUT_DEPTH=0, capped ≤24G serial) to confirm the
  sole-K-spine index path is UAF-clean. 0 new warnings.
- **Commit**: `A5.1 — index GC roots via typed K-spine only; frame_chain spine/VM pushes cfg→slab`.

### A5.2 — re-home module-import / assertion ExprVec (RT-7 completeness item)

- **RE-HOME (first)**: the `push_expr_vec_frame` helper from A5.0 now backs all 11 sites.
- **DELETE/CFG**: replace `maybe_push_frame::<C>` at modules.rs:132/331 and testing_ops.rs
  (9 sites) with `push_expr_vec_frame(label, &vec)`. Delete `maybe_push_frame` (frame_chain.rs:241)
  — index build no longer references it; slab uses the helper's slab arm.
- **ORACLE**: unchanged structure — these Vecs were always in NEW (ExprVec) and (slab) OLD
  (collect_vec_roots). OLD's index assembly already excludes frame_chain after A5.1.
- **SLAB/INDEX**: slab byte-identical; index pushes bare ExprVec.
- **GREEN-WALL**: both nextest + index conf + debug oracle + ASAN index (module-import +
  assertion-heavy fixtures: the `include`/`import!`/`assertEqual` corpus). 0 new warnings.
- **Commit**: `A5.2 — re-home module/assert ExprVec to typed K-spine (RT-7 completeness)`.

### A5.3 — cfg-wall the RootProvider impls + registrations to slab

- **RE-HOME (first)**: §2.3 confirms MettaState fully covered in index feed (re-assert by
  running the index debug oracle with MettaState registration ALREADY removed — it must stay
  green, proving `collect_driver_program_roots` suffices). §2.5 confirms CurrentIter/Parallel are
  parallel-only.
- **DELETE/CFG** (`#[cfg(not(feature="index-gc"))]` on impl + registration call, KEEP the
  inherent collector body unconditionally):
  - core.rs:2268 impl + reg calls (~2200/2250) — KEEP `collect_roots_into` (2168).
  - metta_state.rs:25 impl + reg (124) — KEEP `collect_driver_program_roots` (233).
  - cache.rs:242 impl + reg (303) — KEEP `collect_bytecode_cache_roots` (255).
  - space_registry.rs:165 impl + reg (182) — KEEP `collect_all_gc_values` (146).
  - memo_cache.rs:247 impl + reg (264) — KEEP `collect_all_values` (198).
  - tiered_cache.rs:1929 impl + reg (1946) — KEEP `collect_roots_into` (1104).
  - iterative.rs:49 impl + reg (85) — KEEP `collect_compiler_atom_roots` (62).
  - types.rs:298/373 impls + regs (eval_loop.rs:2534/3002) — no KEEP body (parallel-only).
  - current_iter_root.rs — WHOLE FILE cfg→slab; cfg-wall `store_current_iter`/`clear_current_iter`/
    `CurrentIterScope` call sites in worker closures.
  - gc_allocator.rs:4236 `try_register_env_roots` call cfg→slab.
- **ORACLE**: the INDEX `collect_all_roots()` OLD-term (roots.rs:384, eval_loop.rs:3568) now
  returns FEWER providers (the cfg-walled ones aren't registered in index) — OLD shrinks. NEW
  unchanged (anchors read the bodies by name). `OLD ⊆ NEW ∪ KEPT` preserved. **This is the key
  consistency step**: every provider removed from OLD has its body still in NEW via
  `collect_global_anchors` (the 5 OnceLock + 4 thread-local anchors) or the env/state structural
  reads. Slab oracle unaffected.
- **SLAB/INDEX**: slab byte-identical (all providers still registered + collected). Index no
  longer registers any provider (registry is EMPTY in index, but still exists — A5.5 removes it).
- **GREEN-WALL**: both nextest + index conf + debug oracle (index, MIN_BYTES low) + **ASAN BOTH
  builds** (slab too — this step touches slab registration cfg; a mis-inverted cfg drops a slab
  provider → slab UAF). 0 new warnings.
- **Commit**: `A5.3 — cfg-wall RootProvider impls+registrations to slab; index roots fully structural`.

### A5.4 — narrow SAFEPOINT_ROOTS to the driver-publication transport

- **RE-HOME (first)**: none needed (the transport already exists and is kept). §2.4.
- **DELETE/CFG**: nothing structural beyond confirming `collect_safepoint_roots` is reachable in
  index ONLY through the 3 kept driver flips (eval/mod.rs:297, tier_forced.rs:320,
  eval_loop.rs:3699) and the driver publication sites (main/conformance/CACHE_ROOT_HANDLE).
  Correct the stale "delete SAFEPOINT_ROOTS" comments to "KEEP narrow transport; provider-registry
  coupling removed in A5.5". (The actual provider-loop removal lands in A5.5.)
- **ORACLE**: KEPT term = `collect_driver_program_roots ∪ collect_safepoint_roots` — unchanged
  (both oracles already include `collect_safepoint_roots` in KEPT per the A4.4 refinement).
- **SLAB/INDEX**: both unchanged behaviorally (this is the narrowing-of-intent step; the
  mechanical narrowing is A5.5's `collect_all_roots` cfg-wall).
- **GREEN-WALL**: both nextest + index conf + debug oracle + 0 new warnings. (Light step — mostly
  comment/intent; can be folded into A5.5's commit if the implementer prefers, but keeping it
  separate documents the reconciliation.)
- **Commit**: `A5.4 — SAFEPOINT_ROOTS = narrow driver transport (reconcile keep-vs-delete)`.

### A5.5 — cfg-wall the registry CORE; drop the index oracle's collect_all_roots OLD-term

- **RE-HOME (first)**: A5.1-A5.3 removed every index consumer/registrant. Confirm `rg -n
  'collect_all_roots\b' src/ | grep -v test | grep -v 'feature'` shows only the (about-to-be-walled)
  oracle OLD + slab GC pool.
- **DELETE/CFG** (`#[cfg(not(feature="index-gc"))]`):
  - `RootProvider` trait (3637), `ROOT_REGISTRY` (3653), `root_registry` (3655),
    `register_root_provider` (3663), `collect_all_roots` (3686), `collect_all_roots_readonly`
    (3726), `collect_provider_roots_readonly` (3854), `trace_safepoint_live_set`'s env-roots
    branch (3926) — all cfg→slab.
  - The index oracle OLD-term: in roots.rs:384 (`assert_quiescence_superset`) and eval_loop.rs:3568
    (midloop oracle), the `collect_all_roots()` line becomes `#[cfg(not(feature="index-gc"))]` — in
    the INDEX build the oracle's OLD now = `root_set` (already-cfg-reduced to S∪C∪K + caches +
    deferred, sans frame_chain). Since `collect_all_roots()` in index returned only the 9 anchors
    (all in NEW) ∪ safepoint roots (in KEPT), dropping it from OLD keeps `OLD ⊆ NEW ∪ KEPT` — and
    the oracle remains NON-VACUOUS (OLD still contains the live `root_set` S∪C∪K + caches + deferred
    envs, which NEW must cover).
  - `models/mod.rs` re-exports of `collect_all_roots` / `register_root_provider` / `RootProvider`
    → cfg→slab.
- **ORACLE**: index oracle now asserts "the structural NEW (`collect_machine_roots`) ∪ KEPT
  (driver-C ∪ safepoint) ⊇ the still-assembled `root_set` (S∪C∪K + 4 caches + deferred envs)".
  This is the genuine internal-consistency invariant (no registry term left). Slab oracle still
  has the full `collect_all_roots()` OLD-term.
- **SLAB/INDEX**: slab byte-identical (registry intact). **Index build no longer COMPILES any
  reference to `ROOT_REGISTRY`/`RootProvider`/`collect_all_roots`** — the genuine-CESK milestone:
  the index collector's roots are `σ|_Reachable(⟨C,E,K⟩) ∪ reach(E₀)`, no discovery side-channel.
- **GREEN-WALL**: both nextest + index conf + debug oracle (index) + **ASAN BOTH builds** (this is
  the riskiest cfg step — the slab GC pool MUST still see `collect_all_roots`). 0 new warnings
  (watch for `dead_code` on now-index-unused items — they're cfg'd, so should be clean; if
  rust-analyzer flags stale dead_code, it's the known STALE-cache artifact, re-verify with a clean
  `cargo build`). 20-run byte-identical (index, parallel superpose + forced sweep) per standing
  discipline before trusting the registry-free index path.
- **Commit**: `A5.5 — registry core cfg→slab; index GC is registry-free (genuine CESK roots)`.

### A5.6 — delete frame_chain.rs outright (both builds)

- **RE-HOME (first)**: §2.6 stack-trace decision (re-home `FrameLabel` trace or confirm unused).
  The slab build's frame_chain roots are now reached through: the cfg-walled providers (A5.3,
  still registered in slab) + `worker_cooperative_safepoint` (which calls
  `collect_frame_chain_roots`). So BEFORE deleting frame_chain.rs, `worker_cooperative_safepoint`
  (eval_loop.rs:184) and the slab `_tramp_frame_guard`/`with_vm_roots_frame`-slab-arm/parallel
  `_root_guard` must be re-pointed. **This means the slab build must ALSO migrate to the typed
  K-spine OR retain a minimal frame_chain.** Recommendation: KEEP a `#[cfg(not(feature=
  "index-gc"))]` frame_chain.rs for slab until F4, and in the INDEX build delete it. I.e. A5.6 =
  `#[cfg(not(feature="index-gc"))] mod frame_chain;` (the module is not compiled in index at all).
  Physical file deletion happens at F4 with slab.
  - The 9 frame_chain.rs tests are `#[cfg(test)]` inside the module → compiled only in slab test
    builds; index test build drops them (−9 index tests is expected; assert the index nextest
    delta is exactly these).
- **DELETE/CFG**: `mod frame_chain` declaration → `#[cfg(not(feature="index-gc"))]`. All remaining
  `crate::backend::eval::frame_chain::*` references in the INDEX build must already be gone (A5.1/
  A5.2); the slab references compile under the cfg. `worker_cooperative_safepoint`,
  `ParallelDispatchHandle._root_guard`, `ParallelCollapseDispatchHandle._root_guard`, the slab
  `_tramp_frame_guard`, the slab `with_vm_roots_frame` arm → all `#[cfg(not(feature="index-gc"))]`.
- **ORACLE**: index oracle unchanged from A5.5 (no frame_chain term). Slab oracle unchanged.
- **SLAB/INDEX**: slab byte-identical (frame_chain compiled). Index: frame_chain module does not
  exist → genuine structural-only root surface.
- **GREEN-WALL**: both nextest (index −9 frame_chain tests, asserted) + index conf + debug oracle +
  ASAN index. 0 new warnings.
- **Commit**: `A5.6 — frame_chain.rs cfg→slab-only (index has no frame_chain module)`.

### A5.7 — graduate the oracle to permanent CI invariant; finalize

- **RE-HOME (first)**: none.
- **ACTION**: the oracles stay `#[cfg(debug_assertions)]` and PERMANENT (RT-7). Restate their doc
  comments: in slab debug builds the oracle asserts "structural NEW ⊇ slab discovery apparatus
  (`collect_all_roots`)"; in index debug builds it asserts "structural NEW ∪ KEPT ⊇ the live
  S∪C∪K + caches + deferred-env `root_set`". Add a standing CI doc note: the oracle is the
  mechanical discharge of the reification-equivalence lemma and must never be deleted while the
  slab apparatus exists. Update `a3-a4-a5-implementation-design.md` status → A5 COMPLETE; record
  results in a new `a5-deletion-RESULTS.md`.
- **ORACLE**: now THE standing invariant (no longer a migration scaffold).
- **SLAB/INDEX**: both unchanged.
- **GREEN-WALL**: full re-green-wall both builds + ASAN both + 20-run index byte-identical +
  mmverify "Correct proof" + HE-bisim 40/40 + PLN budgets (per the per-rung gate).
- **Commit**: `A5.7 — graduate machine-equivalence oracle to permanent CI invariant; A5 complete`.

---

## 4. Oracle state, step-by-step (Question A, concrete)

For each step, the index debug oracle's `OLD`, `NEW`, `KEPT` and why `OLD ⊆ NEW ∪ KEPT` holds:

| Step | OLD (index) | NEW (index) | KEPT (index) | Why ⊆ holds |
|------|-------------|-------------|--------------|-------------|
| A5.0 | `root_set`(S∪C∪K + frame_chain + 4 caches + deferred) ∪ `collect_all_roots()`(9 providers ∪ safepoint) ∪ result | `collect_machine_roots`(S∪C∪K + env + anchors + k-spine) ∪ deferred | driver-C ∪ safepoint | A4.4-proven (baseline) |
| A5.1 | OLD − frame_chain(spine/VM) [cfg-walled] | unchanged (k-spine carries spine/VM) | unchanged | OLD shrank, NEW ⊇ removed terms |
| A5.2 | unchanged (ExprVec was via maybe_push_frame; OLD index already excludes frame_chain) | unchanged (ExprVec in k-spine) | unchanged | identical |
| A5.3 | `root_set` ∪ `collect_all_roots()`(NOW 0 providers ∪ safepoint) ∪ result | unchanged (anchors read bodies by name) | unchanged | every removed provider's body ∈ NEW anchors |
| A5.4 | unchanged | unchanged | driver-C ∪ safepoint (explicitly narrow) | identical |
| A5.5 | `root_set`(S∪C∪K + 4 caches + deferred) ∪ result [`collect_all_roots` term cfg-walled out] | unchanged | unchanged | `collect_all_roots` in index returned only NEW-anchors ∪ KEPT-safepoint; dropping it keeps ⊆ AND oracle stays non-vacuous (root_set live) |
| A5.6 | unchanged from A5.5 | unchanged | unchanged | no frame_chain term anywhere |
| A5.7 | (permanent) | (permanent) | (permanent) | standing invariant |

The SLAB debug oracle's OLD KEEPS the full `collect_all_roots()` term throughout (the slab
apparatus is intact), so the slab oracle remains the full equivalence check the master plan
mandates as the permanent CI invariant for as long as slab exists.

---

## 5. Risks (Question D's "single biggest" + the rest)

### THE biggest risk: a cfg seam silently drops a SLAB root → latent UAF

The dangerous failure mode is NOT the index build (the oracle + ASAN cover it densely) — it's the
**slab build losing a root** because a `#[cfg(not(feature="index-gc"))]` is inverted, mis-placed,
or a `register_root_provider` call gets cfg-walled out of slab by accident. A dropped slab root is
a use-after-free that **release tests frequently do NOT surface** (the freed slot is only observed
if a later allocation reuses it and the stale pointer is dereferenced — exactly the FlyingRaven /
017-accumulator class the A4.4 oracle caught). 

**Mitigations (mandatory, baked into the green-wall)**:
1. Run the **SLAB debug oracle** at every cfg step (A5.3, A5.5, A5.6) — it asserts the slab
   discovery apparatus is still fully assembled (`collect_all_roots` ⊆ structural NEW ∪ KEPT in
   slab too; the slab K-spine is empty so this checks the apparatus didn't lose a provider).
2. **ASAN the SLAB build** at A5.3 / A5.5 / A5.6 (not just index), capped ≤24G serial, FOREGROUND.
3. After A5.3 and A5.5, diff the registered-provider count: a debug-only counter or a
   `collect_all_roots().len()` snapshot on a fixed fixture, asserted unchanged in slab.
4. Per-step assertion that the nextest delta = exactly the cfg-dropped *tests* (e.g. A5.6 index
   −9 frame_chain tests) and zero production regressions.

### Other risks

- **Stack-trace loss (A5.6)**: deleting frame_chain.rs drops `format_stack_trace`. Mitigated by
  §2.6 (decide re-home vs confirm-unused before A5.6). Low severity (error-message cosmetics) but
  easy to forget.
- **The yield-path K-spine guard (A5.1)**: missing `drop(_tramp_kspine_guard)` at eval_loop.rs:3819
  once K-spine is the sole index guard (§2.1). A raw pointer into a moved `work_stack`. Unreachable
  in the single-threaded index regime today, but must be made sound unconditionally. Caught by ASAN
  only if the yield path is exercised — add a targeted test or a debug_assert.
- **`dead_code` warnings (A5.5)**: cfg-walled items unused in index may trip rust-analyzer's STALE
  cache (the WORKER_EVER_SPAWNED precedent). Re-verify with a clean `cargo build`, not the LSP, before
  declaring "0 new warnings".
- **Oracle vacuity (A5.5)**: dropping `collect_all_roots()` from index OLD could in principle make
  the oracle vacuous. It does NOT: the index OLD retains the live `root_set` (S∪C∪K + 4 caches +
  deferred envs), so NEW must still structurally cover the whole machine — the check stays
  load-bearing. The A4.4 refinement's non-vacuity argument (the midloop CI test
  `a4_3_oracle_holds_across_safepoint` drives via `eval_trampoline` with empty `CACHE_ROOT_HANDLE`,
  forcing the 9 caches to be covered by NEW) continues to hold.

---

## 6. Cross-references

- Flip this builds on: `a4-4-collector-flip-design.md`.
- SAFEPOINT_ROOTS keep-narrow: `a4-4-safepoint-roots-refinement.md`.
- File:line inventory (pre-A4.4, line numbers slightly stale): `roots-apparatus-extent-a3a4a5.md`.
- Umbrella: `a3-a4-a5-implementation-design.md`.
- Master plan (phases A–F, F4 deletes slab): `~/.claude/plans/help-me-complete-the-shimmying-mochi.md`.
- TLA+ quiescence safety (justifies single-threaded gate): `tla/StoreCentricGC.tla`.
