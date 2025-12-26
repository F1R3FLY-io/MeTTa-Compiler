//! Higher-order list operations.
//!
//! This module implements functional-style higher-order operations:
//! - map-atom: Transform each element with a function
//! - filter-atom: Keep elements that satisfy a predicate
//! - foldl-atom: Reduce a list to a single value from left to right

use std::sync::Arc;
use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::eval;
use super::helpers::{substitute_variable, suggest_variable_format};

/// Map atom: (map-atom $list $var $template)
/// Maps a function over a list of atoms
/// Example: (map-atom (1 2 3 4) $v (+ $v 1)) -> (2 3 4 5)
pub(crate) fn eval_map_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_map_atom", ?items);
    require_args_with_usage!("map-atom", items, 3, env, "(map-atom list $var expr)");

    let list = &items[1];
    let var = &items[2];
    let template = &items[3];

    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
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
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "map-atom: second argument must be a variable (starting with $)".to_string(),
                Arc::new(var.clone()),
            );
            return (vec![err], env);
        }
    };

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
            return (vec![err], env);
        }
    };

    let mut mapped_elements = Vec::new();
    let mut final_env = env;

    for element in elements {
        let instantiated_template = substitute_variable(template, &var_name, &element);
        let (results, new_env) = eval(instantiated_template, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }
            mapped_elements.push(first_result.clone());
        } else {
            mapped_elements.push(MettaValue::Nil);
        }
    }

    let result = if mapped_elements.is_empty() {
        MettaValue::Nil
    } else {
        MettaValue::SExpr(mapped_elements)
    };

    (vec![result], final_env)
}

/// Filter atom: (filter-atom $list $var $predicate)
/// Filters a list keeping only elements that satisfy the predicate
/// Example: (filter-atom (1 2 3 4) $v (> $v 2)) -> (3 4)
pub(crate) fn eval_filter_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_filter_atom", ?items);
    require_args_with_usage!(
        "filter-atom",
        items,
        3,
        env,
        "(filter-atom list $var predicate)"
    );

    let list = &items[1];
    let var = &items[2];
    let predicate = &items[3];

    let var_name = match var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
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
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "filter-atom: second argument must be a variable (starting with $)".to_string(),
                Arc::new(var.clone()),
            );
            return (vec![err], env);
        }
    };

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
            return (vec![err], env);
        }
    };

    let mut filtered_elements = Vec::new();
    let mut final_env = env;

    for element in elements {
        let instantiated_predicate = substitute_variable(predicate, &var_name, &element);

        let (results, new_env) = eval(instantiated_predicate, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }

            let should_include = match first_result {
                MettaValue::Bool(true) => true,
                MettaValue::Bool(false) => false,
                _ => !matches!(first_result, MettaValue::Nil),
            };

            if should_include {
                filtered_elements.push(element);
            }
        }
    }

    let result = if filtered_elements.is_empty() {
        MettaValue::Nil
    } else {
        MettaValue::SExpr(filtered_elements)
    };

    (vec![result], final_env)
}

/// Fold left atom: (foldl-atom $list $init $acc $item $op)
/// Folds (reduces) a list from left to right using an operation and initial value
/// Example: (foldl-atom (1 2 3) 0 $acc $x (+ $acc $x)) -> 6
pub(crate) fn eval_foldl_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_foldl_atom", ?items);
    if items.len() != 6 {
        let err = MettaValue::Error(
            "foldl-atom requires exactly 5 arguments: list, init, acc-var, item-var, operation"
                .to_string(),
            Arc::new(MettaValue::SExpr(items.to_vec())),
        );
        return (vec![err], env);
    }

    let list = &items[1];
    let init = &items[2];
    let acc_var = &items[3];
    let item_var = &items[4];
    let operation = &items[5];

    let acc_var_name = match acc_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
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
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: third argument must be a variable (starting with $)".to_string(),
                Arc::new(acc_var.clone()),
            );
            return (vec![err], env);
        }
    };

    let item_var_name = match item_var {
        MettaValue::Atom(name) if name.starts_with('$') => name.clone(),
        MettaValue::Atom(name) => {
            // Try to suggest variable format
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
            return (vec![err], env);
        }
        _ => {
            let err = MettaValue::Error(
                "foldl-atom: fourth argument must be a variable (starting with $)".to_string(),
                Arc::new(item_var.clone()),
            );
            return (vec![err], env);
        }
    };

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
            return (vec![err], env);
        }
    };

    let mut accumulator = init.clone();
    let mut final_env = env;

    for element in elements {
        let mut instantiated_op = substitute_variable(operation, &acc_var_name, &accumulator);
        instantiated_op = substitute_variable(&instantiated_op, &item_var_name, &element);

        let (results, new_env) = eval(instantiated_op, final_env);
        final_env = new_env;

        if let Some(first_result) = results.first() {
            if matches!(first_result, MettaValue::Error(_, _)) {
                return (vec![first_result.clone()], final_env);
            }
            accumulator = first_result.clone();
        }
    }

    (vec![accumulator], final_env)
}
