//! Logical operations for built-in evaluation.
//!
//! This module implements logical operations:
//! - And (and)
//! - Or (or)
//! - Not (not)

use crate::backend::models::MettaValue;

use super::extractors::extract_bool;

/// Evaluate a binary logical operation (and, or)
pub(crate) fn eval_logical_binary<F>(args: &[MettaValue], op: F, op_name: &str) -> MettaValue
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
pub(crate) fn eval_logical_not(args: &[MettaValue]) -> MettaValue {
    require_builtin_args!("'not'", args, 1, "(not bool)");

    match extract_bool(&args[0], "'not'") {
        Ok(b) => MettaValue::Bool(!b),
        Err(e) => e,
    }
}
