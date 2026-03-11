//! Generic Step-based Evaluation
//!
//! This module provides generic versions of the step evaluation functions
//! that work with any value type implementing `MettaValueTrait`.
//!
//! ## Design
//!
//! The generic step functions use:
//! - `MettaValueTrait` for type checking and value inspection
//! - `MettaValueFactory` (via `EvalContext`) for value construction
//! - `GenericEnvironment<V, F>` directly for environment operations

use smallvec::{SmallVec, smallvec};
use tracing::trace;

use crate::backend::eval::trampoline::{ContextEnv, EvalContext};
use crate::backend::models::{MettaValueFactory, MettaValueInner, MettaValueTrait};

use super::generic_sexpr::{eval_sexpr_step_generic, eval_sexpr_step_with_original};
use super::generic_types::GenericEvalStep;

/// Perform a single step of generic evaluation.
///
/// This is the generic version of `eval_step` that works with any value type
/// implementing `MettaValueTrait`. Returns either a final result or indicates
/// more work is needed.
///
/// **Design Note**: Uses `GenericEnvironment<C::Value, C::Factory>` directly.
/// - Rules stored in generic format, no serialization needed
/// - Pattern matching uses `MettaValueTrait` methods (no conversion)
/// - Environment operations use `GenericEnvironment` methods directly
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticEvalContext`)
///
/// # Arguments
///
/// - `value`: The value to evaluate
/// - `env`: The evaluation environment (`GenericEnvironment<C::Value, C::Factory>`)
/// - `depth`: Current evaluation depth (for debugging/metrics)
/// - `ctx`: The evaluation context providing the factory
///
/// # Returns
///
/// A `GenericEvalStep` indicating either:
/// - `Done`: Evaluation complete with results
/// - Various other variants indicating more work is needed
pub fn eval_step_generic<C: EvalContext>(
    value: C::Value,
    env: ContextEnv<C>,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    trace!(target: "mettatron::backend::eval::eval_step_generic", ?value, depth);

    // Peel outer Spanned layer — will be re-attached to Done results.
    // Non-Spanned values pass through unchanged (outer_span = None).
    let outer_span = value.span().copied();
    let value = value.strip_one_span();

    let step = eval_step_generic_inner(value, env, depth, ctx);

    // Re-wrap Done results with the original expression's span.
    // Non-Done results (delegations to the trampoline) pass through — inner
    // expressions carry their own spans from compilation/apply_bindings.
    match (outer_span, step) {
        (Some(span), GenericEvalStep::Done((results, env))) => {
            let wrapped = results
                .into_iter()
                .map(|v| ctx.factory().spanned(v, span))
                .collect();
            GenericEvalStep::Done((wrapped, env))
        }
        (_, step) => step,
    }
}

/// Inner evaluation logic — operates on span-stripped values.
fn eval_step_generic_inner<C: EvalContext>(
    value: C::Value,
    env: ContextEnv<C>,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    // Errors propagate immediately
    if value.is_error() {
        return GenericEvalStep::Done((smallvec![value], env));
    }

    // Ground types evaluate to themselves
    if matches!(value.inner_raw(),
        MettaValueInner::Bool(_) | MettaValueInner::Long(_) | MettaValueInner::Float(_)
        | MettaValueInner::String(_) | MettaValueInner::Space(_) | MettaValueInner::State(_)
        | MettaValueInner::Unit | MettaValueInner::Memo(_))
    {
        return GenericEvalStep::Done((smallvec![value], env));
    }

    // Atoms: check special tokens first, then tokenizer, then evaluate to themselves
    if let Some(name) = value.as_atom() {
        // Special handling for &self - evaluates to the current module's space
        if name == "&self" {
            let space_handle = env.self_space();
            return GenericEvalStep::Done((smallvec![ctx.factory().space(space_handle)], env));
        }

        // Use generic lookup to avoid heap conversion
        if let Some(bound_value) = env.lookup_token_generic(name, ctx.factory()) {
            // Token was registered via bind! - return the bound value
            return GenericEvalStep::Done((smallvec![bound_value], env));
        }

        // No binding - atom evaluates to itself
        return GenericEvalStep::Done((smallvec![value], env));
    }

    // Empty sentinel - gets filtered out at result collection
    if value.is_empty() {
        return GenericEvalStep::Done((smallvec![], env));
    }

    // S-expressions need special handling
    if let Some(items) = value.as_sexpr() {
        let items_vec: Vec<C::Value> = items.iter().cloned().collect();
        return eval_sexpr_step_with_original(items_vec, value, env, depth, ctx);
    }

    // For conjunctions, evaluate goals left-to-right with binding threading
    if let Some(goals) = value.as_conjunction() {
        let goals_vec: Vec<C::Value> = goals.iter().cloned().collect();
        return eval_conjunction_step_generic(goals_vec, env, depth, ctx);
    }

    // Type values - return as is
    if value.is_type() {
        return GenericEvalStep::Done((smallvec![value], env));
    }

    // Quoted values are self-evaluating — they preserve the quote wrapper.
    // This matches HE behavior: !(quote X) → (quote X).
    // Unwrapping happens only in the (eval ...) and (unquote ...) special forms.
    if value.is_quoted() {
        return GenericEvalStep::Done((smallvec![value], env));
    }

    // Fallback - return value unchanged
    GenericEvalStep::Done((smallvec![value], env))
}

/// Generic conjunction step evaluation.
///
/// Evaluates conjunction goals sequentially, threading bindings through.
///
/// **Design Note**: Uses `GenericEnvironment<C::Value, C::Factory>` directly.
fn eval_conjunction_step_generic<C: EvalContext>(
    goals: Vec<C::Value>,
    env: ContextEnv<C>,
    depth: usize,
    _ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    trace!(target: "mettatron::backend::eval::eval_conjunction_step_generic", ?goals, depth);

    // Conjunctions are evaluated by the trampoline's StartConjunction handling
    GenericEvalStep::StartConjunction { goals, env, depth }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::trampoline::StaticEvalContext;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_eval_step_generic_ground_types() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        // Bool
        let value = factory.bool(true);
        match eval_step_generic(value, env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_bool(), Some(true));
            }
            _ => panic!("Expected Done"),
        }

        // Long
        let value = factory.long(42);
        match eval_step_generic(value, env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_long(), Some(42));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_generic_atom() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let value = factory.atom("foo");
        match eval_step_generic(value, env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_atom(), Some("foo"));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_generic_sexpr() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        // S-expression should dispatch to eval_sexpr_step_generic
        let value = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        let result = eval_step_generic(value, env, 0, &ctx);
        // Should return some step that's not Done (needs more work)
        // The exact step type depends on whether + is a grounded op
        match result {
            GenericEvalStep::Done(_) => {}                  // Could be immediate if grounded
            GenericEvalStep::StartGroundedOp { .. } => {}   // TCO grounded op
            GenericEvalStep::EvalGroundedArgs { .. } => {}  // Needs arg eval
            GenericEvalStep::EvalRuleMatchesLazy { .. } => {} // Rule matching
            GenericEvalStep::EvalSExpr { .. } => {}         // Needs sub-eval
            _ => {} // Other step types are valid too
        }
    }

    #[test]
    fn test_eval_step_generic_error_propagation() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let error = factory.error("test error", factory.atom("TestError"));
        match eval_step_generic(error, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_error());
            }
            _ => panic!("Expected Done with error"),
        }
    }

    #[test]
    fn test_eval_step_generic_empty() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let value = factory.empty();
        match eval_step_generic(value, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert!(results.is_empty());
            }
            _ => panic!("Expected Done with empty results"),
        }
    }

    // ================================================================
    // Phase 4: Span threading tests
    // ================================================================

    #[test]
    fn test_eval_step_span_preserved_on_ground_type() {
        use crate::ir::{Position, Span};

        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let span = Span {
            start: Position { row: 1, column: 5, byte_offset: 5 },
            end: Position { row: 1, column: 7, byte_offset: 7 },
        };
        let value = factory.spanned(factory.long(42), span);

        match eval_step_generic(value, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                // Result should still have the span
                assert!(results[0].is_spanned());
                let result_span = results[0].span().expect("should have span");
                assert_eq!(result_span.start.row, 1);
                assert_eq!(result_span.start.column, 5);
                // Inner value should be Long(42)
                assert_eq!(results[0].as_long(), Some(42));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_span_preserved_on_atom() {
        use crate::ir::{Position, Span};

        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 3, byte_offset: 3 },
        };
        let value = factory.spanned(factory.atom("foo"), span);

        match eval_step_generic(value, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_spanned());
                assert_eq!(results[0].as_atom(), Some("foo"));
                let result_span = results[0].span().expect("should have span");
                assert_eq!(result_span.start.byte_offset, 0);
                assert_eq!(result_span.end.byte_offset, 3);
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_span_on_error_propagation() {
        use crate::ir::{Position, Span};

        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let span = Span {
            start: Position { row: 2, column: 0, byte_offset: 20 },
            end: Position { row: 2, column: 10, byte_offset: 30 },
        };
        let error = factory.error("test error", factory.atom("TestError"));
        let spanned_error = factory.spanned(error, span);

        match eval_step_generic(spanned_error, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                // Error result should be wrapped with the original span
                assert!(results[0].is_spanned());
                assert!(results[0].is_error());
                let result_span = results[0].span().expect("should have span");
                assert_eq!(result_span.start.row, 2);
            }
            _ => panic!("Expected Done with error"),
        }
    }

    #[test]
    fn test_eval_step_no_span_unchanged() {
        // Values without spans should not gain spans through evaluation
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let value = factory.long(99);
        match eval_step_generic(value, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert!(!results[0].is_spanned());
                assert_eq!(results[0].as_long(), Some(99));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_span_on_sexpr_done_result() {
        use crate::ir::{Position, Span};

        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        // (quote foo) should return Done with Quoted(foo)
        let span = Span {
            start: Position { row: 0, column: 0, byte_offset: 0 },
            end: Position { row: 0, column: 11, byte_offset: 11 },
        };
        let sexpr = factory.sexpr(vec![
            factory.atom("quote"),
            factory.atom("foo"),
        ]);
        let spanned_sexpr = factory.spanned(sexpr, span);

        match eval_step_generic(spanned_sexpr, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                // The result should carry the outer expression's span
                assert!(results[0].is_spanned());
                let result_span = results[0].span().expect("should have span");
                assert_eq!(result_span.end.byte_offset, 11);
                // Inner value should be Quoted("foo")
                assert!(results[0].is_quoted());
            }
            _ => panic!("Expected Done"),
        }
    }
}
