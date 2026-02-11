//! Arena-based Trampoline Engine
//!
//! This module provides zero-conversion arena evaluation using the global
//! slab allocator via `GcFactory`.
//!
//! ## Global Slab Allocator Model
//!
//! All allocations go through the process-wide `SlabAllocator` via `GcFactory`.
//! There is no dual-arena split — the slab's snapshot-based mark-sweep GC
//! handles reclamation of unreachable values.
//!
//! ## Zero-Conversion Pipeline
//!
//! 1. `compile()` compiles directly to `MettaState` with `MettaValue`
//! 2. `eval_trampoline()` evaluates `MettaValue` using `SessionContext`
//! 3. Results are `Vec<MettaValue>` — no conversion needed

use crate::backend::models::{MettaState, MettaValue, GcFactory, global_factory};

use super::context::MettaEnvironment;
use super::generic_trampoline::eval_trampoline_generic;
use super::generic_types::GenericEvalResult;
use super::session_context::SessionContext;

/// Type alias for arena evaluation result.
///
/// This is the return type of `eval_trampoline` — a tuple of:
/// - `Vec<MettaValue>`: The evaluation results
/// - `MettaEnvironment`: The updated environment
pub type EvalResult = GenericEvalResult<MettaValue, MettaEnvironment>;

/// Zero-conversion arena evaluation using the global slab allocator.
///
/// Evaluates `MettaValue` using the unified generic trampoline
/// engine with `SessionContext` for GC-backed allocation.
///
/// # Arguments
///
/// - `value`: The value to evaluate (must be `MettaValue`)
/// - `env`: The evaluation environment (`MettaEnvironment`)
/// - `state`: The `MettaState` owning the compiled source
///
/// # Returns
///
/// A tuple of (results, final_environment) where results are `Vec<MettaValue>`.
#[inline]
pub fn eval_trampoline(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
) -> EvalResult {
    let ctx = SessionContext::new(state);
    eval_trampoline_generic(value, env, &ctx)
}

/// Check if arena mode is available.
///
/// Returns `true` — arena evaluation is always available.
#[inline]
pub fn is_arena_mode_available() -> bool {
    true
}

/// Get the global factory for creating `MettaValue`.
///
/// Returns the `GcFactory` backed by the process-wide `SlabAllocator`.
#[inline]
pub fn get_static_factory() -> GcFactory {
    global_factory()
}

/// Create a new `MettaEnvironment` for session-based evaluation.
///
/// The environment uses the global `GcFactory` for all allocations.
#[inline]
pub fn new_env() -> MettaEnvironment {
    MettaEnvironment::new(global_factory())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_arena_mode_available() {
        assert!(is_arena_mode_available());
    }

    #[test]
    fn test_get_static_factory() {
        let factory = get_static_factory();
        let value = factory.atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_eval_simple_atom() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();
        let value = factory.atom("hello");

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_atom());
        assert_eq!(results[0].as_atom(), Some("hello"));
    }

    #[test]
    fn test_eval_simple_number() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();
        let value = factory.long(42);

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(42));
    }

    #[test]
    fn test_eval_simple_arithmetic() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Create (+ 1 2)
        let value = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(3));
    }
}
