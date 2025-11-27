use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::{apply_bindings, eval, pattern_match};

/// Eval: force evaluation of quoted expressions
/// (eval expr) - complementary to quote
pub(super) fn eval_eval(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_one_arg!("eval", items, env);

    // First evaluate the argument to get the expression
    let (arg_results, arg_env) = eval(items[1].clone(), env);
    if let Some(expr) = arg_results.first() {
        // Then evaluate the result
        eval(expr.clone(), arg_env)
    } else {
        (vec![MettaValue::Nil], arg_env)
    }
}

/// Evaluation: ! expr - force evaluation
pub(super) fn force_eval(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_one_arg!("!", items, env);
    // Evaluate the expression after !
    eval(items[1].clone(), env)
}

/// Function: creates an evaluation loop that continues
/// until it encounters a return value
pub(super) fn eval_function(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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
pub(super) fn eval_return(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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

    (return_results, arg_env)
}

/// Subsequently tests multiple pattern-matching conditions (second argument) for the
/// given value (first argument)
pub(super) fn eval_chain(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_three_args!("chain", items, env);

    let expr = &items[1];
    let var = &items[2];
    let body = &items[3];

    let (expr_results, expr_env) = eval(expr.clone(), env);
    for result in &expr_results {
        if matches!(result, MettaValue::Error(_, _)) {
            return (vec![result.clone()], expr_env);
        }
    }

    let mut all_results = Vec::new();
    for value in expr_results {
        if let Some(bindings) = pattern_match(var, &value) {
            let instantiated_body = apply_bindings(body, &bindings);
            let (body_results, _) = eval(instantiated_body, expr_env.clone());
            all_results.extend(body_results);
        }
    }

    (all_results, expr_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rule;

    #[test]
    fn test_force_eval_missing_argument() {
        let env = Environment::new();

        // (!) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("!".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("!"));
                assert!(msg.contains("requires exactly 1 argument")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_eval_missing_argument() {
        let env = Environment::new();

        // (eval) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("eval".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("eval"));
                assert!(msg.contains("requires exactly 1 argument")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_evaluation_with_exclaim() {
        let env = Environment::new();

        // First define a rule: (= (f) 42)
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            MettaValue::Long(42),
        ]);
        let (_result, new_env) = eval(rule_def, env);

        // Now evaluate: (! (f))
        let eval_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
        ]);

        let (result, _) = eval(eval_expr, new_env);

        // Should get 42
        assert_eq!(result[0], MettaValue::Long(42));
    }

    #[test]
    fn test_function_factorial_with_return() {
        let mut env = Environment::new();

        // Define factorial rule that only uses return for base case
        let factorial_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("factorial".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (== $n 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (return 1)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("return".to_string()),
                    MettaValue::Long(1),
                ]),
                // Else branch: (factorial-helper $n 1)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("factorial-helper".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(1),
                ]),
            ]),
        };

        // (= (factorial-helper $n $acc)
        //      (if (== $n 0)
        //          (return $acc)
        //          (factorial-helper (- $n 1) (* $n $acc))))
        let helper_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("factorial-helper".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Atom("$acc".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (== $n 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (return $acc)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("return".to_string()),
                    MettaValue::Atom("$acc".to_string()),
                ]),
                // Else branch: (factorial-helper (- $n 1) (* $n $acc))
                MettaValue::SExpr(vec![
                    MettaValue::Atom("factorial-helper".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("*".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Atom("$acc".to_string()),
                    ]),
                ]),
            ]),
        };

        env.add_rule(factorial_rule);
        env.add_rule(helper_rule);

        // Test factorial(3) = 6
        let test_factorial_3 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("factorial".to_string()),
                MettaValue::Long(3),
            ]),
        ]);
        let (results, _) = eval(test_factorial_3, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));

        // Test factorial(4) = 24
        let test_factorial_4 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("factorial".to_string()),
                MettaValue::Long(4),
            ]),
        ]);
        let (results, _) = eval(test_factorial_4, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(24));
    }

    #[test]
    fn test_function_fibonacci_with_return() {
        let mut env = Environment::new();

        // Use tail-recursive fibonacci with accumulator
        // (= (fib $n) (fib-helper $n 0 1))
        let fib_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fib".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fib-helper".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Long(0),
                MettaValue::Long(1),
            ]),
        };

        // (= (fib-helper $n $a $b)
        //    (if (== $n 0)
        //        (return $a)
        //        (fib-helper (- $n 1) $b (+ $a $b))))
        let fib_helper_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("fib-helper".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (== $n 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (return $a)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("return".to_string()),
                    MettaValue::Atom("$a".to_string()),
                ]),
                // Else branch: (fib-helper (- $n 1) $b (+ $a $b))
                MettaValue::SExpr(vec![
                    MettaValue::Atom("fib-helper".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                    MettaValue::Atom("$b".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("+".to_string()),
                        MettaValue::Atom("$a".to_string()),
                        MettaValue::Atom("$b".to_string()),
                    ]),
                ]),
            ]),
        };

        env.add_rule(fib_rule);
        env.add_rule(fib_helper_rule);

        // Test fib(0) = 0
        let test_fib_0 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fib".to_string()),
                MettaValue::Long(0),
            ]),
        ]);
        let (results, env) = eval(test_fib_0, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(0));

        // Test fib(6) = 8
        let test_fib_6 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fib".to_string()),
                MettaValue::Long(6),
            ]),
        ]);
        let (results, _) = eval(test_fib_6, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(8));
    }

    #[test]
    fn test_function_power_with_return() {
        let mut env = Environment::new();

        // Use tail-recursive power with accumulator
        // (= (power $base $exp) (power-helper $base $exp 1))
        let power_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("power".to_string()),
                MettaValue::Atom("$base".to_string()),
                MettaValue::Atom("$exp".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("power-helper".to_string()),
                MettaValue::Atom("$base".to_string()),
                MettaValue::Atom("$exp".to_string()),
                MettaValue::Long(1),
            ]),
        };

        // (= (power-helper $base $exp $acc)
        //    (if (== $exp 0)
        //        (return $acc)
        //        (power-helper $base (- $exp 1) (* $acc $base))))
        let power_helper_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("power-helper".to_string()),
                MettaValue::Atom("$base".to_string()),
                MettaValue::Atom("$exp".to_string()),
                MettaValue::Atom("$acc".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                // Condition: (== $exp 0)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("==".to_string()),
                    MettaValue::Atom("$exp".to_string()),
                    MettaValue::Long(0),
                ]),
                // Then branch: (return $acc)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("return".to_string()),
                    MettaValue::Atom("$acc".to_string()),
                ]),
                // Else branch: (power-helper $base (- $exp 1) (* $acc $base))
                MettaValue::SExpr(vec![
                    MettaValue::Atom("power-helper".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$exp".to_string()),
                        MettaValue::Long(1),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("*".to_string()),
                        MettaValue::Atom("$acc".to_string()),
                        MettaValue::Atom("$base".to_string()),
                    ]),
                ]),
            ]),
        };

        env.add_rule(power_rule);
        env.add_rule(power_helper_rule);

        // Test power(2, 0) = 1
        let test_power_2_0 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("power".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(0),
            ]),
        ]);
        let (results, env) = eval(test_power_2_0, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));

        // Test power(3, 4) = 81
        let test_power_3_4 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("power".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(4),
            ]),
        ]);
        let (results, _) = eval(test_power_3_4, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(81));
    }

    #[test]
    fn test_chain_basic() {
        let env = Environment::new();

        // (chain (+ 1 2) $x (* $x 2)) should bind 3 to $x, then evaluate (* 3 2) = 6
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("chain".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_chain_with_return() {
        let env = Environment::new();

        // (chain 42 $x (return (* $x 3))) should bind 42 to $x, then return (* 42 3) = 126 wrapped in return
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("chain".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("return".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(3),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should return a return expression: (return 126)
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], MettaValue::Atom("return".to_string()));
                assert_eq!(items[1], MettaValue::Long(126));
            }
            other => panic!("Expected SExpr with return, got {:?}", other),
        }
    }

    #[test]
    fn test_chain_with_function_and_return() {
        let mut env = Environment::new();

        // Define a simple increment rule: (= (inc $x) (+ $x 1))
        let inc_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("inc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        };
        env.add_rule(inc_rule);

        // Define computation that uses chain: (= (compute $n) (chain (inc $n) $result (return $result)))
        let compute_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("compute".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("chain".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("inc".to_string()),
                    MettaValue::Atom("$n".to_string()),
                ]),
                MettaValue::Atom("$result".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("return".to_string()),
                    MettaValue::Atom("$result".to_string()),
                ]),
            ]),
        };
        env.add_rule(compute_rule);

        // Test: (function (compute 5)) should increment 5 to 6, bind to $result, then return 6
        let test = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("compute".to_string()),
                MettaValue::Long(5),
            ]),
        ]);

        let (results, _) = eval(test, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_chain_variable_scoping() {
        let env = Environment::new();

        // (chain 10 $x (chain 20 $y (+ $x $y))) - nested chains with different variables
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("chain".to_string()),
            MettaValue::Long(10),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("chain".to_string()),
                MettaValue::Long(20),
                MettaValue::Atom("$y".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_chain_complex_computation_pipeline() {
        let mut env = Environment::new();

        // Define helper functions for computation pipeline
        // (= (double $x) (* $x 2))
        let double_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        };

        // (= (square $x) (* $x $x))
        let square_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("square".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        };

        // Complex chained computation: (= (complex-calc $n)
        //   (chain (+ $n 3) $step1
        //     (chain (double $step1) $step2
        //       (chain (square $step2) $result
        //         (return $result)))))
        let complex_calc_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("complex-calc".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("chain".to_string()),
                // Step 1: (+ $n 3)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(3),
                ]),
                MettaValue::Atom("$step1".to_string()),
                // Nested chain: Step 2: (double $step1)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("chain".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("double".to_string()),
                        MettaValue::Atom("$step1".to_string()),
                    ]),
                    MettaValue::Atom("$step2".to_string()),
                    // Nested chain: Step 3: (square $step2)
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("chain".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("square".to_string()),
                            MettaValue::Atom("$step2".to_string()),
                        ]),
                        MettaValue::Atom("$result".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("return".to_string()),
                            MettaValue::Atom("$result".to_string()),
                        ]),
                    ]),
                ]),
            ]),
        };

        env.add_rule(double_rule);
        env.add_rule(square_rule);
        env.add_rule(complex_calc_rule);

        // Test: (function (complex-calc 2))
        // Should: 2 + 3 = 5, double(5) = 10, square(10) = 100, return 100
        let test = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("complex-calc".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(test, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(100));
    }

    #[test]
    fn test_chain_conditional_branching_with_function() {
        let mut env = Environment::new();

        // Define conditional computation with early termination
        // (= (process-number $n)
        //   (chain (> $n 10) $is-large
        //     (if $is-large
        //         (return $n)
        //         (chain (* $n $n) $squared
        //           (chain (+ $squared 1) $incremented
        //             (return $incremented))))))
        let process_rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("process-number".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("chain".to_string()),
                // Check if $n > 10
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(10),
                ]),
                MettaValue::Atom("$is-large".to_string()),
                // Conditional processing
                MettaValue::SExpr(vec![
                    MettaValue::Atom("if".to_string()),
                    MettaValue::Atom("$is-large".to_string()),
                    // If large: return as-is
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("return".to_string()),
                        MettaValue::Atom("$n".to_string()),
                    ]),
                    // If small: square and increment
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("chain".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("*".to_string()),
                            MettaValue::Atom("$n".to_string()),
                            MettaValue::Atom("$n".to_string()),
                        ]),
                        MettaValue::Atom("$squared".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("chain".to_string()),
                            MettaValue::SExpr(vec![
                                MettaValue::Atom("+".to_string()),
                                MettaValue::Atom("$squared".to_string()),
                                MettaValue::Long(1),
                            ]),
                            MettaValue::Atom("$incremented".to_string()),
                            MettaValue::SExpr(vec![
                                MettaValue::Atom("return".to_string()),
                                MettaValue::Atom("$incremented".to_string()),
                            ]),
                        ]),
                    ]),
                ]),
            ]),
        };

        env.add_rule(process_rule);

        // Test case 1: Small number (3) -> 3² + 1 = 10
        let test1 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("process-number".to_string()),
                MettaValue::Long(3),
            ]),
        ]);

        let (results1, env1) = eval(test1, env);
        assert_eq!(results1.len(), 1);
        assert_eq!(results1[0], MettaValue::Long(10));

        // Test case 2: Large number (15) -> return 15 (early termination)
        let test2 = MettaValue::SExpr(vec![
            MettaValue::Atom("function".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("process-number".to_string()),
                MettaValue::Long(15),
            ]),
        ]);

        let (results2, _) = eval(test2, env1);
        assert_eq!(results2.len(), 1);
        assert_eq!(results2[0], MettaValue::Long(15));
    }
}
