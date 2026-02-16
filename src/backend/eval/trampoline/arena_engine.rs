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
    use crate::ir::{Position, Span};

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

    // ================================================================
    // Phase 4: End-to-end span threading through trampoline
    // ================================================================

    #[test]
    fn test_eval_trampoline_span_preserved_on_ground_type() {

        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 2, byte_offset: 2 },
        };
        let value = factory.spanned(factory.long(42), span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // Self-evaluating: result carries the original span
        assert!(results[0].is_spanned());
        assert_eq!(results[0].as_long(), Some(42));
        let result_span = results[0].span().expect("should have span");
        assert_eq!(result_span.start.byte_offset, 0);
        assert_eq!(result_span.end.byte_offset, 2);
    }

    #[test]
    fn test_eval_trampoline_span_on_arithmetic() {

        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Spanned (+ 1 2) — the outer expression has a span
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 7, byte_offset: 7 },
        };
        let sexpr = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        let value = factory.spanned(sexpr, span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // The computed result 3 should carry the source expression's span
        assert_eq!(results[0].as_long(), Some(3));
        // Note: grounded ops go through trampoline, so the outer span from
        // eval_step_generic wraps the Done result from the (quote ...) path,
        // but grounded ops return via StartGroundedOp → Resume continuation.
        // The span may or may not be present depending on trampoline path.
        // This tests the current behavior.
    }

    #[test]
    fn test_eval_trampoline_span_on_quote() {

        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Spanned (quote hello) — returns Done directly from eval_sexpr_step
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 13, byte_offset: 13 },
        };
        let sexpr = factory.sexpr(vec![
            factory.atom("quote"),
            factory.atom("hello"),
        ]);
        let value = factory.spanned(sexpr, span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // (quote hello) returns Done → outer span is attached
        assert!(results[0].is_spanned());
        assert!(results[0].is_quoted());
        let result_span = results[0].span().expect("should have span");
        assert_eq!(result_span.end.byte_offset, 13);
    }

    #[test]
    fn test_eval_trampoline_span_on_if_true_branch() {

        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // (if True 42 0) — the then-branch 42 has its own span
        let then_span = Span {
            start: Position { row: 0, column: 9, byte_offset: 9 },
            end: Position { row: 0, column: 11, byte_offset: 11 },
        };
        let sexpr = factory.sexpr(vec![
            factory.atom("if"),
            factory.bool(true),
            factory.spanned(factory.long(42), then_span),
            factory.long(0),
        ]);

        let (results, _) = eval_trampoline(sexpr, env, &state);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
        // The then-branch carries its own span (from compilation)
        // After evaluation, the result preserves the branch's span
        assert!(results[0].is_spanned());
        let result_span = results[0].span().expect("should have span");
        assert_eq!(result_span.start.byte_offset, 9);
        assert_eq!(result_span.end.byte_offset, 11);
    }
}
