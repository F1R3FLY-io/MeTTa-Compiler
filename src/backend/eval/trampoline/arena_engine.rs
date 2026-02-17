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

    // ================================================================
    // add-atom / remove-atom MeTTa HE semantic alignment tests
    // ================================================================

    /// Helper: compile and evaluate all expressions sequentially, returning
    /// only the results of `!`-prefixed (forced) evaluation expressions.
    fn eval_metta(source: &str) -> Vec<MettaValue> {
        let state = crate::compile(source).expect("compile failed");
        let mut env = new_env();
        let mut all_results = Vec::new();

        let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in exprs {
            let (results, new_env) = eval_trampoline(expr, env, &state);
            env = new_env;
            all_results.extend(results);
        }
        all_results
    }

    #[test]
    fn test_add_atom_rule_becomes_reducible() {
        // Bug A+B fix: add-atom should not evaluate its atom arg AND should
        // update the rule table so the rule becomes usable for reduction.
        let results = eval_metta(r#"
            !(add-atom &self (= (foo) 42))
            !(foo)
        "#);
        // add-atom returns Unit
        assert!(results[0].is_unit(), "add-atom should return Unit, got: {:?}", results[0]);
        // foo should now reduce to 42
        assert_eq!(results[1].as_long(), Some(42), "foo should reduce to 42 via add-atom rule");
    }

    #[test]
    fn test_add_atom_rule_with_variables() {
        // Rules with variables should also work
        let results = eval_metta(r#"
            !(add-atom &self (= (double $x) (* 2 $x)))
            !(double 5)
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(results[1].as_long(), Some(10), "double 5 should be 10");
    }

    #[test]
    fn test_add_atom_non_rule() {
        // Non-rule atoms should be added to space without error
        let results = eval_metta(r#"
            !(add-atom &self (parent Alice Bob))
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit for non-rule atoms");
    }

    #[test]
    fn test_add_atom_does_not_evaluate_atom() {
        // Verify add-atom does NOT evaluate its atom argument.
        // If it did evaluate (= (foo) 42), the = special form handler would
        // add the rule as a side effect but return empty, causing an error.
        let results = eval_metta(r#"
            !(add-atom &self (= (bar) 99))
        "#);
        assert!(results[0].is_unit(), "add-atom should NOT evaluate its atom arg (no error), got: {:?}", results[0]);
    }

    #[test]
    fn test_remove_atom_multiplicity_tracking() {
        // Test multiplicity in separate steps to avoid index confusion from
        // multiplicity expansion (a rule with multiplicity 2 returns 2 results).

        // Step 1: Add rule, verify it works
        let results = eval_metta(r#"
            !(add-atom &self (= (baz) 77))
            !(baz)
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(results[1].as_long(), Some(77), "baz should reduce to 77");

        // Step 2: Add same rule again (multiplicity 2), then remove once — should still work
        let results = eval_metta(r#"
            !(add-atom &self (= (baz2) 88))
            !(add-atom &self (= (baz2) 88))
            !(remove-atom &self (= (baz2) 88))
            !(baz2)
        "#);
        assert!(results[0].is_unit(), "first add-atom should return Unit");
        assert!(results[1].is_unit(), "second add-atom should return Unit");
        assert!(results[2].is_unit(), "remove-atom should return Unit");
        // After removing one copy, the rule still works (multiplicity was 2, now 1)
        assert_eq!(results[3].as_long(), Some(88),
                   "baz2 should still reduce to 88 after removing one of two copies");

        // Step 3: Remove the last copy — rule should stop working
        let results = eval_metta(r#"
            !(add-atom &self (= (baz3) 99))
            !(remove-atom &self (= (baz3) 99))
            !(baz3)
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert!(results[1].is_unit(), "remove-atom should return Unit");
        // After removing the only copy, baz3 should be unreduced
        let last = &results[2];
        assert!(last.as_atom().is_some() || last.as_sexpr().is_some(),
                "baz3 should be unreduced after rule removed, got: {:?}", last);
    }

    #[test]
    fn test_remove_atom_non_existent() {
        // Removing a non-existent atom should return Unit (no error)
        let results = eval_metta(r#"
            !(remove-atom &self (= (nonexistent) 0))
        "#);
        assert!(results[0].is_unit(), "remove-atom for non-existent should return Unit");
    }

    #[test]
    fn test_add_atom_type_assertion() {
        // add-atom with type assertion should register in the type system
        let results = eval_metta(r#"
            !(add-atom &self (: myvar Int))
            !(get-type myvar)
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit");
        // get-type should return Int
        assert_eq!(results[1].as_atom(), Some("Int"), "get-type myvar should return Int");
    }

    #[test]
    fn test_add_atom_then_match() {
        // Rules added via add-atom should be queryable via match &self
        let results = eval_metta(r#"
            !(add-atom &self (parent Alice Bob))
            !(match &self (parent $x Bob) $x)
        "#);
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(results[1].as_atom(), Some("Alice"),
                   "match should find atom added via add-atom");
    }
}
