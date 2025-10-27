use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, Rule};

use super::EvalOutput;

/// Rule definition: (= lhs rhs) - add to MORK Space and rule cache
pub(super) fn eval_add(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_two_args!("=", items, env);

    let lhs = items[1].clone();
    let rhs = items[2].clone();
    let mut new_env = env.clone();

    // Add rule using add_rule (stores in both rule_cache and MORK Space)
    new_env.add_rule(Rule { lhs, rhs });

    // Return empty list (rule definitions don't produce output)
    return (vec![], new_env);
}
