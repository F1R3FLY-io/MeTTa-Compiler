//! TCO logical operations.
//!
//! Provides tail-call optimized logical operations with short-circuit semantics:
//! - `AndOpTCO` - Logical AND (and)
//! - `OrOpTCO` - Logical OR (or)
//! - `NotOpTCO` - Logical NOT (not)

use super::{
    find_error, friendly_type_name, ExecError, GroundedOperationTCO, GroundedState, GroundedWork,
    MettaValue,
};

/// TCO Logical AND operation: (and a b)
/// Preserves short-circuit semantics: False AND _ = False without evaluating second arg
pub struct AndOpTCO;

impl GroundedOperationTCO for AndOpTCO {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                // Step 0: Validate arity and request first argument
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "and requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                // Step 1: Check first arg - short-circuit if all False
                // Clone the results to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state
                    .get_arg(0)
                    .expect("arg 0 should be evaluated")
                    .to_vec();
                if let Some(err) = find_error(&a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut need_second_arg = false;
                for a in &a_results {
                    match a {
                        MettaValue::Bool(false) => {
                            // SHORT-CIRCUIT: False and _ = False
                            state
                                .accumulated_results
                                .push((MettaValue::Bool(false), None));
                        }
                        MettaValue::Bool(true) => {
                            // Need to evaluate second argument for this branch
                            need_second_arg = true;
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'and': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }

                if need_second_arg {
                    // At least one True - need to evaluate second arg
                    state.step = 2;
                    GroundedWork::EvalArg {
                        arg_idx: 1,
                        state: state.clone(),
                    }
                } else {
                    // All results were False (short-circuited)
                    GroundedWork::Done(state.accumulated_results.clone())
                }
            }
            2 => {
                // Step 2: Second arg evaluated for True branches
                // Clone both to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state
                    .get_arg(0)
                    .expect("arg 0 should be evaluated")
                    .to_vec();
                let b_results: Vec<_> = state
                    .get_arg(1)
                    .expect("arg 1 should be evaluated")
                    .to_vec();

                if let Some(err) = find_error(&b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // For each True in first arg, add second arg's results
                for a in &a_results {
                    if matches!(a, MettaValue::Bool(true)) {
                        for b in &b_results {
                            match b {
                                MettaValue::Bool(val) => {
                                    state
                                        .accumulated_results
                                        .push((MettaValue::Bool(*val), None));
                                }
                                _ => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Cannot perform 'and': expected Bool, got {}",
                                        friendly_type_name(b)
                                    )));
                                }
                            }
                        }
                    }
                }

                GroundedWork::Done(state.accumulated_results.clone())
            }
            _ => unreachable!("Invalid step {} for AndOpTCO", state.step),
        }
    }
}

/// TCO Logical OR operation: (or a b)
/// Preserves short-circuit semantics: True OR _ = True without evaluating second arg
pub struct OrOpTCO;

impl GroundedOperationTCO for OrOpTCO {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "or requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                // Clone to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state
                    .get_arg(0)
                    .expect("arg 0 should be evaluated")
                    .to_vec();
                if let Some(err) = find_error(&a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut need_second_arg = false;
                for a in &a_results {
                    match a {
                        MettaValue::Bool(true) => {
                            // SHORT-CIRCUIT: True or _ = True
                            state
                                .accumulated_results
                                .push((MettaValue::Bool(true), None));
                        }
                        MettaValue::Bool(false) => {
                            // Need to evaluate second argument for this branch
                            need_second_arg = true;
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'or': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }

                if need_second_arg {
                    state.step = 2;
                    GroundedWork::EvalArg {
                        arg_idx: 1,
                        state: state.clone(),
                    }
                } else {
                    // All results were True (short-circuited)
                    GroundedWork::Done(state.accumulated_results.clone())
                }
            }
            2 => {
                // Clone both to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state
                    .get_arg(0)
                    .expect("arg 0 should be evaluated")
                    .to_vec();
                let b_results: Vec<_> = state
                    .get_arg(1)
                    .expect("arg 1 should be evaluated")
                    .to_vec();

                if let Some(err) = find_error(&b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // For each False in first arg, add second arg's results
                for a in &a_results {
                    if matches!(a, MettaValue::Bool(false)) {
                        for b in &b_results {
                            match b {
                                MettaValue::Bool(val) => {
                                    state
                                        .accumulated_results
                                        .push((MettaValue::Bool(*val), None));
                                }
                                _ => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Cannot perform 'or': expected Bool, got {}",
                                        friendly_type_name(b)
                                    )));
                                }
                            }
                        }
                    }
                }

                GroundedWork::Done(state.accumulated_results.clone())
            }
            _ => unreachable!("Invalid step {} for OrOpTCO", state.step),
        }
    }
}

/// TCO Logical NOT operation: (not a)
pub struct NotOpTCO;

impl GroundedOperationTCO for NotOpTCO {
    fn name(&self) -> &str {
        "not"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "not requires 1 argument, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    match a {
                        MettaValue::Bool(v) => {
                            results.push((MettaValue::Bool(!v), None));
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'not': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for NotOpTCO", state.step),
        }
    }
}
