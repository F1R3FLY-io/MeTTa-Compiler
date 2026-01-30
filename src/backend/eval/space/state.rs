//! State operations.
//!
//! This module handles mutable state cells:
//! - new-state: Create a new mutable state cell
//! - get-state: Get the current value from a state cell
//! - change-state!: Change the value in a state cell

use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::EvalStep;

/// Step version of eval_new_state - defers evaluation to trampoline.
/// Usage: (new-state initial-value)
pub(crate) fn eval_new_state_step(
    items: Vec<MettaValue>,
    env: Environment,
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
    env: Environment,
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
    env: Environment,
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

