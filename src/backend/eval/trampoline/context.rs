//! Evaluation Context for Generic Trampoline Engine
//!
//! This module provides the `EvalContext` trait that bundles a value type with its
//! factory, enabling generic evaluation code to work with different allocation
//! strategies.
//!
//! ## Design
//!
//! The evaluation context provides:
//! 1. A value type (`V: MettaValueTrait`) - the type being manipulated during evaluation
//! 2. A factory type (`F: MettaValueFactory<V>`) - for constructing new values
//! 3. Access to the factory instance
//!
//! The environment type is derived from Value + Factory:
//! `GenericEnvironment<Self::Value, Self::Factory>`
//!
//! ## Context Implementation
//!
//! `StaticEvalContext` is the production context using the global slab allocator
//! via `GcFactory` for zero-conversion evaluation.

use std::cell::RefCell;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{
    MettaValue, GcFactory, MettaValueFactory,
    MettaValueTrait, global_factory,
};

/// Evaluation context that bundles a value type with its factory.
///
/// This trait enables writing generic evaluation code that works with
/// different allocation strategies. The context provides:
///
/// - `Value`: The concrete value type (e.g., MettaValue)
/// - `Factory`: The factory type for constructing values
/// - `factory()`: Access to the factory instance
///
/// The environment type is always `GenericEnvironment<Self::Value, Self::Factory>`,
/// which provides type-safe rule and binding storage.
pub trait EvalContext {
    /// The value type used during evaluation
    type Value: MettaValueTrait + Clone + Send + Sync + Unpin + 'static;

    /// The factory type for constructing values
    type Factory: MettaValueFactory<Self::Value> + Copy + Clone;

    /// Get a reference to the factory for constructing values
    fn factory(&self) -> &Self::Factory;

    /// Hint to the context that it may trigger GC if memory pressure is high.
    ///
    /// Called periodically from the trampoline loop (every 256 iterations).
    /// Default implementation is a no-op. Override in contexts that own or
    /// coordinate GC (e.g., `SessionContext` with `MettaState`).
    #[inline]
    fn maybe_gc(&self) {
        // no-op by default
    }
}

/// Type alias for the environment associated with an EvalContext.
/// This is always `GenericEnvironment<C::Value, C::Factory>` for any context C.
pub type ContextEnv<C> = GenericEnvironment<<C as EvalContext>::Value, <C as EvalContext>::Factory>;

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
    type Value = MettaValue;
    type Factory = GcFactory;

    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generic_create_error<C: EvalContext>(ctx: &C, msg: &str) -> C::Value {
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
