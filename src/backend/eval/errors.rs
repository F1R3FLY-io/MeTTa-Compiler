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
