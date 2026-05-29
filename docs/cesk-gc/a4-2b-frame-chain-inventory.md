# A4.2b — `frame_chain` K-spine inventory (execution map)

CESK GC migration, Phase A4.2b. Replaces the `frame_chain.rs` native-stack-rooting
mechanism with **typed thread-locals** read structurally as part of the K register:
`SUSPENDED_ACTIVATIONS` (nested trampoline K-spine) + `LIVE_VM_STACK` (bytecode-VM leaf).
Cross-thread transport (parallel dispatch) stays as-is (already Arc-pinned, not on the
Rust stack). Mapped by Explore agent 2026-05-29 (read-only). All sites verified by file:line.

## Mechanism (`src/backend/eval/frame_chain.rs`)
- Thread-local: `FRAME_CHAIN_HEAD: Cell<*const EvalFrame>` (intrusive singly-linked list).
- Node (`EvalFrame`, ~40 B): `parent: *const EvalFrame`, `root_data: *const ()` (type-erased),
  `root_collector: RootCollectorFn` (`unsafe fn(*const (), &mut Vec<MettaValue>)` — no `dyn`),
  `label: FrameLabel` (diagnostic).
- RAII guard `EvalFrameGuard` (Box-wrapped): push in ctor, pop in `Drop`.
- Collection: `collect_frame_chain_roots()` walks the chain, calling each `root_collector`.
- Zero-copy: every `root_data` pointer names a **stack local** (`&work_stack`, `&_tramp_roots`,
  `&*root_frame`); drop order guarantees the data outlives its guard. The module doc (lines ~5-11)
  confesses the nested-safepoint UAF: a nested `eval_trampoline` leaves the *caller's* live values
  invisible to GC unless re-registered — the precise hazard CESK structural roots eliminate by theorem.

## The 13 active push sites

| # | Site (file:line) | Function | Label | Protects |
|---|---|---|---|---|
| 1 | eval_loop.rs:3329 | `eval_trampoline_inner` | `Eval` | **CORE** S/C: `TrampolineFrameRoots`→ `&work_stack: Vec<WorkItem>` + `&continuations: Vec<Continuation>` (via `collect_trampoline_frame_roots`) |
| 2 | modules.rs:130 | `eval_include_generic` | `Include` | `Vec<MettaValue>` compiled file exprs, live across nested `eval_trampoline` |
| 3 | modules.rs:333 | `eval_import_generic` | `Import` | `Vec<MettaValue>` compiled module exprs, live across nested `eval_trampoline` |
| 4 | testing_ops.rs:146 | `eval_test_generic` | `AssertEqual` | test s-expr items `Vec<MettaValue>` |
| 5 | testing_ops.rs:235 | `eval_assert_equal_generic` | `AssertEqual` | assertEqual s-expr (both arg pointers) |
| 6 | testing_ops.rs:281 | `eval_assert_alpha_equal_generic` | `AssertAlphaEqual` | assertAlphaEqual s-expr |
| 7 | vm/mod.rs:1210 | `GenericBytecodeVM::with_vm_roots_frame` | `BytecodeVm` | whole VM: value_stack/locals/current_bindings/results/choice_points/call_frame/collapse_frames |
| 8 | vm/mod.rs:8564 | `eval_sub_expr_vm` | `BytecodeVm` | VM registered before nested trampoline |
| 9 | vm/mod.rs:8660 | `eval_sub_expr_vm_all_with_bindings` | `BytecodeVm` | VM registered before nested trampoline |
| 10 | vm/mod.rs:4050 | `execute_generic_template_with_binding` | `Custom("vm-template-saved-bindings")` | `Vec<MettaValue>` = `saved_current_bindings.iter_full().collect()` |
| 11 | vm/mod.rs:4200 | `execute_generic_foldl_template` | `Custom("vm-foldl-template-saved-bindings")` | `Vec<MettaValue>` foldl saved bindings |
| 12 | eval_loop.rs:2364 | `parallel_branch_eval` | `Custom("parallel-branch")` | `Vec<(MettaValue, SharedBindings)>` + `Arc<Mutex>` results — **cross-thread** |
| 13 | eval_loop.rs:2850 | `parallel_collapse_eval` | `Custom("parallel-collapse")` | `Vec<BoundValue>` + `Arc<Mutex>` results — **cross-thread** |

## Classification → target

- **SUSPENDED_ACTIVATIONS** (nested trampoline K-spine, typed thread-local Vec of activation records):
  #1 (core S/C snapshot — the load-bearing one), #2/#3 (module import/include Vecs), #4/#5/#6
  (testing-op s-exprs). These are caller-held roots live across a *nested* `eval_trampoline`.
- **LIVE_VM_STACK** (bytecode-VM leaf, typed thread-local of live VM operand views):
  #7/#8/#9 (the VM object), #10/#11 (VM template saved-binding Vecs).
- **Cross-thread transport — KEEP (do not migrate in A4.2b)**: #12/#13 parallel dispatch. The roots
  already live in `Arc<Mutex<…>>` on the worker heap, not the spawning thread's Rust stack; these are
  the legitimate `SAFEPOINT_ROOTS`-class transport the plan keeps (narrow).
- **A5 completeness item (re-home before deleting frame_chain.rs)**: #2/#3 (and #4/#5/#6) hold a
  caller `Vec<MettaValue>` of compiled exprs that must be reachable from C or E₀ of the *nested*
  activation. A4.2b makes them visible via SUSPENDED_ACTIVATIONS (additive); A5 must re-home the Vec
  into the nested call's work_stack (C) or a scoped E₀ root so the typed thread-local can be deleted
  with the file. Until then the thread-local mirrors frame_chain exactly.

## A4.2b plan (additive, byte-identical)
1. Add `SUSPENDED_ACTIVATIONS: RefCell<Vec<ActivationRoots>>` + `LIVE_VM_STACK: RefCell<Vec<VmLeafRoots>>`
   thread-locals (new module `cesk/k_spine.rs` or in `roots.rs`), with RAII guards that push/pop in
   lock-step with the existing `EvalFrameGuard` (same scopes).
2. At each migrated site (#1-#11), push the typed record alongside the existing frame_chain guard
   (do NOT remove frame_chain yet). The typed records carry the SAME root data (zero behavior change).
3. Extend the structural reader (`collect_structural` / a new `collect_k_spine`) to read the two
   thread-locals — but keep it additive/unused-in-hot-path until A4.4.
4. A4.3 oracle: assert `collect_structural ∪ collect_global_anchors ∪ collect_k_spine` (multiset)
   ⊇ `collect_frame_chain_roots ∪ registry`. Green-wall, commit.
5. A4.4 flips the safepoint to the structural reader; A5 deletes frame_chain.rs after re-homing #2/#3.

## Verify
Each step byte-identical: slab nextest, index nextest (+the new K-spine tests), index conformance
483/221/40 cycles>0. The typed thread-locals are pushed in the same RAII scopes as frame_chain, so
the live-root set is provably identical (same data, same lifetimes) — the oracle discharges it.
