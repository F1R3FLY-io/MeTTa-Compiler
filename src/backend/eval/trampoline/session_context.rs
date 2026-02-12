//! Session-based Evaluation Context
//!
//! This module provides `SessionContext`, the `EvalContext` implementation for
//! session-scoped slab-allocated evaluation. It bridges the `MettaState` (which
//! coordinates GC) to the generic trampoline engine.
//!
//! ## Single Factory Model
//!
//! With the slab allocator, the dual-arena model (eval + storage) collapses into
//! a single `GcFactory` backed by the global `SlabAllocator`. All allocations —
//! intermediates and persistent values alike — go through the same factory. The
//! slab's mark-sweep GC handles reclamation of unreachable values.
//!
//! ## Usage
//!
//! `SessionContext` is used internally by `eval_trampoline` and `eval`
//! when an `MettaState` is provided. It implements `EvalContext` to route all
//! allocations through the global slab factory.

use crate::backend::models::{global_factory, MettaState, MettaValue, GcFactory};

use super::context::EvalContext;

// ============================================================================
// SessionContext
// ============================================================================

/// Session-based evaluation context using slab-allocated `GcFactory`.
///
/// Provides a single `GcFactory` for all allocations while implementing
/// `EvalContext` for use with the generic trampoline engine.
///
/// ## Single Factory Model
///
/// All allocations go through `GcFactory`, backed by the global `SlabAllocator`.
/// The previous dual-arena split (eval vs. storage) is no longer needed because
/// the slab's snapshot-based mark-sweep GC handles reclamation. Both
/// `eval_factory()` and `storage_factory()` return the same `GcFactory` for
/// backward compatibility.
#[derive(Debug)]
pub struct SessionContext<'s> {
    /// Reference to the MettaState coordinating GC
    state: &'s MettaState,

    /// Factory backed by the global SlabAllocator
    factory: GcFactory,
}

impl<'s> SessionContext<'s> {
    /// Create a new session context from an MettaState reference.
    ///
    /// The factory is obtained from `global_factory()`, which returns a
    /// `GcFactory` backed by the process-wide `SlabAllocator`.
    #[inline]
    pub fn new(state: &'s MettaState) -> Self {
        Self {
            state,
            factory: global_factory(),
        }
    }

    /// Get the factory for intermediate (eval) allocations.
    ///
    /// Returns the same `GcFactory` used for all allocations. This method
    /// exists for backward compatibility with code that distinguished between
    /// eval and storage factories.
    #[inline]
    pub fn eval_factory(&self) -> GcFactory {
        self.factory
    }

    /// Get the factory for persistent (storage) allocations.
    ///
    /// Returns the same `GcFactory` used for all allocations. This method
    /// exists for backward compatibility with code that distinguished between
    /// eval and storage factories.
    #[inline]
    pub fn storage_factory(&self) -> GcFactory {
        self.factory
    }

    /// Get reference to the MettaState.
    #[inline]
    pub fn state(&self) -> &'s MettaState {
        self.state
    }
}

// EvalContext routes all allocations through the single GcFactory
impl<'s> EvalContext for SessionContext<'s> {
    type Value = MettaValue;
    type Factory = GcFactory;

    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    /// Apply Tier 1 back-pressure during evaluation.
    ///
    /// Called every 256 trampoline iterations. Applies graduated yield/sleep
    /// to slow allocation when GC can't keep up. Never hard-blocks because
    /// the caller still holds an EvalGuard.
    ///
    /// GC response processing is NOT done here — per the TLA+ model
    /// (`SlabGC_Quiescent.tla`), `ProcessGcResponse` requires
    /// `threadPhase[t] = "between"`, meaning responses must only be
    /// processed between top-level expressions (at quiescent points),
    /// never during eval when the trampoline holds stack roots.
    #[inline]
    fn maybe_gc(&self) {
        // NOTE: maybe_process_gc_response() was previously called here but
        // this violates the TLA+ model — response processing inside eval
        // can free values that are still on the trampoline continuation stack
        // (stack roots not visible to the GC root registry). Responses are
        // now processed exclusively between expressions in main.rs/run_repl().
        //
        // crate::backend::models::gc_allocator::maybe_process_gc_response();

        // Lazily spawn the GC cron manager (idempotent via OnceLock).
        // This must still happen during eval to ensure the cron starts.
        let _ = crate::backend::models::gc_allocator::global_gc_cron();

        // Tier 1 back-pressure: non-blocking yield/sleep during evaluation.
        // Safe because we still hold EvalGuard (never hard-blocks).
        crate::backend::models::gc_allocator::apply_backpressure_tier1();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_session_context_creation() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // Both factory accessors should work and produce valid values
        let eval_value = ctx.eval_factory().atom("eval");
        let storage_value = ctx.storage_factory().atom("storage");

        assert!(eval_value.is_atom());
        assert!(storage_value.is_atom());
    }

    #[test]
    fn test_session_context_factory_trait() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // EvalContext::factory() should return the GcFactory
        let value = ctx.factory().atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_single_factory_model() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // eval_factory and storage_factory return the same GcFactory,
        // so allocations from either are interchangeable
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
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("SessionContext"));
    }

}
