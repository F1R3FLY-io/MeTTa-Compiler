//! Session-based Evaluation Context
//!
//! This module provides `SessionContext`, the `EvalContext` implementation for
//! session-scoped dual-arena evaluation. It bridges the `ArenaState` (which owns
//! the storage arena) to the generic trampoline engine.
//!
//! ## Dual Arena Model
//!
//! - **Eval Arena**: Thread-local, generation-based reset for intermediates (~95% of allocations)
//! - **Storage Arena**: Session-owned via ArenaState, O(1) bulk free on drop (~5% of allocations)
//!
//! ## Usage
//!
//! `SessionContext` is used internally by `eval_trampoline_arena` and `eval_arena`
//! when an `ArenaState` is provided. It implements `EvalContext` to route
//! default allocations through the eval arena while providing access to the
//! storage arena for persistent values.

use crate::backend::models::{
    get_eval_arena, ArenaState, ArenaValue, ArenaValueFactory, StorageFactory,
};

use super::context::EvalContext;

// ============================================================================
// SessionContext
// ============================================================================

/// Session-based evaluation context using dual-arena model.
///
/// Provides access to both eval and storage factories while implementing
/// `EvalContext` for use with the generic trampoline engine.
///
/// ## Dual Arena Model
///
/// - **Eval Arena**: Thread-local, used for intermediates during evaluation.
///   Accessed via `eval_factory()`. ~95% of allocations go here.
///
/// - **Storage Arena**: Session-owned via ArenaState reference, used for
///   persistent values (rules, bindings, results). Accessed via `storage_factory()`.
///   ~5% of allocations go here.
///
/// ## EvalContext Implementation
///
/// The `EvalContext` trait is implemented to use the **eval arena** by default,
/// which is the correct behavior for the trampoline engine since most evaluation
/// intermediates are short-lived. When values need to persist, they should be
/// explicitly cloned to the storage arena using `clone_value()`.
#[derive(Debug)]
pub struct SessionContext<'s> {
    /// Reference to the ArenaState owning the storage arena
    state: &'s ArenaState,

    /// Factory for eval arena (thread-local)
    eval_factory: ArenaValueFactory<'static>,

    /// Factory for storage arena (session-owned)
    storage_factory: StorageFactory,
}

impl<'s> SessionContext<'s> {
    /// Create a new session context from an ArenaState reference.
    #[inline]
    pub fn new(state: &'s ArenaState) -> Self {
        Self {
            state,
            eval_factory: ArenaValueFactory::new(get_eval_arena()),
            storage_factory: state.storage_factory(),
        }
    }

    /// Get the eval factory for intermediate allocations.
    ///
    /// Use this for short-lived values during evaluation.
    #[inline]
    pub fn eval_factory(&self) -> ArenaValueFactory<'static> {
        self.eval_factory
    }

    /// Get the storage factory for persistent allocations.
    ///
    /// Use this for values that need to persist beyond the current evaluation step:
    /// - Rule definitions
    /// - Bindings
    /// - Results to be returned
    #[inline]
    pub fn storage_factory(&self) -> StorageFactory {
        self.storage_factory
    }

    /// Get reference to the ArenaState.
    #[inline]
    pub fn state(&self) -> &'s ArenaState {
        self.state
    }
}

// EvalContext uses eval arena by default (correct for intermediates)
impl<'s> EvalContext for SessionContext<'s> {
    type Value = ArenaValue<'static>;
    type Factory = ArenaValueFactory<'static>;

    #[inline]
    fn factory(&self) -> &ArenaValueFactory<'static> {
        &self.eval_factory
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{get_eval_factory, MettaValueFactory, MettaValueTrait};

    #[test]
    fn test_session_context_creation() {
        let state = ArenaState::new();
        let ctx = SessionContext::new(&state);

        // Both factories should work
        let eval_value = ctx.eval_factory().atom("eval");
        let storage_value = ctx.storage_factory().atom("storage");

        assert!(eval_value.is_atom());
        assert!(storage_value.is_atom());
    }

    #[test]
    fn test_session_context_factory_trait() {
        let state = ArenaState::new();
        let ctx = SessionContext::new(&state);

        // EvalContext::factory() should return eval factory
        let value = ctx.factory().atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_dual_arena_isolation() {
        let state = ArenaState::new();
        let ctx = SessionContext::new(&state);

        // Allocate in both arenas
        let eval_values: Vec<_> = (0..100)
            .map(|i| ctx.eval_factory().long(i))
            .collect();
        let storage_values: Vec<_> = (0..100)
            .map(|i| ctx.storage_factory().long(i + 1000))
            .collect();

        // Both sets should be independently accessible
        for (i, v) in eval_values.iter().enumerate() {
            assert_eq!(v.as_long(), Some(i as i64));
        }
        for (i, v) in storage_values.iter().enumerate() {
            assert_eq!(v.as_long(), Some((i + 1000) as i64));
        }
    }

    #[test]
    fn test_session_context_debug() {
        let state = ArenaState::new();
        let ctx = SessionContext::new(&state);
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("SessionContext"));
    }

    #[test]
    fn test_clone_between_arenas() {
        use crate::backend::models::clone_value;

        let state = ArenaState::new();
        let eval_factory = get_eval_factory();
        let storage_factory = state.storage_factory();

        // Create value in eval arena
        let eval_value = eval_factory.sexpr(vec![
            eval_factory.atom("list"),
            eval_factory.long(1),
            eval_factory.long(2),
            eval_factory.long(3),
        ]);

        // Clone to storage arena
        let storage_value = clone_value(&eval_value, &storage_factory);

        // Verify structure is preserved
        assert!(storage_value.is_sexpr());
        let items = storage_value.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].as_atom(), Some("list"));
        assert_eq!(items[1].as_long(), Some(1));
        assert_eq!(items[2].as_long(), Some(2));
        assert_eq!(items[3].as_long(), Some(3));
    }
}
