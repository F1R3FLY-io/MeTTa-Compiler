//! Basic list operations.
//!
//! This module implements fundamental list operations:
//! - car-atom: Get the first element (head)
//! - cdr-atom: Get the rest of the list (tail)
//! - cons-atom: Construct a list by prepending an element
//! - decons-atom: Deconstruct into (head tail) pair
//! - size-atom: Get the number of elements
//! - max-atom: Get the maximum numeric value

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::eval;

/// car-atom: (car-atom expr) -> first element
/// Returns the first element of an expression (head)
/// Example: (car-atom (a b c)) -> a
#[allow(dead_code)]
pub(crate) fn eval_car_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("car-atom", items, 1, env, "(car-atom expr)");

    // EVALUATE the argument first to handle lazy evaluation
    let (expr_results, new_env) = eval(items[1].clone(), env);

    // Handle nondeterminism - car-atom on each result
    let mut all_results = vec![];
    for expr in expr_results {
        match &expr {
            MettaValue::SExpr(elements) if !elements.is_empty() => {
                all_results.push(elements[0].clone());
            }
            MettaValue::SExpr(_) => {
                let err = MettaValue::Error(
                    "car-atom expects a non-empty expression as argument".to_string(),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
            MettaValue::Nil => {
                let err = MettaValue::Error(
                    "car-atom expects a non-empty expression as argument".to_string(),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "car-atom: expected expression, got {}. Usage: (car-atom expr)",
                        super::super::friendly_value_repr(&expr)
                    ),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, new_env)
}

/// cdr-atom: (cdr-atom expr) -> rest of expression (tail)
/// Returns all elements except the first
/// Example: (cdr-atom (a b c)) -> (b c)
#[allow(dead_code)]
pub(crate) fn eval_cdr_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("cdr-atom", items, 1, env, "(cdr-atom expr)");

    // EVALUATE the argument first to handle lazy evaluation
    let (expr_results, new_env) = eval(items[1].clone(), env);

    // Handle nondeterminism - cdr-atom on each result
    let mut all_results = vec![];
    for expr in expr_results {
        match &expr {
            MettaValue::SExpr(elements) if !elements.is_empty() => {
                let tail = elements[1..].to_vec();
                all_results.push(if tail.is_empty() {
                    MettaValue::SExpr(vec![])
                } else {
                    MettaValue::SExpr(tail)
                });
            }
            MettaValue::SExpr(_) => {
                let err = MettaValue::Error(
                    "cdr-atom expects a non-empty expression as argument".to_string(),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
            MettaValue::Nil => {
                let err = MettaValue::Error(
                    "cdr-atom expects a non-empty expression as argument".to_string(),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "cdr-atom: expected expression, got {}. Usage: (cdr-atom expr)",
                        super::super::friendly_value_repr(&expr)
                    ),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, new_env)
}

/// cons-atom: (cons-atom head tail) -> (head elements...)
/// Constructs an expression by prepending head to tail
/// Example: (cons-atom a (b c)) -> (a b c)
#[allow(dead_code)]
pub(crate) fn eval_cons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("cons-atom", items, 2, env, "(cons-atom head tail)");

    // EVALUATE both arguments first to handle lazy evaluation
    let (head_results, env1) = eval(items[1].clone(), env);
    let (tail_results, new_env) = eval(items[2].clone(), env1);

    // Handle nondeterminism - cons-atom on each combination
    let mut all_results = vec![];
    for head in &head_results {
        for tail in &tail_results {
            match tail {
                MettaValue::SExpr(elements) => {
                    let mut result = vec![head.clone()];
                    result.extend(elements.iter().cloned());
                    all_results.push(MettaValue::SExpr(result));
                }
                MettaValue::Nil => {
                    all_results.push(MettaValue::SExpr(vec![head.clone()]));
                }
                _ => {
                    let err = MettaValue::Error(
                        format!(
                            "cons-atom expected Expression as tail, got {}. Usage: (cons-atom head tail)",
                            super::super::friendly_value_repr(tail)
                        ),
                        Arc::new(tail.clone()),
                    );
                    all_results.push(err);
                }
            }
        }
    }

    (all_results, new_env)
}

/// decons-atom: (decons-atom expr) -> (head tail)
/// Deconstructs an expression into (head tail) pair
/// Example: (decons-atom (a b c)) -> (a (b c))
#[allow(dead_code)]
pub(crate) fn eval_decons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("decons-atom", items, 1, env, "(decons-atom expr)");

    // EVALUATE the argument first to handle lazy evaluation
    let (expr_results, new_env) = eval(items[1].clone(), env);

    // Handle nondeterminism - decons-atom on each result
    let mut all_results = vec![];
    for expr in expr_results {
        match &expr {
            MettaValue::SExpr(elements) if !elements.is_empty() => {
                let head = elements[0].clone();
                let tail = MettaValue::SExpr(elements[1..].to_vec());
                all_results.push(MettaValue::SExpr(vec![head, tail]));
            }
            MettaValue::SExpr(_) | MettaValue::Nil | MettaValue::Unit => {
                // Empty expression/Unit - nondeterministic failure (return nothing for this result)
                // HE-compatible: silent failure, not Error
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "decons-atom: expected expression, got {}. Usage: (decons-atom expr)",
                        super::super::friendly_value_repr(&expr)
                    ),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, new_env)
}

/// size-atom: (size-atom expr) -> number
/// Returns the number of elements in an expression
/// Example: (size-atom (a b c)) -> 3
#[allow(dead_code)]
pub(crate) fn eval_size_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("size-atom", items, 1, env, "(size-atom expr)");

    // EVALUATE the argument first to handle lazy evaluation
    let (expr_results, new_env) = eval(items[1].clone(), env);

    // Handle nondeterminism - size-atom on each result
    let mut all_results = vec![];
    for expr in expr_results {
        match &expr {
            MettaValue::SExpr(elements) => {
                all_results.push(MettaValue::Long(elements.len() as i64));
            }
            MettaValue::Nil => {
                all_results.push(MettaValue::Long(0));
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "size-atom: expected expression, got {}. Usage: (size-atom expr)",
                        super::super::friendly_value_repr(&expr)
                    ),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, new_env)
}

/// max-atom: (max-atom expr) -> maximum number
/// Returns the maximum numeric value in an expression
/// Example: (max-atom (1 5 3 2)) -> 5
#[allow(dead_code)]
pub(crate) fn eval_max_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("max-atom", items, 1, env, "(max-atom expr)");

    // EVALUATE the argument first to handle lazy evaluation
    let (expr_results, new_env) = eval(items[1].clone(), env);

    // Handle nondeterminism - max-atom on each result
    let mut all_results = vec![];
    for expr in expr_results {
        match &expr {
            MettaValue::SExpr(elements) if !elements.is_empty() => {
                let mut max_val: Option<i64> = None;
                let mut error_result = None;

                for elem in elements {
                    match elem {
                        MettaValue::Long(n) => {
                            max_val = Some(max_val.map_or(*n, |m| m.max(*n)));
                        }
                        _ => {
                            error_result = Some(MettaValue::Error(
                                format!(
                                    "max-atom: found non-numeric value {}",
                                    super::super::friendly_value_repr(elem)
                                ),
                                Arc::new(elem.clone()),
                            ));
                            break;
                        }
                    }
                }

                if let Some(err) = error_result {
                    all_results.push(err);
                } else {
                    all_results.push(MettaValue::Long(max_val.unwrap()));
                }
            }
            MettaValue::SExpr(_) | MettaValue::Nil => {
                let err = MettaValue::Error(
                    "max-atom expects a non-empty expression of numbers".to_string(),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "max-atom: expected expression of numbers, got {}. Usage: (max-atom expr)",
                        super::super::friendly_value_repr(&expr)
                    ),
                    Arc::new(expr.clone()),
                );
                all_results.push(err);
            }
        }
    }

    (all_results, new_env)
}
