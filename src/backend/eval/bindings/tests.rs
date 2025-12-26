//! Tests for binding operations.

use super::super::eval;
use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

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

    // (let (foo $x) (bar 42) $x) - pattern mismatch returns Empty (HE-compatible)
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
    // HE-compatible: pattern mismatch returns Empty (no results)
    assert_eq!(results.len(), 0);
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

    // Test inconsistent variables - returns Empty (HE-compatible)
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
            MettaValue::Long(7), // Different value - pattern doesn't match
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]),
    ]);

    let (results, _) = eval(inconsistent_vars, env);
    // HE-compatible: pattern mismatch returns Empty (no results)
    assert_eq!(results.len(), 0);
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

// === Tests for pattern mismatch scenarios (HE-compatible Empty semantics) ===
// Note: In strict mode, these would log warnings. Here we just verify Empty is returned.

#[test]
fn test_pattern_mismatch_arity_hint() {
    let env = Environment::new();

    // (let ($a $b) (tuple 1 2 3) ...) - pattern has 2 elements, value has 4
    // Pattern mismatch returns Empty (HE-compatible)
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
    assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
}

#[test]
fn test_pattern_mismatch_head_hint() {
    let env = Environment::new();

    // (let (foo $x) (bar 42) $x) - head atoms don't match
    // Pattern mismatch returns Empty (HE-compatible)
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
    assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
}

#[test]
fn test_pattern_mismatch_literal_hint() {
    let env = Environment::new();

    // (let (pair 42 $x) (pair 99 hello) $x) - literal 42 doesn't match 99
    // Pattern mismatch returns Empty (HE-compatible)
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
    assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
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

    // Test failure case where literal doesn't match - returns Empty (HE-compatible)
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
            MettaValue::Long(43), // Different literal - pattern doesn't match
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
    // HE-compatible: pattern mismatch returns Empty (no results)
    assert_eq!(results.len(), 0);
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

#[test]
fn test_let_star_with_discard_pattern() {
    // Test that wildcard _ works as a discard pattern in let*
    // This is the proper way to discard values
    let env = Environment::new();

    // (let* ((_ 42)) "success") should succeed, discarding 42
    let discard_binding = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
            MettaValue::Long(42),
        ])]),
        MettaValue::String("success".to_string()),
    ]);

    let (results, _) = eval(discard_binding, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::String("success".to_string()));
}

#[test]
fn test_let_star_with_discard_and_binding() {
    // (let* ((_ (+ 1 2)) ($x 5)) $x) should return 5
    let env = Environment::new();

    let mixed_bindings = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![
            // First binding: discard the result of (+ 1 2)
            MettaValue::SExpr(vec![
                MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
            ]),
            // Second binding: $x = 5
            MettaValue::SExpr(vec![
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(5),
            ]),
        ]),
        MettaValue::Atom("$x".to_string()),
    ]);

    let (results, _) = eval(mixed_bindings, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(5));
}

#[test]
fn test_let_with_discard_pattern() {
    // (let _ "any-value" "ok") should succeed
    let env = Environment::new();

    let discard_let = MettaValue::SExpr(vec![
        MettaValue::Atom("let".to_string()),
        MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
        MettaValue::String("any-value".to_string()),
        MettaValue::String("ok".to_string()),
    ]);

    let (results, _) = eval(discard_let, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::String("ok".to_string()));
}

#[test]
fn test_let_star_wildcard_matches_any_type() {
    // Test that _ wildcard matches different types in let*
    let env = Environment::new();

    // Discard a string
    let discard_string = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("_".to_string()), // _ - wildcard
            MettaValue::String("ignored".to_string()),
        ])]),
        MettaValue::Long(1),
    ]);
    let (results, _) = eval(discard_string, env.clone());
    assert_eq!(results[0], MettaValue::Long(1));

    // Discard a boolean
    let discard_bool = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("_".to_string()), // _ - wildcard
            MettaValue::Bool(true),
        ])]),
        MettaValue::Long(2),
    ]);
    let (results, _) = eval(discard_bool, env.clone());
    assert_eq!(results[0], MettaValue::Long(2));

    // Discard an S-expression
    let discard_sexpr = MettaValue::SExpr(vec![
        MettaValue::Atom("let*".to_string()),
        MettaValue::SExpr(vec![MettaValue::SExpr(vec![
            MettaValue::Atom("_".to_string()), // _ - wildcard
            MettaValue::SExpr(vec![
                MettaValue::Atom("some".to_string()),
                MettaValue::Atom("expression".to_string()),
            ]),
        ])]),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(discard_sexpr, env);
    assert_eq!(results[0], MettaValue::Long(3));
}
