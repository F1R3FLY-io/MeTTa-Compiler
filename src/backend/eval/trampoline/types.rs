//! Trampoline Types for Iterative Evaluation
//!
//! Monomorphized types for the trampoline-based evaluator. These types use
//! concrete `MettaValue` and `MettaEnvironment` types directly, eliminating
//! generic type parameter overhead and enabling Copy semantics for MettaValue
//! (8-byte tagged pointer).
//!
//! ## Design Notes
//!
//! - `WorkItem`, `Continuation`, `EvalResult` are concrete
//! - MettaValue is Copy — all `.clone()` calls are zero-cost 8-byte memcpy
//! - Bindings use `GenericBindings<MettaValue>` (heap-allocated binding map)
//! - Names retain the `Generic` prefix for now; renaming is a separate step

use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::Arc;
use std::sync::Mutex;

use smallvec::SmallVec;

use crate::backend::eval::cesk::coroutine::CancelToken;
// A5.6: frame_chain is cfg-walled to the slab build (`#[cfg(not(feature = "index-gc"))]`
// on `mod frame_chain` at eval/mod.rs). The `EvalFrameGuard` import — and the two
// `_root_guard` fields it types below — are therefore slab-only. In the index build the
// parallel-dispatch handles carry no frame_chain pop handle (the parallel push sites are
// walled too, and `note_worker_spawned()` closes the single-threaded collector gate the
// instant such a handle could exist — see A5.6 audit).
#[cfg(not(feature = "index-gc"))]
use crate::backend::eval::frame_chain::EvalFrameGuard;
use crate::backend::grounded::GroundedState;
use crate::backend::models::{GenericBindings, MemoHandle, MettaValue};
// SpaceHandle was previously used by ProcessAddAtomAtom and ProcessRemoveAtomAtom,
// which are now disabled (see comments on those variants below).

// Import Cartesian product iterator
use super::super::processing::GenericCartesianProductIter;

use super::context::SharedEnv;

/// Per-result bound value: a produced `MettaValue` paired with the
/// variable bindings that were active when it was produced. Mirrors MeTTa
/// HE's `InterpretedAtom = (Stack, Bindings)` — bindings travel with each
/// nondeterministic alternative through the evaluation pipeline.
///
/// For the vast majority of evaluations (all code outside an active
/// `collapse-bind` scope), the bindings are `GenericBindings::Empty` — a
/// cheap inline representation with no heap allocation. Within a
/// `collapse-bind` scope, the bindings reflect the tracked-variable
/// groundings established during that specific branch's rule unification.
pub type BoundValue = (MettaValue, GenericBindings<MettaValue>);

/// Layer C: `SharedBindings = Arc<GenericBindings<MettaValue>>`.
///
/// Carrying-bindings are shared across parallel fork branches and copied
/// into each spawned continuation. Under `Box`, each clone allocates a new
/// `GenericBindings` (O(N) in key count); across mmverify's ~200-deep
/// nested dispatches this creates O(N²) heap churn — the root cause of the
/// 41 GB memory spike and 90× slowdown previously attributed to Stage
/// 1d-revised.
///
/// `Arc` clones are O(1) reference-count bumps. Mutations of the
/// carrying-bindings (fewer than a dozen sites — rotating to a sibling
/// branch's bindings) become `shared = Arc::new(new_bindings)` rather than
/// in-place `*ptr = ...`. Reads/derefs (`&*shared`, `shared.iter()`) are
/// unchanged since `Arc<T>: Deref<Target=T>`.
pub type SharedBindings = Arc<GenericBindings<MettaValue>>;

/// Layer C: cached empty-bindings singleton. Every trampoline continuation
/// and work item that does not carry per-branch bindings clones this value,
/// turning a frequent allocation into a refcount bump. The constant is
/// wrapped in `OnceLock` so initialization is lazy and safe across threads.
#[inline]
pub fn empty_shared_bindings() -> SharedBindings {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<SharedBindings> = OnceLock::new();
    EMPTY
        .get_or_init(|| Arc::new(GenericBindings::new()))
        .clone()
}

/// Evaluation result: (bound_values, environment)
///
/// Each element of the `SmallVec` is a `(value, bindings)` pair: the value
/// produced by a nondeterministic branch and the bindings accumulated
/// along that branch's rule-matching chain. The parallel structure across
/// branches is what enables `collapse-bind` to emit correct per-branch
/// bindings in its output.
///
/// Uses SmallVec<[BoundValue; 2]> to inline up to 2 elements, avoiding
/// heap allocation for the common single-result case (93%+ of evaluations
/// produce 1 result). The environment is Arc-wrapped for O(1) sharing
/// across continuations and work items.
pub type EvalResult = (SmallVec<[BoundValue; 2]>, SharedEnv);

/// Helper: construct a `BoundValue` from a `MettaValue` with empty bindings.
/// Used by the common no-collapse-bind path where every result has trivial
/// (empty) bindings.
#[inline(always)]
pub fn bv(value: MettaValue) -> BoundValue {
    (value, GenericBindings::new())
}

/// Helper: construct a `BoundValue` with specific bindings.
#[inline(always)]
pub fn bv_with(value: MettaValue, bindings: GenericBindings<MettaValue>) -> BoundValue {
    (value, bindings)
}

/// Helper: extract just the values (drop bindings) from a bound result set.
/// Used by code paths that don't need per-result bindings.
#[inline]
pub fn values_of(results: &SmallVec<[BoundValue; 2]>) -> SmallVec<[MettaValue; 2]> {
    results.iter().map(|(v, _)| v.clone()).collect()
}

/// Merge mode for `WaitForParallel` continuation result composition.
///
/// Different call sites of `parallel_dispatch` merge results differently:
/// - `dispatch_rule_matches` composes per-branch bindings with the outer
///   carrying bindings.
/// - `StartAmb` / `ProcessLet` simply concatenate per-branch results
///   without re-composing.
#[derive(Debug, Clone, Copy)]
pub enum ParallelMergeMode {
    /// Rule-match dispatch: compose each branch's bindings with `outer_carrying`.
    RuleMatch,
    /// Amb / superpose / let parallel-body dispatch: concatenate results into
    /// `base_results` without re-composing outer bindings.
    AmbConcat,
}

/// Merge mode for `WaitForParallelCollapse` continuation.
///
/// - `Plain`: produce a single tuple of all collected branch values.
/// - `Bind`: per-branch `(value (Bindings ...))` sidecar encoding used by
///   `collapse-bind`.
#[derive(Debug, Clone, Copy)]
pub enum CollapseMergeMode {
    Plain,
    Bind,
}

/// Per-call mutable bookkeeping for the trampolinized parallel-dispatch wait.
///
/// Tracks the previous `remaining` snapshot and consecutive-stall count so
/// the WaitForParallel arm can spawn overflow workers if no progress is
/// observed across several pump ticks (mirrors the original `stall_count`
/// state in `parallel_branch_eval`'s synchronous wait loop).
#[derive(Debug, Clone, Copy, Default)]
pub struct StallState {
    pub prev_remaining: u32,
    pub stall_count: u32,
    pub overflow_requested: bool,
}

/// Handle for a trampolinized parallel-branch dispatch.
///
/// Carries every piece of state that the synchronous `parallel_branch_eval`
/// wait loop previously kept on its stack frame. The handle is owned by a
/// `Continuation::WaitForParallel` variant; when that continuation is
/// consumed (at completion or cancellation), the handle drops — releasing
/// the budget, popping the frame-chain entry via `_root_guard.Drop`, and
/// freeing the `Box<ParallelBranchRootFrame>` whose raw pointer the guard
/// held.
///
/// **Thread-safety**: the handle is `Send + Sync` so it can be wrapped in
/// `Arc<ParallelDispatchHandle>` and registered with the global GC
/// `ROOT_REGISTRY` as a `RootProvider`. This is REQUIRED for correctness:
/// worker branches (running on the eval work-pool) write their results
/// into `results[slot]` between parent pump-ticks. The parent's
/// `frame_chain` registration is thread-local and invisible to GC pool
/// workers running on different threads; without the global root provider,
/// a GC cycle triggered while the parent is parked on `cvar.wait_timeout`
/// would not see the worker writes → mark-sweep frees them → SIGSEGV /
/// SIGBUS on next dereference. The Sync requirement is satisfied by using
/// `AtomicU64` / `Mutex` for the previously-`Cell` fields.
pub struct ParallelDispatchHandle {
    /// Per-branch result slots: `Arc<Mutex<Vec<Option<Vec<BoundValue>>>>>`.
    /// Each spawned branch closure writes its slot when complete.
    pub results: super::eval_loop::ParallelEvalResults,
    /// Number of branches still in-flight. Decrements to zero when all
    /// branches have completed (or been cancelled).
    pub remaining: Arc<AtomicU32>,
    /// `(done_flag, condvar)` pair used to wake the wait loop when a branch
    /// completes. The flag is set when `remaining` reaches zero.
    pub done_pair: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    /// Cooperative-cancellation token. For `Demand::Exactly(N)`, the first
    /// satisfying branch flips the token; sibling workers observe it at
    /// their next safepoint and bail with `BranchCancelled`.
    pub cancel_token: Arc<CancelToken>,
    /// Total number of branches dispatched (informational; equal to the
    /// initial `remaining` value).
    pub num_branches: usize,
    /// Heap-allocated frame whose raw pointer was registered with the
    /// frame_chain. The `_root_guard` field holds the LIFO pop handle;
    /// both must drop together (guard first, then box) so the pop happens
    /// before the backing allocation is freed.
    ///
    /// Visibility is `pub(crate)` because `ParallelBranchRootFrame` itself
    /// is crate-private — external callers can't name the type anyway.
    /// `allow(dead_code)`: the field's purpose is to keep the heap box alive
    /// for the lifetime of `_root_guard` (which holds a raw pointer to it);
    /// the box is never read directly after construction.
    #[allow(dead_code)]
    pub(crate) root_frame: Box<super::eval_loop::ParallelBranchRootFrame>,
    /// RAII handle that pops the frame_chain entry on drop. Stored as
    /// `Option` so it can be `.take()`-ed for early release if needed.
    /// **Drop order**: declared BEFORE `root_frame` so it drops first.
    ///
    /// A5.6: slab-only. The index build has no frame_chain module, and the
    /// `push_custom` that produces this guard (eval_loop `parallel_dispatch`)
    /// is cfg-walled in lock-step.
    #[cfg(not(feature = "index-gc"))]
    pub _root_guard: Option<EvalFrameGuard>,
    /// Snapshot of the global allocation counter at the start of the
    /// last cooperative GC drop. Used by the WaitForParallel pump to gate
    /// periodic guard drops.
    pub started_at_alloc_count: AtomicU64,
    /// Per-call stall-detection state. Mutex contention is zero in
    /// practice: only the pump-driver thread reads/writes this.
    pub stall_state: Mutex<StallState>,
    /// Strong reference to the GC root provider registered with the
    /// global `ROOT_REGISTRY` at dispatch construction. Drops when the
    /// handle drops (on the trampoline thread), at which point the
    /// `Weak` in the registry is auto-pruned on the next root walk.
    /// See `ParallelDispatchRootProvider`'s doc for the race this closes.
    #[allow(dead_code)]
    pub(crate) _root_provider_arc: Arc<ParallelDispatchRootProvider>,
    /// Phase 10.A — Stage 1e closure: tracked-vars hint captured from the
    /// parent thread's `BINDING_CAPTURE_STACK` at dispatch construction.
    /// Workers re-establish a shadow capture frame from this hint so
    /// `in_collapse_bind_scope()` returns `true` on the worker side and
    /// `active_tracked_vars()` matches the parent's union. None when no
    /// collapse-bind is active on the parent.
    ///
    /// Threading this through unblocks parallel dispatch under the
    /// dominant `in_collapse_bind_scope() → 0` veto (eval_loop.rs:408)
    /// which previously forced all four dispatch sites sequential inside
    /// PLN's `(let $derivations (collapse ...) ...)` body.
    pub tracked_vars_hint: Option<Arc<SmallVec<[&'static str; 4]>>>,
}

impl std::fmt::Debug for ParallelDispatchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelDispatchHandle")
            .field("num_branches", &self.num_branches)
            .field(
                "remaining",
                &self.remaining.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("stall_state", &self.stall_state.lock().ok().map(|g| *g))
            .finish()
    }
}

/// GC root provider for an active parallel-dispatch.
///
/// Holds only the Send+Sync portion of `ParallelDispatchHandle` (the
/// `results` Mutex). Registered with the global `ROOT_REGISTRY` at
/// dispatch creation; kept alive by `WaitForParallel._root_provider`
/// for the dispatch's lifetime.
///
/// **Why a separate struct instead of `impl RootProvider for
/// ParallelDispatchHandle`**: `ParallelDispatchHandle` contains
/// `_root_guard: EvalFrameGuard`, which is `!Sync` because it holds a
/// raw pointer into the thread-local frame_chain that MUST be popped on
/// the same thread that pushed it. Making the handle `Send + Sync` and
/// wrapping it in `Arc` would let the GC pool worker (different thread)
/// hold the last strong reference and run `Drop` cross-thread → wrong
/// frame_chain popped → memory corruption. Separating the GC-visible
/// data (`results` Arc — already Sync) keeps thread-bound state on the
/// trampoline thread while still exposing roots to mark-sweep.
///
/// **Why `try_lock` (not `lock`)**: if a worker holds `results` while
/// writing, the worker is by construction holding its `EvalGuard`, so
/// `ACTIVE_EVALUATORS >= 1`, so quiescent mark-sweep GC cannot start.
/// Roots skipped during that exact moment are guaranteed-safe to skip —
/// the worker's `EvalGuard` already inhibits the collection that would
/// otherwise observe a stale snapshot.
#[derive(Debug)]
// A5.3: the index-gc build cfg-walls the `RootProvider` impl (it registers ZERO
// providers — roots are structural), so these fields are read only in the slab
// build. The struct is still constructed and held alive via the dispatch handle's
// `_root_provider_arc` field (byte-identical construction in both builds), so it is
// intentionally dead-but-present in the index regime.
#[cfg_attr(feature = "index-gc", allow(dead_code))]
pub struct ParallelDispatchRootProvider {
    pub(crate) results: super::eval_loop::ParallelEvalResults,
    /// Stable snapshot of worker INPUTS for the dispatch lifetime.
    /// Shares the same Arc as `WaitForParallel.stable_branches_snapshot`
    /// (one allocation, two strong references). Closes the worker-INPUT
    /// vs GC-pool-walker race documented in Phase 8 (Robot.metta SIGSEGV
    /// at can_compile_with_env + MettaValue::serialize):
    ///
    /// - The worker closure (`eval_loop.rs:1704`) captures
    ///   `branch_expr: MettaValue` by-move from this Vec's contents.
    /// - The parent's `WaitForParallel.stable_branches_snapshot`
    ///   references the same Arc, but is rooted only via thread-local
    ///   `frame_chain` (invisible to GC pool workers) plus per-safepoint
    ///   `register_temporary_roots` (only fires on gc_pending /
    ///   delta_crossed). Between pump ticks, no global root references
    ///   the inputs.
    /// - Registering this provider with `ROOT_REGISTRY` makes the input
    ///   branches globally visible to mark-sweep for the dispatch's
    ///   full lifetime, closing the race.
    ///
    /// Walked without a lock — `Arc<Vec<…>>` is `Sync` when contents
    /// are `Sync` (`ParallelBranch = (MettaValue, SharedBindings)` —
    /// both Copy/Sync) and the Vec is immutable across the dispatch
    /// (built once in `parallel_dispatch`, never mutated). Contrast
    /// `results: Mutex<…>` which needs `try_lock` because workers
    /// actively write.
    pub(crate) branches: Arc<Vec<super::eval_loop::ParallelBranch>>,
}

#[cfg(not(feature = "index-gc"))]
impl crate::backend::models::gc_allocator::RootProvider for ParallelDispatchRootProvider {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        // INPUTS first — no Mutex, direct iter (immutable Arc).
        for (value, bindings) in self.branches.iter() {
            roots.push(*value);
            for (_, bound) in bindings.iter() {
                roots.push(*bound);
            }
        }
        // OUTPUTS — try_lock; safe to skip if contended (worker holds
        // EvalGuard while writing, inhibiting quiescent GC anyway).
        if let Ok(guard) = self.results.try_lock() {
            for slot in guard.iter().flatten() {
                for (v, bindings) in slot.iter() {
                    roots.push(*v);
                    for (_, bound) in bindings.iter() {
                        roots.push(*bound);
                    }
                }
            }
        }
    }
}

/// Handle for a trampolinized parallel-collapse dispatch.
///
/// Mirrors `ParallelDispatchHandle` for the `parallel_collapse_eval` path.
/// Differs in that the root frame stores `items: Vec<BoundValue>` instead
/// of `branches: Vec<ParallelBranch>`.
pub struct ParallelCollapseDispatchHandle {
    pub results: super::eval_loop::ParallelEvalResults,
    pub remaining: Arc<AtomicU32>,
    pub done_pair: Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    pub cancel_token: Arc<CancelToken>,
    pub num_branches: usize,
    /// `pub(crate)` because `ParallelCollapseRootFrame` is crate-private.
    /// `allow(dead_code)`: held alive for the lifetime of `_root_guard`.
    #[allow(dead_code)]
    pub(crate) root_frame: Box<super::eval_loop::ParallelCollapseRootFrame>,
    /// A5.6: slab-only (see `ParallelDispatchHandle::_root_guard`).
    #[cfg(not(feature = "index-gc"))]
    pub _root_guard: Option<EvalFrameGuard>,
    pub started_at_alloc_count: AtomicU64,
    pub stall_state: Mutex<StallState>,
    /// See `ParallelDispatchHandle::_root_provider_arc`.
    #[allow(dead_code)]
    pub(crate) _root_provider_arc: Arc<ParallelCollapseRootProvider>,
    /// See `ParallelDispatchHandle::tracked_vars_hint` (Phase 10.A).
    pub tracked_vars_hint: Option<Arc<SmallVec<[&'static str; 4]>>>,
}

impl std::fmt::Debug for ParallelCollapseDispatchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParallelCollapseDispatchHandle")
            .field("num_branches", &self.num_branches)
            .field(
                "remaining",
                &self.remaining.load(std::sync::atomic::Ordering::Relaxed),
            )
            .field("stall_state", &self.stall_state.lock().ok().map(|g| *g))
            .finish()
    }
}

/// GC root provider for an active parallel-collapse dispatch. See
/// `ParallelDispatchRootProvider` for the design rationale (the same
/// reasoning applies — collapse handles also carry `EvalFrameGuard`).
#[derive(Debug)]
// A5.3: see `ParallelDispatchRootProvider` — the `RootProvider` impl is slab-only;
// the struct stays alive via `ParallelCollapseDispatchHandle::_root_provider_arc`.
#[cfg_attr(feature = "index-gc", allow(dead_code))]
pub struct ParallelCollapseRootProvider {
    pub(crate) results: super::eval_loop::ParallelEvalResults,
    /// Stable snapshot of worker INPUTS for the dispatch lifetime.
    /// Shares the same Arc as `WaitForParallelCollapse.stable_items_snapshot`.
    /// See `ParallelDispatchRootProvider::branches` for the full
    /// rationale — collapse workers capture items by-move identically.
    pub(crate) items: Arc<Vec<BoundValue>>,
}

#[cfg(not(feature = "index-gc"))]
impl crate::backend::models::gc_allocator::RootProvider for ParallelCollapseRootProvider {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        // INPUTS first — no Mutex, direct iter (immutable Arc).
        for (value, bindings) in self.items.iter() {
            roots.push(*value);
            for (_, bound) in bindings.iter() {
                roots.push(*bound);
            }
        }
        // OUTPUTS — try_lock per Phase 6 rationale.
        if let Ok(guard) = self.results.try_lock() {
            for slot in guard.iter().flatten() {
                for (v, bindings) in slot.iter() {
                    roots.push(*v);
                    for (_, bound) in bindings.iter() {
                        roots.push(*bound);
                    }
                }
            }
        }
    }
}

/// Work item representing pending evaluation work.
///
/// Each variant represents a different kind of evaluation task that the
/// trampoline loop can process. MettaValue is Copy (8-byte tagged pointer),
/// so all value passing is zero-cost.
#[derive(Debug)]
pub enum WorkItem {
    /// Evaluate a value and send result to continuation at stack top
    Eval {
        value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// If true, this is a tail call - don't increment depth
        is_tail_call: bool,
        /// Phase 8.7: Expected return type for branch pruning.
        /// When set, rules whose `rhs_type` is incompatible with this type
        /// are pruned from the match set before evaluation.
        expected_type: Option<MettaValue>,
        /// Demand from consumer context for branch pruning.
        /// When `Some(Demand::AtLeast(1))`, nondeterministic dispatch uses lazy
        /// BranchCoroutine evaluation — stops after first result, eliminating
        /// 97% of wasted branch exploration in patterns like `match-atom`.
        demand: Option<crate::backend::eval::cesk::coroutine::Demand>,
        /// Stage 1d-revised: the ambient binding context inherited from the
        /// caller's alternative. Mirrors HE's InterpretedAtom(Stack, Bindings):
        /// each in-flight alternative carries its own bindings, propagated
        /// into sub-evaluations via this field. At Done→Resume, each leaf
        /// result's BoundValue.1 starts as this carrying (then merged with
        /// any step-produced bindings). Default = empty (no ambient).
        carrying_bindings: SharedBindings,
    },
    /// Evaluate a template with deferred bindings (lazy binding).
    ///
    /// Instead of calling `apply_bindings` upfront to materialize
    /// a fully-substituted expression tree, this carries `(template, bindings)`
    /// and resolves variables lazily:
    /// - Variables: look up in bindings, push result
    /// - Ground (no variables): push as Eval directly
    /// - Special forms (let, if, chain): resolve only immediate args, forward
    ///   remaining bindings to child evaluations via binding composition
    /// - Other S-expressions: fall back to `apply_bindings` + Eval
    ///
    /// This avoids O(tree_depth) recursive allocation for nested `let*` chains,
    /// where each level would otherwise materialize the entire remaining body.
    EvalWithBindings {
        template: MettaValue,
        bindings: SharedBindings,
        env: SharedEnv,
        depth: usize,
        is_tail_call: bool,
        expected_type: Option<MettaValue>,
        /// Stage 1d-revised: see `WorkItem::Eval.carrying_bindings`.
        carrying_bindings: SharedBindings,
    },
    /// Resume the continuation at stack top with a result
    Resume { result: EvalResult },
}

/// Continuation representing what to do with an evaluation result.
///
/// Each variant captures the state needed to continue processing after
/// a sub-evaluation completes. MettaValue is Copy (8-byte tagged pointer),
/// so all value fields are zero-cost to store and retrieve.
///
/// # Note on env/depth fields
///
/// Some variants store `env` and `depth` fields that are not read directly.
/// These fields are preserved for context but the actual environment from the
/// evaluation result is used instead. This is intentional - continuations track
/// the original environment for debugging/reference.
#[derive(Debug)]
pub enum Continuation {
    /// Final result - return from eval()
    Done,

    /// Collecting S-expression sub-results before processing
    CollectSExpr {
        remaining: std::vec::IntoIter<MettaValue>,
        collected: Vec<EvalResult>,
        original_env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        /// Propagated to child Eval pushes so per-branch bindings flow correctly.
        outer_carrying: SharedBindings,
    },

    /// Processing rule match results with bindings.
    ProcessRuleMatches {
        remaining_matches: std::vec::IntoIter<(MettaValue, GenericBindings<MettaValue>)>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Pre-fork mutation epoch for cache isolation between sequential branches.
        /// Restored before evaluating each subsequent branch so that side effects
        /// from branch N don't invalidate caches for branch N+1.
        pre_fork_epoch: u64,
        /// Pre-fork scope generation for generation-based cache isolation.
        /// Used with `enter_fork_scope` / `next_branch_scope` / `leave_fork_scope`
        /// to isolate cache entries between nondeterministic branches without
        /// clearing caches.
        pre_fork_gen: u64,
        /// Fork depth — retained for trace/scope bookkeeping (BranchStart/End
        /// pairing, `leave_fork`). The Prolog-style cut linkage now lives in
        /// `cut_barrier`/`saved_barrier` (Phase 1 cut-barrier), not here.
        fork_depth: u32,
        /// Phase 1 cut-barrier: the cut-scope barrier id this fan-out belongs
        /// to. If the matched rule body can fire `(cut)`, `dispatch_rule_matches`
        /// opens a FRESH barrier and stores it here; otherwise this INHERITS
        /// the enclosing `current_barrier()` so an inner non-cut fan-out still
        /// commits to an outer cut scope. The advance arm calls
        /// `cut_fired_for(cut_barrier)` before pulling the next match and, if
        /// it fired, drops `remaining_matches` and commits. `0` = no scope.
        cut_barrier: u64,
        /// Phase 1 cut-barrier: the `current_barrier()` value that was active
        /// immediately BEFORE this dispatch opened/inherited `cut_barrier`.
        /// Restored via `set_current_barrier(saved_barrier)` when this fan-out
        /// completes (all matches consumed or cut fired), so the enclosing
        /// scope's barrier is correctly re-established for sibling work.
        saved_barrier: u64,
        /// Stage 1c: The match unification bindings for the branch whose RHS
        /// is CURRENTLY being evaluated, composed with the outer ambient
        /// `outer_carrying` (Stage 1d-revised). When the RHS result arrives,
        /// these are merged into every result's BoundValue.1 to establish
        /// per-branch binding provenance. Rotated to `compose(outer_carrying,
        /// next_match_bindings)` before advancing to the next branch.
        current_branch_bindings: SharedBindings,
        /// Stage 1d-revised: ambient bindings from the caller's context
        /// (e.g., CollectSExpr's merged child bindings). Retained so that
        /// when rotating to the next branch we can compose anew with that
        /// branch's match bindings. Empty when no ambient is passed.
        outer_carrying: SharedBindings,
        /// Stage 1c: The tracked-variable set of the innermost active
        /// `collapse-bind` (if any). Used to project composed bindings so
        /// only the caller-relevant variables flow through. `None` when no
        /// collapse-bind is active (99% of evaluations) — zero overhead.
        /// Shared via Arc so parallel workers can see the same projection
        /// set without reading the thread-local.
        tracked_vars_hint: Option<std::sync::Arc<SmallVec<[&'static str; 4]>>>,
        /// Span correlation ID for the current branch (format v2).
        #[cfg(feature = "trace")]
        branch_span_id: u64,
        /// Start timestamp of the current branch (format v2).
        #[cfg(feature = "trace")]
        branch_start_ns: u64,
        /// Index of the current branch (0-based).
        #[cfg(feature = "trace")]
        branch_index: u32,
        /// Total number of nondeterministic branches.
        #[cfg(feature = "trace")]
        total_branches: u32,
        /// **H7 Stage 1 (2026-05-05)**: `true` when this continuation represents
        /// a REAL nondeterministic-fork branch (paired with `BranchStart`/
        /// `NondeterministicFork`). `false` for the single-match compose-shim
        /// path (`eval_loop.rs:477-496`) which doesn't fork. Gates `BranchEnd`
        /// emission at `eval_loop.rs:5418-5439` to prevent malformed events
        /// (`start_ns=0`, `dur=elapsed-since-trace-start`) from polluting
        /// trace-analyzer lints (branch-imbalance/speculative-waste/etc).
        #[cfg(feature = "trace")]
        is_real_fork: bool,
    },

    /// I-15: Lazy nondeterministic branch evaluation via coroutine.
    /// Evaluates branches one-at-a-time until demand is satisfied.
    ProcessRuleMatchesLazy {
        /// The coroutine managing unevaluated branches.
        coroutine: Box<crate::backend::eval::cesk::coroutine::BranchCoroutine<MettaValue>>,
        /// Results accumulated so far.
        results: Vec<BoundValue>,
        /// Environment for evaluation.
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Stage 1c: see `ProcessRuleMatches.current_branch_bindings`.
        current_branch_bindings: SharedBindings,
        /// Stage 1d-revised: see `ProcessRuleMatches.outer_carrying`.
        outer_carrying: SharedBindings,
        /// Stage 1c: see `ProcessRuleMatches.tracked_vars_hint`.
        tracked_vars_hint: Option<std::sync::Arc<SmallVec<[&'static str; 4]>>>,
    },

    /// Processing TCO grounded operation.
    ProcessGroundedOp {
        state: Box<GroundedState<MettaValue>>,
        /// The arg index whose evaluation result is pending.
        pending_arg_idx: usize,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d: accumulated bindings from all evaluated args MERGE'd
        /// together. Grounded ops produce a new ground value whose bindings
        /// = merge of all arg bindings (conflict → empty output result).
        arg_bindings: SharedBindings,
    },

    /// HE-faithful fan-out over nondeterministic arg-eval alternatives.
    ///
    /// Pushed by `ProcessGroundedOp` when an `EvalArg` returns N>1
    /// alternatives. Self-repushes once per alternative (processing one at
    /// a time with its own cloned `GroundedState` carrying a single value
    /// at `pending_arg_idx` + its own composed `arg_bindings`), and
    /// accumulates all branches' outputs into a single `EvalResult` handed
    /// to the outer `Resume`.
    ///
    /// This mirrors MeTTa HE's plan-vector fan-out at grounded-call sites
    /// (`eval_impl` in `interpreter.rs:504-549`): each `(value, bindings)`
    /// alternative is an independent plan item, the grounded op fires once
    /// per substituted single-valued alternative, and outputs from all
    /// alternatives are collected into a flat result list.
    ProcessGroundedOpFanout {
        /// Remaining arg-eval alternatives to process (value +
        /// branch-bindings from that alt's Eval return).
        remaining_alts: std::vec::IntoIter<BoundValue>,
        /// Accumulated outputs across all alternatives processed so far.
        results: Vec<BoundValue>,
        /// Template state BEFORE `pending_arg_idx` was installed. Cloned
        /// per alternative and the single value is installed on the clone.
        template_state: Box<GroundedState<MettaValue>>,
        /// Arg index whose alternatives are being iterated.
        pending_arg_idx: usize,
        /// Prior `arg_bindings` (accumulated from earlier args). Each alt
        /// composes this with its own `alt_bindings` to form that alt's
        /// full binding bag before the grounded op fires.
        prior_arg_bindings: SharedBindings,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing lazy Cartesian product combinations.
    ProcessCombinations {
        combinations: Box<GenericCartesianProductIter<MettaValue>>,
        results: Vec<BoundValue>,
        pending_rule_matches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Phase 2.B: binding-preserving Cartesian-product iteration.
    ///
    /// Used when `CollectSExpr` detects multi-alternative children. Each
    /// yielded combination already carries its composed bindings (with
    /// conflicts pruned inside the iterator). `pending_combo_bindings` is
    /// the current combo's bindings while its rule matches are dispatched.
    ProcessCombinationsBound {
        combinations: Box<crate::backend::eval::processing::ops::GenericCartesianProductBoundIter>,
        results: Vec<BoundValue>,
        pending_rule_matches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        /// The current combo's composed bindings — threaded to
        /// `dispatch_rule_matches` as `outer_carrying` for RHS evaluation,
        /// and attached to no-match data results.
        pending_combo_bindings: GenericBindings<MettaValue>,
        env: SharedEnv,
        depth: usize,
        /// Ambient bindings from the caller's context. NOTE: these are NOT
        /// the same as `pending_combo_bindings` — they are the outer context
        /// INPUT to CollectSExpr. The iterator already composed them into
        /// each combo's bindings, so this field is retained only for
        /// symmetry with `ProcessCombinations` (unused at dispatch time;
        /// see `dispatch_rule_matches` call sites).
        outer_carrying: SharedBindings,
    },

    /// Processing let binding
    ProcessLet {
        pending_values: Option<Vec<BoundValue>>,
        pattern: MettaValue,
        body: MettaValue,
        /// Outer bindings from an `EvalWithBindings` dispatch. When `Some`,
        /// these are composed with pattern-match bindings and the body is
        /// evaluated via `EvalWithBindings` instead of `apply_bindings`.
        /// This enables O(N) instead of O(N^2) work for nested `let*` chains.
        outer_bindings: Option<SharedBindings>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// PeTTa `(once X)` barrier owner (Phase 2). Opened by the `StartOnce`
    /// handler, which allocates a fresh `once_barrier`, makes it the innermost
    /// cut scope, and dispatches the desugared `(let $r X (let $_ (cut) $r))`.
    /// When the body's value bubbles back, this CONSUMES the once's cut signal
    /// (it OWNS the barrier — mirrors the `is_barrier_owner` lifecycle in
    /// ProcessRuleMatches) and restores `saved_barrier`, so the enclosing
    /// clause's nondeterminism is untouched (scope-precision). Carries no
    /// fan-out state — X's own fan-out continuations did the pruning by peeking
    /// `cut_fired_peek(once_barrier)`.
    ProcessOnceRestore {
        saved_barrier: u64,
        once_barrier: u64,
        depth: usize,
    },

    /// Collecting grounded arg evaluation results.
    ///
    /// `evaluated_results` stores ALL results per arg (Vec<Vec<MettaValue>>) to
    /// preserve nondeterminism. After all grounded args are evaluated,
    /// the Cartesian product is computed and each combination is evaluated.
    CollectGroundedArg {
        items: Vec<MettaValue>,
        grounded_indices: Vec<usize>,
        current_idx: usize,
        evaluated_results: Vec<Vec<BoundValue>>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Collecting results from applicative evaluation of Cartesian product
    /// combinations produced by nondeterministic grounded arg evaluation.
    CollectApplicativeResults {
        remaining: std::vec::IntoIter<MettaValue>,
        /// Stage 1d-revised: per-combination pre-computed arg-bindings,
        /// parallel to `remaining`. Combined with outer_carrying via
        /// compose_outer_inner_generic when dispatching each combo's Eval.
        remaining_bindings: Vec<GenericBindings<MettaValue>>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing map-atom iteration
    ProcessMapAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        template: MettaValue,
        collected_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// H1 (2026-05-05): per-iteration bindings accumulator. Each iteration's
        /// emitted bindings are composed into this register; on final emit the
        /// result list is wrapped with `compose(outer_carrying, acc_bindings)`.
        /// Mirrors HE's `chain (eval (sealed (V) B)) ... (cons-atom ...)` rule
        /// expansion semantics where bindings flow through `chain`.
        acc_bindings: SharedBindings,
    },

    /// Processing filter-atom iteration
    ProcessFilterAtom {
        current_element: Option<MettaValue>,
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        predicate: MettaValue,
        filtered_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// H13 (2026-05-05) — mirror of map-atom's H1 full per-iteration
        /// bindings accumulator. Each iteration's emitted bindings are
        /// composed into this register; on final emit the result list is
        /// wrapped with `compose(outer_carrying, acc_bindings)`. Mirrors
        /// HE's `chain (eval (sealed (V) F)) ... (cons-atom ...)` rule
        /// expansion semantics where bindings flow through `chain`.
        acc_bindings: SharedBindings,
    },

    /// Processing foldl-atom iteration
    ProcessFoldlAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        acc_var_name: String,
        item_var_name: String,
        operation: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d: accumulated bindings from all consumed iterations
        /// MERGE'd together. Each iteration's eval result carries its own
        /// bindings (from inner rule dispatches) which we merge here.
        /// On conflict, the fold branch emits zero results.
        acc_bindings: SharedBindings,
    },

    /// Processing if condition
    ProcessIfCondition {
        then_branch: MettaValue,
        else_branch: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, the taken branch is evaluated via EvalWithBindings
        /// instead of Eval, avoiding materialization of the untaken branch.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Demand active before the condition was narrowed to `Exactly(1)`.
        outer_demand: crate::backend::eval::cesk::coroutine::Demand,
    },

    /// Processing case atom
    ProcessCaseAtom {
        cases: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, case templates are evaluated via EvalWithBindings
        /// instead of Eval, deferring binding application to the matched arm.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing (eval expr)
    ProcessEvalEval {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// T04/105 (2026-05-17): HE `metta_call_return` parity.
        /// Original `(eval <arg>)` expression. When the eval result is
        /// `NotReducible`, replace with this — HE-bisim: empirical
        /// `(eval 42)` → `[(eval 42)]`, `(eval (eval 5))` → `[(eval (eval 5))]`.
        /// `None` for callers that don't need this conversion (e.g. internal
        /// full-reduction paths). When `Some`, the NotReducible-detect
        /// branch in `ProcessEvalEval` returns this value.
        original_eval_expr: Option<MettaValue>,
    },

    /// Processing (return value)
    ProcessReturn {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing chain expression
    ProcessChainExpr {
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, chain body is evaluated via EvalWithBindings after
        /// composing the chain variable binding with outer_bindings.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing chain body evaluations
    ProcessChainBody {
        /// Remaining `(value, per-branch bindings)` alternatives from the
        /// chain-expr evaluation. Each alt's bindings are merged into the
        /// body's `outer_bindings` AND `carrying_bindings` when dispatched
        /// so the chain-var substitution carries its originating branch's
        /// binding context through the body evaluation.
        remaining_values: std::vec::IntoIter<BoundValue>,
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        outer_bindings: Option<SharedBindings>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing function loop
    ProcessFunction {
        iteration_count: usize,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing is-error
    ProcessIsError {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing catch
    ProcessCatch {
        default: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing conjunction
    ProcessConjunction {
        remaining_goals: std::vec::IntoIter<MettaValue>,
        accumulated_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Phase 1 cut-barrier: the cut-scope barrier id active when this
        /// goal-sequence fan-out was constructed. Captured via
        /// `current_barrier()` so a `(cut)` evaluated while resolving a goal
        /// prunes the enclosing clause. `0` = no active cut scope.
        cut_barrier: u64,
    },

    /// Processing unify pattern1
    ProcessUnifyPattern1 {
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify pattern1 iteration.
    ///
    /// Phase 2 Part A fix (task #64): `remaining_pattern1_results` carries
    /// `BoundValue` so per-pattern1-result bindings (from the pattern1 eval)
    /// compose into both the pattern-2 eval and the success-body eval.
    /// Previously `IntoIter<MettaValue>` stripped per-result bindings,
    /// losing variable unifications established by pattern1.
    ProcessUnifyPattern1Iter {
        remaining_pattern1_results: std::vec::IntoIter<BoundValue>,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        all_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify pattern2
    ProcessUnifyPattern2 {
        val1: MettaValue,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify bodies
    ProcessUnifyBodies {
        remaining_bodies: std::vec::IntoIter<MettaValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing collapse
    ProcessCollapse {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Plan Phase E (2026-05-20): propagated from `StartCollapse`.
        /// If true (default), the assembled tuple is sorted by canonical
        /// printable form (HE behavior). False for the MTT-only
        /// `collapse-defined-order` operator.
        sort_results: bool,
    },

    /// Processing collapse-bind
    ProcessCollapseBind {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        /// Note: collapse-bind opens a fresh binding scope for the inner expr,
        /// so this is mainly retained for downstream propagation symmetry.
        outer_carrying: SharedBindings,
    },

    /// Evaluating individual collapse results before assembling the tuple.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// before wrapping in an S-expression tuple. This mirrors HE's use of `metta`
    /// (the full recursive interpreter) inside `collapse`.
    ProcessCollapseEvalResults {
        /// Remaining unevaluated results (with per-branch bindings) to evaluate.
        /// Each pair is `(raw_value, carrying_bindings)` — the bindings were
        /// active for the nondeterministic branch that produced `raw_value`.
        /// For plain `collapse` (is_bind=false), the bindings are discarded
        /// after evaluation. For `collapse-bind` (is_bind=true), the bindings
        /// are encoded into the `(value (Bindings …))` pair output.
        remaining_raw: std::vec::IntoIter<BoundValue>,
        /// Fully evaluated results collected so far, paired with the bindings
        /// they carry. For collapse-bind, these bindings become the sidecar
        /// `(Bindings …)` in each output pair.
        evaluated: Vec<BoundValue>,
        /// Whether this is for collapse-bind (vs plain collapse)
        is_bind: bool,
        /// Stage 1e: the bindings carried by the raw value CURRENTLY being
        /// re-evaluated. On result arrival, these are merged into each
        /// received eval result's bindings so the (re-)evaluated value
        /// retains its source branch's binding provenance — essential for
        /// `collapse-bind` to emit correct `(Bindings …)` sidecars.
        current_raw_bindings: SharedBindings,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Layer A: tracked variable names from the enclosing `collapse-bind`
        /// frame, captured when `ProcessCollapseBind` pops the scope. Used at
        /// the sidecar encoding site to project each result's bindings to
        /// just the user-visible set (matching HE `Bindings::resolve()` at
        /// the observation point, not earlier during match composition).
        /// None for plain `collapse` or when no free vars were tracked.
        tracked_vars_hint: Option<std::sync::Arc<smallvec::SmallVec<[&'static str; 4]>>>,
        /// Plan Phase E (2026-05-20): if true (default for plain `collapse`),
        /// the assembled tuple is sorted by canonical printable form before
        /// emission (HE behavior, fixture T04/063 / §06.11). If false (used
        /// by `collapse-defined-order` and `collapse-bind`), the
        /// rule-firing order is preserved.
        sort_results: bool,
        /// Caller bindings active around the collapse form. Plain collapse
        /// re-evaluates raw results under this context; collapse-bind keeps it
        /// rooted for continuation-state completeness.
        outer_carrying: SharedBindings,
    },

    /// Processing amb
    ProcessAmb {
        /// Remaining nondeterministic alternatives still to be dispatched.
        /// Each alternative carries its own per-branch bindings so that
        /// downstream evaluation inherits the alt's binding context via
        /// `carrying_bindings`, mirroring HE's per-plan-item `(atom, bindings)`
        /// dispatch model.
        remaining_alts: std::vec::IntoIter<BoundValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Whether to project each alt's carrying bindings down to the
        /// consumer's free variables via `project_carrying_for_consumer`
        /// before dispatch (the default, `true`).
        ///
        /// Set to `false` by the two fan-out sites that re-dispatch an alt's
        /// own already-evaluated VALUE as the consumer (often a ground value
        /// with no variables), rather than a template that mentions the
        /// carried vars:
        ///   1. the `foldl-atom` multi-branch fan-out (`ProcessFoldlAtom`),
        ///   2. the `EvalEval` (reduce/progn/metta/capture) multi-result
        ///      fan-out (`ProcessEvalEval`).
        /// In both, each alt's per-branch bindings are SOLUTION bindings that
        /// must propagate to the OUTPUT even when they are not free variables
        /// of the re-dispatched consumer (e.g. an outer query var `$who` bound
        /// by a premise / a multi-clause rule match that no later consumer
        /// references). With projection on, `project_carrying_for_consumer`
        /// returns an empty map for a ground consumer (its `live` set is
        /// empty), dropping such a var.
        /// Projection would drop such a var, and because the first alt is
        /// dispatched WITHOUT projection (at the fan-out site) while the
        /// rest flow through here, the surviving binding's fate would
        /// depend on nondeterministic alt ORDER — a HashMap-order flake
        /// (PLN-main `(? (grandfather $who c))`). Preserving the full
        /// per-branch bindings for every alt makes propagation
        /// order-independent. The dropped vars are never re-bound by the
        /// consumer (it does not reference them), so preserving them
        /// cannot introduce a spurious conflict.
        project_alt_carrying: bool,
        /// Phase 1 cut-barrier: the cut-scope barrier id active when this
        /// disjunction fan-out was constructed (`current_barrier()`). This is
        /// the exact `cut.metta` path: the `let*` multi-result fallback spawns
        /// a `ProcessAmb` while the cut-carrying rule body's barrier is
        /// current, so a `(cut)` in a later `let*` pair prunes the value-expr
        /// fan-out. The advance arm calls `cut_fired_for(cut_barrier)` before
        /// pulling the next alternative. `0` = no active cut scope.
        cut_barrier: u64,
    },

    /// **Stack-safety mandate (2026-05-15)**: wait state for a trampolinized
    /// parallel-branch dispatch.
    ///
    /// Previously, `parallel_branch_eval` held a synchronous condvar-wait
    /// loop inside its own stack frame, which combined with inline branch-0
    /// evaluation and work-stealing to produce unbounded same-thread C-stack
    /// recursion (Robot.metta crash, 68-frame work-pool stack overflow).
    ///
    /// After trampolinization, `parallel_dispatch` returns a handle and
    /// immediately yields to the trampoline outer loop via this continuation.
    /// Each pump tick of the trampoline checks `handle.remaining`, optionally
    /// steals one task or sleeps briefly, then re-pushes the continuation —
    /// no C-stack growth across ticks regardless of `MAX_PARALLEL_DEPTH`.
    WaitForParallel {
        /// Per-call state for the dispatch (shared via Arc with workers).
        /// The handle's `_root_provider_arc` keeps a GC root provider
        /// alive for the dispatch's lifetime; see
        /// `ParallelDispatchRootProvider` for the race it closes.
        handle: ParallelDispatchHandle,
        /// How to merge per-branch results into `base_results`.
        merge_mode: ParallelMergeMode,
        /// Results already accumulated by the caller (e.g., from prior
        /// sequential evaluation rounds). Branch results are appended to
        /// these per `merge_mode`.
        base_results: SmallVec<[BoundValue; 2]>,
        /// Outer (caller's) carrying bindings, used by `RuleMatch` mode to
        /// compose with each branch's per-branch bindings.
        outer_carrying: SharedBindings,
        env: SharedEnv,
        depth: usize,
        /// Total parallel budget acquired at dispatch — released on completion.
        budget_acquired: u32,
        /// `PARALLEL_BRANCH_DEPTH` value at the call site; informational for
        /// trace/scheduler routing (not used for cap checks here).
        caller_depth: u32,
        /// Stable snapshot of the input branches, used by safepoint root
        /// collection in `pump_parallel_wait` (mirrors `frame_chain`
        /// registration's branch set).
        stable_branches_snapshot: Arc<Vec<super::eval_loop::ParallelBranch>>,
    },

    /// **Stack-safety mandate (2026-05-15)**: wait state for a trampolinized
    /// parallel-collapse dispatch. Mirrors `WaitForParallel`.
    WaitForParallelCollapse {
        /// The handle's `_root_provider_arc` keeps a GC root provider
        /// alive for the dispatch's lifetime; see
        /// `ParallelCollapseRootProvider`.
        handle: ParallelCollapseDispatchHandle,
        merge_mode: CollapseMergeMode,
        /// The original items being collapsed (preserved for safepoint roots
        /// and for `Bind`-mode sidecar reconstruction).
        stable_items_snapshot: Arc<Vec<BoundValue>>,
        /// Caller's carrying bindings (preserved for downstream continuation
        /// state completeness; collapse-bind opens its own scope but the
        /// outer scope is still tracked).
        outer_carrying: SharedBindings,
        /// Layer A: tracked-variable hints from the enclosing collapse-bind
        /// frame, projected at the sidecar encoding step. None for plain
        /// collapse.
        tracked_vars_hint: Option<Arc<SmallVec<[&'static str; 4]>>>,
        env: SharedEnv,
        depth: usize,
        budget_acquired: u32,
        caller_depth: u32,
        /// Plan Phase E (2026-05-20): propagated from `StartCollapse`.
        /// When `merge_mode == Plain` and this is true (default for
        /// plain `collapse`), the assembled tuple is sorted by canonical
        /// printable form (HE behavior). False for the MTT-only
        /// `collapse-defined-order`.
        sort_results: bool,
    },

    /// Processing guard
    ProcessGuard {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-atoms
    ProcessGetAtoms {
        space_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-type-space — receives evaluated space, queries types of atom.
    ///
    /// Phase 4 (2026-05-19): pre-eval the space arg so `bind!`-bound tokens
    /// resolve via `lookup_token_generic` (the same path get-atoms uses).
    ProcessGetTypeSpace {
        /// Original space-ref expression (for error reporting).
        space_ref: MettaValue,
        /// Atom whose types we want to query in the resolved space.
        atom: MettaValue,
        /// Original call form (for error reporting).
        call_form: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Processing memo table
    ProcessMemoTable {
        memo_ref: MettaValue,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing memo expression
    ProcessMemoExpr {
        memo_handle: MemoHandle,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing new-memo name
    ProcessNewMemoName {
        name_arg: MettaValue,
        size_arg: Option<MettaValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing new-memo size
    ProcessNewMemoSize {
        name: String,
        size_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing memo operation
    ProcessMemoOp {
        memo_ref: MettaValue,
        is_clear: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing match space
    ProcessMatchSpace {
        space_arg: MettaValue,
        pattern: MettaValue,
        template: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Phase 1 cut-barrier: the cut-scope barrier id active when this
        /// match dispatch was constructed (`current_barrier()`). Propagated to
        /// the `ProcessMatchTemplates` fan-out this handler spawns so a `(cut)`
        /// evaluated while reducing a matched template prunes the enclosing
        /// clause's remaining template alternatives. `0` = no active cut scope.
        cut_barrier: u64,
    },

    /// Processing match templates
    ProcessMatchTemplates {
        remaining_templates: std::vec::IntoIter<MettaValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
        /// Phase 1 cut-barrier: the cut-scope barrier id active when this
        /// template fan-out was constructed (`current_barrier()`). The advance
        /// arm calls `cut_fired_for(cut_barrier)` before pulling the next
        /// matched template, committing to the matches collected so far when a
        /// `(cut)` fired this barrier. `0` = no active cut scope.
        cut_barrier: u64,
    },

    /// Processing add-atom space
    ProcessAddAtomSpace {
        space_ref: MettaValue,
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    // Disabled: ProcessAddAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — add-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessAddAtomSpace.
    // ProcessAddAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: MettaValue,
    //     env: SharedEnv,
    //     depth: usize,
    //     parent_cont: usize,
    // },
    /// Processing remove-atom space
    ProcessRemoveAtomSpace {
        space_ref: MettaValue,
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    // Disabled: ProcessRemoveAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — remove-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessRemoveAtomSpace.
    // ProcessRemoveAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: MettaValue,
    //     env: SharedEnv,
    //     depth: usize,
    //     parent_cont: usize,
    // },
    /// Processing new-state
    ProcessNewState {
        initial_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-state
    ProcessGetState {
        state_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing change-state reference
    ProcessChangeStateRef {
        state_ref: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing change-state value
    ProcessChangeStateValue {
        state_value: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Phase I.3 — compare-and-swap-state! state-ref eval completed.
    /// Captures the resolved state ref, evaluates expected next.
    ProcessCasStateRef {
        state_ref: MettaValue,
        expected: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Phase I.3 — compare-and-swap-state! expected eval completed.
    /// Captures resolved expected; evaluates new_value next.
    ProcessCasExpected {
        state_value: MettaValue,
        expected_value: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Phase I.3 — compare-and-swap-state! new-value eval completed.
    /// Performs the CAS atomically and emits True/False.
    ProcessCasNewValue {
        state_value: MettaValue,
        expected_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Phase I.5 — loop-until-state state-ref eval completed.
    /// On completion, polls the cell and either returns or re-loops.
    ProcessLoopStateRef {
        state_ref: MettaValue,
        target: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Phase I.5 — loop-until-state target eval completed.
    /// Captures resolved target; begins polling.
    ProcessLoopTarget {
        state_value: MettaValue,
        target_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },

    /// Processing repr
    ProcessRepr {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing format-args string
    ProcessFormatArgsString {
        format_arg: MettaValue,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing format-args args
    ProcessFormatArgsArgs {
        format_str: String,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing println
    ProcessPrintln {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing trace message
    ProcessTraceMessage {
        message: MettaValue,
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing trace value
    ProcessTraceValue {
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-metatype
    ProcessGetMetatype {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing bind
    ProcessBind {
        token: String,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing if-reducible: expr has been evaluated, now compare to original.
    ProcessIfReducible {
        /// Original expression (before evaluation) for comparison
        original_expr: MettaValue,
        /// Branch to evaluate if expr reduced
        then_branch: MettaValue,
        /// Branch to evaluate if expr is irreducible
        else_branch: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing match-or space evaluation
    ProcessMatchOrSpace {
        /// Space reference being evaluated
        space_arg: MettaValue,
        /// Pattern to match
        pattern: MettaValue,
        /// Default if no matches
        default: MettaValue,
        /// Template to instantiate
        template: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing sort-tuple: insertion sort via trampoline comparator evaluation.
    /// Sorted elements accumulate in `sorted`, unsorted elements wait in `unsorted`.
    /// `current` is being inserted into `sorted` at position `insert_pos`.
    ProcessSortTuple {
        /// Already-sorted elements
        sorted: Vec<MettaValue>,
        /// Remaining elements to insert
        unsorted: Vec<MettaValue>,
        /// Element currently being inserted
        current: MettaValue,
        /// Current comparison position in sorted
        insert_pos: usize,
        /// Variable name for left operand
        var1_name: String,
        /// Variable name for right operand
        var2_name: String,
        /// Comparator expression template
        comparator: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing best-candidate: linear scan evaluating rank function.
    /// Tracks the best element and its rank, evaluating remaining elements.
    ProcessBestCandidate {
        /// Best element so far (None = first iteration)
        best: Option<MettaValue>,
        /// Rank of best element
        best_rank: Option<f64>,
        /// Elements still to evaluate
        remaining: std::vec::IntoIter<MettaValue>,
        /// Element whose rank we're currently evaluating
        current: MettaValue,
        /// Variable name for rank function binding
        var_name: String,
        /// Rank function expression template
        rank_fn: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing case multi-results.
    ///
    /// Phase 2 Part A fix (task #63): `remaining_atoms` carries BoundValue
    /// so per-scrutinee-result bindings (from the scrutinee's evaluation)
    /// compose with the outer carrying and flow into the case body eval.
    /// Previously `IntoIter<MettaValue>` stripped per-result bindings,
    /// making variables bound in the scrutinee invisible to the case body.
    ProcessCaseMultiResults {
        remaining_atoms: std::vec::IntoIter<BoundValue>,
        cases: MettaValue,
        collected: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Evaluating individual scrutinee results for case before pattern matching.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// from the scrutinee expression before matching against case patterns.
    /// This mirrors HE's `(let $c (collapse $atom) ...)` which invokes the full
    /// interpreter on the scrutinee, ensuring rule applications are completed.
    ProcessCaseEvalScrutineeResults {
        /// Remaining unevaluated scrutinee results to evaluate (paired with
        /// their carrying bindings from nondeterministic branching).
        remaining_raw: std::vec::IntoIter<BoundValue>,
        /// Fully evaluated scrutinee results collected so far.
        evaluated: Vec<BoundValue>,
        /// Case patterns to match against
        cases: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Per-raw bindings of the value currently being re-evaluated.
        /// Used as the carrying when re-pushing for the next raw value.
        current_raw_bindings: SharedBindings,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Memoize evaluation results for a pure expression.
    ///
    /// When evaluation completes, the results are stored in the thread-local
    /// EVAL_MEMO cache keyed by the expression's content hash. Subsequent
    /// evaluations of structurally identical expressions skip evaluation entirely.
    MemoizeResult {
        /// Content hash of the expression (via `hash_value()`)
        expr_hash: u64,
        /// Mutation epoch at the time this continuation was pushed.
        /// If the epoch has advanced by the time evaluation completes,
        /// the result must not be cached (a side effect occurred transitively).
        mutation_epoch: u64,
        /// Environment (for result forwarding)
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Re-export scrutinee free-variable bindings onto a `let`/`progn`/`chain`
    /// body's RESULT sidecar (PeTTa clause-global unification).
    ///
    /// When a `let`/`progn` scrutinee evaluation binds a free variable other
    /// than the let-bound pattern variable — e.g. the non-final `progn`
    /// statement `(reduce (grandfather $who c))` binds `$who=a` — Prolog's
    /// clause-global unification keeps that binding visible to sibling goals
    /// in the enclosing clause. MeTTaTron consumes the scrutinee binding to
    /// instantiate the body but otherwise drops it, so the bare-`$term`
    /// sibling in PLN's `?` macro `(collapse ($term (progn (reduce $term) …)))`
    /// stays unbound. This continuation composes the captured `reexport` set
    /// into each body-result's sidecar so it threads back out through the
    /// enclosing `CollectSExpr` to the sibling.
    ///
    /// Pushed BELOW the body's evaluation only when `reexport` is non-empty;
    /// the let pattern variable(s) are already removed and freshened
    /// (`$__fr_*`) names filtered out when `reexport` is built.
    ReexportLetBindings {
        /// Scrutinee free-variable bindings to re-export (pattern var removed,
        /// `$__fr_*` canary-filtered). Never pushed when empty.
        reexport: GenericBindings<MettaValue>,
        /// Evaluation depth (for depth_hint accounting).
        depth: usize,
    },

    /// Processing `let*` sequential bindings as a tight loop.
    ///
    /// Instead of desugaring `(let* ((p1 v1) (p2 v2) ...) body)` to nested
    /// `let` forms (which creates N S-expr allocations + 3N trampoline iterations),
    /// this continuation evaluates value expressions one at a time, accumulating
    /// bindings. When all bindings are resolved, the body is evaluated with the
    /// composed bindings via `EvalWithBindings`.
    ///
    /// **Savings**: For N bindings, reduces from 3N+2 trampoline iterations to
    /// N+2 iterations (eval each value + body), and eliminates N nested `let`
    /// S-expr allocations.
    ///
    /// **Nondeterminism**: If a value expression produces zero results, the
    /// entire `let*` produces zero results (pattern match fails). If it produces
    /// multiple results, we branch (materialize and use ProcessLet fallback).
    ProcessLetStar {
        /// The pattern for the CURRENT binding whose value is being evaluated.
        current_pattern: MettaValue,
        /// Remaining (pattern, value_expr) pairs to process after the current one.
        remaining_pairs: Vec<(MettaValue, MettaValue)>,
        /// The body template — kept raw until all bindings are resolved.
        body: MettaValue,
        /// Accumulated bindings from resolved pattern matches + outer context.
        accumulated_bindings: SharedBindings,
        /// Environment for evaluation.
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Whether this is a tail call.
        is_tail_call: bool,
        /// I-5: Region ID for region-based allocation scoping.
        region_id: u32,
    },

    /// I-4: Complete a tabled subgoal after evaluation finishes.
    /// Stores the results in the SubgoalTable for future cache hits.
    CompleteSubgoal {
        /// Content hash of the expression being tabled.
        expr_hash: u64,
        /// Environment (for result forwarding).
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Mutation epoch when evaluation started (before any side effects).
        /// If the epoch changed during evaluation, the result is impure and
        /// must NOT be cached (it would suppress re-execution of side effects).
        start_epoch: u64,
    },

    /// I-6: Complete a thunk after evaluation finishes.
    /// Updates the ThunkTable with the cached results.
    CompleteThunk {
        /// Content hash of the (template, bindings) pair.
        thunk_hash: u64,
        /// Environment (for result forwarding).
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Mutation epoch when evaluation started.
        start_epoch: u64,
    },

    /// Collecting evaluated arguments for `freeze-tuple`. After all args
    /// are evaluated, constructs the tuple and marks it as normal form
    /// (via `memoize_normal_form`) instead of re-evaluating — preventing
    /// the trampoline's fixpoint loop from reducing a data tuple whose
    /// head happens to be a reducible expression.
    CollectFreezeArgs {
        /// The argument slots (some already evaluated in-place).
        args: Vec<MettaValue>,
        /// Indices within `args` that need evaluation.
        reducible_indices: Vec<usize>,
        /// Current position in `reducible_indices`.
        current_idx: usize,
        /// Per-reducible-arg evaluation results.
        evaluated_results: Vec<Vec<BoundValue>>,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
    },
}

// ============================================================================
// GC Root Collection — collect_values() for Safepoint GC
// ============================================================================
//
// These methods extract all MettaValue values reachable from trampoline state
// (work items and continuations) so the GC can trace them as roots during
// intra-evaluation safepoints. Environment values (rules, bindings, space facts)
// are NOT collected here — they are already registered via ROOT_REGISTRY +
// RootProvider on GenericEnvironmentShared.
//
// The exhaustive match on each enum ensures compile-time safety: adding a new
// variant without updating collect_values() causes a compile error.

impl WorkItem {
    /// Collect all MettaValue values reachable from this work item into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard.
    pub fn collect_values(&self, out: &mut Vec<MettaValue>) {
        match self {
            Self::Eval {
                value,
                expected_type,
                carrying_bindings,
                ..
            } => {
                out.push(*value);
                if let Some(et) = expected_type {
                    out.push(*et);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(carrying_bindings, out);
            }
            Self::EvalWithBindings {
                template,
                bindings,
                expected_type,
                carrying_bindings,
                ..
            } => {
                out.push(*template);
                collect_bindings_values(bindings, out);
                if let Some(et) = expected_type {
                    out.push(*et);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(carrying_bindings, out);
            }
            Self::Resume {
                result: (values, _),
                ..
            } => {
                for (v, bindings) in values.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }
        }
    }
}

/// Helper: collect all MettaValue values from a GenericBindings into `out`.
fn collect_bindings_values(bindings: &GenericBindings<MettaValue>, out: &mut Vec<MettaValue>) {
    for (_scope, _name, val) in bindings.iter_full() {
        out.push(*val);
    }
}

/// Helper: collect all MettaValue values from a GroundedState into `out`.
fn collect_grounded_state_values(state: &GroundedState<MettaValue>, out: &mut Vec<MettaValue>) {
    out.extend(state.args.iter().copied());
    for vals in state.evaluated_args.values() {
        out.extend(vals.iter().copied());
    }
    for (v, bindings_opt) in &state.accumulated_results {
        out.push(*v);
        if let Some(bindings) = bindings_opt {
            collect_bindings_values(bindings, out);
        }
    }
}

/// Helper: collect all MettaValue values from a GenericCartesianProductIter into `out`.
fn collect_cartesian_values(
    iter: &GenericCartesianProductIter<MettaValue>,
    out: &mut Vec<MettaValue>,
) {
    for input_vec in iter.inputs() {
        out.extend(input_vec.iter().copied());
    }
}

impl Continuation {
    /// Collect all MettaValue values reachable from this continuation into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard. The exhaustive match
    /// ensures compile-time safety — any new variant causes a compile error
    /// until root collection is added.
    pub fn collect_values(&self, out: &mut Vec<MettaValue>) {
        match self {
            Self::Done => {}

            Self::CollectSExpr {
                remaining,
                collected,
                outer_carrying,
                ..
            } => {
                out.extend(remaining.as_slice().iter().copied());
                for (vals, _env) in collected {
                    for (v, bindings) in vals.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessRuleMatches {
                remaining_matches,
                results,
                current_branch_bindings,
                outer_carrying,
                ..
            } => {
                for (rhs, bindings) in remaining_matches.as_slice() {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk per-branch + caller-scope bindings.
                collect_bindings_values(current_branch_bindings, out);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGroundedOp {
                state,
                arg_bindings,
                ..
            } => {
                collect_grounded_state_values(state, out);
                collect_bindings_values(arg_bindings, out);
            }

            Self::ProcessGroundedOpFanout {
                remaining_alts,
                results,
                template_state,
                prior_arg_bindings,
                ..
            } => {
                collect_grounded_state_values(template_state, out);
                collect_bindings_values(prior_arg_bindings, out);
                for (v, bindings) in remaining_alts.as_slice() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessCombinations {
                combinations,
                results,
                pending_rule_matches,
                outer_carrying,
                ..
            } => {
                collect_cartesian_values(combinations, out);
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (rhs, bindings) in pending_rule_matches {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCombinationsBound {
                combinations,
                results,
                pending_rule_matches,
                pending_combo_bindings,
                outer_carrying,
                ..
            } => {
                // Iterator inputs: each alternative carries value + bindings
                // (both need GC tracking to survive mark-sweep).
                for input_vec in combinations.inputs() {
                    for (v, bindings) in input_vec.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                // Iterator's outer_carrying also needs tracking.
                collect_bindings_values(combinations.outer_carrying(), out);
                // Current combo's composed bindings (during rule-match dispatch).
                collect_bindings_values(pending_combo_bindings, out);
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (rhs, bindings) in pending_rule_matches {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk variant's own outer_carrying.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessLet {
                pending_values,
                pattern,
                body,
                outer_bindings,
                outer_carrying,
                results,
                ..
            } => {
                if let Some(pending) = pending_values {
                    for (v, bindings) in pending.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                out.push(*pattern);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            // ProcessOnceRestore holds only barrier ids (u64) + depth — no
            // MettaValue roots to walk.
            Self::ProcessOnceRestore { .. } => {}

            Self::CollectGroundedArg {
                items,
                evaluated_results,
                outer_carrying,
                ..
            } => {
                out.extend(items.iter().copied());
                for result_vec in evaluated_results {
                    for (v, bindings) in result_vec.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::CollectApplicativeResults {
                remaining,
                remaining_bindings,
                results,
                outer_carrying,
                ..
            } => {
                out.extend(remaining.as_slice().iter().copied());
                for bindings in remaining_bindings.iter() {
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMapAtom {
                remaining_elements,
                template,
                collected_results,
                outer_carrying,
                acc_bindings,
                ..
            } => {
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*template);
                for (v, bindings) in collected_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk per-call binding registers.
                // Both fields are Arc<GenericBindings<MettaValue>>, holding
                // live MettaValue slab pointers. Pre-fix, GC freed these
                // between iterations under H10's increased GC cadence.
                collect_bindings_values(outer_carrying, out);
                collect_bindings_values(acc_bindings, out);
            }

            Self::ProcessFilterAtom {
                current_element,
                remaining_elements,
                predicate,
                filtered_results,
                outer_carrying,
                acc_bindings,
                ..
            } => {
                if let Some(elem) = current_element {
                    out.push(*elem);
                }
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*predicate);
                for (v, bindings) in filtered_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
                // H13 (2026-05-05): root-walk per-call accumulator.
                collect_bindings_values(acc_bindings, out);
            }

            Self::ProcessFoldlAtom {
                remaining_elements,
                operation,
                acc_bindings,
                ..
            } => {
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*operation);
                // H14 (2026-05-05): root-walk accumulated fold bindings.
                collect_bindings_values(acc_bindings, out);
            }

            Self::ProcessIfCondition {
                then_branch,
                else_branch,
                outer_bindings,
                outer_carrying,
                outer_demand: _,
                ..
            } => {
                out.push(*then_branch);
                out.push(*else_branch);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCaseAtom {
                cases,
                outer_bindings,
                outer_carrying,
                ..
            } => {
                out.push(*cases);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessEvalEval { outer_carrying, .. } => {
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }
            Self::ProcessReturn { outer_carrying, .. } => {
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessChainExpr {
                var,
                body,
                outer_bindings,
                outer_carrying,
                ..
            } => {
                out.push(*var);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessChainBody {
                remaining_values,
                var,
                body,
                outer_bindings,
                outer_carrying,
                results,
                ..
            } => {
                for (v, bindings) in remaining_values.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*var);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessFunction { outer_carrying, .. } => {
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }
            Self::ProcessIsError { outer_carrying, .. } => {
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCatch {
                default,
                outer_carrying,
                ..
            } => {
                out.push(*default);
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessConjunction {
                remaining_goals,
                accumulated_results,
                outer_carrying,
                ..
            } => {
                out.extend(remaining_goals.as_slice().iter().copied());
                for (v, bindings) in accumulated_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk caller-scope bindings.
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessUnifyPattern1 {
                pattern2,
                success_body,
                failure_body,
                outer_carrying,
                ..
            } => {
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessUnifyPattern1Iter {
                remaining_pattern1_results,
                pattern2,
                success_body,
                failure_body,
                all_results,
                outer_carrying,
                ..
            } => {
                for (v, bindings) in remaining_pattern1_results.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
                for (v, bindings) in all_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessUnifyPattern2 {
                val1,
                pattern2,
                success_body,
                failure_body,
                outer_carrying,
                ..
            } => {
                out.push(*val1);
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessUnifyBodies {
                remaining_bodies,
                results,
                outer_carrying,
                ..
            } => {
                out.extend(remaining_bodies.as_slice().iter().copied());
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCollapse { outer_carrying, .. } => {
                collect_bindings_values(outer_carrying, out);
            }
            Self::ProcessCollapseBind { outer_carrying, .. } => {
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCollapseEvalResults {
                remaining_raw,
                evaluated,
                current_raw_bindings,
                outer_carrying,
                ..
            } => {
                for (v, bindings) in remaining_raw.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in evaluated.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                // H14 (2026-05-05): root-walk current iteration's bindings.
                collect_bindings_values(current_raw_bindings, out);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessAmb {
                remaining_alts,
                results,
                outer_carrying,
                ..
            } => {
                for (v, bindings) in remaining_alts.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::WaitForParallel {
                handle,
                base_results,
                outer_carrying,
                stable_branches_snapshot,
                ..
            } => {
                // 1. Walk the input branches snapshot (mirrors what
                //    `collect_parallel_branch_frame_roots` does for the
                //    frame_chain-registered entry — the two collectors fire
                //    independently from the safepoint, both must report the
                //    same roots so this is intentional).
                for (value, bindings) in stable_branches_snapshot.iter() {
                    out.push(*value);
                    collect_bindings_values(bindings, out);
                }
                // 2. Walk any partial results that branch workers have
                //    written so far (slots are `Option<Vec<BoundValue>>`).
                let guard = handle
                    .results
                    .lock()
                    .expect("parallel results mutex poisoned");
                for slot in guard.iter().flatten() {
                    for (v, bindings) in slot.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                drop(guard);
                // 3. Walk caller-side accumulators.
                for (v, bindings) in base_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::WaitForParallelCollapse {
                handle,
                stable_items_snapshot,
                outer_carrying,
                ..
            } => {
                // 1. Walk the input collapse items.
                for (value, bindings) in stable_items_snapshot.iter() {
                    out.push(*value);
                    collect_bindings_values(bindings, out);
                }
                // 2. Walk any partial results.
                let guard = handle
                    .results
                    .lock()
                    .expect("parallel collapse results mutex poisoned");
                for slot in guard.iter().flatten() {
                    for (v, bindings) in slot.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                drop(guard);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGuard { outer_carrying, .. } => {
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGetAtoms {
                space_ref,
                outer_carrying,
                ..
            } => {
                out.push(*space_ref);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGetTypeSpace {
                space_ref,
                atom,
                call_form,
                outer_carrying,
                ..
            } => {
                out.push(*space_ref);
                out.push(*atom);
                out.push(*call_form);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMemoTable {
                memo_ref,
                expr,
                outer_carrying,
                ..
            } => {
                out.push(*memo_ref);
                out.push(*expr);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMemoExpr {
                expr,
                outer_carrying,
                ..
            } => {
                out.push(*expr);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessNewMemoName {
                name_arg,
                size_arg,
                outer_carrying,
                ..
            } => {
                out.push(*name_arg);
                if let Some(size) = size_arg {
                    out.push(*size);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessNewMemoSize {
                size_arg,
                outer_carrying,
                ..
            } => {
                out.push(*size_arg);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMemoOp {
                memo_ref,
                outer_carrying,
                ..
            } => {
                out.push(*memo_ref);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMatchSpace {
                space_arg,
                pattern,
                template,
                outer_carrying,
                ..
            } => {
                out.push(*space_arg);
                out.push(*pattern);
                out.push(*template);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMatchTemplates {
                remaining_templates,
                results,
                outer_carrying,
                ..
            } => {
                out.extend(remaining_templates.as_slice().iter().copied());
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessAddAtomSpace {
                space_ref,
                atom,
                outer_carrying,
                ..
            } => {
                out.push(*space_ref);
                out.push(*atom);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessRemoveAtomSpace {
                space_ref,
                atom,
                outer_carrying,
                ..
            } => {
                out.push(*space_ref);
                out.push(*atom);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessNewState {
                initial_value,
                outer_carrying,
                ..
            } => {
                out.push(*initial_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGetState {
                state_ref,
                outer_carrying,
                ..
            } => {
                out.push(*state_ref);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessChangeStateRef {
                state_ref,
                new_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_ref);
                out.push(*new_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessChangeStateValue {
                state_value,
                new_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_value);
                out.push(*new_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCasStateRef {
                state_ref,
                expected,
                new_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_ref);
                out.push(*expected);
                out.push(*new_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCasExpected {
                state_value,
                expected_value,
                new_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_value);
                out.push(*expected_value);
                out.push(*new_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCasNewValue {
                state_value,
                expected_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_value);
                out.push(*expected_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessLoopStateRef {
                state_ref,
                target,
                outer_carrying,
                ..
            } => {
                out.push(*state_ref);
                out.push(*target);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessLoopTarget {
                state_value,
                target_value,
                outer_carrying,
                ..
            } => {
                out.push(*state_value);
                out.push(*target_value);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessRepr {
                atom,
                outer_carrying,
                ..
            } => {
                out.push(*atom);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessFormatArgsString {
                format_arg,
                args_arg,
                outer_carrying,
                ..
            } => {
                out.push(*format_arg);
                out.push(*args_arg);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessFormatArgsArgs {
                args_arg,
                outer_carrying,
                ..
            } => {
                out.push(*args_arg);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessPrintln {
                atom,
                outer_carrying,
                ..
            } => {
                out.push(*atom);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessTraceMessage {
                message,
                value_expr,
                outer_carrying,
                ..
            } => {
                out.push(*message);
                out.push(*value_expr);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessTraceValue {
                value_expr,
                outer_carrying,
                ..
            } => {
                out.push(*value_expr);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessGetMetatype {
                atom,
                outer_carrying,
                ..
            } => {
                out.push(*atom);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessBind { outer_carrying, .. } => {
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessIfReducible {
                original_expr,
                then_branch,
                else_branch,
                outer_carrying,
                ..
            } => {
                out.push(*original_expr);
                out.push(*then_branch);
                out.push(*else_branch);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessMatchOrSpace {
                space_arg,
                pattern,
                default,
                template,
                outer_carrying,
                ..
            } => {
                out.push(*space_arg);
                out.push(*pattern);
                out.push(*default);
                out.push(*template);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessSortTuple {
                sorted,
                unsorted,
                current,
                comparator,
                outer_carrying,
                ..
            } => {
                out.extend(sorted.iter().copied());
                out.extend(unsorted.iter().copied());
                out.push(*current);
                out.push(*comparator);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessBestCandidate {
                best,
                remaining,
                current,
                rank_fn,
                outer_carrying,
                ..
            } => {
                if let Some(b) = best {
                    out.push(*b);
                }
                out.extend(remaining.as_slice().iter().copied());
                out.push(*current);
                out.push(*rank_fn);
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCaseMultiResults {
                remaining_atoms,
                cases,
                collected,
                outer_carrying,
                ..
            } => {
                for (v, bindings) in remaining_atoms.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*cases);
                for (v, bindings) in collected.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }

            Self::ProcessCaseEvalScrutineeResults {
                remaining_raw,
                evaluated,
                cases,
                current_raw_bindings,
                outer_carrying,
                ..
            } => {
                for (v, bindings) in remaining_raw.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in evaluated.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*cases);
                collect_bindings_values(current_raw_bindings, out);
                collect_bindings_values(outer_carrying, out);
            }

            Self::MemoizeResult { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::ReexportLetBindings { reexport, .. } => {
                collect_bindings_values(reexport, out);
            }

            Self::ProcessLetStar {
                current_pattern,
                remaining_pairs,
                body,
                accumulated_bindings,
                ..
            } => {
                out.push(*current_pattern);
                for (pattern, value_expr) in remaining_pairs {
                    out.push(*pattern);
                    out.push(*value_expr);
                }
                out.push(*body);
                collect_bindings_values(accumulated_bindings, out);
            }

            Self::ProcessRuleMatchesLazy {
                coroutine,
                results,
                current_branch_bindings,
                outer_carrying,
                ..
            } => {
                coroutine.collect_values(out);
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(current_branch_bindings, out);
                collect_bindings_values(outer_carrying, out);
            }

            Self::CompleteSubgoal { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::CompleteThunk { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::CollectFreezeArgs {
                args,
                evaluated_results,
                outer_carrying,
                ..
            } => {
                for v in args.iter() {
                    out.push(*v);
                }
                for results in evaluated_results.iter() {
                    for (v, bindings) in results.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                collect_bindings_values(outer_carrying, out);
            }
        }
    }

    /// Increment D (C2): abstract-GC live-variable marking (Might–Shivers) over the reified
    /// K-frame. Identical roots to [`collect_values`] EXCEPT it narrows the THREE post-cut
    /// variants, skipping the one iterator field a FIRED cut has provably made dead — the
    /// variant's advance arm takes the commit branch that DROPS that iterator on the next
    /// transition (eval_loop.rs `ProcessRuleMatches`:8506 / `ProcessAmb`:14734 /
    /// `ProcessMatchTemplates`:15578), and no other transition reads it. When the cut has
    /// NOT fired — and for EVERY other (and future) variant via the `_` delegate — it is
    /// BYTE-IDENTICAL to `collect_values`, so `collect_live_values ⊆ collect_values` ALWAYS
    /// and equal absent a fired cut. Used ONLY by the MIDLOOP root-build
    /// (`collect_machine_roots_live`): at quiescence K is empty, so there is nothing to
    /// narrow. The `_` catch-all full-walks every non-narrowed variant, so a narrowing can
    /// never silently under-root (the soundness default). `cut_fired_peek` is a pure
    /// thread-local read on the sole eval thread at the midloop safepoint (consistent).
    pub fn collect_live_values(&self, out: &mut Vec<MettaValue>) {
        use crate::backend::eval::trampoline::eval_loop::cut_fired_peek;
        match self {
            Self::ProcessRuleMatches {
                remaining_matches,
                results,
                current_branch_bindings,
                outer_carrying,
                cut_barrier,
                ..
            } => {
                // `remaining_matches` is dead once the cut fired for this barrier.
                if !cut_fired_peek(*cut_barrier) {
                    for (rhs, bindings) in remaining_matches.as_slice() {
                        out.push(*rhs);
                        collect_bindings_values(bindings, out);
                    }
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(current_branch_bindings, out);
                collect_bindings_values(outer_carrying, out);
            }
            Self::ProcessAmb {
                remaining_alts,
                results,
                outer_carrying,
                cut_barrier,
                ..
            } => {
                if !cut_fired_peek(*cut_barrier) {
                    for (v, bindings) in remaining_alts.as_slice().iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }
            Self::ProcessMatchTemplates {
                remaining_templates,
                results,
                outer_carrying,
                cut_barrier,
                ..
            } => {
                if !cut_fired_peek(*cut_barrier) {
                    out.extend(remaining_templates.as_slice().iter().copied());
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                collect_bindings_values(outer_carrying, out);
            }
            // Every other (and future) variant: conservative full walk — no narrowing.
            _ => self.collect_values(out),
        }
    }

    /// Return the depth hint from whichever variant is active.
    ///
    /// Used by the cooperative yield logic to record the evaluation depth
    /// at the suspension point for priority scheduling. All variants except
    /// `Done` carry a `depth` field.
    pub fn depth_hint(&self) -> usize {
        match self {
            Self::Done => 0,
            Self::CollectSExpr { depth, .. }
            | Self::ProcessRuleMatches { depth, .. }
            | Self::ProcessRuleMatchesLazy { depth, .. }
            | Self::ProcessGroundedOp { depth, .. }
            | Self::ProcessGroundedOpFanout { depth, .. }
            | Self::ProcessCombinations { depth, .. }
            | Self::ProcessCombinationsBound { depth, .. }
            | Self::ProcessLet { depth, .. }
            | Self::ProcessOnceRestore { depth, .. }
            | Self::CollectGroundedArg { depth, .. }
            | Self::CollectApplicativeResults { depth, .. }
            | Self::ProcessMapAtom { depth, .. }
            | Self::ProcessFilterAtom { depth, .. }
            | Self::ProcessFoldlAtom { depth, .. }
            | Self::ProcessIfCondition { depth, .. }
            | Self::ProcessCaseAtom { depth, .. }
            | Self::ProcessEvalEval { depth, .. }
            | Self::ProcessReturn { depth, .. }
            | Self::ProcessChainExpr { depth, .. }
            | Self::ProcessChainBody { depth, .. }
            | Self::ProcessFunction { depth, .. }
            | Self::ProcessIsError { depth, .. }
            | Self::ProcessCatch { depth, .. }
            | Self::ProcessConjunction { depth, .. }
            | Self::ProcessUnifyPattern1 { depth, .. }
            | Self::ProcessUnifyPattern1Iter { depth, .. }
            | Self::ProcessUnifyPattern2 { depth, .. }
            | Self::ProcessUnifyBodies { depth, .. }
            | Self::ProcessCollapse { depth, .. }
            | Self::ProcessCollapseBind { depth, .. }
            | Self::ProcessCollapseEvalResults { depth, .. }
            | Self::ProcessAmb { depth, .. }
            | Self::WaitForParallel { depth, .. }
            | Self::WaitForParallelCollapse { depth, .. }
            | Self::ProcessGuard { depth, .. }
            | Self::ProcessGetAtoms { depth, .. }
            | Self::ProcessGetTypeSpace { depth, .. }
            | Self::ProcessMemoTable { depth, .. }
            | Self::ProcessMemoExpr { depth, .. }
            | Self::ProcessNewMemoName { depth, .. }
            | Self::ProcessNewMemoSize { depth, .. }
            | Self::ProcessMemoOp { depth, .. }
            | Self::ProcessBind { depth, .. }
            | Self::ProcessMatchSpace { depth, .. }
            | Self::ProcessMatchTemplates { depth, .. }
            | Self::ProcessAddAtomSpace { depth, .. }
            | Self::ProcessRemoveAtomSpace { depth, .. }
            | Self::ProcessNewState { depth, .. }
            | Self::ProcessGetState { depth, .. }
            | Self::ProcessChangeStateRef { depth, .. }
            | Self::ProcessChangeStateValue { depth, .. }
            | Self::ProcessCasStateRef { depth, .. }
            | Self::ProcessCasExpected { depth, .. }
            | Self::ProcessCasNewValue { depth, .. }
            | Self::ProcessLoopStateRef { depth, .. }
            | Self::ProcessLoopTarget { depth, .. }
            | Self::ProcessRepr { depth, .. }
            | Self::ProcessFormatArgsString { depth, .. }
            | Self::ProcessFormatArgsArgs { depth, .. }
            | Self::ProcessPrintln { depth, .. }
            | Self::ProcessTraceMessage { depth, .. }
            | Self::ProcessTraceValue { depth, .. }
            | Self::ProcessGetMetatype { depth, .. }
            | Self::ProcessIfReducible { depth, .. }
            | Self::ProcessMatchOrSpace { depth, .. }
            | Self::ProcessSortTuple { depth, .. }
            | Self::ProcessBestCandidate { depth, .. }
            | Self::ProcessCaseMultiResults { depth, .. }
            | Self::ProcessCaseEvalScrutineeResults { depth, .. }
            | Self::MemoizeResult { depth, .. }
            | Self::ReexportLetBindings { depth, .. }
            | Self::ProcessLetStar { depth, .. }
            | Self::CompleteSubgoal { depth, .. }
            | Self::CompleteThunk { depth, .. }
            | Self::CollectFreezeArgs { depth, .. } => *depth,
        }
    }

    /// Return a stable static name for this continuation variant.
    /// Used by eval-trace binding-flow instrumentation to label
    /// `ContinuationEnter` / `Emit` events.
    #[cfg(feature = "trace")]
    pub fn discriminant_name(&self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::CollectSExpr { .. } => "CollectSExpr",
            Self::ProcessRuleMatches { .. } => "ProcessRuleMatches",
            Self::ProcessRuleMatchesLazy { .. } => "ProcessRuleMatchesLazy",
            Self::ProcessGroundedOp { .. } => "ProcessGroundedOp",
            Self::ProcessGroundedOpFanout { .. } => "ProcessGroundedOpFanout",
            Self::ProcessCombinations { .. } => "ProcessCombinations",
            Self::ProcessCombinationsBound { .. } => "ProcessCombinationsBound",
            Self::ProcessLet { .. } => "ProcessLet",
            Self::ProcessOnceRestore { .. } => "ProcessOnceRestore",
            Self::CollectGroundedArg { .. } => "CollectGroundedArg",
            Self::CollectApplicativeResults { .. } => "CollectApplicativeResults",
            Self::ProcessMapAtom { .. } => "ProcessMapAtom",
            Self::ProcessFilterAtom { .. } => "ProcessFilterAtom",
            Self::ProcessFoldlAtom { .. } => "ProcessFoldlAtom",
            Self::ProcessIfCondition { .. } => "ProcessIfCondition",
            Self::ProcessCaseAtom { .. } => "ProcessCaseAtom",
            Self::ProcessEvalEval { .. } => "ProcessEvalEval",
            Self::ProcessReturn { .. } => "ProcessReturn",
            Self::ProcessChainExpr { .. } => "ProcessChainExpr",
            Self::ProcessChainBody { .. } => "ProcessChainBody",
            Self::ProcessFunction { .. } => "ProcessFunction",
            Self::ProcessIsError { .. } => "ProcessIsError",
            Self::ProcessCatch { .. } => "ProcessCatch",
            Self::ProcessConjunction { .. } => "ProcessConjunction",
            Self::ProcessUnifyPattern1 { .. } => "ProcessUnifyPattern1",
            Self::ProcessUnifyPattern1Iter { .. } => "ProcessUnifyPattern1Iter",
            Self::ProcessUnifyPattern2 { .. } => "ProcessUnifyPattern2",
            Self::ProcessUnifyBodies { .. } => "ProcessUnifyBodies",
            Self::ProcessCollapse { .. } => "ProcessCollapse",
            Self::ProcessCollapseBind { .. } => "ProcessCollapseBind",
            Self::ProcessCollapseEvalResults { .. } => "ProcessCollapseEvalResults",
            Self::ProcessAmb { .. } => "ProcessAmb",
            Self::WaitForParallel { .. } => "WaitForParallel",
            Self::WaitForParallelCollapse { .. } => "WaitForParallelCollapse",
            Self::ProcessGuard { .. } => "ProcessGuard",
            Self::ProcessGetAtoms { .. } => "ProcessGetAtoms",
            Self::ProcessGetTypeSpace { .. } => "ProcessGetTypeSpace",
            Self::ProcessMemoTable { .. } => "ProcessMemoTable",
            Self::ProcessMemoExpr { .. } => "ProcessMemoExpr",
            Self::ProcessNewMemoName { .. } => "ProcessNewMemoName",
            Self::ProcessNewMemoSize { .. } => "ProcessNewMemoSize",
            Self::ProcessMemoOp { .. } => "ProcessMemoOp",
            Self::ProcessBind { .. } => "ProcessBind",
            Self::ProcessMatchSpace { .. } => "ProcessMatchSpace",
            Self::ProcessMatchTemplates { .. } => "ProcessMatchTemplates",
            Self::ProcessAddAtomSpace { .. } => "ProcessAddAtomSpace",
            Self::ProcessRemoveAtomSpace { .. } => "ProcessRemoveAtomSpace",
            Self::ProcessNewState { .. } => "ProcessNewState",
            Self::ProcessGetState { .. } => "ProcessGetState",
            Self::ProcessChangeStateRef { .. } => "ProcessChangeStateRef",
            Self::ProcessChangeStateValue { .. } => "ProcessChangeStateValue",
            Self::ProcessCasStateRef { .. } => "ProcessCasStateRef",
            Self::ProcessCasExpected { .. } => "ProcessCasExpected",
            Self::ProcessCasNewValue { .. } => "ProcessCasNewValue",
            Self::ProcessLoopStateRef { .. } => "ProcessLoopStateRef",
            Self::ProcessLoopTarget { .. } => "ProcessLoopTarget",
            Self::ProcessRepr { .. } => "ProcessRepr",
            Self::ProcessFormatArgsString { .. } => "ProcessFormatArgsString",
            Self::ProcessFormatArgsArgs { .. } => "ProcessFormatArgsArgs",
            Self::ProcessPrintln { .. } => "ProcessPrintln",
            Self::ProcessTraceMessage { .. } => "ProcessTraceMessage",
            Self::ProcessTraceValue { .. } => "ProcessTraceValue",
            Self::ProcessGetMetatype { .. } => "ProcessGetMetatype",
            Self::ProcessIfReducible { .. } => "ProcessIfReducible",
            Self::ProcessMatchOrSpace { .. } => "ProcessMatchOrSpace",
            Self::ProcessSortTuple { .. } => "ProcessSortTuple",
            Self::ProcessBestCandidate { .. } => "ProcessBestCandidate",
            Self::ProcessCaseMultiResults { .. } => "ProcessCaseMultiResults",
            Self::ProcessCaseEvalScrutineeResults { .. } => "ProcessCaseEvalScrutineeResults",
            Self::MemoizeResult { .. } => "MemoizeResult",
            Self::ReexportLetBindings { .. } => "ReexportLetBindings",
            Self::ProcessLetStar { .. } => "ProcessLetStar",
            Self::CompleteSubgoal { .. } => "CompleteSubgoal",
            Self::CompleteThunk { .. } => "CompleteThunk",
            Self::CollectFreezeArgs { .. } => "CollectFreezeArgs",
        }
    }
}

// ============================================================================
// Tests for GC Root Collection
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValueFactory};
    use smallvec::smallvec;

    fn factory() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

    fn env() -> SharedEnv {
        std::sync::Arc::new(crate::backend::environment::MettaEnvironment::new(factory()))
    }

    /// Increment D (C2): the abstract-GC narrowing is SOUND on the primary narrowed
    /// variant — `collect_live_values` equals `collect_values` when no cut has fired, and
    /// drops EXACTLY the dead `remaining_matches` (keeping `results`) once the cut fired
    /// for the frame's barrier. `ProcessAmb` / `ProcessMatchTemplates` use the IDENTICAL
    /// `if !cut_fired_peek(*cut_barrier) { skip }` pattern (mirroring their `collect_values`
    /// arms), additionally validated end-to-end by the MIDLOOP live-superset oracle over
    /// the corpus + cut fixtures (D-2). `as_slice()` is non-consuming, so one frame serves
    /// every collection. Gated `not(trace)` so the trace-only frame fields need not be built.
    #[cfg(not(feature = "trace"))]
    #[test]
    fn collect_live_values_narrows_process_rule_matches_on_cut() {
        use crate::backend::eval::trampoline::eval_loop::force_cut_signal_for_test;
        let f = factory();
        const B: u64 = 42; // a nonzero cut barrier
        let frame = Continuation::ProcessRuleMatches {
            remaining_matches: vec![
                (f.long(1), GenericBindings::new()),
                (f.long(2), GenericBindings::new()),
            ]
            .into_iter(),
            results: vec![(f.long(3), GenericBindings::new())],
            env: env(),
            depth: 0,
            pre_fork_epoch: 0,
            pre_fork_gen: 0,
            fork_depth: 0,
            cut_barrier: B,
            saved_barrier: 0,
            current_branch_bindings: empty_shared_bindings(),
            outer_carrying: empty_shared_bindings(),
            tracked_vars_hint: None,
        };
        let longs = |v: &[MettaValue]| {
            let mut xs: Vec<i64> = v.iter().filter_map(|x| x.as_long()).collect();
            xs.sort_unstable();
            xs
        };

        // NOT fired (signal 0 != B): live == full == {1,2,3}.
        force_cut_signal_for_test(0);
        let (mut live, mut full) = (Vec::new(), Vec::new());
        frame.collect_live_values(&mut live);
        frame.collect_values(&mut full);
        assert_eq!(longs(&live), longs(&full), "no cut: live == full");
        assert_eq!(longs(&live), vec![1, 2, 3], "no cut: all matches kept");

        // FIRED (signal == B): live drops the dead remaining_matches {1,2}, keeps results {3};
        // collect_values is unchanged; live ⊆ full (the safety invariant).
        force_cut_signal_for_test(B);
        let (mut live, mut full) = (Vec::new(), Vec::new());
        frame.collect_live_values(&mut live);
        frame.collect_values(&mut full);
        assert_eq!(longs(&live), vec![3], "cut fired: only results remain (matches are dead)");
        assert_eq!(longs(&full), vec![1, 2, 3], "collect_values is unchanged by the cut");
        for x in longs(&live) {
            assert!(longs(&full).contains(&x), "live ⊆ full");
        }
        force_cut_signal_for_test(0); // thread-local hygiene for the shared test thread
    }

    #[test]
    fn test_work_item_eval_collects_value() {
        let f = factory();
        let item = WorkItem::Eval {
            value: f.long(42),
            env: env(),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        item.collect_values(&mut roots);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].as_long(), Some(42));
    }

    #[test]
    fn test_work_item_resume_collects_results() {
        let f = factory();
        let item = WorkItem::Resume {
            result: (
                smallvec![bv(f.long(1)), bv(f.long(2)), bv(f.long(3))],
                env(),
            ),
        };
        let mut roots = Vec::new();
        item.collect_values(&mut roots);
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].as_long(), Some(1));
        assert_eq!(roots[1].as_long(), Some(2));
        assert_eq!(roots[2].as_long(), Some(3));
    }

    #[test]
    fn test_continuation_done_collects_nothing() {
        let cont = Continuation::Done;
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_continuation_collect_sexpr_collects_all() {
        let f = factory();
        let cont = Continuation::CollectSExpr {
            remaining: vec![f.long(10), f.long(20)].into_iter(),
            collected: vec![
                (smallvec![bv(f.long(30))], env()),
                (smallvec![bv(f.long(40)), bv(f.long(50))], env()),
            ],
            original_env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 remaining + 1 + 2 collected = 5
        assert_eq!(roots.len(), 5);
    }

    #[test]
    fn test_continuation_if_condition_collects_branches() {
        let f = factory();
        let cont = Continuation::ProcessIfCondition {
            then_branch: f.long(100),
            else_branch: f.long(200),
            outer_bindings: None,
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
            outer_demand: crate::backend::eval::cesk::coroutine::Demand::All,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].as_long(), Some(100));
        assert_eq!(roots[1].as_long(), Some(200));
    }

    #[test]
    fn test_continuation_let_collects_pattern_body_results() {
        let f = factory();
        let cont = Continuation::ProcessLet {
            pending_values: Some(vec![bv(f.atom("a")), bv(f.atom("b"))]),
            pattern: f.atom("$x"),
            body: f.atom("body"),
            outer_bindings: None,
            results: vec![bv(f.long(1))],
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 pending + 1 pattern + 1 body + 1 result = 5
        assert_eq!(roots.len(), 5);
    }

    #[test]
    fn test_continuation_process_bind_collects_nothing() {
        let cont = Continuation::ProcessBind {
            token: "var".to_string(),
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_continuation_match_space_collects_three_values() {
        let f = factory();
        let cont = Continuation::ProcessMatchSpace {
            space_arg: f.atom("&self"),
            pattern: f.atom("$p"),
            template: f.atom("$t"),
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
            cut_barrier: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_continuation_best_candidate_collects_outer_carrying() {
        let f = factory();
        let mut carrying = GenericBindings::new();
        carrying.insert("$Tasks", f.atom("live-task-list"));
        let cont = Continuation::ProcessBestCandidate {
            best: Some(f.long(1)),
            best_rank: Some(1.0),
            remaining: vec![f.long(2), f.long(3)].into_iter(),
            current: f.long(4),
            var_name: "$x".to_string(),
            rank_fn: f.atom("PriorityRank"),
            env: env(),
            depth: 0,
            outer_carrying: std::sync::Arc::new(carrying),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(
            roots.iter().any(|v| v.as_atom() == Some("live-task-list")),
            "ProcessBestCandidate must root values reachable through outer_carrying"
        );
    }

    #[test]
    fn test_collect_applicative_results_collects_remaining_bindings() {
        let f = factory();
        let mut pending = GenericBindings::new();
        pending.insert("$X", f.atom("live-pending-binding"));
        let cont = Continuation::CollectApplicativeResults {
            remaining: vec![f.atom("next-combo")].into_iter(),
            remaining_bindings: vec![pending],
            results: Vec::new(),
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };

        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(
            roots
                .iter()
                .any(|v| v.as_atom() == Some("live-pending-binding")),
            "CollectApplicativeResults must root values held in remaining_bindings"
        );
    }

    #[test]
    fn test_continuation_collapse_eval_results() {
        let f = factory();
        let mut outer = GenericBindings::new();
        outer.insert("$Outer", f.atom("live-collapse-outer"));
        let cont = Continuation::ProcessCollapseEvalResults {
            remaining_raw: vec![bv(f.long(1)), bv(f.long(2))].into_iter(),
            evaluated: vec![bv(f.long(3))],
            is_bind: false,
            current_raw_bindings: empty_shared_bindings(),
            env: env(),
            depth: 0,
            tracked_vars_hint: None,
            outer_carrying: std::sync::Arc::new(outer),
            sort_results: true,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 remaining + 1 evaluated + 1 outer binding.
        assert_eq!(roots.len(), 4);
        assert!(
            roots
                .iter()
                .any(|v| v.as_atom() == Some("live-collapse-outer")),
            "ProcessCollapseEvalResults must root values reachable through outer_carrying"
        );
    }

    #[test]
    fn test_continuation_unify_pattern1_iter_collects_all() {
        let f = factory();
        let cont = Continuation::ProcessUnifyPattern1Iter {
            remaining_pattern1_results: vec![bv(f.long(1))].into_iter(),
            pattern2: f.atom("p2"),
            success_body: f.atom("ok"),
            failure_body: f.atom("fail"),
            all_results: vec![bv(f.long(99))],
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 1 remaining + 1 pattern2 + 1 success + 1 failure + 1 result = 5
        assert_eq!(roots.len(), 5);
    }
}
