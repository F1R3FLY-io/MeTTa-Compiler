//! State operations.
//!
//! This module handles mutable state cells:
//! - new-state: Create a new mutable state cell
//! - get-state: Get the current value from a state cell
//! - change-state!: Change the value in a state cell

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::eval;

/// new-state: Create a new mutable state cell with an initial value
/// Usage: (new-state initial-value)
pub(crate) fn eval_new_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("new-state", items, 1, env, "(new-state initial-value)");

    let initial_value = &items[1];

    // Evaluate the initial value
    let (value_results, mut env1) = eval(initial_value.clone(), env);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "new-state: initial value evaluated to empty".to_string(),
            Arc::new(initial_value.clone()),
        );
        return (vec![err], env1);
    }

    let value = value_results[0].clone();
    let state_id = env1.create_state(value);
    (vec![MettaValue::State(state_id)], env1)
}

/// get-state: Get the current value from a state cell
/// Usage: (get-state state-ref)
pub(crate) fn eval_get_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-state", items, 1, env, "(get-state state)");

    let state_ref = &items[1];

    // Evaluate the state reference
    let (state_results, env1) = eval(state_ref.clone(), env);
    if state_results.is_empty() {
        let err = MettaValue::Error(
            "get-state: state evaluated to empty".to_string(),
            Arc::new(state_ref.clone()),
        );
        return (vec![err], env1);
    }

    let state_value = &state_results[0];

    match state_value {
        MettaValue::State(state_id) => {
            if let Some(value) = env1.get_state(*state_id) {
                (vec![value], env1)
            } else {
                let err = MettaValue::Error(
                    format!("get-state: state {} not found", state_id),
                    Arc::new(state_value.clone()),
                );
                (vec![err], env1)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "get-state: argument must be a state reference, got {}. Usage: (get-state state)",
                    super::super::friendly_value_repr(state_value)
                ),
                Arc::new(state_value.clone()),
            );
            (vec![err], env1)
        }
    }
}

/// change-state!: Change the value in a state cell
/// Usage: (change-state! state-ref new-value)
/// Returns the state reference for chaining
pub(crate) fn eval_change_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!(
        "change-state!",
        items,
        2,
        env,
        "(change-state! state new-value)"
    );

    let state_ref = &items[1];
    let new_value = &items[2];

    // Evaluate the state reference
    let (state_results, env1) = eval(state_ref.clone(), env);
    if state_results.is_empty() {
        let err = MettaValue::Error(
            "change-state!: state evaluated to empty".to_string(),
            Arc::new(state_ref.clone()),
        );
        return (vec![err], env1);
    }

    // Evaluate the new value
    let (value_results, mut env2) = eval(new_value.clone(), env1);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "change-state!: new value evaluated to empty".to_string(),
            Arc::new(new_value.clone()),
        );
        return (vec![err], env2);
    }

    let state_value = &state_results[0];
    let value = value_results[0].clone();

    match state_value {
        MettaValue::State(state_id) => {
            if env2.change_state(*state_id, value) {
                // Return the state reference for chaining
                (vec![state_value.clone()], env2)
            } else {
                let err = MettaValue::Error(
                    format!("change-state!: state {} not found", state_id),
                    Arc::new(state_value.clone()),
                );
                (vec![err], env2)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "change-state!: first argument must be a state reference, got {}. Usage: (change-state! state new-value)",
                    super::super::friendly_value_repr(state_value)
                ),
                Arc::new(state_value.clone()),
            );
            (vec![err], env2)
        }
    }
}
