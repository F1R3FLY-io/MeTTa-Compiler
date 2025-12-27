//! Comparison operations for built-in evaluation.
//!
//! This module implements comparison operations:
//! - Less than (<)
//! - Less than or equal (<=)
//! - Greater than (>)
//! - Greater than or equal (>=)
//! - Equal (==)
//! - Not equal (!=)

use crate::backend::models::MettaValue;

use super::extractors::extract_long;

/// Evaluate a comparison operation with strict type checking
pub(crate) fn eval_comparison<F>(args: &[MettaValue], op: F) -> MettaValue
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
