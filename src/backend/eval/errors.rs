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
