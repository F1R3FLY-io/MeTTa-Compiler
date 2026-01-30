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
//! IMPORTANT: These operations use HYBRID evaluation semantics:
//! - Arguments that are GROUNDED operations (map-atom, filter-atom, if, let, etc.)
//!   are evaluated BEFORE processing to get their values
//! - User-defined expressions are kept unevaluated (lazy evaluation)
//!
//! For example:
//! - (car-atom (map-atom (a b c) $x $x)) evaluates map-atom first, returns a
//! - (cons-atom (+ 1 2) (a b)) keeps (+ 1 2) unevaluated, returns ((+ 1 2) a b)
//!   unless + is in GROUNDED_OPS (which it is), so returns (3 a b)

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

use super::super::is_grounded_op;
use super::super::step::EvalStep;

/// Check if an S-expression argument needs evaluation before being used.
/// Returns true if the argument is a grounded operation that produces a value.
fn needs_evaluation(arg: &MettaValue, env: &Environment) -> bool {
    if let MettaValueInner::SExpr(items) = arg.inner() {
        if let Some(first) = items.first() {
            if let MettaValueInner::Atom(op) = first.inner() {
                // Check if this is a grounded operation or TCO operation
                return is_grounded_op(op) || env.get_grounded_operation_tco(op).is_some();
            }
        }
    }
    false
}

/// Step version of car-atom that handles argument evaluation.
/// If the argument is a grounded operation (map-atom, if, let, etc.),
/// it's evaluated first via the trampoline.
pub(crate) fn eval_car_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "car-atom requires exactly 1 argument, got {}. Usage: (car-atom expr)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let arg = &items[1];

    // Check if argument needs evaluation (is a grounded op)
    if needs_evaluation(arg, &env) {
        // Defer argument evaluation to the trampoline
        return EvalStep::EvalListOpArg {
            op_name: "car-atom".to_string(),
            items,
            arg_index: 1,
            env,
            depth,
        };
    }

    // Argument doesn't need evaluation, process directly
    EvalStep::Done(eval_car_atom(items, env))
}

/// car-atom: (car-atom expr) -> first element
/// Returns the first element of an expression (head)
/// Example: (car-atom (a b c)) -> a
///
/// NOTE: For lazy evaluation semantics. Use eval_car_atom_step for hybrid evaluation.
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

/// Step version of cdr-atom that handles argument evaluation.
pub(crate) fn eval_cdr_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "cdr-atom requires exactly 1 argument, got {}. Usage: (cdr-atom expr)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let arg = &items[1];
    if needs_evaluation(arg, &env) {
        return EvalStep::EvalListOpArg {
            op_name: "cdr-atom".to_string(),
            items,
            arg_index: 1,
            env,
            depth,
        };
    }
    EvalStep::Done(eval_cdr_atom(items, env))
}

/// cdr-atom: (cdr-atom expr) -> rest of expression (tail)
/// Returns all elements except the first
/// Example: (cdr-atom (a b c)) -> (b c)
///
/// NOTE: For lazy evaluation semantics. Use eval_cdr_atom_step for hybrid evaluation.
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

/// Step version of cons-atom that handles argument evaluation.
/// For cons-atom, we evaluate the TAIL argument if it's a grounded op,
/// since we need to know if it's a proper expression to prepend to.
pub(crate) fn eval_cons_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 3 {
        let err = MettaValue::Error(
            format!(
                "cons-atom requires exactly 2 arguments, got {}. Usage: (cons-atom head tail)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    // Check if tail argument needs evaluation
    let tail = &items[2];
    if needs_evaluation(tail, &env) {
        return EvalStep::EvalListOpArg {
            op_name: "cons-atom".to_string(),
            items,
            arg_index: 2, // Evaluate the tail
            env,
            depth,
        };
    }
    EvalStep::Done(eval_cons_atom(items, env))
}

/// cons-atom: (cons-atom head tail) -> (head elements...)
/// Constructs an expression by prepending head to tail
/// Example: (cons-atom a (b c)) -> (a b c)
///
/// NOTE: For lazy evaluation semantics. Use eval_cons_atom_step for hybrid evaluation.
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

/// Step version of decons-atom that handles argument evaluation.
pub(crate) fn eval_decons_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "decons-atom requires exactly 1 argument, got {}. Usage: (decons-atom expr)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let arg = &items[1];
    if needs_evaluation(arg, &env) {
        return EvalStep::EvalListOpArg {
            op_name: "decons-atom".to_string(),
            items,
            arg_index: 1,
            env,
            depth,
        };
    }
    EvalStep::Done(eval_decons_atom(items, env))
}

/// decons-atom: (decons-atom expr) -> (head tail)
/// Deconstructs an expression into (head tail) pair
/// Example: (decons-atom (a b c)) -> (a (b c))
///
/// NOTE: For lazy evaluation semantics. Use eval_decons_atom_step for hybrid evaluation.
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

/// Step version of size-atom that handles argument evaluation.
pub(crate) fn eval_size_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "size-atom requires exactly 1 argument, got {}. Usage: (size-atom expr)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let arg = &items[1];
    if needs_evaluation(arg, &env) {
        return EvalStep::EvalListOpArg {
            op_name: "size-atom".to_string(),
            items,
            arg_index: 1,
            env,
            depth,
        };
    }
    EvalStep::Done(eval_size_atom(items, env))
}

/// size-atom: (size-atom expr) -> number
/// Returns the number of elements in an expression
/// Example: (size-atom (a b c)) -> 3
///
/// NOTE: For lazy evaluation semantics. Use eval_size_atom_step for hybrid evaluation.
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

/// Step version of max-atom that handles argument evaluation.
pub(crate) fn eval_max_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "max-atom requires exactly 1 argument, got {}. Usage: (max-atom expr)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let arg = &items[1];
    if needs_evaluation(arg, &env) {
        return EvalStep::EvalListOpArg {
            op_name: "max-atom".to_string(),
            items,
            arg_index: 1,
            env,
            depth,
        };
    }
    EvalStep::Done(eval_max_atom(items, env))
}

/// max-atom: (max-atom expr) -> maximum number
/// Returns the maximum numeric value in an expression
/// Example: (max-atom (1 5 3 2)) -> 5
///
/// NOTE: For lazy evaluation semantics. Use eval_max_atom_step for hybrid evaluation.
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
