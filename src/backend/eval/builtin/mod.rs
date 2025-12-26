//! Built-in operations for MeTTa evaluation.
//!
//! This module provides built-in operations that are evaluated directly
//! without going through the Rholang interpreter. Operations include:
//!
//! - Arithmetic: +, -, *, /, %, floor-div, pow-math, abs-math
//! - Comparison: <, <=, >, >=, ==, !=
//! - Logical: and, or, not
//! - Math: sqrt-math, log-math, trunc-math, ceil-math, floor-math, round-math
//! - Trigonometry: sin-math, asin-math, cos-math, acos-math, tan-math, atan-math
//! - Special: isnan-math, isinf-math

mod arithmetic;
mod comparison;
mod extractors;
mod logical;
mod math;

#[cfg(test)]
mod tests;

use crate::backend::models::MettaValue;

// Re-export for internal use by eval module
pub(crate) use arithmetic::{
    eval_abs, eval_checked_arithmetic, eval_division, eval_floor_div, eval_modulo, eval_power,
};
pub(crate) use comparison::eval_comparison;
pub(crate) use logical::{eval_logical_binary, eval_logical_not};
pub(crate) use math::{
    eval_acos, eval_asin, eval_atan, eval_ceil, eval_cos, eval_floor, eval_isinf, eval_isnan,
    eval_log, eval_round, eval_sin, eval_sqrt, eval_tan, eval_trunc,
};

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
        "floor-div" => Some(eval_floor_div(args)),
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
        "isnan-math" => Some(eval_isnan(args)),
        "isinf-math" => Some(eval_isinf(args)),
        _ => None,
    }
}
