use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, Rule};

use super::{eval, EvalOutput};

/// Error construction
pub(super) fn eval_error(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    if items.len() < 2 {
        return (vec![], env);
    }

    let msg = match &items[1] {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => s.clone(),
        other => format!("{:?}", other),
    };
    let details = if items.len() > 2 {
        items[2].clone()
    } else {
        MettaValue::Nil
    };

    (vec![MettaValue::Error(msg, Box::new(details))], env)
}

/// Is-error: check if value is an error (for error recovery)
pub(super) fn eval_if_error(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("is-error", items, env);

    let (results, new_env) = eval(items[1].clone(), env);
    if let Some(first) = results.first() {
        let is_err = matches!(first, MettaValue::Error(_, _));
        return (vec![MettaValue::Bool(is_err)], new_env);
    } else {
        return (vec![MettaValue::Bool(false)], new_env);
    }
}

/// Evaluate catch: error recovery mechanism
/// (catch expr default) - if expr returns error, evaluate and return default
/// This prevents error propagation (reduction prevention)
pub(super) fn eval_catch(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    let args = &items[1..];

    if args.len() < 2 {
        let err = MettaValue::Error(
            "catch requires 2 arguments: expr and default".to_string(),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let expr = &args[0];
    let default = &args[1];

    // Evaluate the expression
    let (results, env_after_eval) = eval(expr.clone(), env);

    // Check if result is an error
    if let Some(first) = results.first() {
        if matches!(first, MettaValue::Error(_, _)) {
            // Error occurred - evaluate and return default instead
            // This PREVENTS the error from propagating further
            return eval(default.clone(), env_after_eval);
        }
    }

    // No error - return the result
    (results, env_after_eval)
}

// TODO -> tests
