use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, Rule};

use super::{eval, EvalOutput};

/// Eval: force evaluation of quoted expressions
/// (eval expr) - complementary to quote
pub(super) fn eval_eval(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("eval", items, env);

    // First evaluate the argument to get the expression
    let (arg_results, arg_env) = eval(items[1].clone(), env);
    if let Some(expr) = arg_results.first() {
        // Then evaluate the result
        return eval(expr.clone(), arg_env);
    } else {
        return (vec![MettaValue::Nil], arg_env);
    }
}

/// Evaluation: ! expr - force evaluation
pub(super) fn force_eval(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("!", items, env);
    // Evaluate the expression after !
    return eval(items[1].clone(), env);
}

/// Function: creates an evaluation loop that continues
/// until it encounters a return value
pub(super) fn eval_function(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("function", items, env);

    let mut current_expr = items[1].clone();
    let mut current_env = env;
    const MAX_ITERATIONS: usize = 1000;

    for iteration_count in 1..=MAX_ITERATIONS {
        let (results, new_env) = eval(current_expr.clone(), current_env);
        current_env = new_env;

        if results.is_empty() {
            return (vec![MettaValue::Nil], current_env);
        }

        let (final_results, continue_exprs): (Vec<_>, Vec<_>) =
            results.into_iter().partition(|result| {
                matches!(
                  result,
                  MettaValue::SExpr(items)
                  if items.len() == 2 && items[0] == MettaValue::Atom("return".to_string())
                )
            });

        if !final_results.is_empty() {
            let returns: Vec<_> = final_results
                .into_iter()
                .map(|r| match r {
                    MettaValue::SExpr(items) => items[1].clone(),
                    _ => unreachable!("partition guarantees return expressions"),
                })
                .collect();
            return (returns, current_env);
        }

        if continue_exprs.is_empty() {
            return (vec![MettaValue::Nil], current_env);
        }

        let next_expr = &continue_exprs[0];
        if current_expr == *next_expr {
            return (continue_exprs, current_env);
        }

        current_expr = continue_exprs[0].clone();
        if iteration_count == MAX_ITERATIONS {
            return (
                vec![MettaValue::Error(
                    format!("function exceeded maximum iterations ({})", MAX_ITERATIONS),
                    Box::new(current_expr),
                )],
                current_env,
            );
        }
    }

    unreachable!("Loop should always return within MAX_ITERATIONS")
}

/// Return: signals termination from a function evaluation loop
pub(super) fn eval_return(items: Vec<MettaValue>, env: Environment) -> EvalOutput {
    require_one_arg!("return", items, env);

    let (arg_results, arg_env) = eval(items[1].clone(), env);
    for result in &arg_results {
        if matches!(result, MettaValue::Error(_, _)) {
            return (vec![result.clone()], arg_env);
        }
    }

    let return_results = arg_results
        .into_iter()
        .map(|result| MettaValue::SExpr(vec![MettaValue::Atom("return".to_string()), result]))
        .collect();

    return (return_results, arg_env);
}
