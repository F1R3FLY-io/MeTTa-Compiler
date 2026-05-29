# A3 / A4 / A5 — Executable Implementation Design (genuine-CESK foundation)

Authoritative design for the CESK-foundation core of the genuine-CESK GC migration
(plan: `~/.claude/plans/help-me-complete-the-shimmying-mochi.md`). Produced by a Plan
agent (2026-05-29) over the verified root inventory (`roots-apparatus-extent-a3a4a5.md`).
Goal: **GC = σ|_Reachable(⟨C,E,K⟩) read STRUCTURALLY from the reified machine**; delete
the `ROOT_REGISTRY` / `frame_chain` / `RootProvider` *discovery* apparatus. Hybrid-K
(deterministic K stays native `Vec<Continuation>`).

## Ground-truth corrections (verified against source)

- **C-1 (reshapes A5): `SAFEPOINT_ROOTS`/`register_temporary_roots` is the cross-thread
  TRANSPORT of the trampoline's structural C∪K roots to the slab GC thread**, not a mere
  "transient anchor." Flow: safepoint builds `root_set` (eval_loop.rs:3497 `collect_all`
  = S∪C∪K) → `ctx.perform_safepoint(root_set.drain_into_vec())` (eval_loop.rs:3616) →
  `register_temporary_roots` + `request_gc()` (context.rs:78-80/451-475,
  session_context.rs:236-240) → GC thread's `collect_all_roots()` appends via
  `collect_safepoint_roots()` (gc_allocator.rs:3712). ⇒ KEEP the mechanism (rename →
  publication buffer, narrow contract to structural-reader output); the
  "GC-thread-reads-the-machine-directly" alternative is REJECTED (needs frame_chain-for-GC).
- **C-2 (lines):** A1 safepoint = eval_loop.rs:3480-3655; `RootSet::collect_all` =
  roots.rs:190 (already the live reader); env seam `collect_roots_into` = core.rs:2168
  (trait delegate :2274, committed 2749caa); `with_vm_roots_frame` = vm/mod.rs:1181,
  VM-leaf sites eval_loop.rs:8564/8660; `collect_roots_into` (VM) = vm/mod.rs:1049.
- **C-3: A3 is effectively a no-op** — E₀ is already one shared Arc collected once;
  E_local = `carrying_bindings`/`GenericBindings` already inside C/K frames and already
  walked by `collect_values`. The C/E_local/E₀ decomposition is physically present
  already. ⇒ A3 = a pin/test commit (or fold into A4's first commit); NO 76-variant churn.
- **C-4: TWO collectors** with different root assembly — slab (providers ∪ SAFEPOINT_ROOTS,
  GC thread) and index (quiescence: `collect_all_roots()` only, C∪K empty post-EvalGuard;
  midloop: `collect_all_roots()` ∪ `root_set`). The oracle must green-wall both arms.

## A3 — formalize E₀ (1 commit, byte-identical, or fold into A4.1)
Pin the two invariants A4 relies on: (i) C∪K carries E_local structurally; E₀ read once via
`collect_roots_into`; the 73 `env` fields are E₀ aliases deliberately NOT walked by
`collect_values` (would multiply-count). (ii) Add a `#[cfg(debug_assertions)]` test that a
`Continuation`'s `env` contributes nothing to `collect_values` (formal statement of C-3).
Files: roots.rs, state.rs. Gate: nextest both arms + conformance; no ASAN. **No `E_local`/`E₀`
struct surgery in the 76 variants.**

## A4 — structural reader + VM-as-K-leaf + equivalence oracle (superset-first, additive)
Every A4 sub-increment ADDS the reader/oracle alongside the apparatus; nothing deleted until A5.

- **A4.1 (byte-identical):** add `RootSet::collect_structural` in roots.rs —
  `collect_all(S,C,K)` + `env0.collect_roots_into(...)` + per-VM-leaf `vm.collect_roots_into(...)`.
  Does NOT call frame_chain / collect_all_roots / caches. Unit-test it against synthetic ⟨C,E,K⟩.
  Reader added, not yet authoritative. Gate: nextest+conformance both arms.
- **A4.2 (byte-identical, additive):** replace the frame-chain content with TWO typed
  thread-locals (the structural K-spine): `SUSPENDED_ACTIVATIONS` (Vec of
  `(*const Vec<WorkItem>, *const Vec<Continuation>)` per suspended trampoline activation,
  replacing `TrampolineFrameRoots` eval_loop.rs:3093-3115/3318-3337) and `LIVE_VM_STACK`
  (Vec of `*const VM`, replacing `with_vm_roots_frame` → `with_vm_leaf` at vm/mod.rs:1181 +
  eval_loop.rs:8564/8660). Push/pop RAII, same cost. **Conceptual key:** these typed
  thread-locals are the *structural, by-type* representation of the native-stack K-spine
  (suspended C∪K + live VM) that FRAME_CHAIN_HEAD held as untyped discovery — the CESK
  property ("roots = registers read by type") holds; the type-erased `RootCollectorFn`/
  `push_custom` is what gets deleted. Also add `collect_global_anchors(out)`: direct by-name
  calls to the fixed set of global σ-value holders (bytecode cache, tiered cache, space
  registry, compiler atoms, MettaState output/result_bindings, + the env owner-edge),
  replacing the `Weak<dyn RootProvider>` registry. Caches (eval_memo/match/subgoal/thunk),
  deferred-env-drops (fold into E₀ anchor set), parallel-dispatch outputs (already inline at
  pump sites) are classified per the inventory. Push to the typed thread-locals IN ADDITION
  to frame_chain (both run); structural reader consumed only by the oracle.
- **A4.3 (byte-identical in release; the CENTRAL PROOF):** the machine-equivalence ORACLE.
  At every safepoint (eval_loop.rs after :3524; index quiescence eval/mod.rs:267,
  tier_forced.rs:285; slab collect_all_roots gc_allocator.rs:3686), `#[cfg(debug_assertions)]`
  assert `OLD ⊆ NEW` as a sorted `inner_ptr()` multiset (dedup both sides) — OLD = the
  existing assembled root set (collect_all + frame_chain + caches + deferred + collect_all_roots),
  NEW = `collect_structural` + `collect_global_anchors` + the typed thread-locals.
  **Superset (OLD ⊆ NEW) is the safety direction** (no protected root dropped); over-approx is
  safe. On failure, dump `OLD \ NEW`. Run across the FULL conformance corpus in a DEBUG build +
  debug nextest both arms. **KEEP as a PERMANENT CI invariant.** This green-walls A5.
- **A4.4 (BEHAVIOR-CHANGING — ASAN):** flip the safepoint to FEED the collector from
  `collect_structural` + `collect_global_anchors` (authoritative), replacing the OLD assembly.
  Oracle stays live (debug). Gate: capped `-Zbuild-std` ASAN forced-cycle (slab + index, low
  MIN_BYTES, FANOUT=0) 0-UAF + 20-run determinism + mmverify + HE-bisim 40/40 + PLN budgets.

## A5 — delete the discovery apparatus (each step build-green)
Precondition: A4.3 oracle green across corpus AND A4.4 ASAN-green.

- **A5.1 (FIRST; ASAN) — re-home the module-import Vec.** frame_chain protects a caller-held
  `Vec<MettaValue>` of compiled import/assert expressions (modules.rs:132/331,
  testing_ops.rs ×8, vm/mod.rs:4049/4184) that is NOT in C/K/E₀ today. Re-home into a typed
  control thread-local (`PENDING_EXPR_VECS` or a `SUSPENDED_ACTIVATIONS` arm) — these are the
  outer driver's control (C). MUST precede frame_chain deletion (else UAF). ASAN: forced cycle
  during a nested `include`/`import!`.
- **A5.2 (a byte-identical, b ASAN-FANOUT>0) — delete frame_chain.** After A5.1, nothing feeds
  from FRAME_CHAIN_HEAD. Delete in order: safepoint `collect_frame_chain_roots` call
  (eval_loop.rs:3504), then the parallel sites (184/2651/2752 → structural thread-locals,
  ASAN FANOUT>0), `with_vm_roots_frame`/`vm_roots_collector` (vm/mod.rs), nested-chunk pushes
  (4049/4184), `TrampolineFrameRoots`+`collect_trampoline_frame_roots`, then `frame_chain.rs`
  (whole file) + imports + `tools/gc-root-audit`.
- **A5.3 (a byte-identical, b ASAN-index-quiescence) — delete ROOT_REGISTRY.** Extract the 11
  `RootProvider::collect_roots` bodies into inherent `collect_*_into` methods (env already done),
  repoint every `collect_all_roots()` (gc_allocator.rs:4273/4293/5116, iterative.rs:3675,
  eval/mod.rs:267/357, tier_forced.rs:285) → `collect_global_anchors()` + the env owner-edge,
  then delete `trait RootProvider`/`ROOT_REGISTRY`/`register_root_provider`/`collect_all_roots*`
  + the 10 register sites + the 2 ParallelDispatch/Collapse providers (redundant with inline pump
  collection).
- **A5.4 (ASAN slab) — rename/narrow SAFEPOINT_ROOTS** to the structural-root publication
  buffer (per C-1): keep `SafepointRootHandle` + the parking-lot Vec; the only writes are
  `collect_structural` output + `collect_global_anchors` + the cross-eval cache/accumulator
  handles (the driver's C). Forbid future "discovery" writes (doc invariant + the permanent oracle).

## Highest-risk step + sequencing hazards
- **HIGHEST RISK: A5.3b — replacing the Weak `ROOT_REGISTRY` env-discovery with a structural
  owner-edge at QUIESCENCE.** Must enumerate EVERY transiently-live env (forks ×5 core.rs sites;
  deferred_shared_drops in flight; parallel-worker envs under slab). A missed env → UAF.
  Mitigation: the oracle MUST run at the quiescence sites comparing owner-edge vs the live Weak
  registry across a deferred-drop-heavy AND a fork-heavy workload BEFORE A5.3b deletes the registry.
- **Sequencing:** (1) A5.1 before A5.2 (import Vecs protected only by frame_chain today).
  (2) A4.3 oracle green before ANY A5 deletion. (3) A4.4 (feed from NEW) before A5.2-3 (delete OLD)
  — A4.4 is the one place the live root source changes; it absorbs the ASAN gate alone.
  (4) extract inherent collectors (A5.3a) before deleting the trait (A5.3b). (5) the oracle's OLD
  baseline migrates as OLD shrinks (keep a debug recomputation of the soon-to-be-deleted functions,
  delete it in the same commit; graduate to NEW-vs-mark-closure). (6) two collectors → two ASAN arms
  (slab default + index `--features index-gc` low MIN_BYTES; FANOUT>0 for parallel-path hunks).

## Commit boundaries (each leaves the tree green)
A3.1 pin invariants (byte-id) · A4.1 reader (byte-id) · A4.2 typed K-spine + global-anchors
(byte-id) · A4.3 oracle debug-only (byte-id release) · A4.4 feed-from-structural (ASAN) ·
A5.1 re-home import Vecs (ASAN) · A5.2a/b delete frame_chain (a byte-id / b ASAN FANOUT>0) ·
A5.3a/b delete ROOT_REGISTRY (a byte-id / b ASAN quiescence) · A5.4 publication buffer (ASAN slab).

## Critical files
- `src/backend/eval/cesk/roots.rs` (collect_all:190; new collect_structural — A4.1)
- `src/backend/eval/trampoline/eval_loop.rs` (safepoint 3480-3655; TrampolineFrameRoots
  3093-3115/3318-3337; pump roots 2651/2752; worker_cooperative_safepoint 174-194; VM-leaf
  8564/8660; oracle hook + A4.4 flip)
- `src/backend/models/gc_allocator.rs` (ROOT_REGISTRY/RootProvider/register_root_provider/
  collect_all_roots* 3637-3866; register_temporary_roots/SAFEPOINT_ROOTS/SafepointRootHandle
  3771-3821; GC-thread consumers 4273/4293/5116 — A5.3/A5.4 + collect_global_anchors)
- `src/backend/bytecode/vm/mod.rs` (collect_roots_into:1049 kept; with_vm_roots_frame/
  vm_roots_collector 1181/~132 → with_vm_leaf; nested-chunk 4049/4184)
- `src/backend/environment/core.rs` (collect_roots_into:2168 E₀ reader done; trait delegate 2274
  to delete A5.3; 5 fork/register sites — owner-edge for A5.3b)
- delete: `src/backend/eval/frame_chain.rs` (A5.2); re-home: `modules.rs:132/331`,
  `testing_ops.rs` ×8, `eval/mod.rs:112-126` (A5.1/A5.4); `context.rs:78/451`,
  `session_context.rs:236` (perform_safepoint→publication A5.4); `state.rs:330` (embryo oracle A4.3).
