//! Generic TCO arithmetic operations.
//!
//! Provides generic tail-call optimized arithmetic operations that work with any
//! value type implementing `MettaValueTrait`:
//!
//! - `AddOpGeneric` - Addition (+)
//! - `SubOpGeneric` - Subtraction (-)
//! - `MulOpGeneric` - Multiplication (*)
//! - `DivOpGeneric` - Division (/)
//! - `ModOpGeneric` - Modulo (%)
//!
//! ## Zero-Conversion Design
//!
//! These operations use `MettaValueTrait` methods (e.g., `as_long()`, `as_float()`)
//! instead of pattern matching on `MettaValueInner`, enabling them to work with
//! both heap and arena allocation without conversion.

use super::generic_state::{find_error_generic, GenericGroundedState, GenericGroundedWork};
use super::generic_traits::GenericGroundedOperationTCO;
use super::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Generic TCO Addition operation: (+ a b)
pub struct AddOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for AddOpGeneric {
    fn name(&self) -> &str {
        "+"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                // Step 0: Validate arity and request first argument
                if state.args.len() != 2 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "+ requires 2 arguments, got {}",
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
                // Step 1: Check first arg for errors, request second argument
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GenericGroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                // Step 2: Compute Cartesian product of results
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let b_results = state.get_arg(1).expect("arg 1 should be evaluated");

                if let Some(err) = find_error_generic(b_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        // Use trait methods instead of pattern matching on inner
                        match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                            (Some(x), _, Some(y), _) => {
                                // Long + Long
                                match x.checked_add(y) {
                                    Some(sum) => results.push((factory.long(sum), None)),
                                    None => {
                                        return GenericGroundedWork::Error(ExecError::Runtime(
                                            format!("Integer overflow: {} + {}", x, y),
                                        ))
                                    }
                                }
                            }
                            (_, Some(x), _, Some(y)) => {
                                // Float + Float
                                results.push((factory.float(x + y), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                // Long + Float
                                results.push((factory.float(x as f64 + y), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                // Float + Long
                                results.push((factory.float(x + y as f64), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '+': expected Number (integer), got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )))
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for AddOpGeneric", state.step),
        }
    }
}

/// Generic TCO Subtraction operation: (- a b)
pub struct SubOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for SubOpGeneric {
    fn name(&self) -> &str {
        "-"
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
                        "- requires 2 arguments, got {}",
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
                        match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                            (Some(x), _, Some(y), _) => {
                                match x.checked_sub(y) {
                                    Some(diff) => results.push((factory.long(diff), None)),
                                    None => {
                                        return GenericGroundedWork::Error(ExecError::Runtime(
                                            format!("Integer overflow: {} - {}", x, y),
                                        ))
                                    }
                                }
                            }
                            (_, Some(x), _, Some(y)) => {
                                results.push((factory.float(x - y), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                results.push((factory.float(x as f64 - y), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                results.push((factory.float(x - y as f64), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '-': expected Number (integer), got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )))
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for SubOpGeneric", state.step),
        }
    }
}

/// Generic TCO Multiplication operation: (* a b)
pub struct MulOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for MulOpGeneric {
    fn name(&self) -> &str {
        "*"
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
                        "* requires 2 arguments, got {}",
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
                        match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                            (Some(x), _, Some(y), _) => {
                                match x.checked_mul(y) {
                                    Some(prod) => results.push((factory.long(prod), None)),
                                    None => {
                                        return GenericGroundedWork::Error(ExecError::Runtime(
                                            format!("Integer overflow: {} * {}", x, y),
                                        ))
                                    }
                                }
                            }
                            (_, Some(x), _, Some(y)) => {
                                results.push((factory.float(x * y), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                results.push((factory.float(x as f64 * y), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                results.push((factory.float(x * y as f64), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '*': expected Number (integer), got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )))
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for MulOpGeneric", state.step),
        }
    }
}

/// Generic TCO Division operation: (/ a b)
pub struct DivOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for DivOpGeneric {
    fn name(&self) -> &str {
        "/"
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
                        "/ requires 2 arguments, got {}",
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
                        match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                            (Some(x), _, Some(y), _) => {
                                if y == 0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((factory.long(x / y), None));
                            }
                            (_, Some(x), _, Some(y)) => {
                                if y == 0.0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x / y), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                if y == 0.0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x as f64 / y), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                if y == 0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x / y as f64), None));
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '/': expected Number (integer), got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )))
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for DivOpGeneric", state.step),
        }
    }
}

/// Generic TCO Modulo operation: (% a b)
pub struct ModOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for ModOpGeneric {
    fn name(&self) -> &str {
        "%"
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
                        "% requires 2 arguments, got {}",
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
                        match (a.as_long(), b.as_long()) {
                            (Some(x), Some(y)) => {
                                if y == 0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                match x.checked_rem(y) {
                                    Some(r) => results.push((factory.long(r), None)),
                                    None => {
                                        return GenericGroundedWork::Error(ExecError::Arithmetic(
                                            "Modulo overflow".to_string(),
                                        ))
                                    }
                                }
                            }
                            _ => {
                                return GenericGroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '%': expected Number (integer), got {} and {}",
                                    a.friendly_type_name(),
                                    b.friendly_type_name()
                                )))
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for ModOpGeneric", state.step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{HeapMettaValueFactory, MettaValue};

    #[test]
    fn test_add_op_generic_longs() {
        let factory = HeapMettaValueFactory;
        let args = vec![MettaValue::Long(1), MettaValue::Long(2)];
        let mut state = GenericGroundedState::new("+".to_string(), args);

        let op = AddOpGeneric;

        // Step 0: Request first arg
        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::EvalArg { arg_idx: 0, .. }));

        // Simulate arg 0 evaluation
        state.set_arg(0, vec![MettaValue::Long(10)]);
        state.step = 1;

        // Step 1: Request second arg
        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::EvalArg { arg_idx: 1, .. }));

        // Simulate arg 1 evaluation
        state.set_arg(1, vec![MettaValue::Long(20)]);
        state.step = 2;

        // Step 2: Compute result
        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(30));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_add_op_generic_floats() {
        let factory = HeapMettaValueFactory;
        let args = vec![MettaValue::Float(1.5), MettaValue::Float(2.5)];
        let mut state = GenericGroundedState::new("+".to_string(), args);

        let op = AddOpGeneric;

        // Run all steps
        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(1.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(2.5)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(4.0));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_sub_op_generic() {
        let factory = HeapMettaValueFactory;
        let mut state = GenericGroundedState::new("-".to_string(), vec![MettaValue::Long(10), MettaValue::Long(3)]);

        let op = SubOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(10)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(3)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(7));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_mul_op_generic() {
        let factory = HeapMettaValueFactory;
        let mut state = GenericGroundedState::new("*".to_string(), vec![MettaValue::Long(6), MettaValue::Long(7)]);

        let op = MulOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(6)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(7)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(42));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_div_op_generic() {
        let factory = HeapMettaValueFactory;
        let mut state = GenericGroundedState::new("/".to_string(), vec![MettaValue::Long(20), MettaValue::Long(4)]);

        let op = DivOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(20)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(4)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_div_by_zero() {
        let factory = HeapMettaValueFactory;
        let mut state = GenericGroundedState::new("/".to_string(), vec![MettaValue::Long(10), MettaValue::Long(0)]);

        let op = DivOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(10)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(0)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::Error(ExecError::Arithmetic(_))));
    }

    #[test]
    fn test_mod_op_generic() {
        let factory = HeapMettaValueFactory;
        let mut state = GenericGroundedState::new("%".to_string(), vec![MettaValue::Long(17), MettaValue::Long(5)]);

        let op = ModOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(17)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(5)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(2)); // 17 % 5 = 2
            }
            _ => panic!("Expected Done"),
        }
    }
}
