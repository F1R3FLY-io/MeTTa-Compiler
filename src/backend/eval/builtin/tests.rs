//! Tests for built-in operations.

use crate::backend::environment::Environment;
use crate::backend::eval::eval;
use crate::backend::models::MettaValue;

// Helper macro to evaluate and assert result
macro_rules! assert_eval {
    ($expr:expr, $expected:expr) => {{
        let env = Environment::new();
        let (results, _) = eval($expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], $expected);
    }};
}

// Helper to check for error with specific type
macro_rules! assert_error {
    ($expr:expr, $error_type:expr) => {{
        let env = Environment::new();
        let (results, _) = eval($expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(_, details) => {
                assert_eq!(**details, MettaValue::Atom($error_type.to_string()));
            }
            other => panic!("Expected Error({}), got {:?}", $error_type, other),
        }
    }};
}

#[test]
fn test_basic_operations() {
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::Long(3)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]),
        MettaValue::Bool(true)
    );
}

#[test]
fn test_eval_logical_and() {
    let env = Environment::new();

    // True and True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // True and False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // False and True = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // False and False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_eval_logical_or() {
    let env = Environment::new();

    // True or True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // True or False = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // False or True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // False or False = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::Bool(false),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_eval_logical_not() {
    let env = Environment::new();

    // not True = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // not False = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_eval_logical_type_error() {
    let env = Environment::new();

    // and with non-boolean should error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Long(1),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // or with non-boolean FIRST arg should error
    // (With short-circuit, (or True "hello") returns True without checking second arg)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("or".to_string()),
        MettaValue::String("hello".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // not with non-boolean should error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Long(42),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));
}

#[test]
fn test_eval_logical_arity_error() {
    let env = Environment::new();

    // and with wrong arity
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));

    // not with wrong arity
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("not".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0], MettaValue::Error(_, _)));
}

#[test]
fn test_arithmetic_type_error_string() {
    let env = Environment::new();

    // Test: !(+ 1 "a") should produce TypeError with friendly message
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::String("a".to_string()),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            // Error message should contain the friendly type name
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert!(
                msg.contains("expected Number (integer)"),
                "Expected 'expected Number (integer)' in: {}",
                msg
            );
            // Error details should be TypeError
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_arithmetic_type_error_first_arg() {
    let env = Environment::new();

    // Test: !(+ "a" 1) - first argument wrong type
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::String("a".to_string()),
        MettaValue::Long(1),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert!(
                msg.contains("expected Number (integer)"),
                "Expected type info in: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_arithmetic_type_error_bool() {
    let env = Environment::new();

    // Test: !(* true false) - booleans not valid for arithmetic
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("*".to_string()),
        MettaValue::Bool(true),
        MettaValue::Bool(false),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert!(
                msg.contains("expected Number (integer)"),
                "Expected type info in: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_comparison_strings_lexicographic() {
    let env = Environment::new();

    // Test: !(< "a" "b") - strings support lexicographic comparison
    // "a" < "b" should be true
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::String("a".to_string()),
        MettaValue::String("b".to_string()),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0],
        MettaValue::Bool(true),
        "\"a\" < \"b\" should be true"
    );
}

#[test]
fn test_comparison_type_mismatch() {
    let env = Environment::new();

    // Test: !(< 1 "b") - mixing types should produce type error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::Long(1),
        MettaValue::String("b".to_string()),
    ]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, _) => {
            assert!(
                msg.contains("type mismatch") || msg.contains("Cannot compare"),
                "Expected type mismatch error in: {}",
                msg
            );
        }
        other => panic!("Expected Error for type mismatch, got {:?}", other),
    }
}

#[test]
fn test_arithmetic_wrong_arity() {
    let env = Environment::new();

    // Test: !(+ 1) - wrong number of arguments
    let value = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);

    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, _) => {
            assert!(msg.contains("2 arguments"));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_modulo() {
    // Basic cases
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(3),
        ]),
        MettaValue::Long(1)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(7),
            MettaValue::Long(7),
        ]),
        MettaValue::Long(0)
    );

    // Negative numbers
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(-10),
            MettaValue::Long(3),
        ]),
        MettaValue::Long(-1)
    );

    // Errors: division by zero, overflow, type errors
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(0),
        ]),
        "ArithmeticError"
    );
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(i64::MIN),
            MettaValue::Long(-1),
        ]),
        "ArithmeticError"
    );
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::String("3".to_string()),
        ]),
        "TypeError"
    );
}

#[test]
fn test_power_basic() {
    let env = Environment::new();

    // Test: 2^3 = 8
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(8));

    // Test: 5^2 = 25
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(2),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(25));

    // Test: 3^4 = 81
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(3),
        MettaValue::Long(4),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(81));

    // Test: base^0 = 1 (any base to the power of 0 is 1)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(42),
        MettaValue::Long(0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));
}

#[test]
fn test_power_negative_base() {
    let env = Environment::new();

    // Test: (-2)^3 = -8
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(-2),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(-8));

    // Test: (-2)^2 = 4 (even exponent makes negative base positive)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(-2),
        MettaValue::Long(2),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(4));

    // Test: (-5)^4 = 625
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(-5),
        MettaValue::Long(4),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(625));
}

#[test]
fn test_power_negative_exponent() {
    let env = Environment::new();

    // Test: 2^-3 should produce error (negative exponents not supported for integers)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(-3),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("Negative exponent"),
                "Expected negative exponent error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: 10^-1 should also produce error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(-1),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("Negative exponent"),
                "Expected negative exponent error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_power_type_error() {
    let env = Environment::new();

    // Test: pow-math with string argument should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(2),
        MettaValue::String("3".to_string()),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert!(
                msg.contains("expected Number (integer)"),
                "Expected type info in: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: pow-math with bool argument should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_power_overflow_edge_case() {
    let env = Environment::new();

    // Test: 2^63 should produce overflow error (exceeds i64::MAX)
    // 2^63 = 9223372036854775808, which exceeds i64::MAX (9223372036854775807)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(63),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("Arithmetic overflow"),
                "Expected overflow error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: 10^19 should also produce overflow error
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(19),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("Arithmetic overflow"),
                "Expected overflow error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_sqrt() {
    // Perfect squares and edge cases
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(16),
        ]),
        MettaValue::Long(4)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(10),
        ]),
        MettaValue::Long(3)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(i64::MAX),
        ]),
        MettaValue::Long(3037000499)
    );

    // Errors: negative input, type errors
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(-1),
        ]),
        "ArithmeticError"
    );
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::String("4".to_string()),
        ]),
        "TypeError"
    );
}

#[test]
fn test_abs() {
    // Basic cases
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(-5),
        ]),
        MettaValue::Long(5)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(i64::MAX),
        ]),
        MettaValue::Long(i64::MAX)
    );

    // Errors: overflow (i64::MIN), type errors
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(i64::MIN),
        ]),
        "ArithmeticError"
    );
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::String("-5".to_string()),
        ]),
        "TypeError"
    );
}

#[test]
fn test_log_basic() {
    let env = Environment::new();

    // Test: log_2(8) = 3 (2^3 = 8)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(8),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));

    // Test: log_10(100) = 2 (10^2 = 100)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(100),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(2));

    // Test: log_3(9) = 2 (3^2 = 9)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(3),
        MettaValue::Long(9),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(2));

    // Test: log_5(125) = 3 (5^3 = 125)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(5),
        MettaValue::Long(125),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_log_non_integer_results() {
    let env = Environment::new();

    // Test: log_2(10) = 3 (floor of log_2(10) ≈ 3.32)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(10),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));

    // Test: log_10(50) = 1 (floor of log_10(50) ≈ 1.70)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(50),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(1));

    // Test: log_2(7) = 2 (floor of log_2(7) ≈ 2.81)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(7),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(2));
}

#[test]
fn test_log_invalid_base() {
    let env = Environment::new();

    // Test: log_0(10) should produce error (base <= 0)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(0),
        MettaValue::Long(10),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("base must be positive"),
                "Expected base error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: log_-1(10) should produce error (base <= 0)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(-1),
        MettaValue::Long(10),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("base must be positive"),
                "Expected base error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: log_1(10) should produce error (base == 1)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(10),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("base cannot be 1"),
                "Expected base == 1 error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_log_invalid_input() {
    let env = Environment::new();

    // Test: log_2(0) should produce error (input <= 0)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("input must be positive"),
                "Expected input error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: log_2(-5) should produce error (input <= 0)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(-5),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("input must be positive"),
                "Expected input error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_log_type_error() {
    let env = Environment::new();

    // Test: log-math with string base should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::String("2".to_string()),
        MettaValue::Long(8),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert!(
                msg.contains("expected Number (integer)"),
                "Expected type info in: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: log-math with string input should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::String("8".to_string()),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: log-math with bool argument should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Bool(true),
        MettaValue::Long(8),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_rounding_functions() {
    // Test all four rounding functions with key cases
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::Float(-3.7),
        ]),
        MettaValue::Long(-3)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Float(3.2),
        ]),
        MettaValue::Long(4)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::Float(-3.7),
        ]),
        MettaValue::Long(-4)
    );
    assert_eval!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(3.6),
        ]),
        MettaValue::Long(4)
    );

    // Type errors (one representative case)
    assert_error!(
        MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::String("3.7".to_string()),
        ]),
        "TypeError"
    );
}

#[test]
fn test_sin_basic() {
    let env = Environment::new();

    // Test: sin(0) = 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sin-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "sin(0) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: sin(π/2) ≈ 1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sin-math".to_string()),
        MettaValue::Float(std::f64::consts::PI / 2.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 1.0).abs() < 1e-10, "sin(π/2) should be 1, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: sin with integer (should convert to float)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sin-math".to_string()),
        MettaValue::Long(0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "sin(0) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_asin_basic() {
    let env = Environment::new();

    // Test: asin(0) = 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "asin(0) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: asin(1) = π/2
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Float(1.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            let expected = std::f64::consts::PI / 2.0;
            assert!(
                (f - expected).abs() < 1e-10,
                "asin(1) should be π/2, got {}",
                f
            );
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: asin(-1) = -π/2
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Float(-1.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            let expected = -std::f64::consts::PI / 2.0;
            assert!(
                (f - expected).abs() < 1e-10,
                "asin(-1) should be -π/2, got {}",
                f
            );
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_asin_out_of_range() {
    let env = Environment::new();

    // Test: asin(2) should produce error (out of range)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Float(2.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("range [-1, 1]"),
                "Expected range error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: asin(-2) should produce error (out of range)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Float(-2.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("range [-1, 1]"),
                "Expected range error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_cos_basic() {
    let env = Environment::new();

    // Test: cos(0) = 1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("cos-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 1.0).abs() < 1e-10, "cos(0) should be 1, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: cos(π/2) ≈ 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("cos-math".to_string()),
        MettaValue::Float(std::f64::consts::PI / 2.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => assert!(f.abs() < 1e-10, "cos(π/2) should be 0, got {}", f),
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: cos(π) = -1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("cos-math".to_string()),
        MettaValue::Float(std::f64::consts::PI),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - (-1.0)).abs() < 1e-10, "cos(π) should be -1, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_acos_basic() {
    let env = Environment::new();

    // Test: acos(1) = 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("acos-math".to_string()),
        MettaValue::Float(1.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "acos(1) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: acos(0) = π/2
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("acos-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            let expected = std::f64::consts::PI / 2.0;
            assert!(
                (f - expected).abs() < 1e-10,
                "acos(0) should be π/2, got {}",
                f
            );
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: acos(-1) = π
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("acos-math".to_string()),
        MettaValue::Float(-1.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            let expected = std::f64::consts::PI;
            assert!(
                (f - expected).abs() < 1e-10,
                "acos(-1) should be π, got {}",
                f
            );
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_acos_out_of_range() {
    let env = Environment::new();

    // Test: acos(2) should produce error (out of range)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("acos-math".to_string()),
        MettaValue::Float(2.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("range [-1, 1]"),
                "Expected range error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: acos(-2) should produce error (out of range)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("acos-math".to_string()),
        MettaValue::Float(-2.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(
                msg.contains("range [-1, 1]"),
                "Expected range error: {}",
                msg
            );
            assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_tan_basic() {
    let env = Environment::new();

    // Test: tan(0) = 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("tan-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "tan(0) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: tan(π/4) ≈ 1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("tan-math".to_string()),
        MettaValue::Float(std::f64::consts::PI / 4.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 1.0).abs() < 1e-10, "tan(π/4) should be 1, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_atan_basic() {
    let env = Environment::new();

    // Test: atan(0) = 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("atan-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!((f - 0.0).abs() < 1e-10, "atan(0) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: atan(1) = π/4
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("atan-math".to_string()),
        MettaValue::Float(1.0),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            let expected = std::f64::consts::PI / 4.0;
            assert!(
                (f - expected).abs() < 1e-10,
                "atan(1) should be π/4, got {}",
                f
            );
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_trigonometric_type_error() {
    let env = Environment::new();

    // Test: sin-math with string argument should produce TypeError
    // (All trig functions use the same extract_float helper, so one test is sufficient)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sin-math".to_string()),
        MettaValue::String("0".to_string()),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: asin-math with bool argument should produce TypeError
    // (Tests inverse trig function and different error type)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("asin-math".to_string()),
        MettaValue::Bool(false),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_isnan_basic() {
    let env = Environment::new();

    // Test: isnan(NaN) = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Float(f64::NAN),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // Test: isnan(0.0) = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isnan(1.0) = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Float(1.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isnan(infinity) = False (infinity is not NaN)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Float(f64::INFINITY),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isnan(-infinity) = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Float(f64::NEG_INFINITY),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isnan with integer (should convert to float, not NaN)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::Long(5),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_isinf_basic() {
    let env = Environment::new();

    // Test: isinf(infinity) = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Float(f64::INFINITY),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // Test: isinf(-infinity) = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Float(f64::NEG_INFINITY),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // Test: isinf(0.0) = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Float(0.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isinf(1.0) = False
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Float(1.0),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isinf(NaN) = False (NaN is not infinity)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Float(f64::NAN),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));

    // Test: isinf with integer (should convert to float, not infinity)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Long(5),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(false));
}

#[test]
fn test_isnan_isinf_type_error() {
    let env = Environment::new();

    // Test: isnan-math with string argument should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isnan-math".to_string()),
        MettaValue::String("NaN".to_string()),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }

    // Test: isinf-math with bool argument should produce TypeError
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("isinf-math".to_string()),
        MettaValue::Bool(true),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);

    match &results[0] {
        MettaValue::Error(msg, details) => {
            assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
            assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
        }
        other => panic!("Expected Error, got {:?}", other),
    }
}

#[test]
fn test_mixed_arithmetic_and_power() {
    let env = Environment::new();

    // Test: sqrt(pow(2, 4)) = sqrt(16) = 4
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sqrt-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("pow-math".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(4),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(4));

    // Test: pow(2, sqrt(16)) = pow(2, 4) = 16
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(2),
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(16),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(16));

    // Test: abs(-5) + 3 = 5 + 3 = 8
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(-5),
        ]),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(8));
}

#[test]
fn test_mixed_logarithm_and_power() {
    let env = Environment::new();

    // Test: log(2, pow(2, 3)) = log(2, 8) = 3
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("log-math".to_string()),
        MettaValue::Long(2),
        MettaValue::SExpr(vec![
            MettaValue::Atom("pow-math".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));

    // Test: pow(10, log(10, 100)) = pow(10, 2) = 100
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("pow-math".to_string()),
        MettaValue::Long(10),
        MettaValue::SExpr(vec![
            MettaValue::Atom("log-math".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(100),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(100));
}

#[test]
fn test_mixed_rounding_and_arithmetic() {
    let env = Environment::new();

    // Test: floor(sqrt(10)) = floor(3.16...) = 3
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("floor-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(10),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));

    // Test: ceil(3.5) = 4
    // Using Float directly since sqrt-math only accepts Long
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("ceil-math".to_string()),
        MettaValue::Float(3.5),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(4));

    // Test: round(sqrt(5)) = round(2.23...) = 2
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("round-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(5),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(2));
}

#[test]
fn test_mixed_trigonometric_operations() {
    let env = Environment::new();

    // Test: sin(acos(0)) = sin(π/2) = 1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("sin-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("acos-math".to_string()),
            MettaValue::Float(0.0),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!(
                (f - 1.0).abs() < 1e-10,
                "sin(acos(0)) should be 1, got {}",
                f
            )
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: cos(asin(1)) = cos(π/2) ≈ 0
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("cos-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("asin-math".to_string()),
            MettaValue::Float(1.0),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!(f.abs() < 1e-10, "cos(asin(1)) should be 0, got {}", f)
        }
        other => panic!("Expected Float, got {:?}", other),
    }

    // Test: tan(atan(1)) = tan(π/4) = 1
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("tan-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("atan-math".to_string()),
            MettaValue::Float(1.0),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    match &results[0] {
        MettaValue::Float(f) => {
            assert!(
                (f - 1.0).abs() < 1e-10,
                "tan(atan(1)) should be 1, got {}",
                f
            )
        }
        other => panic!("Expected Float, got {:?}", other),
    }
}

#[test]
fn test_mixed_complex_expressions() {
    let env = Environment::new();

    // Test: abs(-5) * 2 + 3 = 5 * 2 + 3 = 13
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("*".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("abs-math".to_string()),
                MettaValue::Long(-5),
            ]),
            MettaValue::Long(2),
        ]),
        MettaValue::Long(3),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(13));

    // Test: floor(log(2, pow(2, 3))) = floor(log(2, 8)) = floor(3.0) = 3
    // Using base 2 for exact integer result (log(2, 8) = 3.0 exactly)
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("floor-math".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("log-math".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pow-math".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));

    // Test: sqrt(pow(3, 2)) % 5 = sqrt(9) % 5 = 3 % 5 = 3
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("%".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pow-math".to_string()),
                MettaValue::Long(3),
                MettaValue::Long(2),
            ]),
        ]),
        MettaValue::Long(5),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(3));
}

#[test]
fn test_mixed_with_comparisons() {
    let env = Environment::new();

    // Test: < (sqrt 16) 5 = < 4 5 = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("<".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(16),
        ]),
        MettaValue::Long(5),
    ]);
    let (results, _) = eval(value, env.clone());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));

    // Test: == (pow 2 3) 8 = == 8 8 = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("==".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("pow-math".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]),
        MettaValue::Long(8),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}

#[test]
fn test_mixed_with_logical_operators() {
    let env = Environment::new();

    // Test: and (< (abs -5) 10) (> (sqrt 9) 2) = and (< 5 10) (> 3 2) = and True True = True
    let value = MettaValue::SExpr(vec![
        MettaValue::Atom("and".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("abs-math".to_string()),
                MettaValue::Long(-5),
            ]),
            MettaValue::Long(10),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom(">".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("sqrt-math".to_string()),
                MettaValue::Long(9),
            ]),
            MettaValue::Long(2),
        ]),
    ]);
    let (results, _) = eval(value, env);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Bool(true));
}
