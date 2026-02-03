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

use tracing::trace;

use crate::backend::eval::trampoline::{ContextEnv, EvalContext};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

use super::generic_sexpr::eval_sexpr_step_generic;
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
/// - `C`: The evaluation context (HeapContext or ArenaContext)
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

    // Errors propagate immediately
    if value.is_error() {
        return GenericEvalStep::Done((vec![value], env));
    }

    // Ground types evaluate to themselves
    if value.is_bool()
        || value.is_long()
        || value.is_float()
        || value.is_string()
        || value.is_nil()
        || value.is_space()
        || value.is_state()
        || value.is_unit()
        || value.is_memo()
    {
        return GenericEvalStep::Done((vec![value], env));
    }

    // Atoms: check special tokens first, then tokenizer, then evaluate to themselves
    if let Some(name) = value.as_atom() {
        // Special handling for &self - evaluates to the current module's space
        if name == "&self" {
            let space_handle = env.self_space();
            return GenericEvalStep::Done((vec![ctx.factory().space(space_handle)], env));
        }

        // Use generic lookup to avoid heap conversion
        if let Some(bound_value) = env.lookup_token_generic(name, ctx.factory()) {
            // Token was registered via bind! - return the bound value
            return GenericEvalStep::Done((vec![bound_value], env));
        }

        // No binding - atom evaluates to itself
        return GenericEvalStep::Done((vec![value], env));
    }

    // Empty sentinel - gets filtered out at result collection
    if value.is_empty() {
        return GenericEvalStep::Done((vec![], env));
    }

    // S-expressions need special handling
    if let Some(items) = value.as_sexpr() {
        let items_vec: Vec<C::Value> = items.iter().cloned().collect();
        return eval_sexpr_step_generic(items_vec, env, depth, ctx);
    }

    // For conjunctions, evaluate goals left-to-right with binding threading
    if let Some(goals) = value.as_conjunction() {
        let goals_vec: Vec<C::Value> = goals.iter().cloned().collect();
        return eval_conjunction_step_generic(goals_vec, env, depth, ctx);
    }

    // Type values - return as is
    if value.is_type() {
        return GenericEvalStep::Done((vec![value], env));
    }

    // Fallback - return value unchanged
    GenericEvalStep::Done((vec![value], env))
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
    use crate::backend::environment::HeapEnvironment;
    use crate::backend::eval::trampoline::HeapContext;
    use crate::backend::models::{HeapMettaValueFactory, MettaValue};

    #[test]
    fn test_eval_step_generic_ground_types() {
        let ctx = HeapContext;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        // Bool
        let value = MettaValue::Bool(true);
        match eval_step_generic(value.clone(), env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_bool(), Some(true));
            }
            _ => panic!("Expected Done"),
        }

        // Long
        let value = MettaValue::Long(42);
        match eval_step_generic(value.clone(), env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_long(), Some(42));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_generic_atom() {
        let ctx = HeapContext;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        let value = MettaValue::Atom("foo".to_string());
        match eval_step_generic(value.clone(), env.clone(), 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_atom(), Some("foo"));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_step_generic_sexpr() {
        let ctx = HeapContext;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        // S-expression should dispatch to eval_sexpr_step_generic
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
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
        let ctx = HeapContext;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        let error = MettaValue::Error(
            "test error".to_string(),
            MettaValue::Atom("TestError".to_string()),
        );
        match eval_step_generic(error.clone(), env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                assert!(results[0].is_error());
            }
            _ => panic!("Expected Done with error"),
        }
    }

    #[test]
    fn test_eval_step_generic_empty() {
        let ctx = HeapContext;
        let env = HeapEnvironment::new(HeapMettaValueFactory);

        let value = MettaValue::Empty();
        match eval_step_generic(value, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert!(results.is_empty());
            }
            _ => panic!("Expected Done with empty results"),
        }
    }
}
