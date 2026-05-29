# A4.2b — Typed K-spine thread-locals (implementation design)

CESK GC migration, Phase A4.2b. Replaces `frame_chain.rs`'s type-erased native-stack
rooting with TWO **typed** thread-locals that represent the part of the continuation
register K living on the Rust stack across a NESTED `eval_trampoline`:
`SUSPENDED_ACTIVATIONS` (suspended outer trampoline activations) + `LIVE_VM_STACK`
(live bytecode-VM leaves). **Additive / byte-identical**: push typed records ALONGSIDE
the existing frame_chain guards; the typed thread-locals are read only by the new
`collect_k_spine`/`collect_machine_roots` (tests + A4.3 oracle + A4.4 safepoint), never
in the hot path during A4.2b. frame_chain stays (deleted in A5). Designed by Plan agent
2026-05-29, all signatures source-verified before implementation.

## Source-verified ground truth
- `maybe_push_frame<C: EvalContext>(label, data: *const Vec<MettaValue>) -> Option<EvalFrameGuard>`
  (frame_chain.rs:228) is the SINGLE chokepoint for ALL module/testing sites: testing_ops.rs has
  **9** callers (not 3), modules.rs has 2 — all 11 route through it. One edit migrates all 11.
- All 3 VM sites (#7 `with_vm_roots_frame` itself at vm/mod.rs:1181, #8 eval_sub_expr_vm at 8564,
  #9 at 8660) funnel through `with_vm_roots_frame` → `Option<EvalFrameGuard>`. One edit covers all 3.
- Site #1 (eval_loop.rs:3318-3337): `work_stack`/`continuations` are bare `let mut` locals (NOT yet a
  live `SeckState` field); frame_chain push is `push_custom(Eval, &_tramp_roots, collect_trampoline_frame_roots)`
  **gated on `gc_mode_is_index()`** (3323). The Spine guard must be identically gated.
- `WorkItem::collect_values(&self, &mut Vec<MettaValue>)` (types.rs:1736), `Continuation::collect_values`
  (types.rs:1817), `GenericBytecodeVM::collect_roots_into(&self, &mut Vec<V>)` (vm/mod.rs:1049),
  `ActiveFactory` = GcFactory|IndexFactory (models/mod.rs:74/76), `gc_mode_is_index()` pub(crate)
  (models/metta_value.rs). All confirmed present.
- No caller names the `maybe_push_frame`/`with_vm_roots_frame` return type (all `let _g = …` / `drop(_g)` /
  `.is_some()`/`.expect()`), so the return-type changes to bundling structs compile unchanged. The parallel
  sites #12/#13 build `EvalFrameGuard` directly (types.rs structs) — NOT via maybe_push_frame — so they are
  unaffected and stay on frame_chain.

## Decisions
1. **REFERENCE, never materialize.** Records hold raw pointers to the live data and decode at read time
   (exactly like frame_chain). `work_stack` mutates every reduction; a clone-at-push snapshot would go stale
   between push and the safepoint, breaking the A4.3 `OLD ⊆ NEW` oracle and risking UAF. Pinned by a
   dedicated `test_kspine_reference_not_snapshot`.
2. **Gate ALL k_spine pushes on `gc_mode_is_index()`** (refines the Plan agent's "ExprVec ungated"): slab mode
   gets ZERO k_spine work → truly byte-identical (not merely observationally). The A4.3 oracle runs in index
   mode, where every frame_chain push has a matching k_spine push (the records are inserted right beside each
   frame_chain push, under the same gate).
3. **Chokepoint migration** via bundling guards: change `maybe_push_frame` and `with_vm_roots_frame` to return
   small bundling structs that hold the existing `EvalFrameGuard` plus an `Option<…KSpineGuard>` (Some only in
   index mode). All 13 call sites compile unchanged.

## Record types (`src/backend/eval/cesk/k_spine.rs`, new module)
```rust
pub(crate) enum SuspendedActivation {
    Spine { work_stack: *const Vec<WorkItem>, continuations: *const Vec<Continuation> }, // site #1
    ExprVec { exprs: *const Vec<MettaValue> },                                            // sites #2-#11(module/test)
}
pub(crate) enum VmLeaf {
    Vm { vm: *const GenericBytecodeVM<MettaValue, ActiveFactory> },  // sites #7/#8/#9
    SavedBindings { bindings: *const Vec<MettaValue> },             // sites #10/#11
}
```
Thread-locals: `SUSPENDED_ACTIVATIONS: RefCell<Vec<SuspendedActivation>>`, `LIVE_VM_STACK: RefCell<Vec<VmLeaf>>`
(stacks; push on guard ctor, pop on Drop, LIFO matching frame_chain). RAII guards `SuspendedActivationGuard`,
`VmLeafGuard` with `unsafe fn push(record) -> Self` (the unsafe contract = pointers outlive the guard, same as
frame_chain). `Vm` arm pins the only nested-eval-reaching monomorphization `<MettaValue, ActiveFactory>` (same
assumption `with_vm_roots_frame` already makes; JIT gated off under index-gc).

## Edits (5 hunks + new module + reader)
- **NEW** `cesk/k_spine.rs`: thread-locals, enums, guards, unit tests. Add `pub mod k_spine;` to `cesk/mod.rs`.
- **`cesk/roots.rs`**: add `collect_k_spine(out)` (walks both thread-locals: Spine→`collect_values` per work/kont
  item, ExprVec→`extend_from_slice`, Vm→`collect_roots_into`, SavedBindings→`extend_from_slice`) + the top-level
  `collect_machine_roots(out, S, C, work, K, E₀)` = `collect_structural ∪ collect_global_anchors ∪ collect_k_spine`.
  Both `pub fn`, unused in hot path (matches collect_structural/collect_global_anchors). + tests.
- **`frame_chain.rs`** (Hunk 2, covers 11 sites): `maybe_push_frame` returns `Option<FrameAndKSpineGuard>`
  `{ _frame: EvalFrameGuard, _kspine: Option<SuspendedActivationGuard> }`; pushes `ExprVec{exprs:data}` when
  `gc_mode_is_index()`.
- **`eval_loop.rs`** (Hunk 1, site #1): after the `_tramp_frame_guard` block (line 3337), add sibling
  `_tramp_kspine_guard: Option<SuspendedActivationGuard>` referencing the same `&work_stack`/`&continuations`,
  gated on `gc_mode_is_index()`.
- **`bytecode/vm/mod.rs`** (Hunk 3, sites #7/#8/#9): `with_vm_roots_frame` returns a bundling struct
  `VmFrameAndKSpineGuard { _frame, _kspine: Option<VmLeafGuard> }`; pushes `VmLeaf::Vm{vm}` under the existing
  `V==MettaValue` TypeId gate + `gc_mode_is_index()`. (Hunks 4/5, sites #10/#11 at ~4048/4183): after each
  `materialized_box` frame_chain push, add a `VmLeaf::SavedBindings{bindings: ptr}` guard into the existing
  guard tuple (drop order: frame guard → kspine guard → materialized_box, so data outlives both).

## Reader / composition (roots.rs)
`collect_k_spine` reuses the EXACT decoders frame_chain uses (`WorkItem/Continuation::collect_values`,
`extend_from_slice`, `GenericBytecodeVM::collect_roots_into`) ⇒ term-by-term identity to the frame_chain
collectors ⇒ the oracle's `OLD ⊆ NEW` is immediate (== for these sites). `collect_machine_roots` is the single
entry the A4.3 oracle and A4.4 safepoint will consume; kept unused-in-hot-path for A4.2b.

## Tests (k_spine.rs + roots.rs)
1. `test_kspine_spine_equals_frame_chain_over_same_data` — push BOTH a TrampolineFrameRoots frame_chain guard and
   a Spine guard over the same work_stack/continuations; assert `collect_k_spine` == `collect_frame_chain_roots`
   (sorted inner_ptr multiset). **Load-bearing.**
2. `test_kspine_exprvec_equals_frame_chain` — via the bundled `maybe_push_frame` guard over a `Vec<MettaValue>`.
3. `test_kspine_vmleaf_equals_vm_collect_roots` — a VM with non-empty stacks; assert == `vm.collect_roots_into`.
4. `test_kspine_nested_lifo_pop` — nested push/pop visibility (mirrors frame_chain's nested test).
5. `test_kspine_reference_not_snapshot` — push Spine over empty work_stack, then push an item; assert
   `collect_k_spine` sees it (clone-at-push would fail). Defends decision #1.
6. `test_collect_machine_roots_superset_of_old` (roots.rs) — embryo of the A4.3 oracle.
   All gated/run under index mode where the pushes are active; slab-arm tests assert empty (no push).

## Soundness
Every typed pointer names the SAME live datum its sibling `EvalFrameGuard` already pins, with LIFO drop order
and data-outlives-guard — so no new UAF mode beyond frame_chain's (already sound). The records are write-only
until the oracle/safepoint reads them, and all pushes are `gc_mode_is_index()`-gated, so the slab build does
ZERO extra work and every observable result is byte-identical. The `RefCell<Vec>` push/pop fires only at
nested-eval entry/exit (not per reduction) — immaterial cost, matching the plan's "K push/pop is free."

## A5 forward-link
To delete frame_chain.rs, A5 re-homes the `ExprVec` data (module-import/assert Vecs, modules.rs:132/331 +
9 testing_ops) into the nested activation's C (work_stack) or a scoped E₀ root, then deletes the safepoint
`collect_frame_chain_roots` calls, `with_vm_roots_frame`/`vm_roots_collector`, `TrampolineFrameRoots`/
`collect_trampoline_frame_roots`, and the file. The bundling structs collapse to bare `…KSpineGuard` returns.
#12/#13 parallel sites move to structural roots in Phase D (not A5). Nothing in A4.2b blocks this.

## Gate (additive rung — no ASAN/TLA+/Welch; that's A4.4/later)
slab `cargo nextest run --release` (+new tests, byte-identical); index `--features index-gc` (+new tests);
index conformance `FANOUT_DEPTH=0 INDEX_GC_REPORT=1 mtt-conformance --strict` → 483/221/40 byte-identical,
cycles>0; build both arms (confirm the return-type changes compile at all 13 sites); 0 new warnings. Commit at green.
