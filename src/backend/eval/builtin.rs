use crate::backend::models::MettaValue;
use std::sync::Arc;

/// Try to evaluate a built-in operation
/// Dispatches directly to built-in functions without going through Rholang interpreter
/// Uses operator symbols (+, -, *, etc.) instead of normalized names
pub(crate) fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        // Basic arithmetic
        "+" => Some(eval_checked_arithmetic(args, |a, b| a.checked_add(b), "+")),
        "-" => Some(eval_checked_arithmetic(args, |a, b| a.checked_sub(b), "-")),
        "*" => Some(eval_checked_arithmetic(args, |a, b| a.checked_mul(b), "*")),
        "/" => Some(eval_division(args)),

        // Comparison operators
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

        // Math functions
        "%" => Some(eval_modulo(args)),
        "pow-math" => Some(eval_power(args)),
        "sqrt-math" => Some(eval_sqrt(args)),
        "abs-math" => Some(eval_abs(args)),
        "log-math" => Some(eval_log(args)),
        "trunc-math" => Some(eval_trunc(args)),
        "ceil-math" => Some(eval_ceil(args)),
        "floor-math" => Some(eval_floor(args)),
        "round-math" => Some(eval_round(args)),
        "sin-math" => Some(eval_sin(args)),
        "asin-math" => Some(eval_asin(args)),
        "cos-math" => Some(eval_cos(args)),
        "acos-math" => Some(eval_acos(args)),
        "tan-math" => Some(eval_tan(args)),
        "atan-math" => Some(eval_atan(args)),
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

/// Evaluate power (exponentiation) with overflow checking
/// Takes base (first argument) and power (second argument) and returns result of base ^ power
/// Negative exponents are not supported for integer exponentiation
fn eval_power(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("Power", args, 2);

    let base = match extract_long(&args[0], "Cannot perform power") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let exp = match extract_long(&args[1], "Cannot perform power") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Negative exponents result in fractions, which are not integers
    if exp < 0 {
        return MettaValue::Error(
            format!(
                "Negative exponent not supported for integer exponentiation: {} ^ {}",
                base, exp
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Use checked_pow for overflow protection
    match base.checked_pow(exp as u32) {
        Some(result) => MettaValue::Long(result),
        None => MettaValue::Error(
            format!(
                "Arithmetic overflow: {} ^ {} exceeds integer bounds",
                base, exp
            ),
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

/// Evaluate square root (unary)
/// Returns the integer square root (floor) of the input number
/// Input must be >= 0
fn eval_sqrt(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("sqrt", args, 1, "(sqrt number)");

    let value = match extract_long(&args[0], "sqrt") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Negative numbers are not allowed
    if value < 0 {
        return MettaValue::Error(
            format!(
                "Square root of negative number not supported: sqrt({})",
                value
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Calculate integer square root (floor)
    // For perfect squares, this gives the exact result
    // For non-perfect squares, this gives the floor
    let result = (value as f64).sqrt() as i64;
    MettaValue::Long(result)
}

/// Evaluate absolute value (unary)
/// Returns the absolute value of the input number
fn eval_abs(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("abs-math", args, 1, "(abs-math number)");

    let value = match extract_long(&args[0], "abs-math") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Handle i64::MIN edge case: abs(i64::MIN) would overflow
    // i64::MIN = -9223372036854775808, abs would be 9223372036854775808
    // which exceeds i64::MAX (9223372036854775807)
    if value == i64::MIN {
        return MettaValue::Error(
            format!("Arithmetic overflow: abs({}) exceeds integer bounds", value),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    MettaValue::Long(value.abs())
}

/// Evaluate logarithm (binary)
/// Returns the integer logarithm (floor) of input number (second argument) with base (first argument)
/// Base must be > 0 and != 1, input number must be > 0
fn eval_log(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("log-math", args, 2, "(log-math base number)");

    let base = match extract_long(&args[0], "log-math") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let value = match extract_long(&args[1], "log-math") {
        Ok(n) => n,
        Err(e) => return e,
    };

    // Base must be positive and not equal to 1
    if base <= 0 {
        return MettaValue::Error(
            format!(
                "Logarithm base must be positive: log-math({}, {})",
                base, value
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    if base == 1 {
        return MettaValue::Error(
            format!("Logarithm base cannot be 1: log-math({}, {})", base, value),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Input number must be positive
    if value <= 0 {
        return MettaValue::Error(
            format!(
                "Logarithm input must be positive: log-math({}, {})",
                base, value
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Calculate integer logarithm (floor)
    // log_base(value) = ln(value) / ln(base)
    // For integer result, we compute the floor
    let result = (value as f64).ln() / (base as f64).ln();
    MettaValue::Long(result.floor() as i64)
}

/// Evaluate truncation (unary)
/// Returns the integer part of the input value (truncates toward zero)
fn eval_trunc(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("trunc-math", args, 1, "(trunc-math number)");

    let value = match extract_float(&args[0], "trunc-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.trunc() as i64)
}

/// Evaluate ceiling (unary)
/// Returns the smallest integer greater than or equal to the input value
fn eval_ceil(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("ceil-math", args, 1, "(ceil-math number)");

    let value = match extract_float(&args[0], "ceil-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.ceil() as i64)
}

/// Evaluate floor (unary)
/// Returns the largest integer less than or equal to the input value
fn eval_floor(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("floor-math", args, 1, "(floor-math number)");

    let value = match extract_float(&args[0], "floor-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.floor() as i64)
}

/// Evaluate round (unary)
/// Returns the nearest integer to the input value (rounds to nearest, ties round to even)
fn eval_round(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("round-math", args, 1, "(round-math number)");

    let value = match extract_float(&args[0], "round-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.round() as i64)
}

/// Evaluate sine (unary)
/// Returns the sine of the input angle in radians
fn eval_sin(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("sin-math", args, 1, "(sin-math angle)");

    let angle = match extract_float(&args[0], "sin-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(angle.sin())
}

/// Evaluate arcsine (unary)
/// Returns the arcsine of the input value in radians
/// Input must be in the range [-1, 1]
fn eval_asin(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("asin-math", args, 1, "(asin-math number)");

    let value = match extract_float(&args[0], "asin-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    // asin is only defined for values in [-1, 1]
    if value < -1.0 || value > 1.0 {
        return MettaValue::Error(
            format!(
                "Arcsine input must be in range [-1, 1]: asin-math({})",
                value
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    MettaValue::Float(value.asin())
}

/// Evaluate cosine (unary)
/// Returns the cosine of the input angle in radians
fn eval_cos(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("cos-math", args, 1, "(cos-math angle)");

    let angle = match extract_float(&args[0], "cos-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(angle.cos())
}

/// Evaluate arccosine (unary)
/// Returns the arccosine of the input value in radians
/// Input must be in the range [-1, 1]
fn eval_acos(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("acos-math", args, 1, "(acos-math number)");

    let value = match extract_float(&args[0], "acos-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    // acos is only defined for values in [-1, 1]
    if value < -1.0 || value > 1.0 {
        return MettaValue::Error(
            format!(
                "Arccosine input must be in range [-1, 1]: acos-math({})",
                value
            ),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    MettaValue::Float(value.acos())
}

/// Evaluate tangent (unary)
/// Returns the tangent of the input angle in radians
fn eval_tan(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("tan-math", args, 1, "(tan-math angle)");

    let angle = match extract_float(&args[0], "tan-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(angle.tan())
}

/// Evaluate arctangent (unary)
/// Returns the arctangent of the input value in radians
fn eval_atan(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("atan-math", args, 1, "(atan-math number)");

    let value = match extract_float(&args[0], "atan-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(value.atan())
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

/// Extract a Float value from MettaValue, returning a formatted error if not a Float or Long
/// Accepts both Float and Long (converts Long to Float)
fn extract_float(value: &MettaValue, context: &str) -> Result<f64, MettaValue> {
    match value {
        MettaValue::Float(f) => Ok(*f),
        MettaValue::Long(n) => Ok(*n as f64),
        other => Err(MettaValue::Error(
            format!(
                "{}: expected Number (float or integer), got {}",
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
    fn test_sqrt_basic() {
        let env = Environment::new();

        // Test: sqrt(4) = 2 (perfect square)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(4),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(2));

        // Test: sqrt(9) = 3 (perfect square)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(9),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: sqrt(16) = 4 (perfect square)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(16),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4));

        // Test: sqrt(0) = 0
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(0));

        // Test: sqrt(1) = 1
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(1),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));
    }

    #[test]
    fn test_sqrt_non_perfect_squares() {
        let env = Environment::new();

        // Test: sqrt(5) = 2 (floor of square root)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(5),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(2));

        // Test: sqrt(10) = 3 (floor of square root)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(10),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: sqrt(15) = 3 (floor of square root)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(15),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: sqrt(24) = 4 (floor of square root)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(24),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4));
    }

    #[test]
    fn test_sqrt_negative_number() {
        let env = Environment::new();

        // Test: sqrt(-1) should produce error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(-1),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(
                    msg.contains("negative number"),
                    "Expected negative number error: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }

        // Test: sqrt(-100) should also produce error
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(-100),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(
                    msg.contains("negative number"),
                    "Expected negative number error: {}",
                    msg
                );
                assert_eq!(**details, MettaValue::Atom("ArithmeticError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_sqrt_type_error() {
        let env = Environment::new();

        // Test: sqrt-math with string argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::String("4".to_string()),
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

        // Test: sqrt-math with bool argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
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
    fn test_sqrt_large_numbers() {
        let env = Environment::new();

        // Test: sqrt(10000) = 100 (perfect square)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(10000),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(100));

        // Test: sqrt(1000000) = 1000 (perfect square)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(1000000),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1000));

        // Test: sqrt(i64::MAX) should work (floor of square root)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("sqrt-math".to_string()),
            MettaValue::Long(i64::MAX),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        // sqrt(9223372036854775807) ≈ 3037000499 (floor)
        assert_eq!(results[0], MettaValue::Long(3037000499));
    }

    #[test]
    fn test_abs_basic() {
        let env = Environment::new();

        // Test: abs(5) = 5 (positive number)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(5),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: abs(-5) = 5 (negative number)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(-5),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: abs(0) = 0
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(0));

        // Test: abs(-100) = 100
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(-100),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(100));
    }

    #[test]
    fn test_abs_large_numbers() {
        let env = Environment::new();

        // Test: abs(i64::MAX) = i64::MAX
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(i64::MAX),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(i64::MAX));

        // Test: abs(-i64::MAX) = i64::MAX
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(-i64::MAX),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(i64::MAX));
    }

    #[test]
    fn test_abs_overflow_edge_case() {
        let env = Environment::new();

        // Test: abs(i64::MIN) should produce overflow error
        // i64::MIN = -9223372036854775808
        // abs(i64::MIN) would be 9223372036854775808, which exceeds i64::MAX
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::Long(i64::MIN),
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
    fn test_abs_type_error() {
        let env = Environment::new();

        // Test: abs-math with string argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
            MettaValue::String("-5".to_string()),
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

        // Test: abs-math with bool argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("abs-math".to_string()),
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
    fn test_trunc_basic() {
        let env = Environment::new();

        // Test: trunc(3.7) = 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::Float(3.7),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: trunc(-3.7) = -3 (truncates toward zero)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::Float(-3.7),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-3));

        // Test: trunc(5.0) = 5
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::Float(5.0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: trunc with integer (should convert to float first)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_ceil_basic() {
        let env = Environment::new();

        // Test: ceil(3.2) = 4
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Float(3.2),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4));

        // Test: ceil(-3.2) = -3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Float(-3.2),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-3));

        // Test: ceil(5.0) = 5
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Float(5.0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: ceil with integer
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_floor_basic() {
        let env = Environment::new();

        // Test: floor(3.7) = 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::Float(3.7),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: floor(-3.7) = -4
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::Float(-3.7),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-4));

        // Test: floor(5.0) = 5
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::Float(5.0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: floor with integer
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_round_basic() {
        let env = Environment::new();

        // Test: round(3.4) = 3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(3.4),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));

        // Test: round(3.6) = 4
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(3.6),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(4));

        // Test: round(-3.4) = -3
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(-3.4),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-3));

        // Test: round(-3.6) = -4
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(-3.6),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(-4));

        // Test: round(5.0) = 5
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Float(5.0),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));

        // Test: round with integer
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_rounding_type_error() {
        let env = Environment::new();

        // Test: trunc-math with string argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("trunc-math".to_string()),
            MettaValue::String("3.7".to_string()),
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

        // Test: ceil-math with bool argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("ceil-math".to_string()),
            MettaValue::Bool(true),
        ]);
        let (results, _) = eval(value, env.clone());
        assert_eq!(results.len(), 1);

        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert!(msg.contains("Bool"), "Expected 'Bool' in: {}", msg);
                assert_eq!(**details, MettaValue::Atom("TypeError".to_string()));
            }
            other => panic!("Expected Error, got {:?}", other),
        }

        // Test: floor-math with string argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("floor-math".to_string()),
            MettaValue::String("3.7".to_string()),
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

        // Test: round-math with bool argument should produce TypeError
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("round-math".to_string()),
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
}
