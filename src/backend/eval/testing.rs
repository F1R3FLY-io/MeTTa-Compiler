use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::sync::Arc;
use tracing::trace;

use super::eval;

/// Evaluates both expressions and asserts their results are equal.
/// Returns `()` on success, `Error` on failure.
///
/// Syntax: `(assertEqual actual expected)`
pub(super) fn eval_assert_equal(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertEqual", ?items, ?args);

    require_args_with_usage!(
        "assertEqual",
        items,
        2,
        env,
        "(assertEqual actual expected)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let (expected_results, env_after_expected) = eval(args[1].clone(), env_after_actual);

    if results_are_equal(&actual_results, &expected_results) {
        (vec![MettaValue::Nil], env_after_expected)
    } else {
        let err = MettaValue::Error(
            format!(
                "Assertion failed: results are not equal.\nExpected: {:?}\nActual: {:?}",
                expected_results, actual_results
            ),
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertEqual".to_string()),
                args[0].clone(),
                args[1].clone(),
            ])),
        );
        (vec![err], env_after_expected)
    }
}

/// Like `assertEqual` but with a custom error message.
///
/// Syntax: `(assertEqualMsg actual expected message)`
pub(super) fn eval_assert_equal_msg(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertEqualMsg", ?items, ?args);

    require_args_with_usage!(
        "assertEqualMsg",
        items,
        3,
        env,
        "(assertEqualMsg actual expected message)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let (expected_results, env_after_expected) = eval(args[1].clone(), env_after_actual);

    if results_are_equal(&actual_results, &expected_results) {
        (vec![MettaValue::Nil], env_after_expected)
    } else {
        let msg_str = match &args[2] {
            MettaValue::String(s) | MettaValue::Atom(s) => s.clone(),
            other => format!("{:?}", other),
        };

        let err = MettaValue::Error(
            msg_str,
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertEqualMsg".to_string()),
                args[0].clone(),
                args[1].clone(),
            ])),
        );
        (vec![err], env_after_expected)
    }
}

/// Evaluates first expression only and compares with literal expected value.
/// Returns `()` on success, `Error` on failure.
///
/// Syntax: `(assertEqualToResult actual expected-results)`
pub(super) fn eval_assert_equal_to_result(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertEqualToResult", ?items, ?args);

    require_args_with_usage!(
        "assertEqualToResult",
        items,
        2,
        env,
        "(assertEqualToResult actual expected-results)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let expected_as_results = vec![args[1].clone()];

    if results_are_equal(&actual_results, &expected_as_results) {
        (vec![MettaValue::Nil], env_after_actual)
    } else {
        let err = MettaValue::Error(
            format!(
                "Assertion failed: results are not equal.\nExpected: {:?}\nActual: {:?}",
                expected_as_results, actual_results
            ),
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertEqualToResult".to_string()),
                args[0].clone(),
                args[1].clone(),
            ])),
        );
        (vec![err], env_after_actual)
    }
}

/// Like `assertEqualToResult` but with a custom error message.
///
/// Syntax: `(assertEqualToResultMsg actual expected-results message)`
pub(super) fn eval_assert_equal_to_result_msg(
    items: Vec<MettaValue>,
    env: Environment,
) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertEqualToResultMsg", ?items, ?args);

    require_args_with_usage!(
        "assertEqualToResultMsg",
        items,
        3,
        env,
        "(assertEqualToResultMsg actual expected-results message)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let expected_as_results = vec![args[1].clone()];

    if results_are_equal(&actual_results, &expected_as_results) {
        (vec![MettaValue::Nil], env_after_actual)
    } else {
        let msg_str = match &args[2] {
            MettaValue::String(s) | MettaValue::Atom(s) => s.clone(),
            other => format!("{:?}", other),
        };

        let err = MettaValue::Error(
            msg_str,
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertEqualToResultMsg".to_string()),
                args[0].clone(),
                args[1].clone(),
            ])),
        );
        (vec![err], env_after_actual)
    }
}

fn results_are_equal(actual: &[MettaValue], expected: &[MettaValue]) -> bool {
    actual.len() == expected.len() && actual.iter().zip(expected.iter()).all(|(a, e)| a == e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_assert_equal_success_with_literals() {
        let env = Environment::new();

        // (assertEqual 5 5)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqual".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_failure_with_literals() {
        let env = Environment::new();

        // (assertEqual 5 10)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqual".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(10),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_assert_equal_with_expressions() {
        let env = Environment::new();

        // (assertEqual (+ 1 2) 3)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqual".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::Long(3),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_msg_success() {
        let env = Environment::new();

        // (assertEqualMsg 5 5 "Should not fail")
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualMsg".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
            MettaValue::String("Should not fail".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_msg_failure_with_custom_message() {
        let env = Environment::new();

        // (assertEqualMsg 5 10 "Custom error message")
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualMsg".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(10),
            MettaValue::String("Custom error message".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "Custom error message");
            }
            _ => panic!("Expected error result"),
        }
    }

    #[test]
    fn test_assert_equal_with_complex_expressions() {
        let env = Environment::new();

        // (assertEqual (+ 1 2) (+ 2 1))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqual".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(1),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_to_result_success() {
        let env = Environment::new();

        // (assertEqualToResult (+ 1 2) 3)
        // Evaluates (+ 1 2) to 3, compares with literal 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualToResult".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::Long(3),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_to_result_failure() {
        let env = Environment::new();

        // (assertEqualToResult (+ 1 2) 4)
        // Evaluates (+ 1 2) to 3, compares with literal 4
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualToResult".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::Long(4),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_assert_equal_to_result_with_unevaluated_expression() {
        let env = Environment::new();

        // (assertEqualToResult 5 (+ 1 2))
        // Evaluates 5 to 5, compares with literal expression (+ 1 2) - should fail
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualToResult".to_string()),
            MettaValue::Long(5),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        // Should fail because 5 != (+ 1 2) as an unevaluated expression
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_assert_equal_to_result_msg_success() {
        let env = Environment::new();

        // (assertEqualToResultMsg (+ 2 3) 5 "Should work")
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualToResultMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(5),
            MettaValue::String("Should work".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_equal_to_result_msg_failure_with_custom_message() {
        let env = Environment::new();

        // (assertEqualToResultMsg (+ 2 3) 6 "Custom failure message")
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("assertEqualToResultMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(6),
            MettaValue::String("Custom failure message".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "Custom failure message");
            }
            _ => panic!("Expected error result"),
        }
    }
}
