//! Generic TCO comparison operations.
//!
//! Provides generic tail-call optimized comparison operations that work with any
//! value type implementing `MettaValueTrait`:
//!
//! - `LessOpGeneric` - Less than (<)
//! - `LessEqOpGeneric` - Less than or equal (<=)
//! - `GreaterOpGeneric` - Greater than (>)
//! - `GreaterEqOpGeneric` - Greater than or equal (>=)
//! - `EqualOpGeneric` - Equality (==)
//! - `NotEqualOpGeneric` - Not equal (!=)
//!
//! ## Zero-Conversion Design
//!
//! These operations use `MettaValueTrait` methods (e.g., `as_long()`, `as_float()`)
//! instead of pattern matching on `MettaValueInner`, enabling them to work with
//! both heap and arena allocation without conversion.

use super::generic_state::{find_error_generic, GenericGroundedState, GenericGroundedWork};
use super::generic_traits::GenericGroundedOperationTCO;
use super::ExecError;
use crate::backend::models::{numeric_equal_generic, MettaValueFactory, MettaValueTrait};

/// Generic TCO Less than operation: (< a b)
pub struct LessOpGeneric;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GenericGroundedOperationTCO<V> for LessOpGeneric {
    fn name(&self) -> &str {
        "<"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        comparison_step(state, factory, "<", |x, y| x < y, |x, y| x < y)
    }
}

/// Generic TCO Less than or equal operation: (<= a b)
pub struct LessEqOpGeneric;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GenericGroundedOperationTCO<V> for LessEqOpGeneric {
    fn name(&self) -> &str {
        "<="
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        comparison_step(state, factory, "<=", |x, y| x <= y, |x, y| x <= y)
    }
}

/// Generic TCO Greater than operation: (> a b)
pub struct GreaterOpGeneric;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GenericGroundedOperationTCO<V> for GreaterOpGeneric {
    fn name(&self) -> &str {
        ">"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        comparison_step(state, factory, ">", |x, y| x > y, |x, y| x > y)
    }
}

/// Generic TCO Greater than or equal operation: (>= a b)
pub struct GreaterEqOpGeneric;

impl<V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static> GenericGroundedOperationTCO<V> for GreaterEqOpGeneric {
    fn name(&self) -> &str {
        ">="
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        comparison_step(state, factory, ">=", |x, y| x >= y, |x, y| x >= y)
    }
}

/// Generic TCO Equality operation: (== a b)
pub struct EqualOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for EqualOpGeneric {
    fn name(&self) -> &str {
        "=="
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        equality_step(state, factory, "==", true)
    }
}

/// Generic TCO Not equal operation: (!= a b)
pub struct NotEqualOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for NotEqualOpGeneric {
    fn name(&self) -> &str {
        "!="
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        equality_step(state, factory, "!=", false)
    }
}

/// Helper function for numeric comparison operations.
fn comparison_step<V, F, FL, FF>(
    state: &mut GenericGroundedState<V>,
    factory: &F,
    op_name: &str,
    long_cmp: FL,
    float_cmp: FF,
) -> GenericGroundedWork<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
    FL: Fn(i64, i64) -> bool,
    FF: Fn(f64, f64) -> bool,
{
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "{} requires 2 arguments, got {}",
                    op_name,
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
                            results.push((factory.bool(long_cmp(x, y)), None));
                        }
                        (_, Some(x), _, Some(y)) => {
                            results.push((factory.bool(float_cmp(x, y)), None));
                        }
                        (Some(x), _, _, Some(y)) => {
                            results.push((factory.bool(float_cmp(x as f64, y)), None));
                        }
                        (_, Some(x), Some(y), _) => {
                            results.push((factory.bool(float_cmp(x, y as f64)), None));
                        }
                        _ => {
                            // MeTTa HE: Empty sentinel → skip (branch annihilation)
                            if a.is_empty() || b.is_empty() {
                                continue;
                            }
                            return GenericGroundedWork::Error(ExecError::NoReduce)
                        }
                    }
                }
            }
            GenericGroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for {} operation", state.step, op_name),
    }
}

/// Helper function for equality/inequality operations.
///
/// These support more types than numeric comparisons - any two values can be compared.
fn equality_step<V, F>(
    state: &mut GenericGroundedState<V>,
    factory: &F,
    op_name: &str,
    return_true_on_equal: bool,
) -> GenericGroundedWork<V>
where
    V: MettaValueTrait + Clone + PartialEq,
    F: MettaValueFactory<V>,
{
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "{} requires 2 arguments, got {}",
                    op_name,
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
                    // MeTTa HE: Empty sentinel → skip (branch annihilation)
                    if a.is_empty() || b.is_empty() {
                        continue;
                    }
                    let is_equal = numeric_equal_generic(a, b);
                    let result = if return_true_on_equal { is_equal } else { !is_equal };
                    results.push((factory.bool(result), None));
                }
            }
            GenericGroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for {} operation", state.step, op_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    fn run_comparison<Op: GenericGroundedOperationTCO<MettaValue>>(
        op: &Op,
        a: i64,
        b: i64,
    ) -> bool {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(op.name().to_string(), vec![MettaValue::Long(a), MettaValue::Long(b)]);

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(a)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(b)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                results[0].0.as_bool().expect("should be bool")
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_less_op() {
        assert!(run_comparison(&LessOpGeneric, 1, 2));
        assert!(!run_comparison(&LessOpGeneric, 2, 1));
        assert!(!run_comparison(&LessOpGeneric, 2, 2));
    }

    #[test]
    fn test_less_eq_op() {
        assert!(run_comparison(&LessEqOpGeneric, 1, 2));
        assert!(!run_comparison(&LessEqOpGeneric, 2, 1));
        assert!(run_comparison(&LessEqOpGeneric, 2, 2));
    }

    #[test]
    fn test_greater_op() {
        assert!(!run_comparison(&GreaterOpGeneric, 1, 2));
        assert!(run_comparison(&GreaterOpGeneric, 2, 1));
        assert!(!run_comparison(&GreaterOpGeneric, 2, 2));
    }

    #[test]
    fn test_greater_eq_op() {
        assert!(!run_comparison(&GreaterEqOpGeneric, 1, 2));
        assert!(run_comparison(&GreaterEqOpGeneric, 2, 1));
        assert!(run_comparison(&GreaterEqOpGeneric, 2, 2));
    }

    #[test]
    fn test_equal_op() {
        assert!(!run_comparison(&EqualOpGeneric, 1, 2));
        assert!(run_comparison(&EqualOpGeneric, 2, 2));
    }

    #[test]
    fn test_not_equal_op() {
        assert!(run_comparison(&NotEqualOpGeneric, 1, 2));
        assert!(!run_comparison(&NotEqualOpGeneric, 2, 2));
    }

    // --- Mixed-type comparison tests ---

    fn run_comparison_values<Op: GenericGroundedOperationTCO<MettaValue>>(
        op: &Op,
        a: MettaValue,
        b: MettaValue,
    ) -> bool {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            op.name().to_string(),
            vec![a.clone(), b.clone()],
        );

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![a]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![b]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                results[0].0.as_bool().expect("should be bool")
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_equal_long_float() {
        // EqualOpGeneric: Long(2) vs Float(2.0) should be true (key HE fix)
        assert!(run_comparison_values(
            &EqualOpGeneric,
            MettaValue::Long(2),
            MettaValue::Float(2.0),
        ));
    }

    #[test]
    fn test_equal_float_long() {
        // EqualOpGeneric: Float(2.0) vs Long(2) should be true (symmetric)
        assert!(run_comparison_values(
            &EqualOpGeneric,
            MettaValue::Float(2.0),
            MettaValue::Long(2),
        ));
    }

    #[test]
    fn test_equal_float_float() {
        // EqualOpGeneric: Float(3.14) vs Float(3.14) should be true
        assert!(run_comparison_values(
            &EqualOpGeneric,
            MettaValue::Float(3.14),
            MettaValue::Float(3.14),
        ));
    }

    #[test]
    fn test_not_equal_long_float_same() {
        // NotEqualOpGeneric: Long(2) vs Float(2.0) should be false (they are equal)
        assert!(!run_comparison_values(
            &NotEqualOpGeneric,
            MettaValue::Long(2),
            MettaValue::Float(2.0),
        ));
    }

    #[test]
    fn test_not_equal_long_float_different() {
        // NotEqualOpGeneric: Long(2) vs Float(2.5) should be true (they differ)
        assert!(run_comparison_values(
            &NotEqualOpGeneric,
            MettaValue::Long(2),
            MettaValue::Float(2.5),
        ));
    }

    #[test]
    fn test_less_float_float() {
        // LessOpGeneric: Float(1.5) vs Float(2.5) should be true
        assert!(run_comparison_values(
            &LessOpGeneric,
            MettaValue::Float(1.5),
            MettaValue::Float(2.5),
        ));
    }

    #[test]
    fn test_less_long_float() {
        // LessOpGeneric: Long(1) vs Float(2.5) should be true (mixed type)
        assert!(run_comparison_values(
            &LessOpGeneric,
            MettaValue::Long(1),
            MettaValue::Float(2.5),
        ));
    }

    #[test]
    fn test_greater_eq_float_long() {
        // GreaterEqOpGeneric: Float(5.0) vs Long(5) should be true
        assert!(run_comparison_values(
            &GreaterEqOpGeneric,
            MettaValue::Float(5.0),
            MettaValue::Long(5),
        ));
    }
}
