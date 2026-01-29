//! Non-TCO comparison operations.
//!
//! Provides the standard comparison operations:
//! - `LessOp`, `LessEqOp` - Less than / less than or equal
//! - `GreaterOp`, `GreaterEqOp` - Greater than / greater than or equal
//! - `EqualOp`, `NotEqualOp` - Equality / inequality

use super::{
    friendly_type_name, Environment, EvalFn, ExecError, GroundedOperation, GroundedResult,
    MettaValue,
};
use crate::backend::models::MettaValueInner;

/// Less than operation: (< a b)
pub struct LessOp;

impl GroundedOperation for LessOp {
    fn name(&self) -> &str {
        "<"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::Less)
    }
}

/// Less than or equal operation: (<= a b)
pub struct LessEqOp;

impl GroundedOperation for LessEqOp {
    fn name(&self) -> &str {
        "<="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::LessEq)
    }
}

/// Greater than operation: (> a b)
pub struct GreaterOp;

impl GroundedOperation for GreaterOp {
    fn name(&self) -> &str {
        ">"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::Greater)
    }
}

/// Greater than or equal operation: (>= a b)
pub struct GreaterEqOp;

impl GroundedOperation for GreaterEqOp {
    fn name(&self) -> &str {
        ">="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::GreaterEq)
    }
}

/// Equality operation: (== a b)
pub struct EqualOp;

impl GroundedOperation for EqualOp {
    fn name(&self) -> &str {
        "=="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_equality(args, env, eval_fn, true)
    }
}

/// Inequality operation: (!= a b)
pub struct NotEqualOp;

impl GroundedOperation for NotEqualOp {
    fn name(&self) -> &str {
        "!="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_equality(args, env, eval_fn, false)
    }
}

/// Comparison kind for ordering operations
pub(crate) enum CompareKind {
    Less,
    LessEq,
    Greater,
    GreaterEq,
}

impl CompareKind {
    #[inline]
    pub(crate) fn compare<T: PartialOrd>(&self, a: &T, b: &T) -> bool {
        match self {
            CompareKind::Less => a < b,
            CompareKind::LessEq => a <= b,
            CompareKind::Greater => a > b,
            CompareKind::GreaterEq => a >= b,
        }
    }
}

/// Helper function for comparison operations (supports numbers and strings)
fn eval_comparison(
    args: &[MettaValue],
    env: &Environment,
    eval_fn: &EvalFn,
    kind: CompareKind,
) -> GroundedResult {
    if args.len() != 2 {
        return Err(ExecError::IncorrectArgument(format!(
            "Comparison requires 2 arguments, got {}",
            args.len()
        )));
    }

    let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
    let (b_results, _) = eval_fn(args[1].clone(), env1);

    let mut results = Vec::new();
    for a in &a_results {
        for b in &b_results {
            match (a.inner(), b.inner()) {
                (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                (MettaValueInner::Float(x), MettaValueInner::Float(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                (MettaValueInner::Long(x), MettaValueInner::Float(y)) => {
                    results.push((MettaValue::Bool(kind.compare(&(*x as f64), y)), None));
                }
                (MettaValueInner::Float(x), MettaValueInner::Long(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, &(*y as f64))), None));
                }
                // String comparison (lexicographic)
                (MettaValueInner::String(x), MettaValueInner::String(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot compare: type mismatch between {} and {}",
                        friendly_type_name(a),
                        friendly_type_name(b)
                    )))
                }
            }
        }
    }
    Ok(results)
}

/// Helper function for equality/inequality operations
/// Supports comparing all value types, not just numeric
fn eval_equality(
    args: &[MettaValue],
    env: &Environment,
    eval_fn: &EvalFn,
    is_equal: bool,
) -> GroundedResult {
    if args.len() != 2 {
        return Err(ExecError::IncorrectArgument(format!(
            "Equality comparison requires 2 arguments, got {}",
            args.len()
        )));
    }

    let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
    let (b_results, _) = eval_fn(args[1].clone(), env1);

    let mut results = Vec::new();
    for a in &a_results {
        for b in &b_results {
            let equal = values_equal(a, b);
            let result = if is_equal { equal } else { !equal };
            results.push((MettaValue::Bool(result), None));
        }
    }
    Ok(results)
}

/// Check if two MettaValues are equal
pub(crate) fn values_equal(a: &MettaValue, b: &MettaValue) -> bool {
    match (a.inner(), b.inner()) {
        (MettaValueInner::Long(x), MettaValueInner::Long(y)) => x == y,
        (MettaValueInner::Float(x), MettaValueInner::Float(y)) => (x - y).abs() < f64::EPSILON,
        (MettaValueInner::Bool(x), MettaValueInner::Bool(y)) => x == y,
        (MettaValueInner::String(x), MettaValueInner::String(y)) => x == y,
        (MettaValueInner::Atom(x), MettaValueInner::Atom(y)) => x == y,
        (MettaValueInner::Nil, MettaValueInner::Nil) => true,
        (MettaValueInner::Unit, MettaValueInner::Unit) => true,
        // HE compatibility: Nil equals empty SExpr
        (MettaValueInner::Nil, MettaValueInner::SExpr(items))
        | (MettaValueInner::SExpr(items), MettaValueInner::Nil) => items.is_empty(),
        // HE compatibility: Nil equals Unit
        (MettaValueInner::Nil, MettaValueInner::Unit)
        | (MettaValueInner::Unit, MettaValueInner::Nil) => true,
        (MettaValueInner::SExpr(x), MettaValueInner::SExpr(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        // Different types are not equal
        _ => false,
    }
}
