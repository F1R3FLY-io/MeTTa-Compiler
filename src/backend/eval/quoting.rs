use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::EvalOutput;

/// Quote: return argument unevaluated
pub(super) fn eval_quote(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("quote", items, env);
    return (vec![items[1].clone()], env);
}
