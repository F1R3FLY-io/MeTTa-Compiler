//! Conjunction Evaluation
//!
//! This module implements evaluation of MORK-style conjunction expressions
//! with left-to-right goal evaluation and binding threading.

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::EvalStep;

/// Step version of eval_conjunction that defers evaluation to trampoline.
/// This prevents stack overflow for deeply nested conjunction goals.
pub fn eval_conjunction_step(goals: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    EvalStep::StartConjunction { goals, env, depth }
}

/// Evaluate a conjunction: (,), (, expr), or (, expr1 expr2 ...)
/// Implements MORK-style goal evaluation with left-to-right binding threading
///
/// Semantics:
/// - (,)          → succeed with empty result (always true)
/// - (, expr)     → evaluate expr directly (unary passthrough)
/// - (, e1 e2 ... en) → evaluate goals left-to-right, threading bindings through
///
/// DEPRECATED: Use eval_conjunction_step for trampoline-based evaluation.
#[allow(dead_code)]
pub fn eval_conjunction(goals: Vec<MettaValue>, env: Environment, _depth: usize) -> EvalResult {
    use super::eval;

    // Empty conjunction: (,) succeeds with empty result
    if goals.is_empty() {
        return (vec![MettaValue::Nil], env);
    }

    // Unary conjunction: (, expr) evaluates expr directly
    if goals.len() == 1 {
        return eval(goals[0].clone(), env);
    }

    // N-ary conjunction: evaluate left-to-right with binding threading
    // Start with the first goal
    let (mut results, mut current_env) = eval(goals[0].clone(), env);

    // For each subsequent goal, evaluate it in the context of previous results
    for goal in &goals[1..] {
        let mut next_results = Vec::new();

        // For each result from previous goals, evaluate the current goal
        for result in results {
            // If previous result is an error, propagate it
            if matches!(result, MettaValue::Error(_, _)) {
                next_results.push(result);
                continue;
            }

            // Evaluate the current goal
            let (goal_results, goal_env) = eval(goal.clone(), current_env.clone());

            // Union the environment
            current_env = current_env.union(&goal_env);

            // Collect all results from this goal
            next_results.extend(goal_results);
        }

        results = next_results;
    }

    (results, current_env)
}
