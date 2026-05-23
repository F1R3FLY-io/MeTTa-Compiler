//! Evaluation Context for Trampoline Engine
//!
//! This module provides the `EvalContext` trait that encapsulates behavioral
//! differences between evaluation contexts (GC policy, safepoints, tracing,
//! compiled dispatch). All contexts use the same value type (`MettaValue`)
//! and factory (`GcFactory`).
//!
//! ## Context Implementation
//!
//! `StaticEvalContext` is the production context using the global slab allocator
//! via `GcFactory` for zero-conversion evaluation.

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use std::cell::{Cell, RefCell};

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{
    alloc_count_snapshot, global_factory, register_temporary_roots, request_gc, GcFactory,
    MettaValue,
};

/// Evaluation context for the trampoline engine.
///
/// All contexts use `MettaValue` and `GcFactory`. The trait captures
/// behavioral differences: GC policy, safepoints, tracing, and
/// compiled dispatch.
pub trait EvalContext {
    /// Get a reference to the factory for constructing values.
    fn factory(&self) -> &GcFactory;

    /// Hint to the context that it may trigger GC if memory pressure is high.
    ///
    /// Called periodically from the trampoline loop (every 256 iterations).
    /// Default implementation is a no-op.
    #[inline]
    fn maybe_gc(&self) {
        // no-op by default
    }

    /// Check if a GC safepoint should be taken.
    ///
    /// Called every 4096 trampoline iterations. Returns `true` if the evaluator
    /// should pause, register trampoline roots, release the EvalGuard, and
    /// allow the quiescent GC to fire.
    ///
    /// Default honors `gc_allocator::is_gc_requested()` — when GC has
    /// explicitly asked for cooperation, every production context should
    /// respond, regardless of its own per-thread alloc-delta. Contexts
    /// that need richer accounting (per-worker alloc-delta tracking,
    /// cancel-token observation) should override; contexts that should
    /// never safepoint (tests, static no-op adapters) can override to
    /// always return `false`.
    #[inline]
    fn should_safepoint(&self) -> bool {
        crate::backend::models::gc_allocator::is_gc_requested()
    }

    /// Perform a GC safepoint with the provided roots from trampoline state.
    ///
    /// Default runs the Phase 9 purely-async protocol:
    /// 1. Register `roots` as temporary GC roots (held until the function returns).
    /// 2. Signal `gc_requested` so the cron monitor's next tick will fire
    ///    `maybe_async_gc()` opportunistically.
    ///
    /// The trampoline thread NEVER waits on GC progress. Mark-sweep runs
    /// asynchronously on the GC pool against a snapshot of root state at
    /// snapshot time. Phase 8 root providers (parallel-dispatch
    /// inputs/outputs) + per-thread current-iter root cell
    /// (`current_iter_root`) + this temporary-root registration give the
    /// snapshot a complete root view without requiring quiescence.
    fn perform_safepoint(&self, roots: Vec<MettaValue>) {
        let _root_handle = crate::backend::models::register_temporary_roots(roots);
        crate::backend::models::request_gc();
    }

    /// Get the trace collector for emitting evaluation trace events.
    #[cfg(feature = "trace")]
    #[inline]
    fn trace_collector(&self) -> Option<&crate::backend::trace::TraceCollector> {
        None
    }

    /// Try to dispatch a sub-expression to compiled bytecode/JIT.
    ///
    /// Called from the trampoline for S-expressions with compilable heads.
    /// Returns `Some((results, new_env))` on successful dispatch, `None` to
    /// fall through to the tree-walker.
    #[inline]
    fn try_compiled_dispatch(
        &self,
        _value: &MettaValue,
        _env: &MettaEnvironment,
        _compilation_hash: u64,
    ) -> Option<(Vec<MettaValue>, MettaEnvironment)> {
        None
    }

    /// 2026-05-23 PT-canonical binding-thread variant of `try_compiled_dispatch`.
    /// Returns per-result bindings so the T0 trampoline can thread VM-tier
    /// per-alt caller-scope bindings into its continuation context.
    #[inline]
    fn try_compiled_dispatch_with_bindings(
        &self,
        _value: &MettaValue,
        _env: &MettaEnvironment,
        _compilation_hash: u64,
    ) -> Option<(
        Vec<(MettaValue, crate::backend::models::GenericBindings<MettaValue>)>,
        MettaEnvironment,
    )> {
        None
    }
}

// ============================================================================
// Static Arena Context - Global Slab Allocator Evaluation
// ============================================================================

/// Static arena-based evaluation context using the global `GcFactory`.
///
/// This context uses the process-wide `SlabAllocator` via `GcFactory` for
/// all value allocation. `MettaValue` satisfies the `'static` bound
/// required by `GenericEnvironment` and `EvalContext`.
///
/// # Thread Safety
///
/// `GcFactory` is backed by the lock-free global `SlabAllocator`, so it is
/// safe to use from any thread without additional synchronization.
///
/// # Memory Management
///
/// Values are reclaimed by the background GC thread when no longer reachable.
/// No manual arena resets are needed.
#[derive(Debug, Clone, Copy)]
pub struct StaticEvalContext {
    factory: GcFactory,
}

/// Type alias for arena environment using the global GcFactory.
pub type MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>;

/// Arc-wrapped environment for O(1) sharing in continuations and work items.
/// Eliminates per-step clone/drop overhead (8.7% of CPU in DTrace profiles).
pub type SharedEnv = std::sync::Arc<MettaEnvironment>;

// Thread-local persistent environment storage for arena mode.
// This persists state (rules, facts, bindings) across sequential evaluations,
// matching heap mode behavior where environments are threaded through.
thread_local! {
    static STATIC_ENV: RefCell<Option<MettaEnvironment>> = const { RefCell::new(None) };
}

impl StaticEvalContext {
    /// Get the static arena context backed by the global slab allocator.
    #[inline]
    pub fn get() -> Self {
        Self {
            factory: global_factory(),
        }
    }

    /// Create a new MettaEnvironment for this context.
    ///
    /// Note: This creates a fresh environment every time. For persistent state
    /// across sequential evaluations, use `get_or_create_env()` instead.
    #[inline]
    pub fn new_env() -> MettaEnvironment {
        MettaEnvironment::new(global_factory())
    }

    /// Get or create the persistent thread-local environment.
    ///
    /// This environment accumulates state across sequential evaluations,
    /// matching heap mode behavior where environments are threaded through.
    /// Use this instead of `new_env()` for file evaluation where state should
    /// persist between expressions.
    ///
    /// # Returns
    ///
    /// A clone of the persistent environment. The clone shares state via Arc
    /// until first mutation (CoW semantics).
    #[inline]
    pub fn get_or_create_env() -> MettaEnvironment {
        STATIC_ENV.with(|env_cell| {
            let mut env_opt = env_cell.borrow_mut();
            if env_opt.is_none() {
                *env_opt = Some(Self::new_env());
            }
            // Return a clone that shares state via Arc (O(1) clone)
            env_opt.as_ref().expect("env was just initialized").clone()
        })
    }

    /// Update the persistent environment after evaluation.
    ///
    /// Call this after evaluation to preserve state changes (rules, facts, bindings)
    /// for subsequent evaluations. This is essential for correct arena mode semantics
    /// where state must persist across the evaluation of multiple expressions.
    #[inline]
    pub fn update_env(new_env: MettaEnvironment) {
        STATIC_ENV.with(|env_cell| {
            *env_cell.borrow_mut() = Some(new_env);
        });
    }

    /// Reset the persistent environment.
    ///
    /// Clears all accumulated state (rules, facts, bindings). Use this:
    /// - Between test cases to ensure isolation
    /// - When starting a new session
    #[inline]
    pub fn reset_env() {
        STATIC_ENV.with(|env_cell| {
            *env_cell.borrow_mut() = None;
        });
    }

    /// Get a factory for creating `MettaValue`.
    ///
    /// Returns the global `GcFactory` backed by the slab allocator.
    #[inline]
    pub fn get_factory() -> GcFactory {
        global_factory()
    }
}

impl EvalContext for StaticEvalContext {
    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    // should_safepoint / perform_safepoint inherit the trait defaults
    // (honor `is_gc_requested()`, run the canonical quiescent protocol).
}

// ============================================================================
// Parallel Branch Context - Trace-Aware Context for Worker Threads
// ============================================================================

/// Per-worker safepoint allocation threshold.
///
/// Mirrors `SessionContext`'s `SAFEPOINT_ALLOC_THRESHOLD` (500K). The threshold
/// compares against the global slab `alloc_count` so all participants (workers
/// + parent) converge on the same crossing — when one drops its guard for a
/// safepoint, the others are likely to do the same within the 10ms condvar
/// window.
const PARALLEL_SAFEPOINT_THRESHOLD: u64 = 500_000;

/// Per-worker safepoint threshold.
#[inline]
fn parallel_safepoint_threshold() -> u64 {
    PARALLEL_SAFEPOINT_THRESHOLD
}

/// Whether parallel-branch GC cooperation is enabled.
/// Parallel workers always participate in GC cooperation.
#[inline]
pub(super) fn parallel_gc_coop_enabled() -> bool {
    true
}

/// Evaluation context for parallel branch worker threads.
///
/// Each worker thread gets its own `ParallelBranchContext` instance with
/// its own per-worker `last_safepoint_allocs` counter. The context drives
/// GC safepoints independently of the parent's `SessionContext`:
///
/// - `should_safepoint` — returns `true` when the global slab alloc count
///   has grown by `parallel_safepoint_threshold()` since this worker's
///   last safepoint. Caps the per-worker `nursery_ptrs` Vec size and lets
///   workers participate in old-gen mark-sweep coordination.
/// - `perform_safepoint` — registers trampoline roots, drops the worker's
///   `EvalGuard` (decrements `ACTIVE_EVALUATORS`), waits briefly for
///   quiescence, then re-acquires. Combined with the parent thread's
///   cooperative drop in `parallel_branch_eval`'s wait loop, all
///   evaluators converge to `ACTIVE_EVALUATORS == 0` so quiescent GC
///   can fire and reclaim slab slots.
///
/// Without these overrides, `ParallelBranchContext` inherited the
/// trait defaults (`false` / no-op), starving the slab GC for the entire
/// duration of any parallel-branch evaluation. Bytehound on Robot.metta
/// showed this materializing as 33.5M-entry per-worker `nursery_ptrs`
/// Vecs (~268 MB each) live at OOM time.
///
/// Trace collector behavior is unchanged: an upgraded `Arc<TraceCollector>`
/// from the global `WORK_POOL_TRACE_COLLECTOR` keeps parallel branch
/// evaluation visible in trace files.
pub struct ParallelBranchContext {
    factory: GcFactory,
    /// Alloc count at this worker's last safepoint check.
    /// `Cell` for interior mutability — `should_safepoint` takes `&self`.
    last_safepoint_allocs: Cell<u64>,
    /// Optional cancellation token. Set by `with_cancel(...)` when the
    /// worker is spawned by a parallel-dispatch under bounded `Demand`.
    /// Workers observe `is_satisfied()` in `should_safepoint` and bail at
    /// the next safepoint via the `BranchCancelled` panic-unwind mechanism.
    cancel_token: Option<std::sync::Arc<crate::backend::eval::cesk::coroutine::CancelToken>>,
    /// Upgraded `Arc<TraceCollector>` from `WORK_POOL_TRACE_COLLECTOR`.
    /// Held as `Arc` to keep the collector alive for the context's lifetime.
    #[cfg(feature = "trace")]
    trace_collector_arc: Option<std::sync::Arc<crate::backend::trace::TraceCollector>>,
}

impl ParallelBranchContext {
    /// Create a parallel branch context with trace collection + per-worker
    /// safepoint state.
    ///
    /// Each worker calls `get()` from inside its spawned closure (one
    /// instance per worker), so each gets its own `last_safepoint_allocs`
    /// Cell. The Cell is initialized to the current global alloc count so
    /// the first safepoint check fires only after another threshold of
    /// allocations.
    #[inline]
    pub fn get() -> Self {
        Self {
            factory: global_factory(),
            last_safepoint_allocs: Cell::new(alloc_count_snapshot()),
            cancel_token: None,
            #[cfg(feature = "trace")]
            trace_collector_arc: crate::backend::models::work_pool::get_work_pool_trace_collector(),
        }
    }

    /// Variant of `get()` that attaches a cancellation token. Used by
    /// `parallel_branch_eval` and `parallel_collapse_eval` when invoked
    /// with bounded demand — once any sibling satisfies the demand, the
    /// token's `satisfied` flag flips and this worker observes it at its
    /// next safepoint.
    #[inline]
    pub fn with_cancel(
        cancel_token: std::sync::Arc<crate::backend::eval::cesk::coroutine::CancelToken>,
    ) -> Self {
        Self {
            factory: global_factory(),
            last_safepoint_allocs: Cell::new(alloc_count_snapshot()),
            cancel_token: Some(cancel_token),
            #[cfg(feature = "trace")]
            trace_collector_arc: crate::backend::models::work_pool::get_work_pool_trace_collector(),
        }
    }

    /// Borrow the optional cancellation token. Used by external callers
    /// (e.g., the worker closure in `parallel_branch_eval`) to record
    /// branch-completion eligibility against the token.
    #[inline]
    pub fn cancel_token(
        &self,
    ) -> Option<&std::sync::Arc<crate::backend::eval::cesk::coroutine::CancelToken>> {
        self.cancel_token.as_ref()
    }
}

impl EvalContext for ParallelBranchContext {
    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    /// Check whether this worker should safepoint, based on cancellation,
    /// pending GC request, or the alloc-count delta since this worker's
    /// last safepoint.
    ///
    /// Returns `true` when ANY of:
    /// 1. A sibling worker has satisfied the cancellation token (fast bail)
    /// 2. The GC has explicitly requested cooperation via `is_gc_requested()`
    ///    (pressure-driven). This generalizes the safepoint mechanism to
    ///    hot paths that allocate small-but-often: such workloads can
    ///    inflate `committed_bytes` past `gc_threshold` while individual
    ///    workers stay below `parallel_safepoint_threshold()` per-iteration,
    ///    starving GC indefinitely. Honoring `is_gc_requested()` ensures
    ///    cooperation regardless of per-thread allocation cadence.
    /// 3. The per-worker alloc-count delta crossed the per-thread threshold
    ///
    /// Mirrors `SessionContext::should_safepoint` but uses a per-worker
    /// Cell so each thread tracks its own crossings independently. Workers
    /// allocating from the same global slab will see correlated
    /// `alloc_count_snapshot()` values, so when one worker hits the
    /// threshold and decides to safepoint, the others are likely to hit
    /// it within the same 10ms condvar window — this is what enables
    /// `ACTIVE_EVALUATORS` to converge to 0.
    #[inline]
    fn should_safepoint(&self) -> bool {
        // Cancellation observation: if a sibling worker has satisfied the
        // demand, we want to safepoint ASAP so `perform_safepoint` can
        // raise the `BranchCancelled` marker and bail out of the trampoline
        // cooperatively. Checking here (rather than in the hot trampoline
        // loop) means observation is folded into the existing 4096-iter
        // GC cadence — zero hot-path cost, ~microsecond latency to bail.
        if let Some(token) = &self.cancel_token {
            if token.is_satisfied() {
                return true;
            }
        }
        if !parallel_gc_coop_enabled() {
            return false;
        }
        // Pressure-driven path: GC has explicitly asked for cooperation.
        // A single Acquire load; negligible cost when no GC is pending.
        // This generalizes safepoint cooperation to hot paths whose per-
        // iteration allocation rate stays below the per-thread alloc-delta
        // threshold but whose aggregate allocation drives `committed_bytes`
        // past `gc_threshold`.
        if crate::backend::models::gc_allocator::is_gc_requested() {
            self.last_safepoint_allocs.set(alloc_count_snapshot());
            return true;
        }
        // Alloc-delta path.
        let current = alloc_count_snapshot();
        let last = self.last_safepoint_allocs.get();
        if current.wrapping_sub(last) >= parallel_safepoint_threshold() {
            self.last_safepoint_allocs.set(current);
            true
        } else {
            false
        }
    }

    /// Perform a GC safepoint on this worker.
    ///
    /// Identical structure to `SessionContext::perform_safepoint`:
    /// 1. Register roots from the trampoline state (caller passes them in).
    /// 2. Drop this worker's `EvalGuard` so `ACTIVE_EVALUATORS` decrements.
    /// 3. Arm `request_gc` in case the cron hasn't already.
    /// 4. Wait briefly (10ms) for quiescence; if other guards are still
    ///    held, the wait times out and we move on (we'll try again next
    ///    safepoint).
    /// 5. Re-acquire the guard, blocking briefly if `GC_IN_PROGRESS` is
    ///    still set from a concurrent cycle.
    /// 6. The `_root_handle` drops here, unregistering temporary roots.
    ///
    /// Cancellation: if `cancel_token.is_satisfied()`, raise via
    /// `panic::resume_unwind(Box::new(BranchCancelled))` *after* the GC
    /// dance completes. The worker closure in `parallel_branch_eval`
    /// catches the marker via `catch_unwind` and treats it as "this
    /// branch was preempted — set its slot to None and decrement the
    /// remaining counter." Doing the GC dance first ensures we don't
    /// leave roots held across the unwind.
    fn perform_safepoint(&self, roots: Vec<MettaValue>) {
        let cancelled = self
            .cancel_token
            .as_ref()
            .map_or(false, |t| t.is_satisfied());

        if parallel_gc_coop_enabled() {
            // Phase 9 purely-async safepoint: keep ABA-sensitive cache
            // clear + temporary root registration + GC signal, but DO NOT
            // wait on quiescence. See module docs in `current_iter_root`.
            super::eval_loop::clear_aba_sensitive_caches();
            let _root_handle = register_temporary_roots(roots);
            request_gc();
        }

        if cancelled {
            // Bail cooperatively. The marker is caught by the worker
            // closure boundary in `parallel_branch_eval`; the `Resume`
            // path in the wait loop sets results[slot] = None and
            // decrements `remaining`. Budget release happens once at
            // the end of `parallel_branch_eval` regardless.
            std::panic::resume_unwind(Box::new(
                crate::backend::eval::cesk::coroutine::BranchCancelled,
            ));
        }
    }

    #[cfg(feature = "trace")]
    #[inline]
    fn trace_collector(&self) -> Option<&crate::backend::trace::TraceCollector> {
        self.trace_collector_arc.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValueFactory, MettaValueTrait};

    fn generic_create_error<C: EvalContext>(ctx: &C, msg: &str) -> MettaValue {
        let offending = ctx.factory().atom("details");
        ctx.factory().error( ctx.factory().string(msg),offending)
    }

    #[test]
    fn test_generic_function_static_arena() {
        let ctx = StaticEvalContext::get();
        let error = generic_create_error(&ctx, "test error");
        assert!(error.is_error());
    }

    #[test]
    fn test_static_arena_context_factory() {
        let ctx = StaticEvalContext::get();
        let value = ctx.factory().atom("test");
        assert!(value.is_atom());
        assert_eq!(MettaValueTrait::as_atom(&value), Some("test"));
    }

    #[test]
    fn test_static_arena_context_size() {
        // StaticEvalContext should be pointer-sized (holds one GcFactory which has one &'static ref)
        assert_eq!(
            std::mem::size_of::<StaticEvalContext>(),
            std::mem::size_of::<&()>()
        );
    }

    #[test]
    fn test_static_arena_context_env() {
        let _env = StaticEvalContext::new_env();
    }

    #[test]
    fn test_static_arena_persistent_env() {
        // Reset to ensure clean state
        StaticEvalContext::reset_env();

        // First call should create new env
        let env1 = StaticEvalContext::get_or_create_env();
        assert!(env1.owns_data == false); // Clone doesn't own data

        // Second call should return clone of same env
        let env2 = StaticEvalContext::get_or_create_env();
        assert!(std::sync::Arc::ptr_eq(&env1.shared, &env2.shared));

        // Update with a modified env
        let mut modified_env = env1.clone();
        let ctx = StaticEvalContext::get();
        modified_env.bind("test_var", ctx.factory().atom("test_value"));
        StaticEvalContext::update_env(modified_env);

        // Get should now return env with the binding
        let env3 = StaticEvalContext::get_or_create_env();
        assert!(env3.has_binding("test_var"));

        // Reset should clear everything
        StaticEvalContext::reset_env();
        let env4 = StaticEvalContext::get_or_create_env();
        assert!(!env4.has_binding("test_var"));
    }

    #[test]
    fn test_static_arena_env_persists_rules() {
        // Reset to ensure clean state
        StaticEvalContext::reset_env();

        let ctx = StaticEvalContext::get();
        let factory = ctx.factory();

        // Create env and add a rule
        let mut env = StaticEvalContext::get_or_create_env();

        let lhs = factory.sexpr(vec![factory.atom("test-fn"), factory.atom("$x")]);
        let rhs = factory.atom("result");

        env.add_rule(lhs.clone(), rhs);
        StaticEvalContext::update_env(env);

        // Get env again and verify rule persists
        let env2 = StaticEvalContext::get_or_create_env();
        let rules = env2.get_matching_rules_for_expr(&lhs);
        assert_eq!(rules.len(), 1);

        // Clean up
        StaticEvalContext::reset_env();
    }

    #[test]
    fn test_static_arena_env_persists_space_facts() {
        // Reset to ensure clean state
        StaticEvalContext::reset_env();

        let ctx = StaticEvalContext::get();
        let factory = ctx.factory();

        // Create env and add a fact to space
        let mut env = StaticEvalContext::get_or_create_env();

        let fact = factory.sexpr(vec![
            factory.atom("fact"),
            factory.atom("x"),
            factory.long(42),
        ]);

        env.add_to_space(&fact);
        StaticEvalContext::update_env(env);

        // Get env again and verify fact persists
        let env2 = StaticEvalContext::get_or_create_env();
        let pattern = factory.sexpr(vec![
            factory.atom("fact"),
            factory.atom("$var"),
            factory.atom("$val"),
        ]);
        let template = pattern.clone();
        let matches = env2.match_space(&pattern, &template);
        assert!(!matches.is_empty());

        // Clean up
        StaticEvalContext::reset_env();
    }
}
