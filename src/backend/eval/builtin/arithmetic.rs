//! Arithmetic operations for built-in evaluation.
//!
//! This module implements basic arithmetic operations:
//! - Addition (+)
//! - Subtraction (-)
//! - Multiplication (*)
//! - Division (/)
//! - Modulo (%)
//! - Floor division (floor-div)
//! - Power (pow-math)
//! - Absolute value (abs-math)

use std::sync::Arc;

use crate::backend::models::MettaValue;

use super::extractors::extract_long;

/// Evaluate a binary arithmetic operation with overflow checking
pub(crate) fn eval_checked_arithmetic<F>(args: &[MettaValue], op: F, op_name: &str) -> MettaValue
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
pub(crate) fn eval_division(args: &[MettaValue]) -> MettaValue {
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
pub(crate) fn eval_modulo(args: &[MettaValue]) -> MettaValue {
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

/// Evaluate floor division (integer division that rounds toward negative infinity)
/// Returns the floor of dividing the first argument by the second argument
/// Unlike truncation which rounds toward zero, floor division rounds toward negative infinity
/// Examples: floor-div(7, 3) = 2, floor-div(-7, 3) = -3
pub(crate) fn eval_floor_div(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("floor-div", args, 2, "(floor-div dividend divisor)");

    let a = match extract_long(&args[0], "floor-div") {
        Ok(n) => n,
        Err(e) => return e,
    };

    let b = match extract_long(&args[1], "floor-div") {
        Ok(n) => n,
        Err(e) => return e,
    };

    if b == 0 {
        return MettaValue::Error(
            "Division by zero".to_string(),
            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
        );
    }

    // Use div_euclid for floor division semantics (rounds toward negative infinity)
    MettaValue::Long(a.div_euclid(b))
}

/// Evaluate power (exponentiation) with overflow checking
/// Takes base (first argument) and power (second argument) and returns result of base ^ power
/// Negative exponents are not supported for integer exponentiation
pub(crate) fn eval_power(args: &[MettaValue]) -> MettaValue {
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

/// Evaluate absolute value (unary)
/// Returns the absolute value of the input number
pub(crate) fn eval_abs(args: &[MettaValue]) -> MettaValue {
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
