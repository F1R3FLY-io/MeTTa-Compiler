//! Evaluation Context for Generic Trampoline Engine
//!
//! This module provides the `EvalContext` trait that bundles a value type with its
//! factory, enabling generic evaluation code to work with both heap and arena
//! allocation strategies.
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
//! ## Usage
//!
//! ```ignore
//! fn eval_generic<C: EvalContext>(
//!     value: C::Value,
//!     env: GenericEnvironment<C::Value, C::Factory>,
//!     ctx: &C,
//! ) -> (C::Value, GenericEnvironment<C::Value, C::Factory>) {
//!     // Use trait methods for type checking
//!     if value.is_error() {
//!         return (value, env);
//!     }
//!
//!     // Use factory for construction
//!     (ctx.factory().atom("result"), env)
//! }
//! ```
//!
//! ## Context Implementation
//!
//! `StaticArenaContext` is the production arena context using thread-local `'static`
//! arenas for zero-conversion evaluation.

use bumpalo::Bump;

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{
    ArenaValue, ArenaValueFactory, MettaValueFactory,
    MettaValueTrait,
};

/// Evaluation context that bundles a value type with its factory.
///
/// This trait enables writing generic evaluation code that works with
/// both heap and arena allocation strategies. The context provides:
///
/// - `Value`: The concrete value type (e.g., ArenaValue)
/// - `Factory`: The factory type for constructing values
/// - `factory()`: Access to the factory instance
///
/// The environment type is always `GenericEnvironment<Self::Value, Self::Factory>`,
/// which provides type-safe rule and binding storage.
///
/// # Type Parameters
///
/// Implementations specify the value type and factory type, ensuring
/// consistency between how values are created and stored.
pub trait EvalContext {
    /// The value type used during evaluation
    type Value: MettaValueTrait + Clone + Send + Sync + Unpin + 'static;

    /// The factory type for constructing values
    type Factory: MettaValueFactory<Self::Value> + Copy + Clone;

    /// Get a reference to the factory for constructing values
    fn factory(&self) -> &Self::Factory;
}

/// Type alias for the environment associated with an EvalContext.
/// This is always `GenericEnvironment<C::Value, C::Factory>` for any context C.
pub type ContextEnv<C> = GenericEnvironment<<C as EvalContext>::Value, <C as EvalContext>::Factory>;

// ============================================================================
// Static Arena Context - Zero-Conversion Arena Evaluation
// ============================================================================

/// Static arena-based evaluation context.
///
/// This context uses `Box::leak` to create a `'static` arena, enabling
/// `ArenaValue<'static>` which satisfies the `'static` bound required by
/// `GenericEnvironment` and `EvalContext`.
///
/// # Thread Safety
///
/// The static arena is thread-local, so each thread has its own arena.
/// This ensures memory safety and avoids contention between threads.
///
/// # Memory Management
///
/// The leaked arena persists for the lifetime of the program. This is
/// intentional: arena evaluation is designed for batch processing where
/// all allocations happen during evaluation and the arena is never dropped.
/// For long-running processes, consider periodic arena resets.
///
/// # Example
///
/// ```ignore
/// let ctx = StaticArenaContext::get();
/// let env = StaticArenaContext::new_env();
/// let value = ctx.factory().atom("hello");
/// assert!(value.is_atom());
/// // `value` is 'static and can be stored anywhere
/// ```
#[derive(Debug, Clone, Copy)]
pub struct StaticArenaContext {
    factory: ArenaValueFactory<'static>,
}

/// Type alias for arena environment
pub type ArenaEnvironment = GenericEnvironment<ArenaValue<'static>, ArenaValueFactory<'static>>;

use std::cell::RefCell;

// Thread-local static arena storage
thread_local! {
    static STATIC_ARENA: &'static Bump = Box::leak(Box::new(Bump::new()));
}

// Thread-local persistent environment storage for arena mode.
// This persists state (rules, facts, bindings) across sequential evaluations,
// matching heap mode behavior where environments are threaded through.
thread_local! {
    static STATIC_ENV: RefCell<Option<ArenaEnvironment>> = const { RefCell::new(None) };
}

impl StaticArenaContext {
    /// Get the thread-local static arena context.
    ///
    /// This creates a new `StaticArenaContext` pointing to the thread-local
    /// static arena. The arena is created once per thread via `Box::leak`.
    #[inline]
    pub fn get() -> Self {
        STATIC_ARENA.with(|arena| Self {
            factory: ArenaValueFactory::new(arena),
        })
    }

    /// Get the underlying static arena.
    #[inline]
    pub fn arena(&self) -> &'static Bump {
        self.factory.arena()
    }

    /// Create a new ArenaEnvironment for this context.
    ///
    /// Note: This creates a fresh environment every time. For persistent state
    /// across sequential evaluations, use `get_or_create_env()` instead.
    #[inline]
    pub fn new_env() -> ArenaEnvironment {
        STATIC_ARENA.with(|arena| {
            ArenaEnvironment::new(ArenaValueFactory::new(arena))
        })
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
    pub fn get_or_create_env() -> ArenaEnvironment {
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
    ///
    /// # Arguments
    ///
    /// * `new_env` - The environment containing accumulated state from evaluation.
    #[inline]
    pub fn update_env(new_env: ArenaEnvironment) {
        STATIC_ENV.with(|env_cell| {
            *env_cell.borrow_mut() = Some(new_env);
        });
    }

    /// Reset the persistent environment.
    ///
    /// Clears all accumulated state (rules, facts, bindings). Use this:
    /// - Between test cases to ensure isolation
    /// - When starting a new session
    /// - To reclaim memory from the arena
    #[inline]
    pub fn reset_env() {
        STATIC_ENV.with(|env_cell| {
            *env_cell.borrow_mut() = None;
        });
    }

    /// Get the thread-local static arena directly.
    ///
    /// This is useful for external code that needs to allocate values
    /// in the same arena used by evaluation.
    #[inline]
    pub fn get_arena() -> &'static Bump {
        STATIC_ARENA.with(|arena| *arena)
    }

    /// Get the thread-local static factory directly.
    ///
    /// This is useful for external code that needs to create values
    /// in the same arena used by evaluation.
    #[inline]
    pub fn get_factory() -> ArenaValueFactory<'static> {
        STATIC_ARENA.with(|arena| ArenaValueFactory::new(arena))
    }
}

impl EvalContext for StaticArenaContext {
    type Value = ArenaValue<'static>;
    type Factory = ArenaValueFactory<'static>;

    #[inline]
    fn factory(&self) -> &ArenaValueFactory<'static> {
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
        let ctx = StaticArenaContext::get();
        let error = generic_create_error(&ctx, "test error");
        assert!(error.is_error());
    }

    #[test]
    fn test_static_arena_context_factory() {
        let ctx = StaticArenaContext::get();
        let value = ctx.factory().atom("test");
        assert!(value.is_atom());
        assert_eq!(MettaValueTrait::as_atom(&value), Some("test"));
    }

    #[test]
    fn test_static_arena_context_size() {
        // StaticArenaContext should be pointer-sized (holds one factory which has one reference)
        assert_eq!(
            std::mem::size_of::<StaticArenaContext>(),
            std::mem::size_of::<&Bump>()
        );
    }

    #[test]
    fn test_static_arena_context_env() {
        let _env = StaticArenaContext::new_env();
    }

    #[test]
    fn test_static_arena_persistent_env() {
        // Reset to ensure clean state
        StaticArenaContext::reset_env();

        // First call should create new env
        let env1 = StaticArenaContext::get_or_create_env();
        assert!(env1.owns_data == false); // Clone doesn't own data

        // Second call should return clone of same env
        let env2 = StaticArenaContext::get_or_create_env();
        assert!(std::sync::Arc::ptr_eq(&env1.shared, &env2.shared));

        // Update with a modified env
        let mut modified_env = env1.clone();
        let ctx = StaticArenaContext::get();
        modified_env.bind("test_var", ctx.factory().atom("test_value"));
        StaticArenaContext::update_env(modified_env);

        // Get should now return env with the binding
        let env3 = StaticArenaContext::get_or_create_env();
        assert!(env3.has_binding("test_var"));

        // Reset should clear everything
        StaticArenaContext::reset_env();
        let env4 = StaticArenaContext::get_or_create_env();
        assert!(!env4.has_binding("test_var"));
    }

    #[test]
    fn test_static_arena_env_persists_rules() {
        use crate::backend::models::GenericRule;

        // Reset to ensure clean state
        StaticArenaContext::reset_env();

        let ctx = StaticArenaContext::get();
        let factory = ctx.factory();

        // Create env and add a rule
        let mut env = StaticArenaContext::get_or_create_env();

        let lhs = factory.sexpr(vec![
            factory.atom("test-fn"),
            factory.atom("$x"),
        ]);
        let rhs = factory.atom("result");
        let rule = GenericRule::new(lhs, rhs);

        env.add_generic_rule(rule);
        StaticArenaContext::update_env(env);

        // Get env again and verify rule persists
        let env2 = StaticArenaContext::get_or_create_env();
        let rules: Vec<_> = env2.get_matching_rules("test-fn", 1).collect();
        assert_eq!(rules.len(), 1);

        // Clean up
        StaticArenaContext::reset_env();
    }

    #[test]
    fn test_static_arena_env_persists_space_facts() {

        // Reset to ensure clean state
        StaticArenaContext::reset_env();

        let ctx = StaticArenaContext::get();
        let factory = ctx.factory();

        // Create env and add a fact to space
        let mut env = StaticArenaContext::get_or_create_env();

        let fact = factory.sexpr(vec![
            factory.atom("fact"),
            factory.atom("x"),
            factory.long(42),
        ]);

        env.add_to_space(&fact);
        StaticArenaContext::update_env(env);

        // Get env again and verify fact persists
        let env2 = StaticArenaContext::get_or_create_env();
        let pattern = factory.sexpr(vec![
            factory.atom("fact"),
            factory.atom("$var"),
            factory.atom("$val"),
        ]);
        let template = pattern.clone();
        let matches = env2.match_space(&pattern, &template);
        assert!(!matches.is_empty());

        // Clean up
        StaticArenaContext::reset_env();
    }
}
