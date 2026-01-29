//! Basic list operations.
//!
//! This module implements fundamental list operations:
//! - car-atom: Get the first element (head)
//! - cdr-atom: Get the rest of the list (tail)
//! - cons-atom: Construct a list by prepending an element
//! - decons-atom: Deconstruct into (head tail) pair
//! - size-atom: Get the number of elements
//! - max-atom: Get the maximum numeric value
//!
//! IMPORTANT: These operations use LAZY evaluation semantics matching MeTTa HE.
//! Arguments are NOT evaluated before processing. For example:
//! - (cons-atom (+ 1 2) (a b)) returns ((+ 1 2) a b), NOT (3 a b)
//! - This prevents infinite loops in recursive MeTTa programs

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

/// car-atom: (car-atom expr) -> first element
/// Returns the first element of an expression (head)
/// Example: (car-atom (a b c)) -> a
///
/// NOTE: This is a lazy operation - the argument is NOT evaluated first.
pub(crate) fn eval_car_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("car-atom", items, 1, env, "(car-atom expr)");

    let expr = &items[1];

    match expr.inner() {
        MettaValueInner::SExpr(elements) if !elements.is_empty() => {
            (vec![elements[0].clone()], env)
        }
        MettaValueInner::SExpr(_) | MettaValueInner::Nil => {
            let err = MettaValue::Error(
                "car-atom expects a non-empty expression as argument".to_string(),
                expr.clone(),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "car-atom: expected expression, got {}. Usage: (car-atom expr)",
                    super::super::friendly_value_repr(expr)
                ),
                expr.clone(),
            );
            (vec![err], env)
        }
    }
}

/// cdr-atom: (cdr-atom expr) -> rest of expression (tail)
/// Returns all elements except the first
/// Example: (cdr-atom (a b c)) -> (b c)
///
/// NOTE: This is a lazy operation - the argument is NOT evaluated first.
pub(crate) fn eval_cdr_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("cdr-atom", items, 1, env, "(cdr-atom expr)");

    let expr = &items[1];

    match expr.inner() {
        MettaValueInner::SExpr(elements) if !elements.is_empty() => {
            let tail = elements[1..].to_vec();
            (
                vec![if tail.is_empty() {
                    MettaValue::SExpr(vec![])
                } else {
                    MettaValue::SExpr(tail)
                }],
                env,
            )
        }
        MettaValueInner::SExpr(_) | MettaValueInner::Nil => {
            let err = MettaValue::Error(
                "cdr-atom expects a non-empty expression as argument".to_string(),
                expr.clone(),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "cdr-atom: expected expression, got {}. Usage: (cdr-atom expr)",
                    super::super::friendly_value_repr(expr)
                ),
                expr.clone(),
            );
            (vec![err], env)
        }
    }
}

/// cons-atom: (cons-atom head tail) -> (head elements...)
/// Constructs an expression by prepending head to tail
/// Example: (cons-atom a (b c)) -> (a b c)
///
/// NOTE: This is a lazy operation - arguments are NOT evaluated first.
/// (cons-atom (+ 1 2) (a b)) returns ((+ 1 2) a b), NOT (3 a b)
pub(crate) fn eval_cons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("cons-atom", items, 2, env, "(cons-atom head tail)");

    let head = &items[1];
    let tail = &items[2];

    match tail.inner() {
        MettaValueInner::SExpr(elements) => {
            let mut result = vec![head.clone()];
            result.extend(elements.iter().cloned());
            (vec![MettaValue::SExpr(result)], env)
        }
        MettaValueInner::Nil => (vec![MettaValue::SExpr(vec![head.clone()])], env),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "cons-atom expected Expression as tail, got {}. Usage: (cons-atom head tail)",
                    super::super::friendly_value_repr(tail)
                ),
                tail.clone(),
            );
            (vec![err], env)
        }
    }
}

/// decons-atom: (decons-atom expr) -> (head tail)
/// Deconstructs an expression into (head tail) pair
/// Example: (decons-atom (a b c)) -> (a (b c))
///
/// NOTE: This is a lazy operation - the argument is NOT evaluated first.
pub(crate) fn eval_decons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("decons-atom", items, 1, env, "(decons-atom expr)");

    let expr = &items[1];

    match expr.inner() {
        MettaValueInner::SExpr(elements) if !elements.is_empty() => {
            let head = elements[0].clone();
            let tail = MettaValue::SExpr(elements[1..].to_vec());
            (vec![MettaValue::SExpr(vec![head, tail])], env)
        }
        MettaValueInner::SExpr(_) | MettaValueInner::Nil | MettaValueInner::Unit => {
            // Empty expression/Unit - nondeterministic failure (return nothing)
            // HE-compatible: silent failure, not Error
            (vec![], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "decons-atom: expected expression, got {}. Usage: (decons-atom expr)",
                    super::super::friendly_value_repr(expr)
                ),
                expr.clone(),
            );
            (vec![err], env)
        }
    }
}

/// size-atom: (size-atom expr) -> number
/// Returns the number of elements in an expression
/// Example: (size-atom (a b c)) -> 3
///
/// NOTE: This is a lazy operation - the argument is NOT evaluated first.
pub(crate) fn eval_size_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("size-atom", items, 1, env, "(size-atom expr)");

    let expr = &items[1];

    match expr.inner() {
        MettaValueInner::SExpr(elements) => (vec![MettaValue::Long(elements.len() as i64)], env),
        MettaValueInner::Nil => (vec![MettaValue::Long(0)], env),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "size-atom: expected expression, got {}. Usage: (size-atom expr)",
                    super::super::friendly_value_repr(expr)
                ),
                expr.clone(),
            );
            (vec![err], env)
        }
    }
}

/// max-atom: (max-atom expr) -> maximum number
/// Returns the maximum numeric value in an expression
/// Example: (max-atom (1 5 3 2)) -> 5
///
/// NOTE: This is a lazy operation - the argument is NOT evaluated first.
pub(crate) fn eval_max_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("max-atom", items, 1, env, "(max-atom expr)");

    let expr = &items[1];

    match expr.inner() {
        MettaValueInner::SExpr(elements) if !elements.is_empty() => {
            let mut max_val: Option<i64> = None;
            let mut error_result = None;

            for elem in elements {
                match elem.inner() {
                    MettaValueInner::Long(n) => {
                        max_val = Some(max_val.map_or(*n, |m| m.max(*n)));
                    }
                    _ => {
                        error_result = Some(MettaValue::Error(
                            format!(
                                "max-atom: found non-numeric value {}",
                                super::super::friendly_value_repr(elem)
                            ),
                            elem.clone(),
                        ));
                        break;
                    }
                }
            }

            if let Some(err) = error_result {
                (vec![err], env)
            } else {
                (
                    vec![MettaValue::Long(max_val.expect("non-empty expression"))],
                    env,
                )
            }
        }
        MettaValueInner::SExpr(_) | MettaValueInner::Nil => {
            let err = MettaValue::Error(
                "max-atom expects a non-empty expression of numbers".to_string(),
                expr.clone(),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "max-atom: expected expression of numbers, got {}. Usage: (max-atom expr)",
                    super::super::friendly_value_repr(expr)
                ),
                expr.clone(),
            );
            (vec![err], env)
        }
    }
}
