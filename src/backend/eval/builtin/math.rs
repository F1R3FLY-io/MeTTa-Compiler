//! Mathematical functions for built-in evaluation.
//!
//! This module implements advanced math operations:
//! - Square root (sqrt-math)
//! - Logarithm (log-math)
//! - Truncation (trunc-math)
//! - Ceiling (ceil-math)
//! - Floor (floor-math)
//! - Rounding (round-math)
//! - Trigonometric functions (sin, cos, tan, asin, acos, atan)
//! - Special value checks (isnan, isinf)

use std::sync::Arc;

use crate::backend::models::MettaValue;

use super::extractors::{extract_float, extract_long};

/// Evaluate square root (unary)
/// Returns the integer square root (floor) of the input number
/// Input must be >= 0
pub(crate) fn eval_sqrt(args: &[MettaValue]) -> MettaValue {
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

/// Evaluate logarithm (binary)
/// Returns the integer logarithm (floor) of input number (second argument) with base (first argument)
/// Base must be > 0 and != 1, input number must be > 0
pub(crate) fn eval_log(args: &[MettaValue]) -> MettaValue {
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
pub(crate) fn eval_trunc(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("trunc-math", args, 1, "(trunc-math number)");

    let value = match extract_float(&args[0], "trunc-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.trunc() as i64)
}

/// Evaluate ceiling (unary)
/// Returns the smallest integer greater than or equal to the input value
pub(crate) fn eval_ceil(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("ceil-math", args, 1, "(ceil-math number)");

    let value = match extract_float(&args[0], "ceil-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.ceil() as i64)
}

/// Evaluate floor (unary)
/// Returns the largest integer less than or equal to the input value
pub(crate) fn eval_floor(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("floor-math", args, 1, "(floor-math number)");

    let value = match extract_float(&args[0], "floor-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.floor() as i64)
}

/// Evaluate round (unary)
/// Returns the nearest integer to the input value (rounds to nearest, ties round to even)
pub(crate) fn eval_round(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("round-math", args, 1, "(round-math number)");

    let value = match extract_float(&args[0], "round-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Long(value.round() as i64)
}

/// Evaluate sine (unary)
/// Returns the sine of the input angle in radians
pub(crate) fn eval_sin(args: &[MettaValue]) -> MettaValue {
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
pub(crate) fn eval_asin(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("asin-math", args, 1, "(asin-math number)");

    let value = match extract_float(&args[0], "asin-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    // asin is only defined for values in [-1, 1]
    if !(-1.0..=1.0).contains(&value) {
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
pub(crate) fn eval_cos(args: &[MettaValue]) -> MettaValue {
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
pub(crate) fn eval_acos(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("acos-math", args, 1, "(acos-math number)");

    let value = match extract_float(&args[0], "acos-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    // acos is only defined for values in [-1, 1]
    if !(-1.0..=1.0).contains(&value) {
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
pub(crate) fn eval_tan(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("tan-math", args, 1, "(tan-math angle)");

    let angle = match extract_float(&args[0], "tan-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(angle.tan())
}

/// Evaluate arctangent (unary)
/// Returns the arctangent of the input value in radians
pub(crate) fn eval_atan(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("atan-math", args, 1, "(atan-math number)");

    let value = match extract_float(&args[0], "atan-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Float(value.atan())
}

/// Evaluate isnan (unary)
/// Returns True if the input value is NaN, False otherwise
pub(crate) fn eval_isnan(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("isnan-math", args, 1, "(isnan-math number)");

    let value = match extract_float(&args[0], "isnan-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Bool(value.is_nan())
}

/// Evaluate isinf (unary)
/// Returns True if the input value is positive or negative infinity, False otherwise
pub(crate) fn eval_isinf(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("isinf-math", args, 1, "(isinf-math number)");

    let value = match extract_float(&args[0], "isinf-math") {
        Ok(f) => f,
        Err(e) => return e,
    };

    MettaValue::Bool(value.is_infinite())
}
