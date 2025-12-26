//! Comparison operations for built-in evaluation.
//!
//! This module implements comparison operations:
//! - Less than (<)
//! - Less than or equal (<=)
//! - Greater than (>)
//! - Greater than or equal (>=)
//! - Equal (==)
//! - Not equal (!=)
//!
//! Supports both numeric (integer/float) and string (lexicographic) comparisons.

use std::sync::Arc;

use crate::backend::models::MettaValue;

/// Comparison kind for polymorphic comparison
enum CompareKind {
    Less,
    LessEq,
    Greater,
    GreaterEq,
    Equal,
    NotEqual,
}

impl CompareKind {
    fn compare<T: PartialOrd>(&self, a: &T, b: &T) -> bool {
        match self {
            CompareKind::Less => a < b,
            CompareKind::LessEq => a <= b,
            CompareKind::Greater => a > b,
            CompareKind::GreaterEq => a >= b,
            CompareKind::Equal => a == b,
            CompareKind::NotEqual => a != b,
        }
    }
}

/// Evaluate a comparison operation with polymorphic type checking
/// Supports integers, floats, and strings
fn eval_comparison_impl(args: &[MettaValue], kind: CompareKind) -> MettaValue {
    require_builtin_args!("Comparison operation", args, 2);

    let a = &args[0];
    let b = &args[1];

    match (a, b) {
        // Integer comparisons
        (MettaValue::Long(x), MettaValue::Long(y)) => MettaValue::Bool(kind.compare(x, y)),

        // Float comparisons
        (MettaValue::Float(x), MettaValue::Float(y)) => MettaValue::Bool(kind.compare(x, y)),

        // Mixed integer/float comparisons
        (MettaValue::Long(x), MettaValue::Float(y)) => {
            MettaValue::Bool(kind.compare(&(*x as f64), y))
        }
        (MettaValue::Float(x), MettaValue::Long(y)) => {
            MettaValue::Bool(kind.compare(x, &(*y as f64)))
        }

        // String comparisons (lexicographic)
        (MettaValue::String(x), MettaValue::String(y)) => MettaValue::Bool(kind.compare(x, y)),

        // Type mismatch
        _ => MettaValue::Error(
            format!(
                "Cannot compare: type mismatch between {} and {}",
                a.friendly_type_name(),
                b.friendly_type_name()
            ),
            Arc::new(MettaValue::Atom("TypeError".to_string())),
        ),
    }
}

/// Evaluate a comparison operation with a closure
/// This is the legacy API that only supports i64 comparisons
/// Use the specialized functions below for full type support
pub(crate) fn eval_comparison<F>(args: &[MettaValue], op: F) -> MettaValue
where
    F: Fn(i64, i64) -> bool,
{
    // Detect which kind of comparison this is based on the closure behavior
    // Note: This is a compatibility shim - prefer using eval_less, eval_greater, etc.
    let kind = if op(1, 2) && !op(2, 1) {
        CompareKind::Less
    } else if op(2, 1) && !op(1, 2) {
        CompareKind::Greater
    } else if op(1, 1) && op(1, 2) {
        CompareKind::LessEq
    } else if op(1, 1) && op(2, 1) {
        CompareKind::GreaterEq
    } else if op(1, 1) && !op(1, 2) {
        CompareKind::Equal
    } else {
        CompareKind::NotEqual
    };

    eval_comparison_impl(args, kind)
}
