//! State operations.
//!
//! This module handles mutable state cells:
//! - new-state: Create a new mutable state cell
//! - get-state: Get the current value from a state cell
//! - change-state!: Change the value in a state cell

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::super::eval;
use super::super::EvalStep;

/// Step version of eval_new_state - defers evaluation to trampoline.
/// Usage: (new-state initial-value)
pub(crate) fn eval_new_state_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            format!(
                "new-state requires 1 argument, got {}. Usage: (new-state initial-value)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let initial_value = items[1].clone();

    EvalStep::StartNewState {
        initial_value,
        env,
        depth,
    }
}

/// Step version of eval_get_state - defers evaluation to trampoline.
/// Usage: (get-state state-ref)
pub(crate) fn eval_get_state_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            format!(
                "get-state requires 1 argument, got {}. Usage: (get-state state)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let state_ref = items[1].clone();

    EvalStep::StartGetState {
        state_ref,
        env,
        depth,
    }
}

/// Step version of eval_change_state - defers evaluation to trampoline.
/// Usage: (change-state! state-ref new-value)
pub(crate) fn eval_change_state_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
    depth: usize,
) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "change-state! requires 2 arguments, got {}. Usage: (change-state! state new-value)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let state_ref = items[1].clone();
    let new_value = items[2].clone();

    EvalStep::StartChangeState {
        state_ref,
        new_value,
        env,
        depth,
    }
}

/// new-state: Create a new mutable state cell with an initial value
/// Usage: (new-state initial-value)
///
/// DEPRECATED: Use eval_new_state_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_new_state(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("new-state", items, 1, env, "(new-state initial-value)");

    let initial_value = &items[1];

    // Evaluate the initial value
    let (value_results, mut env1) = eval(initial_value.clone(), env);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "new-state: initial value evaluated to empty".to_string(),
            initial_value.clone(),
        );
        return (vec![err], env1);
    }

    let value = &value_results[0];
    let state_id = env1.create_state(value);
    (vec![MettaValue::State(state_id)], env1)
}

/// get-state: Get the current value from a state cell
/// Usage: (get-state state-ref)
///
/// DEPRECATED: Use eval_get_state_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_get_state(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("get-state", items, 1, env, "(get-state state)");

    let state_ref = &items[1];

    // Evaluate the state reference
    let (state_results, env1) = eval(state_ref.clone(), env);
    if state_results.is_empty() {
        let err = MettaValue::Error(
            "get-state: state evaluated to empty".to_string(),
            state_ref.clone(),
        );
        return (vec![err], env1);
    }

    let state_value = &state_results[0];

    match state_value.inner() {
        MettaValueInner::State(state_id) => {
            if let Some(value) = env1.get_state(*state_id) {
                (vec![value], env1)
            } else {
                let err = MettaValue::Error(
                    format!("get-state: state {} not found", state_id),
                    state_value.clone(),
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
                state_value.clone(),
            );
            (vec![err], env1)
        }
    }
}

/// change-state!: Change the value in a state cell
/// Usage: (change-state! state-ref new-value)
/// Returns the state reference for chaining
///
/// DEPRECATED: Use eval_change_state_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_change_state(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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
            state_ref.clone(),
        );
        return (vec![err], env1);
    }

    // Evaluate the new value
    let (value_results, mut env2) = eval(new_value.clone(), env1);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "change-state!: new value evaluated to empty".to_string(),
            new_value.clone(),
        );
        return (vec![err], env2);
    }

    let state_value = &state_results[0];
    let value = &value_results[0];

    match state_value.inner() {
        MettaValueInner::State(state_id) => {
            if env2.change_state(*state_id, value) {
                // Return the state reference for chaining
                (vec![state_value.clone()], env2)
            } else {
                let err = MettaValue::Error(
                    format!("change-state!: state {} not found", state_id),
                    state_value.clone(),
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
                state_value.clone(),
            );
            (vec![err], env2)
        }
    }
}
