//! Higher-order list operations.
//!
//! This module implements functional-style higher-order operations:
//! - map-atom: Transform each element with a function
//! - filter-atom: Keep elements that satisfy a predicate
//! - foldl-atom: Reduce a list to a single value from left to right
//!
//! These operations use the trampoline for iteration to prevent stack overflow
//! when processing deeply nested operations (e.g., map inside map).

use std::sync::Arc;
use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::step::EvalStep;
use super::helpers::{suggest_variable_format};

/// Map atom: (map-atom $list $var $template)
/// Maps a function over a list of atoms
/// Example: (map-atom (1 2 3 4) $v (+ $v 1)) -> (2 3 4 5)
///
/// Returns an EvalStep to defer iteration to the trampoline, preventing
/// stack overflow for nested map operations.
pub(crate) fn eval_map_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    trace!(target: "mettatron::eval::eval_map_atom", ?items);

    // Validate argument count
    if items.len() != 4 {
        let err = MettaValue::Error(
            format!(
                "map-atom requires exactly 3 arguments, got {}. Usage: (map-atom list $var expr)",
                items.len() - 1
            ),
            Arc::new(MettaValue::SExpr(items)),
        );
        return EvalStep::Done((vec![err], env));
    }

    let list = &items[1];
    let var = &items[2];
    let template = &items[3];

    // Validate variable argument
    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "map-atom: second argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "map-atom: second argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Arc::new(var.clone()));
            return EvalStep::Done((vec![err], env));
        }
        _ => {
            let err = MettaValue::Error(
                "map-atom: second argument must be a variable (starting with $)".to_string(),
                Arc::new(var.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Validate and extract list elements
    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "map-atom: first argument must be a list, got {}. Usage: (map-atom list $var expr)",
                    super::super::friendly_value_repr(list)
                ),
                Arc::new(list.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Return EvalStep to defer iteration to trampoline
    EvalStep::StartMapAtom {
        elements,
        var_name,
        template: template.clone(),
        env,
        depth,
    }
}

/// Filter atom: (filter-atom $list $var $predicate)
/// Filters a list keeping only elements that satisfy the predicate
/// Example: (filter-atom (1 2 3 4) $v (> $v 2)) -> (3 4)
///
/// Returns an EvalStep to defer iteration to the trampoline, preventing
/// stack overflow for nested filter operations.
pub(crate) fn eval_filter_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    trace!(target: "mettatron::eval::eval_filter_atom", ?items);

    // Validate argument count
    if items.len() != 4 {
        let err = MettaValue::Error(
            format!(
                "filter-atom requires exactly 3 arguments, got {}. Usage: (filter-atom list $var predicate)",
                items.len() - 1
            ),
            Arc::new(MettaValue::SExpr(items)),
        );
        return EvalStep::Done((vec![err], env));
    }

    let list = &items[1];
    let var = &items[2];
    let predicate = &items[3];

    // Validate variable argument
    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "filter-atom: second argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "filter-atom: second argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Arc::new(var.clone()));
            return EvalStep::Done((vec![err], env));
        }
        _ => {
            let err = MettaValue::Error(
                "filter-atom: second argument must be a variable (starting with $)".to_string(),
                Arc::new(var.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Validate and extract list elements
    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "filter-atom: first argument must be a list, got {}. Usage: (filter-atom list $var predicate)",
                    super::super::friendly_value_repr(list)
                ),
                Arc::new(list.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Return EvalStep to defer iteration to trampoline
    EvalStep::StartFilterAtom {
        elements,
        var_name,
        predicate: predicate.clone(),
        env,
        depth,
    }
}

/// Fold left atom: (foldl-atom $list $init $acc $item $op)
/// Folds (reduces) a list from left to right using an operation and initial value
/// Example: (foldl-atom (1 2 3) 0 $acc $x (+ $acc $x)) -> 6
///
/// Returns an EvalStep to defer iteration to the trampoline, preventing
/// stack overflow for nested fold operations.
pub(crate) fn eval_foldl_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    trace!(target: "mettatron::eval::eval_foldl_atom", ?items);

    // Validate argument count
    if items.len() != 6 {
        let err = MettaValue::Error(
            "foldl-atom requires exactly 5 arguments: list, init, acc-var, item-var, operation"
                .to_string(),
            Arc::new(MettaValue::SExpr(items)),
        );
        return EvalStep::Done((vec![err], env));
    }

    let list = &items[1];
    let init = &items[2];
    let acc_var = &items[3];
    let item_var = &items[4];
    let operation = &items[5];

    // Validate accumulator variable
    let acc_var_name = match acc_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "foldl-atom: third argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "foldl-atom: third argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Arc::new(acc_var.clone()));
            return EvalStep::Done((vec![err], env));
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: third argument must be a variable (starting with $)".to_string(),
                Arc::new(acc_var.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Validate item variable
    let item_var_name = match item_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            let suggestion = suggest_variable_format(name);
            let msg = match suggestion {
                Some(s) => format!(
                    "foldl-atom: fourth argument must be a variable (starting with $). {}",
                    s
                ),
                None => {
                    "foldl-atom: fourth argument must be a variable (starting with $)".to_string()
                }
            };
            let err = MettaValue::Error(msg, Arc::new(item_var.clone()));
            return EvalStep::Done((vec![err], env));
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: fourth argument must be a variable (starting with $)".to_string(),
                Arc::new(item_var.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Validate and extract list elements
    let elements = match list {
        MettaValue::SExpr(items) => items.clone(),
        MettaValue::Nil => vec![],
        _ => {
            let err = MettaValue::Error(
                format!(
                    "foldl-atom: first argument must be a list, got {}. Usage: (foldl-atom list init $acc $elem expr)",
                    super::super::friendly_value_repr(list)
                ),
                Arc::new(list.clone()),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Return EvalStep to defer iteration to trampoline
    EvalStep::StartFoldlAtom {
        elements,
        init: init.clone(),
        acc_var_name,
        item_var_name,
        operation: operation.clone(),
        env,
        depth,
    }
}
