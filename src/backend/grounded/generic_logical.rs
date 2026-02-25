//! Generic TCO logical operations.
//!
//! Provides generic tail-call optimized logical operations that work with any
//! value type implementing `MettaValueTrait`:
//!
//! - `AndOpGeneric` - Logical AND with short-circuit evaluation
//! - `OrOpGeneric` - Logical OR with short-circuit evaluation
//! - `NotOpGeneric` - Logical NOT
//! - `XorOpGeneric` - Logical XOR (no short-circuit — both operands always needed)
//!
//! ## Zero-Conversion Design
//!
//! These operations use `MettaValueTrait` methods (e.g., `as_bool()`)
//! instead of pattern matching on `MettaValueInner`, enabling them to work with
//! both heap and arena allocation without conversion.

use super::generic_state::{find_error_generic, GenericGroundedState, GenericGroundedWork};
use super::generic_traits::GenericGroundedOperationTCO;
use super::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Generic TCO Logical AND operation: (and a b)
///
/// Short-circuit evaluation: if `a` evaluates to `False`, returns `False`
/// without evaluating `b`.
pub struct AndOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for AndOpGeneric {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "and requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GenericGroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                // Short-circuit: if any result is False, we might return False early
                // But for Cartesian product semantics, we need to evaluate both args
                // unless ALL results are False (then we can short-circuit)
                let all_false = a_results.iter().all(|v| v.as_bool() == Some(false));
                if all_false {
                    // Short-circuit: all False, no need to evaluate b
                    return GenericGroundedWork::Done(vec![(factory.bool(false), None)]);
                }

                state.step = 2;
                GenericGroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error_generic(b_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a.as_bool(), b.as_bool()) {
                            (Some(x), Some(y)) => {
                                results.push((factory.bool(x && y), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform 'and': expected Bool, got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )));
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for and operation", state.step),
        }
    }
}

/// Generic TCO Logical OR operation: (or a b)
///
/// Short-circuit evaluation: if `a` evaluates to `True`, returns `True`
/// without evaluating `b`.
pub struct OrOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for OrOpGeneric {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "or requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GenericGroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                // Short-circuit: if all results are True, return True early
                let all_true = a_results.iter().all(|v| v.as_bool() == Some(true));
                if all_true {
                    // Short-circuit: all True, no need to evaluate b
                    return GenericGroundedWork::Done(vec![(factory.bool(true), None)]);
                }

                state.step = 2;
                GenericGroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error_generic(b_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a.as_bool(), b.as_bool()) {
                            (Some(x), Some(y)) => {
                                results.push((factory.bool(x || y), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform 'or': expected Bool, got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )));
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for or operation", state.step),
        }
    }
}

/// Generic TCO Logical NOT operation: (not a)
pub struct NotOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for NotOpGeneric {
    fn name(&self) -> &str {
        "not"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "not requires 1 argument, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GenericGroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    match a.as_bool() {
                        Some(x) => {
                            results.push((factory.bool(!x), None));
                        }
                        None => {
                            return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'not': expected Bool, got {}",
                                a.friendly_type_name()
                            )));
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for not operation", state.step),
        }
    }
}

/// Generic TCO Logical XOR operation: (xor a b)
///
/// No short-circuit: XOR always needs both operands since the result depends
/// on both values (true iff exactly one operand is true).
pub struct XorOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for XorOpGeneric {
    fn name(&self) -> &str {
        "xor"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "xor requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GenericGroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                // No short-circuit for XOR — always need both operands
                state.step = 2;
                GenericGroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error_generic(b_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a.as_bool(), b.as_bool()) {
                            (Some(x), Some(y)) => {
                                results.push((factory.bool(x ^ y), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform 'xor': expected Bool, got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )));
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for xor operation", state.step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    fn run_binary_logical<Op: GenericGroundedOperationTCO<MettaValue>>(
        op: &Op,
        a: bool,
        b: bool,
    ) -> bool {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            op.name().to_string(),
            vec![MettaValue::Bool(a), MettaValue::Bool(b)],
        );

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(a)]);
        state.step = 1;

        let work = op.execute_step_generic(&mut state, &factory);

        // Check for short-circuit
        match work {
            GenericGroundedWork::Done(results) => {
                return results[0].0.as_bool().expect("should be bool");
            }
            GenericGroundedWork::EvalArg { .. } => {
                // Need to continue
                state.set_arg(1, vec![MettaValue::Bool(b)]);
                state.step = 2;
            }
            _ => panic!("Unexpected work"),
        }

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => results[0].0.as_bool().expect("should be bool"),
            _ => panic!("Expected Done"),
        }
    }

    fn run_unary_logical<Op: GenericGroundedOperationTCO<MettaValue>>(op: &Op, a: bool) -> bool {
        let factory = GcFactory::default();
        let mut state =
            GenericGroundedState::new(op.name().to_string(), vec![MettaValue::Bool(a)]);

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(a)]);
        state.step = 1;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => results[0].0.as_bool().expect("should be bool"),
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_and_op() {
        assert!(run_binary_logical(&AndOpGeneric, true, true));
        assert!(!run_binary_logical(&AndOpGeneric, true, false));
        assert!(!run_binary_logical(&AndOpGeneric, false, true));
        assert!(!run_binary_logical(&AndOpGeneric, false, false));
    }

    #[test]
    fn test_or_op() {
        assert!(run_binary_logical(&OrOpGeneric, true, true));
        assert!(run_binary_logical(&OrOpGeneric, true, false));
        assert!(run_binary_logical(&OrOpGeneric, false, true));
        assert!(!run_binary_logical(&OrOpGeneric, false, false));
    }

    #[test]
    fn test_not_op() {
        assert!(!run_unary_logical(&NotOpGeneric, true));
        assert!(run_unary_logical(&NotOpGeneric, false));
    }

    #[test]
    fn test_xor_op() {
        assert!(!run_binary_logical(&XorOpGeneric, true, true));
        assert!(run_binary_logical(&XorOpGeneric, true, false));
        assert!(run_binary_logical(&XorOpGeneric, false, true));
        assert!(!run_binary_logical(&XorOpGeneric, false, false));
    }

    #[test]
    fn test_and_short_circuit() {
        // When first arg is all False, should short-circuit
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "and".to_string(),
            vec![MettaValue::Bool(false), MettaValue::Bool(true)],
        );

        AndOpGeneric.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(false)]);
        state.step = 1;

        let work = AndOpGeneric.execute_step_generic(&mut state, &factory);
        // Should be Done (short-circuit), not EvalArg
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results[0].0.as_bool(), Some(false));
            }
            _ => panic!("Expected short-circuit Done"),
        }
    }

    #[test]
    fn test_or_short_circuit() {
        // When first arg is all True, should short-circuit
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "or".to_string(),
            vec![MettaValue::Bool(true), MettaValue::Bool(false)],
        );

        OrOpGeneric.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(true)]);
        state.step = 1;

        let work = OrOpGeneric.execute_step_generic(&mut state, &factory);
        // Should be Done (short-circuit), not EvalArg
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results[0].0.as_bool(), Some(true));
            }
            _ => panic!("Expected short-circuit Done"),
        }
    }
}
