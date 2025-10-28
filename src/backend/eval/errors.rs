use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_error_missing_argument() {
        let env = Environment::new();

        // (is-error) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("is-error".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("is-error"));
                assert!(msg.contains("requires exactly 1 argument")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_error_propagation() {
        let env = Environment::new();

        // Create an error
        let error = MettaValue::Error("test error".to_string(), Box::new(MettaValue::Long(42)));

        // Errors should propagate unchanged
        let (results, _) = eval(error.clone(), env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], error);
    }

    #[test]
    fn test_error_in_subexpression() {
        let env = Environment::new();

        // (+ (error "fail" 42) 10)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("fail".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Long(10),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should return the error
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "fail");
            }
            other => panic!("Expected error, got {:?}", other),
        }
    }

    #[test]
    fn test_error_construction() {
        let env = Environment::new();

        // (error "my error" (+ 1 2))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("my error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "my error");
                // Details should be unevaluated
                match **details {
                    MettaValue::SExpr(_) => {}
                    _ => panic!("Expected SExpr as error details"),
                }
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_is_error_with_error() {
        let env = Environment::new();

        // (is-error (error "test" 42))
        // Should return true
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("test".to_string()),
                MettaValue::Long(42),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_is_error_with_normal_value() {
        let env = Environment::new();

        // (is-error (+ 1 2))
        // Should return false
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_catch_with_successful_expression() {
        let env = Environment::new();

        // Test catch where expression succeeds (no error)
        // (catch (+ 2 3) (error "should not reach" nil))
        let catch_success = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("should not reach".to_string()),
                MettaValue::Nil,
            ]),
        ]);

        let (results, _) = eval(catch_success, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5)); // Successful result, not the error
    }

    #[test]
    fn test_error_propagation_through_complex_nested_expressions() {
        let env = Environment::new();

        // Test error propagation through deeply nested arithmetic
        // (+ 1 (* 2 (/ 6 (- 4 (error "deep" nil)))))
        let deep_nested = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(2),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("/".to_string()),
                    MettaValue::Long(6),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Long(4),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("error".to_string()),
                            MettaValue::String("deep".to_string()),
                            MettaValue::Nil,
                        ]),
                    ]),
                ]),
            ]),
        ]);

        let (results, _) = eval(deep_nested, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "deep");
            }
            _ => panic!("Expected error to propagate up"),
        }
    }

    #[test]
    fn test_multiple_errors_in_expression() {
        let env = Environment::new();

        // Test expression with multiple errors - first one should win
        // (+ (error "first" nil) (error "second" nil))
        let multiple_errors = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("first".to_string()),
                MettaValue::Nil,
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("second".to_string()),
                MettaValue::Nil,
            ]),
        ]);

        let (results, _) = eval(multiple_errors, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "first"); // First error encountered should propagate
            }
            _ => panic!("Expected error to propagate"),
        }
    }

    #[test]
    fn test_is_error_with_catch_combinations() {
        let env = Environment::new();

        // Test is-error applied to catch results
        // (is-error (catch (+ 1 2) "default"))
        let is_error_catch_success = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("catch".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
                MettaValue::String("default".to_string()),
            ]),
        ]);

        let (results, _) = eval(is_error_catch_success, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false)); // Catch succeeded, no error

        // (is-error (catch (error "fail" nil) "recovered"))
        let is_error_catch_recovery = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("catch".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("fail".to_string()),
                    MettaValue::Nil,
                ]),
                MettaValue::String("recovered".to_string()),
            ]),
        ]);

        let (results, _) = eval(is_error_catch_recovery, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false)); // Catch recovered, no error
    }

    #[test]
    fn test_error_in_conditional_expressions() {
        let env = Environment::new();

        // Test error in if condition
        // (if (error "condition-error" nil) "then" "else")
        let error_in_condition = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("condition-error".to_string()),
                MettaValue::Nil,
            ]),
            MettaValue::String("then".to_string()),
            MettaValue::String("else".to_string()),
        ]);

        let (results, _) = eval(error_in_condition, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "condition-error");
            }
            _ => panic!("Expected error to propagate from condition"),
        }

        // Test error in then branch (should not be evaluated due to lazy evaluation)
        // (if false (error "then-error" nil) "else")
        let error_in_then = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(false),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("then-error".to_string()),
                MettaValue::Nil,
            ]),
            MettaValue::String("else".to_string()),
        ]);

        let (results, _) = eval(error_in_then, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("else".to_string())); // No error due to lazy eval
    }

    #[test]
    fn test_catch_with_error() {
        let env = Environment::new();

        // (catch (error "fail" 42) "recovered")
        // Should return "recovered" instead of propagating error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("fail".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::String("recovered".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("recovered".to_string()));
    }

    #[test]
    fn test_catch_without_error() {
        let env = Environment::new();

        // (catch (+ 1 2) "default")
        // Should return 3 (no error occurred)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::String("default".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_catch_prevents_error_propagation() {
        let env = Environment::new();

        // (+ 10 (catch (error "fail" 0) 5))
        // The error should be caught and replaced with 5, so result is 15
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("catch".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("fail".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::Long(5),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(15));
    }

    #[test]
    fn test_error_construction_variants() {
        let env = Environment::new();

        // Test error with just a message (no details)
        let error_msg_only = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("simple error".to_string()),
        ]);
        let (results, _) = eval(error_msg_only, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "simple error");
                assert_eq!(**details, MettaValue::Nil);
            }
            _ => panic!("Expected error"),
        }

        // Test error with atom as message
        let error_atom_msg = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::Atom("BadType".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(error_atom_msg, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "BadType");
                assert_eq!(**details, MettaValue::Long(42));
            }
            _ => panic!("Expected error"),
        }

        // Test error with complex S-expression as details
        let error_complex_details = MettaValue::SExpr(vec![
            MettaValue::Atom("error".to_string()),
            MettaValue::String("complex error".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("context".to_string()),
                MettaValue::String("function".to_string()),
                MettaValue::Atom("add".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("args".to_string()),
                    MettaValue::Long(1),
                    MettaValue::String("not-a-number".to_string()),
                ]),
            ]),
        ]);
        let (results, _) = eval(error_complex_details, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "complex error");
                match details.as_ref() {
                    MettaValue::SExpr(items) => {
                        assert_eq!(items.len(), 4);
                        assert_eq!(items[0], MettaValue::Atom("context".to_string()));
                    }
                    _ => panic!("Expected S-expression as details"),
                }
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_is_error_with_various_types() {
        let env = Environment::new();

        // Test is-error with different MettaValue types
        let test_cases = vec![
            (MettaValue::Long(42), false),
            (MettaValue::Bool(true), false),
            (MettaValue::String("hello".to_string()), false),
            (MettaValue::Nil, false),
            (
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
                false,
            ),
            (
                MettaValue::Error("test".to_string(), Box::new(MettaValue::Nil)),
                true,
            ),
        ];

        for (test_value, expected) in test_cases {
            let is_error_test =
                MettaValue::SExpr(vec![MettaValue::Atom("is-error".to_string()), test_value]);
            let (results, _) = eval(is_error_test, env.clone());
            assert_eq!(results.len(), 1);
            assert_eq!(results[0], MettaValue::Bool(expected));
        }
    }

    #[test]
    fn test_is_error_with_empty_results() {
        let mut env = Environment::new();

        // Create a rule that returns empty: (= (returns-empty) ())
        use crate::backend::models::Rule;
        let empty_rule = Rule {
            lhs: MettaValue::SExpr(vec![MettaValue::Atom("returns-empty".to_string())]),
            rhs: MettaValue::SExpr(vec![]),
        };
        env.add_rule(empty_rule);

        // Test is-error with expression that returns empty
        let is_error_empty = MettaValue::SExpr(vec![
            MettaValue::Atom("is-error".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("returns-empty".to_string())]),
        ]);
        let (results, _) = eval(is_error_empty, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false)); // Empty is not an error
    }

    #[test]
    fn test_catch_with_nested_errors() {
        let env = Environment::new();

        // Test catch with nested error construction
        // (catch (error "outer" (error "inner" 42)) "recovered")
        let nested_catch = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("outer".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("error".to_string()),
                    MettaValue::String("inner".to_string()),
                    MettaValue::Long(42),
                ]),
            ]),
            MettaValue::String("recovered".to_string()),
        ]);

        let (results, _) = eval(nested_catch, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("recovered".to_string()));
    }

    #[test]
    fn test_catch_missing_arguments() {
        let env = Environment::new();

        // Test catch with only one argument
        let catch_one_arg = MettaValue::SExpr(vec![
            MettaValue::Atom("catch".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(catch_one_arg, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("catch"));
                assert!(msg.contains("2 arguments"));
            }
            _ => panic!("Expected error for missing arguments"),
        }

        // Test catch with no arguments
        let catch_no_args = MettaValue::SExpr(vec![MettaValue::Atom("catch".to_string())]);
        let (results, _) = eval(catch_no_args, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("catch"));
                assert!(msg.contains("2 arguments"));
            }
            _ => panic!("Expected error for missing arguments"),
        }
    }

    #[test]
    fn test_reduction_prevention_combo() {
        let env = Environment::new();

        // Complex reduction prevention:
        // (if (is-error (catch (/ 10 0) (error "caught" 0)))
        //     "has-error"
        //     "no-error")
        // catch prevents error, but creates new error, is-error detects it
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("is-error".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("catch".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("error".to_string()),
                        MettaValue::String("div-by-zero".to_string()),
                        MettaValue::Long(0),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("error".to_string()),
                        MettaValue::String("caught".to_string()),
                        MettaValue::Long(0),
                    ]),
                ]),
            ]),
            MettaValue::String("has-error".to_string()),
            MettaValue::String("no-error".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("has-error".to_string()));
    }
}
