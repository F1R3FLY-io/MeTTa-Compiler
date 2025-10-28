use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::{apply_bindings, eval, pattern_match, EvalOutput};

/// Evaluate let binding: (let pattern value body)
/// Evaluates value, binds it to pattern, and evaluates body with those bindings
/// Supports both simple variable binding and pattern matching:
///   - (let $x 42 body) - simple binding
///   - (let ($a $b) (tuple 1 2) body) - destructuring pattern
pub(super) fn eval_let(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    let args = &items[1..];

    if args.len() < 3 {
        let err = MettaValue::Error(
            "let requires 3 arguments: pattern, value, and body".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let value_expr = &args[1];
    let body = &args[2];

    // Evaluate the value expression first
    let (value_results, value_env) = eval(value_expr.clone(), env);

    // Handle nondeterminism: if value evaluates to multiple results, try each one
    let mut all_results = Vec::new();

    for value in value_results {
        // Try to match the pattern against the value
        if let Some(bindings) = pattern_match(pattern, &value) {
            // Apply bindings to the body and evaluate it
            let instantiated_body = apply_bindings(body, &bindings);
            let (body_results, _) = eval(instantiated_body, value_env.clone());
            all_results.extend(body_results);
        } else {
            // Pattern match failed
            let err = MettaValue::Error(
                format!("let pattern {:?} does not match value {:?}", pattern, value),
                Box::new(MettaValue::SExpr(args.to_vec())),
            );
            all_results.push(err);
        }
    }

    (all_results, value_env)
}

// TODO -> types
