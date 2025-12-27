//! TCO arithmetic operations.
//!
//! Provides tail-call optimized arithmetic operations:
//! - `AddOpTCO` - Addition (+)
//! - `SubOpTCO` - Subtraction (-)
//! - `MulOpTCO` - Multiplication (*)
//! - `DivOpTCO` - Division (/)
//! - `ModOpTCO` - Modulo (%)

use super::{
    find_error, friendly_type_name, ExecError, GroundedOperationTCO, GroundedState, GroundedWork,
    MettaValue,
};

/// TCO Addition operation: (+ a b)
pub struct AddOpTCO;

impl GroundedOperationTCO for AddOpTCO {
    fn name(&self) -> &str {
        "+"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                // Step 0: Validate arity and request first argument
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "+ requires 2 arguments, got {}",
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
                // Step 1: Check first arg for errors, request second argument
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
                // Step 2: Compute Cartesian product of results
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_add(*y) {
                                Some(sum) => results.push((MettaValue::Long(sum), None)),
                                None => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Integer overflow: {} + {}",
                                        x, y
                                    )))
                                }
                            },
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x + y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 + y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x + *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '+': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_))
                                        {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for AddOpTCO", state.step),
        }
    }
}

/// TCO Subtraction operation: (- a b)
pub struct SubOpTCO;

impl GroundedOperationTCO for SubOpTCO {
    fn name(&self) -> &str {
        "-"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "- requires 2 arguments, got {}",
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
                            (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_sub(*y) {
                                Some(diff) => results.push((MettaValue::Long(diff), None)),
                                None => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Integer overflow: {} - {}",
                                        x, y
                                    )))
                                }
                            },
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x - y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 - y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x - *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '-': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_))
                                        {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for SubOpTCO", state.step),
        }
    }
}

/// TCO Multiplication operation: (* a b)
pub struct MulOpTCO;

impl GroundedOperationTCO for MulOpTCO {
    fn name(&self) -> &str {
        "*"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "* requires 2 arguments, got {}",
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
                            (MettaValue::Long(x), MettaValue::Long(y)) => match x.checked_mul(*y) {
                                Some(prod) => results.push((MettaValue::Long(prod), None)),
                                None => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Integer overflow: {} * {}",
                                        x, y
                                    )))
                                }
                            },
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x * y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 * y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x * *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '*': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_))
                                        {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for MulOpTCO", state.step),
        }
    }
}

/// TCO Division operation: (/ a b)
pub struct DivOpTCO;

impl GroundedOperationTCO for DivOpTCO {
    fn name(&self) -> &str {
        "/"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "/ requires 2 arguments, got {}",
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
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Long(x / y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                if *y == 0.0 {
                                    return GroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(x / y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                if *y == 0.0 {
                                    return GroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(*x as f64 / y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(x / *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '/': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_))
                                        {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for DivOpTCO", state.step),
        }
    }
}

/// TCO Modulo operation: (% a b)
pub struct ModOpTCO;

impl GroundedOperationTCO for ModOpTCO {
    fn name(&self) -> &str {
        "%"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "% requires 2 arguments, got {}",
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
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Arithmetic(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                match x.checked_rem(*y) {
                                    Some(r) => results.push((MettaValue::Long(r), None)),
                                    None => {
                                        return GroundedWork::Error(ExecError::Arithmetic(
                                            "Modulo overflow".to_string(),
                                        ))
                                    }
                                }
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '%': expected Number (integer), got {}",
                                    friendly_type_name(if !matches!(a, MettaValue::Long(_)) {
                                        a
                                    } else {
                                        b
                                    })
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for ModOpTCO", state.step),
        }
    }
}
