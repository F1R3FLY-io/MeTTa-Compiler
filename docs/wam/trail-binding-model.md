# WAM Trail-Based Binding Model — Design

**Status:** design complete (2026-05-24); implementation pending (multi-week, 6 increments).
**Goal:** clause-scoped, mutable, trail-backed binding store that makes variable bindings
GLOBAL within a clause activation, eliminating the entire class of "binding dropped by a
values-only cache / per-consumer projection" bugs (the PLN `Direct.metta` tests 2/3
flaky-`$who`-unbound residual).

## Why the trail (the bug the sidecar model cannot beat)

MeTTaTron threads bindings as immutable per-`BoundValue` `GenericBindings` SIDECARS
(`SharedBindings = Arc<GenericBindings<MettaValue>>`, `types.rs:59`). Every value cache in T0
(`cesk/tabling.rs` subgoal table, `EVAL_MEMO`, the normal-form bloom) stores **values only**
and reconstitutes the binding on a hit by re-tagging with the *retrieving* caller's carrying.
The three hit sites are byte-identical (`eval_loop.rs:3375`, `:3516`, `:3600`):
`if cb.is_empty() { bv(value) } else { bv_with(value, cb.clone()) }`. When `cb.is_empty()` on a
hit, the binding (`$who=a`) silently vanishes — and it can be empty for one derivation while
populated for another, producing the *flaky* spurious-unbound `(grandfather $who c)` that
`unique-atom` cannot dedup.

Six commits this session (library resolution fix; `07ee1a1` ReexportLetBindings; `573e58e`
plain-collapse instantiate; `3fe1d2a` Lazy-transparent generic apply_bindings; `d362751` Lazy
descent in the concrete `apply_bindings_inner`; `39d6501` eval-memo namespacing) peeled layers
and got Direct test 1 = `(stv 0.51 …)` ✅ and test 2 to PRODUCE the bound `(grandfather a c)`.
But the sidecar model is a leaky abstraction: each cache hit / projection is a fresh place a
binding can be dropped. The **rejected** lighter alternative (generalize `ReexportLetBindings`
so every hit re-exports the full carrying) only makes the *retriever's* binding survive — it
cannot fix a binding that belongs to a *different* derivation than the retriever.

The trail fixes it at the root: a binding made anywhere in a clause activation lives in a
shared store, not in any value, so no cache or projection can strip it. This is exactly PeTTa's
model (`PeTTa/src/translator.pl:17-64`): clause vars are Prolog logic vars, clause-global by
construction; `reduce/2` unifies into the WAM trail; backtracking unbinds.

## Reference implementation (port this discipline)

The bytecode VM already implements a real trail: `bytecode/vm/mod.rs` `trail` (`:310`),
`TrailEntry::{NewBinding,Rebinding}` (`vm/types.rs:457`), `unwind_trail`/`trail_mark`/`trail_undo`
(`:944-979`), `GenericChoicePoint{trail_height, bindings_stack_height}` (`vm/types.rs:405/425`),
`op_fail` (`:6279`), `op_cut` (`:6453`). On success a deep binding REMAINS in the shared frame
(clause-global); on backtrack the trail unwinds it. Bring this to the T0 trampoline
(`eval/trampoline/`) WITHOUT a tier-crossing call per goal.

## Representation — LAYER, don't replace

Add a clause-scoped trail store ALONGSIDE the existing `GenericBindings` sidecar; keep the
sidecar as the *materialized* binding view at activation boundaries (it is the wire format PLN's
`?` macro destructures via `(Bindings ($var val) …)`, `eval_loop.rs:13780-13837`). The store is
authoritative during evaluation; the sidecar becomes a read-out projection.

```
// new src/backend/eval/trampoline/binding_store.rs
struct BindingStore { cell_of: HashMap<(ScopeId, Name), CellId>, cells: Vec<Cell>, trail: Vec<TrailEntry> }
enum Cell { Unbound { parent: CellId }, Bound { value: MettaValue } }   // union-find + bound term
enum TrailEntry { NewBinding { cell: CellId }, Rebinding { cell: CellId, old: Cell } }  // mirror vm/types.rs:457
```
- Cell key `(ScopeId, BindingName)` reuses the EXISTING scope discipline (`generic_bindings.rs:47/73`):
  query vars at `ROOT_SCOPE`, rule-locals at `dispatch_scope = allocate_scope_id()`
  (`engine.rs:839-843`). `(dispatch_scope,$rule_var) → (ROOT_SCOPE,$who)` is a union edge, not a
  copied entry. `lookup` = union-find resolve + deref along the scope chain `[dispatch_scope, ROOT_SCOPE]`
  — subsumes `apply_chain_generic` (alias chains become path compression).
- `MettaValue` is `Copy` (8-byte GC handle), so cells/trail entries are cheap (as the VM notes).
- Lives as `thread_local! { static BINDING_STORE: RefCell<BindingStore> }` (next to the
  `dispatch_hints.rs:379` cluster), saved/restored at `eval_trampoline_with_carrying` entry/exit
  (`eval_loop.rs:2877-2911`), seeded from `initial_carrying_bindings`, read out into result
  sidecars at the `Complete` arm. Nested trampoline calls get a fresh clause scope (like `FORK_DEPTH`).

**Why layer:** the sidecar is the observable contract at collapse-bind output, `WaitForParallel`
merge, and `EvalResult`; replacing it is a flag-day rewrite that breaks the `(Bindings …)`
round-trip. Materialize the store into a `GenericBindings` only at clause/output boundaries via
the existing `project_bindings_for_consumer_generic` (`bindings.rs:3662`) + freshened-name filter
→ byte-identical downstream. Layering also makes Increment 1 inert.

## Ops + trampoline mapping

`bind(cell,value)` (trail NewBinding|Rebinding then mutate), `union(a,b)` (var-var alias, trail
both), `lookup(scope_chain,name)`, `mark()->usize` (= trail len), `undo_to(mark)` (pop+invert ==
`unwind_trail`). Fork points become choice points: `ProcessAmb`/superpose/`ProcessRuleMatches`
take `mark()` when the CP is pushed; on branch death (empty result, `eval_loop.rs:13954`)
`undo_to(mark)` — exactly `op_fail`. Replaces copy-on-fork (`outer_carrying.clone()` everywhere).
Heap-trampolined: the trail is a heap Vec, marks ride in heap `Continuation` variants, the
existing loop mutates it; union-find `find` is iterative w/ path compression.

## Cache-robustness (the payoff)

Bindings live in the store, not the cached value. The three hit sites collapse to
`result: (cached.into_iter().map(bv).collect(), env)` — **no `cb`, no `bv_with`, no re-tag,
no projection**. Goals resolve vars via `store.lookup` BEFORE computing `value.hash_value()`, so
derivations table under *resolved* sub-terms and the ghost-manufacturing hash collision never
arises. `scope_gen`/`current_memo_tracked_key()` namespacing (the 39d6501 class) becomes dead for
binding-correctness (keep `mutation_epoch` + `query_generation` for impurity/`!`-isolation).

## Parallelism — per-branch store segments (snapshot-at-fork)

`parallel_dispatch` (`eval_loop.rs:2077`) spawns each branch on the work-stealing pool with its
own thread-locals. Each worker gets a FRESH thread-local `BINDING_STORE` seeded from a snapshot of
the parent store at fork (the same snapshot that becomes `branch_bindings` today). Workers mutate
private stores; the join (`WaitForParallel` done-arm `:14017`) reads each branch's materialized
sidecar — UNCHANGED. No shared mutable trail, no new locks (only the existing `results` mutex).
The parent store is read-only during the parallel region (parent parked in the pump). HAMT
(persistent immutable trail) documented as the forward-compatible upgrade if profiling shows
snapshot cost on wide fan-outs.

## PLN-unchanged + PathMap/MORK invariant

Canonical PeTTa-targeting PLN needs NO `.metta` changes (its foldl/collapse/? macros were written
against clause-global bindings). PathMap/MORK/MM2 stays the atom space; the trail is purely the
control/binding layer (records SECK env cells, backtracking unbinds — never touches atom storage).
`enumerate_rules_via_unification_detailed` (`engine.rs:802`) still queries MORK; it writes match
results into the store instead of returning a copied `GenericBindings`.

## Increment plan (smallest-first, each baseline-preserving + committable)

Each preserves nextest 4209/4209, mtt-conformance --strict 480, M11-pt 220, M11-he 40, Robot ~9.7s.
Nextest harness: `systemd-run -p MemoryMax=96G -p CPUQuota=1800%` + full parallelism (~7s run +
~2.5min compile; lower caps SIGTERM during compile — NOT a hang). Build cap 24G. The PLN bug is
FLAKY, so behavior-flip increments MUST gate on a multi-run stability assertion modeled on
`tests/ghost_branch_regression.rs:561` (`conjunction_ghost_elimination_deterministic_20_runs`).

1. **Inert trail scaffolding** — new `binding_store.rs` (store + ops + `snapshot_scoped`/`seed_from`)
   + thread-local + install in `eval_trampoline_with_carrying`, READ BY NOTHING. Unit tests +
   `debug_assert!` that `snapshot ⊇ output sidecar keys`. Baselines byte-identical. Rollback: delete module.
2. **Flip subgoal-tabling hit** (`eval_loop.rs:3516`) — write match bindings into the store at
   `engine.rs:842-918`; resolve goal vars via `lookup` before `tabling_hash`; hit returns `map(bv)`.
   STABILIZES PLN Direct test 2. Gate: new `pln_direct_test2_grandfather_stable_20_runs`.
3. **Flip eval-memo + normal-form hits** (`:3600`, `:3375`) — same transform; resolve-before-hash.
   Whole "values-only cache re-tags carrying" pattern gone.
4. **Convert fork points to mark/undo** — add `trail_mark` to `ProcessAmb`/superpose/`ProcessRuleMatches`;
   `undo_to` on branch death; delete the per-alt clone+compose+project. WAM discipline proper.
5. **Parallel per-branch store segments** — snapshot seed at `parallel_dispatch`; per-worker store install.
6. **Retire dead namespacing** — remove `current_memo_tracked_key()` XOR + `scope_gen` binding-visibility
   (keep `mutation_epoch`/`query_generation`). Small speedup. Pure deletions.

## Critical files
- `src/backend/eval/trampoline/eval_loop.rs` (hit sites :3375/:3516/:3600; forks `ProcessAmb` :13908,
  `WaitForParallel` :13980; `parallel_dispatch` :2077; activation boundary :2866; collapse-bind
  materialization :13780)
- `src/backend/bytecode/vm/mod.rs` + `vm/types.rs` (reference trail — port into new `binding_store.rs`)
- `src/backend/eval/cesk/tabling.rs` (values-only contract :92-126 — becomes binding-neutral)
- `src/backend/eval/trampoline/engine.rs` (rule-match binding production :802-918; scope retag :839-843)
- `src/backend/models/generic_bindings.rs` (`ScopeId`/`ROOT_SCOPE`/`allocate_scope_id` :47-63 — cell key)
- `src/backend/eval/trampoline/dispatch_hints.rs` (thread-local cluster :379 — where BINDING_STORE goes)
- `tests/ghost_branch_regression.rs:561` (multi-run stability template for every flip increment)
- reference-only model: `PeTTa/src/translator.pl:17-64`
