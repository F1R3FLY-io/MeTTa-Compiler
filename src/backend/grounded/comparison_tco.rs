//! TCO comparison operations.
//!
//! Provides tail-call optimized comparison operations:
//! - `LessOpTCO`, `LessEqOpTCO` - Less than / less than or equal
//! - `GreaterOpTCO`, `GreaterEqOpTCO` - Greater than / greater than or equal
//! - `EqualOpTCO`, `NotEqualOpTCO` - Equality / inequality

use super::comparison::{values_equal, CompareKind};
use super::{
    find_error, friendly_type_name, ExecError, GroundedOperationTCO, GroundedState, GroundedWork,
    MettaValue,
};

/// TCO Less than operation: (< a b)
pub struct LessOpTCO;

impl GroundedOperationTCO for LessOpTCO {
    fn name(&self) -> &str {
        "<"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::Less)
    }
}

/// TCO Less than or equal operation: (<= a b)
pub struct LessEqOpTCO;

impl GroundedOperationTCO for LessEqOpTCO {
    fn name(&self) -> &str {
        "<="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::LessEq)
    }
}

/// TCO Greater than operation: (> a b)
pub struct GreaterOpTCO;

impl GroundedOperationTCO for GreaterOpTCO {
    fn name(&self) -> &str {
        ">"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::Greater)
    }
}

/// TCO Greater than or equal operation: (>= a b)
pub struct GreaterEqOpTCO;

impl GroundedOperationTCO for GreaterEqOpTCO {
    fn name(&self) -> &str {
        ">="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::GreaterEq)
    }
}

/// TCO Equality operation: (== a b)
pub struct EqualOpTCO;

impl GroundedOperationTCO for EqualOpTCO {
    fn name(&self) -> &str {
        "=="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_equality_tco(state, true)
    }
}

/// TCO Inequality operation: (!= a b)
pub struct NotEqualOpTCO;

impl GroundedOperationTCO for NotEqualOpTCO {
    fn name(&self) -> &str {
        "!="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_equality_tco(state, false)
    }
}

/// Helper function for TCO comparison operations
fn eval_comparison_tco(state: &mut GroundedState, kind: CompareKind) -> GroundedWork {
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "Comparison requires 2 arguments, got {}",
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
            state.step = 2;
            GroundedWork::EvalArg {
                arg_idx: 1,
                state: state.clone(),
            }
        }
        2 => {
            let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
            let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

            if let Some(err) = find_error(b_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }

            let mut results = Vec::new();
            for a in a_results {
                for b in b_results {
                    match (a, b) {
                        (MettaValue::Long(x), MettaValue::Long(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        (MettaValue::Float(x), MettaValue::Float(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        (MettaValue::Long(x), MettaValue::Float(y)) => {
                            results.push((MettaValue::Bool(kind.compare(&(*x as f64), y)), None));
                        }
                        (MettaValue::Float(x), MettaValue::Long(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, &(*y as f64))), None));
                        }
                        (MettaValue::String(x), MettaValue::String(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot compare: type mismatch between {} and {}",
                                friendly_type_name(a),
                                friendly_type_name(b)
                            )))
                        }
                    }
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for comparison", state.step),
    }
}

/// Helper function for TCO equality/inequality operations
fn eval_equality_tco(state: &mut GroundedState, is_equal: bool) -> GroundedWork {
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "Equality comparison requires 2 arguments, got {}",
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
            state.step = 2;
            GroundedWork::EvalArg {
                arg_idx: 1,
                state: state.clone(),
            }
        }
        2 => {
            let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
            let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

            if let Some(err) = find_error(b_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }

            let mut results = Vec::new();
            for a in a_results {
                for b in b_results {
                    let equal = values_equal(a, b);
                    let result = if is_equal { equal } else { !equal };
                    results.push((MettaValue::Bool(result), None));
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for equality", state.step),
    }
}
