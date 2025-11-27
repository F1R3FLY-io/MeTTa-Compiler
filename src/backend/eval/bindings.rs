use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::{apply_bindings, eval, pattern_match};

/// Generate helpful message for pattern mismatch in let bindings
fn pattern_mismatch_suggestion(pattern: &MettaValue, value: &MettaValue) -> String {
    let pattern_arity = match pattern {
        MettaValue::SExpr(items) => items.len(),
        _ => 1,
    };
    let value_arity = match value {
        MettaValue::SExpr(items) => items.len(),
        _ => 1,
    };

    // Check for arity mismatch
    if pattern_arity != value_arity {
        return format!(
            "Hint: pattern has {} element(s) but value has {}. Adjust pattern to match value structure.",
            pattern_arity, value_arity
        );
    }

    // Check for structure mismatch (different head atoms)
    if let (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) = (pattern, value) {
        if let (Some(MettaValue::Atom(p_head)), Some(MettaValue::Atom(v_head))) =
            (p_items.first(), v_items.first())
        {
            if p_head != v_head {
                return format!(
                    "Hint: pattern head '{}' doesn't match value head '{}'.",
                    p_head, v_head
                );
            }
        }
    }

    // Check for literal mismatch inside structures
    if let (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) = (pattern, value) {
        for (i, (p, v)) in p_items.iter().zip(v_items.iter()).enumerate() {
            // Skip if pattern is a variable (starts with $, &, or ')
            if let MettaValue::Atom(name) = p {
                if name.starts_with('$')
                    || name.starts_with('&')
                    || name.starts_with('\'')
                    || name == "_"
                {
                    continue;
                }
            }
            // Check for literal mismatch
            if p != v && !matches!(p, MettaValue::SExpr(_)) {
                return format!(
                    "Hint: element at position {} doesn't match - pattern has {:?} but value has {:?}.",
                    i, p, v
                );
            }
        }
    }

    // Default hint
    "Hint: pattern structure doesn't match value. Check that variable names align with value positions.".to_string()
}

/// Evaluate let binding: (let pattern value body)
/// Evaluates value, binds it to pattern, and evaluates body with those bindings
/// Supports both simple variable binding and pattern matching:
///   - (let $x 42 body) - simple binding
///   - (let ($a $b) (tuple 1 2) body) - destructuring pattern
pub(super) fn eval_let(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    if args.len() < 3 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                got
            ),
            Box::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let value_expr = &args[1];
    let body = &args[2];

    // Evaluate the value expression first
    let (value_results, value_env) = eval(value_expr.clone(), env);

    // Handle nondeterminism: if value evaluates to multiple results, try each one
    let mut all_results = Vec::new();

    for value in value_results {
        // Try to match the pattern against the value
        if let Some(bindings) = pattern_match(pattern, &value) {
            // Apply bindings to the body and evaluate it
            let instantiated_body = apply_bindings(body, &bindings);
            let (body_results, _) = eval(instantiated_body, value_env.clone());
            all_results.extend(body_results);
        } else {
            // Pattern match failed - provide helpful suggestion
            let suggestion = pattern_mismatch_suggestion(pattern, &value);
            let err = MettaValue::Error(
                format!(
                    "let pattern {:?} does not match value {:?}. {}",
                    pattern, value, suggestion
                ),
                Box::new(MettaValue::SExpr(args.to_vec())),
            );
            all_results.push(err);
        }
    }

    (all_results, value_env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_let_simple_binding() {
        let env = Environment::new();

        // (let $x 42 $x)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_let_with_expression() {
        let env = Environment::new();

        // (let $y (+ 10 5) (* $y 2))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$y".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(10),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$y".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_let_with_pattern_matching() {
        let env = Environment::new();

        // (let (tuple $a $b) (tuple 1 2) (+ $a $b))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_let_nested() {
        let env = Environment::new();

        // (let $z 3 (let $w 4 (+ $z $w)))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$z".to_string()),
            MettaValue::Long(3),
            MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$w".to_string()),
                MettaValue::Long(4),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$z".to_string()),
                    MettaValue::Atom("$w".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_let_with_if() {
        let env = Environment::new();

        // (let $base 10 (if (> $base 5) (* $base 2) $base))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$base".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(5),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(2),
                ]),
                MettaValue::Atom("$base".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(20));
    }

    #[test]
    fn test_let_pattern_mismatch() {
        let env = Environment::new();

        // (let (foo $x) (bar 42) $x) - pattern mismatch should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("does not match"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_let_with_wildcard_pattern() {
        let env = Environment::new();

        // (let _ 42 "ignored")
        // Wildcard should match anything but not bind
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("_".to_string()),
            MettaValue::Long(42),
            MettaValue::String("ignored".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("ignored".to_string()));
    }

    #[test]
    fn test_let_with_complex_pattern_structures() {
        let env = Environment::new();

        // (let (nested (inner $x $y) $z) (nested (inner 1 2) 3) (+ $x (+ $y $z)))
        let complex_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nested".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("inner".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::Atom("$z".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nested".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("inner".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
                MettaValue::Long(3),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Atom("$z".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(complex_pattern, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6)); // 1 + (2 + 3)
    }

    #[test]
    fn test_let_with_variable_consistency() {
        let env = Environment::new();

        // Test that same variable in pattern must match same value
        // (let (same $x $x) (same 5 5) (* $x 2))
        let consistent_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(consistent_vars, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10)); // 5 * 2

        // Test inconsistent variables should fail
        // (let (same $x $x) (same 5 7) (* $x 2))
        let inconsistent_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(7), // Different value - should fail
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(inconsistent_vars, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("does not match"));
            }
            _ => panic!("Expected pattern match error"),
        }
    }

    #[test]
    fn test_let_with_different_variable_types() {
        let env = Environment::new();

        // Test different variable prefixes: $, &, '
        let mixed_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("triple".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("&y".to_string()),
                MettaValue::Atom("'z".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("triple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("&y".to_string()),
                    MettaValue::Atom("'z".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(mixed_vars, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6)); // 1 + (2 + 3)
    }

    #[test]
    fn test_let_missing_arguments() {
        let env = Environment::new();

        // Test let with only 2 arguments
        let let_two_args = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(let_two_args, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 2"), "Expected 'got 2' in: {}", msg);
                assert!(msg.contains("Usage:"), "Expected 'Usage:' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }

        // Test let with only 1 argument
        let let_one_arg = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let (results, _) = eval(let_one_arg, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 1"), "Expected 'got 1' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }

        // Test let with no arguments
        let let_no_args = MettaValue::SExpr(vec![MettaValue::Atom("let".to_string())]);
        let (results, _) = eval(let_no_args, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 0"), "Expected 'got 0' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }
    }

    #[test]
    fn test_let_with_evaluated_value_expression() {
        let env = Environment::new();

        // Test let where value needs evaluation
        // (let $result (+ (* 3 4) 5) (if (> $result 10) "big" "small"))
        let eval_value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$result".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(4),
                ]),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$result".to_string()),
                    MettaValue::Long(10),
                ]),
                MettaValue::String("big".to_string()),
                MettaValue::String("small".to_string()),
            ]),
        ]);

        let (results, _) = eval(eval_value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("big".to_string())); // 17 > 10
    }

    #[test]
    fn test_let_with_error_in_value() {
        let env = Environment::new();

        // Test let where value expression produces error
        // (let $x (error "value-error" nil) $x)
        let error_value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("value-error".to_string()),
                MettaValue::Nil,
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(error_value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "value-error");
            }
            _ => panic!("Expected error to be bound and returned"),
        }
    }

    // === Tests for "Did You Mean" pattern mismatch suggestions ===

    #[test]
    fn test_pattern_mismatch_arity_hint() {
        let env = Environment::new();

        // (let ($a $b) (tuple 1 2 3) ...) - pattern has 2 elements, value has 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$a".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
                assert!(
                    msg.contains("2 element"),
                    "Expected arity mismatch hint in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with pattern mismatch hint"),
        }
    }

    #[test]
    fn test_pattern_mismatch_head_hint() {
        let env = Environment::new();

        // (let (foo $x) (bar 42) $x) - head atoms don't match
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
                assert!(
                    msg.contains("foo") && msg.contains("bar"),
                    "Expected head mismatch hint in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with pattern mismatch hint"),
        }
    }

    #[test]
    fn test_pattern_mismatch_literal_hint() {
        let env = Environment::new();

        // (let (pair 42 $x) (pair 99 hello) $x) - literal 42 doesn't match 99
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(99),
                MettaValue::Atom("hello".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("Hint"), "Expected 'Hint' in: {}", msg);
                // Should mention the position and values that don't match
                assert!(
                    msg.contains("position") || msg.contains("doesn't match"),
                    "Expected position mismatch hint in: {}",
                    msg
                );
            }
            _ => panic!("Expected error with pattern mismatch hint"),
        }
    }

    #[test]
    fn test_let_with_mixed_pattern_elements() {
        let env = Environment::new();

        // Pattern with mix of literals and variables
        // (let (mixed 42 $x "literal" $y) (mixed 42 100 "literal" 200) (+ $x $y))
        let mixed_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
                MettaValue::String("literal".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Long(100),
                MettaValue::String("literal".to_string()),
                MettaValue::Long(200),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);

        let (results, _) = eval(mixed_pattern, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(300)); // 100 + 200

        // Test failure case where literal doesn't match
        // (let (mixed 42 $x "literal" $y) (mixed 43 100 "literal" 200) (+ $x $y))
        let mixed_fail = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
                MettaValue::String("literal".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(43), // Different literal - should fail
                MettaValue::Long(100),
                MettaValue::String("literal".to_string()),
                MettaValue::Long(200),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);

        let (results, _) = eval(mixed_fail, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("does not match"));
            }
            _ => panic!("Expected pattern match error"),
        }
    }

    #[test]
    fn test_let_with_complex_body_expressions() {
        let env = Environment::new();

        // Test let with complex body containing multiple operations
        // (let $base 5
        //   (if (> $base 0)
        //     (let $squared (* $base $base)
        //       (+ $squared $base))
        //     0))
        let complex_body = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$base".to_string()),
            MettaValue::Long(5),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("let".to_string()),
                    MettaValue::Atom("$squared".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("*".to_string()),
                        MettaValue::Atom("$base".to_string()),
                        MettaValue::Atom("$base".to_string()),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("+".to_string()),
                        MettaValue::Atom("$squared".to_string()),
                        MettaValue::Atom("$base".to_string()),
                    ]),
                ]),
                MettaValue::Long(0),
            ]),
        ]);

        let (results, _) = eval(complex_body, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30)); // (5 * 5) + 5 = 30
    }
}
