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

use std::cell::RefCell;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{
    MettaValue, GcFactory, global_factory,
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
    #[inline]
    fn should_safepoint(&self) -> bool {
        false
    }

    /// Perform a GC safepoint with the provided roots from trampoline state.
    ///
    /// Default implementation is a no-op. Override in production contexts.
    fn perform_safepoint(&self, _roots: Vec<MettaValue>) {
        // no-op by default — non-production contexts don't safepoint
    }

    /// Get the trace collector for emitting evaluation trace events.
    #[cfg(feature = "eval-trace")]
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
}

// ============================================================================
// Parallel Branch Context - Trace-Aware Context for Worker Threads
// ============================================================================

/// Evaluation context for parallel branch worker threads.
///
/// Wraps `StaticEvalContext` and overrides `trace_collector()` to use the
/// global `WORK_POOL_TRACE_COLLECTOR`, making parallel branch evaluation
/// visible in trace files.
///
/// Without this, worker threads using bare `StaticEvalContext` return
/// `trace_collector() -> None`, causing all parallel branch evaluation
/// events to be silently dropped. This makes parallel execution invisible
/// to the trace analyzer (e.g., `parallel`, `fanout`, `critical-path`
/// subcommands see only the main thread).
///
/// # Trace Collector Lifetime
///
/// The `Arc<TraceCollector>` is upgraded from the global `Weak` at context
/// creation time. If the collector has been dropped (session ended), the
/// `Arc` will be `None` and `trace_collector()` degrades to the default
/// (no tracing), which is correct.
pub struct ParallelBranchContext {
    factory: GcFactory,
    /// Upgraded `Arc<TraceCollector>` from `WORK_POOL_TRACE_COLLECTOR`.
    /// Held as `Arc` to keep the collector alive for the context's lifetime.
    #[cfg(feature = "eval-trace")]
    trace_collector_arc: Option<std::sync::Arc<crate::backend::trace::TraceCollector>>,
}

impl ParallelBranchContext {
    /// Create a parallel branch context with trace collection support.
    ///
    /// Attempts to upgrade the global `WORK_POOL_TRACE_COLLECTOR` weak ref.
    /// If tracing is active, the context will emit trace events for all
    /// evaluation within the parallel branch.
    #[inline]
    pub fn get() -> Self {
        Self {
            factory: global_factory(),
            #[cfg(feature = "eval-trace")]
            trace_collector_arc: crate::backend::models::work_pool::get_work_pool_trace_collector(),
        }
    }
}

impl EvalContext for ParallelBranchContext {
    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    #[cfg(feature = "eval-trace")]
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
        let details = ctx.factory().atom("details");
        ctx.factory().error(msg, details)
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

        let lhs = factory.sexpr(vec![
            factory.atom("test-fn"),
            factory.atom("$x"),
        ]);
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
