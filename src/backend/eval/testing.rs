use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::trace;

use super::eval;

/// Alpha equality operation: (=alpha expr1 expr2) -> Bool
/// Checks if two expressions are equivalent up to variable renaming
pub(super) fn eval_alpha_eq(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("=alpha", items, 2, env, "(=alpha expr1 expr2)");

    let result = atoms_are_alpha_equivalent(&items[1], &items[2]);
    (vec![MettaValue::Bool(result)], env)
}

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

/// Evaluates both expressions and asserts their results are alpha equal.
/// Returns `()` on success, `Error` on failure.
///
/// Syntax: `(assertAlphaEqual actual expected)`
pub(super) fn eval_assert_alpha_equal(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertAlphaEqual", ?items, ?args);

    require_args_with_usage!(
        "assertAlphaEqual",
        items,
        2,
        env,
        "(assertAlphaEqual actual expected)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let (expected_results, env_after_expected) = eval(args[1].clone(), env_after_actual);

    if results_are_alpha_equal(&actual_results, &expected_results) {
        (vec![MettaValue::Nil], env_after_expected)
    } else {
        let err = MettaValue::Error(
            format!(
                "Alpha equality assertion failed: results are not alpha equal.\nExpected: {:?}\nActual: {:?}",
                expected_results, actual_results
            ),
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertAlphaEqual".to_string()),
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

/// Like `assertAlphaEqual` but with a custom error message.
///
/// Syntax: `(assertAlphaEqualMsg actual expected message)`
pub(super) fn eval_assert_alpha_equal_msg(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertAlphaEqualMsg", ?items, ?args);

    require_args_with_usage!(
        "assertAlphaEqualMsg",
        items,
        3,
        env,
        "(assertAlphaEqualMsg actual expected message)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let (expected_results, env_after_expected) = eval(args[1].clone(), env_after_actual);

    if results_are_alpha_equal(&actual_results, &expected_results) {
        (vec![MettaValue::Nil], env_after_expected)
    } else {
        let msg_str = match &args[2] {
            MettaValue::String(s) | MettaValue::Atom(s) => s.clone(),
            other => format!("{:?}", other),
        };

        let err = MettaValue::Error(
            msg_str,
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertAlphaEqualMsg".to_string()),
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

/// Evaluates first expression only and compares with literal expected value using alpha equality.
/// Returns `()` on success, `Error` on failure.
///
/// Syntax: `(assertAlphaEqualToResult actual expected-results)`
pub(super) fn eval_assert_alpha_equal_to_result(
    items: Vec<MettaValue>,
    env: Environment,
) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertAlphaEqualToResult", ?items, ?args);

    require_args_with_usage!(
        "assertAlphaEqualToResult",
        items,
        2,
        env,
        "(assertAlphaEqualToResult actual expected-results)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let expected_as_results = vec![args[1].clone()];

    if results_are_alpha_equal(&actual_results, &expected_as_results) {
        (vec![MettaValue::Nil], env_after_actual)
    } else {
        let err = MettaValue::Error(
            format!(
                "Alpha equality assertion failed: results are not alpha equal.\nExpected: {:?}\nActual: {:?}",
                expected_as_results, actual_results
            ),
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertAlphaEqualToResult".to_string()),
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

/// Like `assertAlphaEqualToResult` but with a custom error message.
///
/// Syntax: `(assertAlphaEqualToResultMsg actual expected-results message)`
pub(super) fn eval_assert_alpha_equal_to_result_msg(
    items: Vec<MettaValue>,
    env: Environment,
) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::assertAlphaEqualToResultMsg", ?items, ?args);

    require_args_with_usage!(
        "assertAlphaEqualToResultMsg",
        items,
        3,
        env,
        "(assertAlphaEqualToResultMsg actual expected-results message)"
    );

    let (actual_results, env_after_actual) = eval(args[0].clone(), env);
    let expected_as_results = vec![args[1].clone()];

    if results_are_alpha_equal(&actual_results, &expected_as_results) {
        (vec![MettaValue::Nil], env_after_actual)
    } else {
        let msg_str = match &args[2] {
            MettaValue::String(s) | MettaValue::Atom(s) => s.clone(),
            other => format!("{:?}", other),
        };

        let err = MettaValue::Error(
            msg_str,
            Arc::new(MettaValue::SExpr(vec![
                MettaValue::Atom("assertAlphaEqualToResultMsg".to_string()),
                args[0].clone(),
                args[1].clone(),
            ])),
        );
        (vec![err], env_after_actual)
    }
}

/// Check if an atom string represents a variable (starts with $)
fn is_variable(atom: &str) -> bool {
    atom.starts_with('$')
}

fn results_are_equal(actual: &[MettaValue], expected: &[MettaValue]) -> bool {
    actual.len() == expected.len() && actual.iter().zip(expected.iter()).all(|(a, e)| a == e)
}

/// Check if two result sets are alpha equal (comparing element-wise with alpha equivalence)
fn results_are_alpha_equal(actual: &[MettaValue], expected: &[MettaValue]) -> bool {
    actual.len() == expected.len()
        && actual
            .iter()
            .zip(expected.iter())
            .all(|(a, e)| atoms_are_alpha_equivalent(a, e))
}

/// Check if two expressions are alpha equivalent
fn atoms_are_alpha_equivalent(left: &MettaValue, right: &MettaValue) -> bool {
    let mut left_to_right = HashMap::new();
    let mut right_to_left = HashMap::new();

    alpha_equivalent_with_mappings(left, right, &mut left_to_right, &mut right_to_left)
}

/// Core alpha equivalence check with bidirectional variable mappings
fn alpha_equivalent_with_mappings(
    left: &MettaValue,
    right: &MettaValue,
    left_to_right: &mut HashMap<String, String>, // left var -> right var
    right_to_left: &mut HashMap<String, String>, // right var -> left var
) -> bool {
    use std::collections::hash_map::Entry;

    // Helper: check if variable can be consistently mapped
    fn can_map_variable(map: &mut HashMap<String, String>, from_var: &str, to_var: &str) -> bool {
        match map.entry(from_var.to_string()) {
            Entry::Occupied(entry) => entry.get() == to_var,
            Entry::Vacant(entry) => {
                entry.insert(to_var.to_string());
                true
            }
        }
    }

    match (left, right) {
        // Both variables: check bidirectional mapping consistency
        (MettaValue::Atom(left_var), MettaValue::Atom(right_var))
            if is_variable(left_var) && is_variable(right_var) =>
        {
            can_map_variable(left_to_right, left_var, right_var)
                && can_map_variable(right_to_left, right_var, left_var)
        }

        // Both non-variable atoms: must be identical
        (MettaValue::Atom(left_atom), MettaValue::Atom(right_atom))
            if !is_variable(left_atom) && !is_variable(right_atom) =>
        {
            left_atom == right_atom
        }

        // Primitive values: direct equality
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Float(a), MettaValue::Float(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,

        // S-expressions: recursive comparison
        (MettaValue::SExpr(left_items), MettaValue::SExpr(right_items)) => {
            left_items.len() == right_items.len()
                && left_items.iter().zip(right_items.iter()).all(|(l, r)| {
                    alpha_equivalent_with_mappings(l, r, left_to_right, right_to_left)
                })
        }

        // Type expressions: recursive comparison
        (MettaValue::Type(left_type), MettaValue::Type(right_type)) => {
            alpha_equivalent_with_mappings(left_type, right_type, left_to_right, right_to_left)
        }

        // Conjunctions: element-wise comparison
        (MettaValue::Conjunction(left_goals), MettaValue::Conjunction(right_goals)) => {
            left_goals.len() == right_goals.len()
                && left_goals.iter().zip(right_goals.iter()).all(|(l, r)| {
                    alpha_equivalent_with_mappings(l, r, left_to_right, right_to_left)
                })
        }

        // Different types: never equivalent
        _ => false,
    }
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

    #[test]
    fn test_identical_expressions() {
        // (foo A B) ≡ (foo A B)
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("A".to_string()),
            MettaValue::Atom("B".to_string()),
        ]);
        let expr2 = expr1.clone();

        assert!(atoms_are_alpha_equivalent(&expr1, &expr2));
    }

    #[test]
    fn test_variable_renaming() {
        // (R $x $y) ≡ (R $a $b)
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("R".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("R".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]);

        assert!(atoms_are_alpha_equivalent(&expr1, &expr2));
    }

    #[test]
    fn test_inconsistent_variable_usage() {
        // (R $x $y) ≢ (R $x $x) - inconsistent variable mapping
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("R".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("R".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        assert!(!atoms_are_alpha_equivalent(&expr1, &expr2));
    }

    #[test]
    fn test_complex_expressions() {
        // (= (foo $x) (+ $x $y)) ≡ (= (foo $a) (+ $a $b))
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);

        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$a".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
        ]);

        assert!(atoms_are_alpha_equivalent(&expr1, &expr2));
    }

    #[test]
    fn test_different_symbols() {
        // (foo A) ≢ (foo B) - different symbols
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("A".to_string()),
        ]);
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("B".to_string()),
        ]);

        assert!(!atoms_are_alpha_equivalent(&expr1, &expr2));
    }

    #[test]
    fn test_mixed_variables_and_symbols() {
        // (foo $x A $y) ≡ (foo $a A $b)
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("A".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("A".to_string()),
            MettaValue::Atom("$b".to_string()),
        ]);

        assert!(atoms_are_alpha_equivalent(&expr1, &expr2));

        // But not: (foo $x A $y) ≢ (foo $a B $b)
        let expr3 = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$a".to_string()),
            MettaValue::Atom("B".to_string()), // Different symbol
            MettaValue::Atom("$b".to_string()),
        ]);

        assert!(!atoms_are_alpha_equivalent(&expr1, &expr3));
    }

    #[test]
    fn test_assert_alpha_equal_success() {
        let env = Environment::new();

        // Create the function call directly
        let items = vec![
            MettaValue::Atom("assertAlphaEqual".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
        ];

        let (results, _) = eval_assert_alpha_equal(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_alpha_equal_failure() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqual".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ];

        let (results, _) = eval_assert_alpha_equal(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_assert_alpha_equal_msg_with_custom_message() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::String("Should be alpha equal".to_string()),
        ];

        let (results, _) = eval_assert_alpha_equal_msg(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_alpha_equal_msg_failure_with_custom_message() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::String("Custom alpha error".to_string()),
        ];

        let (results, _) = eval_assert_alpha_equal_msg(items, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "Custom alpha error");
            }
            _ => panic!("Expected error result, got: {:?}", results[0]),
        }
    }

    #[test]
    fn test_assert_alpha_equal_to_result_success() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualToResult".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("lambda".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("lambda".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ];

        let (results, _) = eval_assert_alpha_equal_to_result(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_alpha_equal_to_result_failure() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualToResult".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$a".to_string()),
            ]),
        ];

        let (results, _) = eval_assert_alpha_equal_to_result(items, env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_assert_alpha_equal_to_result_msg_success() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualToResultMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("R".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::String("Variables should rename".to_string()),
        ];

        let (results, _) = eval_assert_alpha_equal_to_result_msg(items, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Nil);
    }

    #[test]
    fn test_assert_alpha_equal_to_result_msg_failure() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("assertAlphaEqualToResultMsg".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("A".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("B".to_string()),
            ]),
            MettaValue::String("Symbols must match exactly".to_string()),
        ];

        let (results, _) = eval_assert_alpha_equal_to_result_msg(items, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "Symbols must match exactly");
            }
            _ => panic!("Expected error result, got: {:?}", results[0]),
        }
    }
}
