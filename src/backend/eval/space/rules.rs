//! Rule definition operations.
//!
//! This module handles the `(= lhs rhs)` rule definition form.

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, Rule};

/// Rule definition: (= lhs rhs) - add to MORK Space and rule cache
pub(crate) fn eval_add(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_add", ?items);
    require_args_with_usage!("=", items, 2, env, "(= pattern body)");

    let lhs = items[1].clone();
    let rhs = items[2].clone();
    let mut new_env = env.clone();

    // Add rule using add_rule (stores in both rule_cache and MORK Space)
    new_env.add_rule(Rule::new(lhs, rhs));

    // Return empty list (rule definitions don't produce output)
    (vec![], new_env)
}
