use crate::backend::models::MettaValue;
use std::sync::Arc;

/// Try to evaluate a built-in operation
/// Dispatches directly to built-in functions without going through Rholang interpreter
/// Uses operator symbols (+, -, *, etc.) instead of normalized names
pub(crate) fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        "+" => Some(eval_checked_arithmetic(args, |a, b| a.checked_add(b), "+")),
        "-" => Some(eval_checked_arithmetic(args, |a, b| a.checked_sub(b), "-")),
        "*" => Some(eval_checked_arithmetic(args, |a, b| a.checked_mul(b), "*")),
        "/" => Some(eval_division(args)),
        "%" => Some(eval_modulo(args)),
        "<" => Some(eval_comparison(args, |a, b| a < b)),
        "<=" => Some(eval_comparison(args, |a, b| a <= b)),
        ">" => Some(eval_comparison(args, |a, b| a > b)),
        ">=" => Some(eval_comparison(args, |a, b| a >= b)),
        "==" => Some(eval_comparison(args, |a, b| a == b)),
        "!=" => Some(eval_comparison(args, |a, b| a != b)),
        // Logical operators
        "and" => Some(eval_logical_binary(args, |a, b| a && b, "and")),
        "or" => Some(eval_logical_binary(args, |a, b| a || b, "or")),
        "not" => Some(eval_logical_not(args)),
        _ => None,
    }
}

/// Evaluate a binary arithmetic operation with overflow checking
fn eval_checked_arithmetic<F>(args: &[MettaValue], op: F, op_name: &str) -> MettaValue
where
    F: Fn(i64, i64) -> Option<i64>,
{
    if args.len() != 2 {
        return MettaValue::Error(
            format!(
                "Arithmetic operation '{}' requires exactly 2 arguments, got {}",
                op_name,
                args.len()
            ),
            Arc::new(MettaValue::Nil),
        );
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot perform '{}': expected Number (integer), got {}",
                    op_name,
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot perform '{}': expected Number (integer), got {}",
                    op_name,
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    match op(a, b) {
        Some(result) => MettaValue::Long(result),
        None => MettaValue::Error(
            format!(
                "Arithmetic overflow: {} {} {} exceeds integer bounds",
                a, op_name, b
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        ),
    }
}

/// Evaluate division with division-by-zero and overflow checking
fn eval_division(args: &[MettaValue]) -> MettaValue {
    if args.len() != 2 {
        return MettaValue::Error(
            format!("Division requires exactly 2 arguments, got {}", args.len()),
            Arc::new(MettaValue::Nil),
        );
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot divide: expected Number (integer), got {}",
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot divide: expected Number (integer), got {}",
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    if b == 0 {
        return MettaValue::Error(
            "Division by zero".to_string(),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Use checked_div for overflow protection (e.g., i64::MIN / -1)
    match a.checked_div(b) {
        Some(result) => MettaValue::Long(result),
        None => MettaValue::Error(
            format!("Arithmetic overflow: {} / {} exceeds integer bounds", a, b),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        ),
    }
}

fn eval_modulo(_args: &[MettaValue]) -> MettaValue {
    todo!()
}

/// Evaluate a comparison operation with strict type checking
fn eval_comparison<F>(args: &[MettaValue], op: F) -> MettaValue
where
    F: Fn(i64, i64) -> bool,
{
    if args.len() != 2 {
        return MettaValue::Error(
            format!(
                "Comparison operation requires exactly 2 arguments, got {}",
                args.len()
            ),
            Arc::new(MettaValue::Nil),
        );
    }

    let a = match &args[0] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    let b = match &args[1] {
        MettaValue::Long(n) => *n,
        other => {
            return MettaValue::Error(
                format!(
                    "Cannot compare: expected Number (integer), got {}",
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    MettaValue::Bool(op(a, b))
}

/// Evaluate a binary logical operation (and, or)
fn eval_logical_binary<F>(args: &[MettaValue], op: F, op_name: &str) -> MettaValue
where
    F: Fn(bool, bool) -> bool,
{
    if args.len() != 2 {
        return MettaValue::Error(
            format!(
                "'{}' requires exactly 2 arguments, got {}. Usage: ({} bool1 bool2)",
                op_name,
                args.len(),
                op_name
            ),
            Arc::new(MettaValue::Atom("ArityError".to_string())),
        );
    }

    let a = match &args[0] {
        MettaValue::Bool(b) => *b,
        other => {
            return MettaValue::Error(
                format!(
                    "'{}': expected Bool, got {}",
                    op_name,
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    let b = match &args[1] {
        MettaValue::Bool(b) => *b,
        other => {
            return MettaValue::Error(
                format!(
                    "'{}': expected Bool, got {}",
                    op_name,
                    other.friendly_type_name()
                ),
                Arc::new(MettaValue::Atom("TypeError".to_string())),
            );
        }
    };

    MettaValue::Bool(op(a, b))
}

/// Evaluate logical not (unary)
fn eval_logical_not(args: &[MettaValue]) -> MettaValue {
    if args.len() != 1 {
        return MettaValue::Error(
            format!(
                "'not' requires exactly 1 argument, got {}. Usage: (not bool)",
                args.len()
            ),
            Arc::new(MettaValue::Atom("ArityError".to_string())),
        );
    }

    match &args[0] {
        MettaValue::Bool(b) => MettaValue::Bool(!b),
        other => MettaValue::Error(
            format!("'not': expected Bool, got {}", other.friendly_type_name()),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        ),
    }
}

// TODO -> tests with edge cases: NaN, infinity, negative numbers
// TODO -> tests with integer vs float arguments
// TODO -> error handling tests

// TODO -> test more combined operations
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::Environment;
    use crate::backend::eval::eval;

    #[test]
    fn test_eval_builtin_add() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_eval_builtin_comparison() {
        let env = Environment::new();
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
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

        // or with non-boolean should error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("or".to_string()),
            MettaValue::Bool(true),
            MettaValue::String("hello".to_string()),
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
    fn test_comparison_type_error() {
        let env = Environment::new();

        // Test: !(< "a" "b") - strings not valid for comparison
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::String("b".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("String"), "Expected 'String' in: {}", msg);
                assert!(
                    msg.contains("Cannot compare"),
                    "Expected 'Cannot compare' in: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
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
}
