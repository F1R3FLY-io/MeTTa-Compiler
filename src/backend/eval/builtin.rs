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
    require_builtin_args!(format!("Arithmetic operation '{}'", op_name), args, 2);

    let a = match extract_long(&args[0], &format!("Cannot perform '{}'", op_name)) {
        Ok(n) => n,
        Err(e) => return e,
    };

    let b = match extract_long(&args[1], &format!("Cannot perform '{}'", op_name)) {
        Ok(n) => n,
        Err(e) => return e,
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
    require_builtin_args!("Division", args, 2);

    let a = match extract_long(&args[0], "Cannot divide") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let b = match extract_long(&args[1], "Cannot divide") {
        Ok(n) => n,
        Err(e) => return e,
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

/// Evaluate modulo with division-by-zero and overflow checking
/// Returns the remainder of dividing the first argument (dividend) by the second argument (divisor)
fn eval_modulo(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("Modulo", args, 2);

    let a = match extract_long(&args[0], "Cannot perform modulo") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let b = match extract_long(&args[1], "Cannot perform modulo") {
        Ok(n) => n,
        Err(e) => return e,
    };

    if b == 0 {
        return MettaValue::Error(
            "Division by zero".to_string(),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Use checked_rem for overflow protection (e.g., i64::MIN % -1)
    match a.checked_rem(b) {
        Some(result) => MettaValue::Long(result),
        None => MettaValue::Error(
            format!("Arithmetic overflow: {} % {} exceeds integer bounds", a, b),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        ),
    }
}

/// Evaluate a comparison operation with strict type checking
fn eval_comparison<F>(args: &[MettaValue], op: F) -> MettaValue
where
    F: Fn(i64, i64) -> bool,
{
    require_builtin_args!("Comparison operation", args, 2);

    let a = match extract_long(&args[0], "Cannot compare") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let b = match extract_long(&args[1], "Cannot compare") {
        Ok(n) => n,
        Err(e) => return e,
    };

    MettaValue::Bool(op(a, b))
}

/// Evaluate a binary logical operation (and, or)
fn eval_logical_binary<F>(args: &[MettaValue], op: F, op_name: &str) -> MettaValue
where
    F: Fn(bool, bool) -> bool,
{
    require_builtin_args!(
        format!("'{}'", op_name),
        args,
        2,
        format!("({} bool1 bool2)", op_name)
    );

    let a = match extract_bool(&args[0], &format!("'{}'", op_name)) {
        Ok(b) => b,
        Err(e) => return e,
    };

    let b = match extract_bool(&args[1], &format!("'{}'", op_name)) {
        Ok(b) => b,
        Err(e) => return e,
    };

    MettaValue::Bool(op(a, b))
}

/// Evaluate logical not (unary)
fn eval_logical_not(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("'not'", args, 1, "(not bool)");

    match extract_bool(&args[0], "'not'") {
        Ok(b) => MettaValue::Bool(!b),
        Err(e) => e,
    }
}

/// Extract a Long (integer) value from MettaValue, returning a formatted error if not a Long
fn extract_long(value: &MettaValue, context: &str) -> Result<i64, MettaValue> {
    match value {
        MettaValue::Long(n) => Ok(*n),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Number (integer), got {}",
                context,
                other.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
    }
}

/// Extract a Bool value from MettaValue, returning a formatted error if not a Bool
fn extract_bool(value: &MettaValue, context: &str) -> Result<bool, MettaValue> {
    match value {
        MettaValue::Bool(b) => Ok(*b),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Bool, got {}",
                context,
                other.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        )),
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

    #[test]
    fn test_modulo_basic() {
        let env = Environment::new();

        // Test: 10 % 3 = 1
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));

        // Test: 15 % 4 = 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(15),
            MettaValue::Long(4),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: 7 % 7 = 0 (exact division)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(7),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(0));
    }

    #[test]
    fn test_modulo_negative_numbers() {
        let env = Environment::new();

        // Test: -10 % 3 = -1 (Rust's remainder behavior)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(-10),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-1));

        // Test: 10 % -3 = 1 (Rust's remainder behavior)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(-3),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));

        // Test: -10 % -3 = -1
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(-10),
            MettaValue::Long(-3),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-1));
    }

    #[test]
    fn test_modulo_division_by_zero() {
        let env = Environment::new();

        // Test: 10 % 0 should produce division by zero error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(
                    msg.contains("Division by zero"),
                    "Expected division by zero error: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }

        // Test: -5 % 0 should also produce division by zero error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(-5),
            MettaValue::Long(0),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(
                    msg.contains("Division by zero"),
                    "Expected division by zero error: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_modulo_type_error() {
        let env = Environment::new();

        // Test: % with string argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(10),
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

        // Test: % with bool argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
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
    fn test_modulo_overflow_edge_case() {
        let env = Environment::new();

        // Test: i64::MIN % -1 should produce overflow error
        // This is an edge case where checked_rem returns None
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("%".to_string()),
            MettaValue::Long(i64::MIN),
            MettaValue::Long(-1),
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
}
