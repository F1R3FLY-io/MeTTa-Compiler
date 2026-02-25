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
            _ => unreachable!("Invalid step {} for AddOpGeneric", state.step),
        }
    }
}

/// Generic TCO Subtraction/Negation operation: (- a b) or (- a)
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
                match state.args.len() {
                    1 => {
                        // Unary minus: (- x) => negate x
                        state.step = 10;
                        GenericGroundedWork::EvalArg {
                            arg_idx: 0,
                            state: state.clone(),
                        }
                    }
                    2 => {
                        // Binary minus: (- a b) => a - b
                        state.step = 1;
                        GenericGroundedWork::EvalArg {
                            arg_idx: 0,
                            state: state.clone(),
                        }
                    }
                    _ => GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "- requires 1 or 2 arguments, got {}",
                        state.args.len()
                    ))),
                }
            }
            // --- Unary minus path ---
            10 => {
                let a_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(a_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }
                let mut results = Vec::with_capacity(a_results.len());
                for a in a_results {
                    match (a.as_long(), a.as_float()) {
                        (Some(x), _) => match x.checked_neg() {
                            Some(neg) => results.push((factory.long(neg), None)),
                            None => {
                                return GenericGroundedWork::Error(ExecError::Runtime(
                                    format!("Integer overflow: -({})", x),
                                ))
                            }
                        },
                        (_, Some(x)) => results.push((factory.float(-x), None)),
                        _ => {
                            // MeTTa HE: Empty sentinel → skip (branch annihilation)
                            if a.is_empty() { continue; }
                            return GenericGroundedWork::Error(ExecError::NoReduce);
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            // --- Binary minus path ---
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
                        match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                            (Some(x), _, Some(y), _) => {
                                // Long % Long
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
                            (_, Some(x), _, Some(y)) => {
                                // Float % Float
                                if y == 0.0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x % y), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                // Long % Float
                                if y == 0.0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x as f64 % y), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                // Float % Long
                                if y == 0 {
                                    return GenericGroundedWork::Error(ExecError::Arithmetic(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                results.push((factory.float(x % y as f64), None));
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
            _ => unreachable!("Invalid step {} for ModOpGeneric", state.step),
        }
    }
}

/// Generic TCO Minimum operation: (min a b)
pub struct MinOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for MinOpGeneric {
    fn name(&self) -> &str {
        "min"
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
                        "min requires 2 arguments, got {}",
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
                                // Long min Long → Long
                                results.push((factory.long(x.min(y)), None));
                            }
                            (_, Some(x), _, Some(y)) => {
                                // Float min Float → Float
                                results.push((factory.float(x.min(y)), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                // Long min Float → Float
                                results.push((factory.float((x as f64).min(y)), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                // Float min Long → Float
                                results.push((factory.float(x.min(y as f64)), None));
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
            _ => unreachable!("Invalid step {} for MinOpGeneric", state.step),
        }
    }
}

/// Generic TCO Maximum operation: (max a b)
pub struct MaxOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for MaxOpGeneric {
    fn name(&self) -> &str {
        "max"
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
                        "max requires 2 arguments, got {}",
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
                                // Long max Long → Long
                                results.push((factory.long(x.max(y)), None));
                            }
                            (_, Some(x), _, Some(y)) => {
                                // Float max Float → Float
                                results.push((factory.float(x.max(y)), None));
                            }
                            (Some(x), _, _, Some(y)) => {
                                // Long max Float → Float
                                results.push((factory.float((x as f64).max(y)), None));
                            }
                            (_, Some(x), Some(y), _) => {
                                // Float max Long → Float
                                results.push((factory.float(x.max(y as f64)), None));
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
            _ => unreachable!("Invalid step {} for MaxOpGeneric", state.step),
        }
    }
}

/// Generic TCO Safe Division operation: (/safe A B)
/// Returns A/B if B > 0.0, else zero results (empty = branch annihilation).
/// PLN uses this for safe division: (if (> $B 0.0) (/ $A $B) (empty))
pub struct SafeDivOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for SafeDivOpGeneric {
    fn name(&self) -> &str {
        "/safe"
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
                        "/safe requires 2 arguments, got {}",
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
                        // Extract numeric values as f64
                        let a_val = a.as_float().or_else(|| a.as_long().map(|l| l as f64));
                        let b_val = b.as_float().or_else(|| b.as_long().map(|l| l as f64));

                        match (a_val, b_val) {
                            (Some(x), Some(y)) if y > 0.0 => {
                                results.push((factory.float(x / y), None));
                            }
                            (Some(_), Some(_)) => {
                                // b <= 0.0: zero results (branch annihilation)
                                // Don't push anything — this branch is pruned.
                            }
                            _ => {
                                if a.is_empty() || b.is_empty() {
                                    continue;
                                }
                                return GenericGroundedWork::Error(ExecError::NoReduce);
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for SafeDivOpGeneric", state.step),
        }
    }
}

/// Generic TCO Clamp operation: (clamp val min max)
/// Returns min(max, max(val, min)), clamping val to [min, max].
pub struct ClampOpGeneric;

impl<V: MettaValueTrait + Clone> GenericGroundedOperationTCO<V> for ClampOpGeneric {
    fn name(&self) -> &str {
        "clamp"
    }

    fn execute_step_generic<F: MettaValueFactory<V>>(
        &self,
        state: &mut GenericGroundedState<V>,
        factory: &F,
    ) -> GenericGroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 3 {
                    return GenericGroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "clamp requires 3 arguments, got {}",
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
                let val_results = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error_generic(val_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GenericGroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let min_results = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error_generic(min_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 3;
                GenericGroundedWork::EvalArg {
                    arg_idx: 2,
                    state: state.clone(),
                }
            }
            3 => {
                let val_results = state.get_arg(0).expect("arg 0 should be evaluated");
                let min_results = state.get_arg(1).expect("arg 1 should be evaluated");
                let max_results = state.get_arg(2).expect("arg 2 should be evaluated");

                if let Some(err) = find_error_generic(max_results) {
                    return GenericGroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for val in val_results {
                    for min_v in min_results {
                        for max_v in max_results {
                            let v = val.as_float().or_else(|| val.as_long().map(|l| l as f64));
                            let mn = min_v.as_float().or_else(|| min_v.as_long().map(|l| l as f64));
                            let mx = max_v.as_float().or_else(|| max_v.as_long().map(|l| l as f64));

                            match (v, mn, mx) {
                                (Some(v), Some(mn), Some(mx)) => {
                                    let clamped = v.max(mn).min(mx);
                                    results.push((factory.float(clamped), None));
                                }
                                _ => {
                                    if val.is_empty() || min_v.is_empty() || max_v.is_empty() {
                                        continue;
                                    }
                                    return GenericGroundedWork::Error(ExecError::NoReduce);
                                }
                            }
                        }
                    }
                }
                GenericGroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for ClampOpGeneric", state.step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_add_op_generic_longs() {
        let factory = GcFactory::default();
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
        let factory = GcFactory::default();
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
        let factory = GcFactory::default();
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

    // --- Unary minus tests ---

    #[test]
    fn test_sub_op_generic_unary() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("-".to_string(), vec![MettaValue::Long(5)]);
        let op = SubOpGeneric;

        // Step 0: should request arg 0 evaluation (unary path → step 10)
        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::EvalArg { arg_idx: 0, .. }));

        // Simulate arg 0 evaluation
        state.set_arg(0, vec![MettaValue::Long(5)]);
        state.step = 10;

        // Step 10: compute negation
        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(-5));
            }
            _ => panic!("Expected Done, got {:?}", work),
        }
    }

    #[test]
    fn test_sub_op_generic_unary_float() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("-".to_string(), vec![MettaValue::Float(3.14)]);
        let op = SubOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(3.14)]);
        state.step = 10;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(-3.14));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_sub_op_generic_unary_zero() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("-".to_string(), vec![MettaValue::Long(0)]);
        let op = SubOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(0)]);
        state.step = 10;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(0));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_sub_op_generic_no_args() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("-".to_string(), vec![]);
        let op = SubOpGeneric;

        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::Error(ExecError::IncorrectArgument(_))));
    }

    #[test]
    fn test_sub_op_generic_three_args() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "-".to_string(),
            vec![MettaValue::Long(1), MettaValue::Long(2), MettaValue::Long(3)],
        );
        let op = SubOpGeneric;

        let work = op.execute_step_generic(&mut state, &factory);
        assert!(matches!(work, GenericGroundedWork::Error(ExecError::IncorrectArgument(_))));
    }

    #[test]
    fn test_mul_op_generic() {
        let factory = GcFactory::default();
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
        let factory = GcFactory::default();
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
        let factory = GcFactory::default();
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
        let factory = GcFactory::default();
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

    // --- Mixed-type modulo tests ---

    #[test]
    fn test_mod_float_float() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "%".to_string(),
            vec![MettaValue::Float(10.5), MettaValue::Float(3.0)],
        );

        let op = ModOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(10.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(3.0)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(1.5)); // 10.5 % 3.0 = 1.5
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_mod_long_float() {
        // Key HE example: Long(85) % Float(43.5) = Float(41.5)
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "%".to_string(),
            vec![MettaValue::Long(85), MettaValue::Float(43.5)],
        );

        let op = ModOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(85)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(43.5)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                // 85.0 % 43.5 = 85.0 - 1*43.5 = 41.5
                assert_eq!(results[0].0.as_float(), Some(41.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_mod_float_long() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "%".to_string(),
            vec![MettaValue::Float(85.5), MettaValue::Long(43)],
        );

        let op = ModOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(85.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(43)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                // 85.5 % 43.0 = 85.5 - 1*43.0 = 42.5
                assert_eq!(results[0].0.as_float(), Some(42.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_mod_float_by_zero() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new(
            "%".to_string(),
            vec![MettaValue::Float(10.5), MettaValue::Float(0.0)],
        );

        let op = ModOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(10.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(0.0)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        assert!(
            matches!(work, GenericGroundedWork::Error(ExecError::Arithmetic(_))),
            "Float modulo by zero should produce an Arithmetic error"
        );
    }

    // --- Min operation tests ---

    #[test]
    fn test_min_op_generic_longs() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("min".to_string(), vec![MettaValue::Long(10), MettaValue::Long(3)]);
        let op = MinOpGeneric;

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
                assert_eq!(results[0].0.as_long(), Some(3));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_min_op_generic_floats() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("min".to_string(), vec![MettaValue::Float(1.5), MettaValue::Float(2.5)]);
        let op = MinOpGeneric;

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
                assert_eq!(results[0].0.as_float(), Some(1.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_min_op_generic_long_float() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("min".to_string(), vec![MettaValue::Long(10), MettaValue::Float(3.5)]);
        let op = MinOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(10)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(3.5)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(3.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_min_op_generic_float_long() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("min".to_string(), vec![MettaValue::Float(2.5), MettaValue::Long(10)]);
        let op = MinOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(2.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(10)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(2.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    // --- Max operation tests ---

    #[test]
    fn test_max_op_generic_longs() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("max".to_string(), vec![MettaValue::Long(10), MettaValue::Long(3)]);
        let op = MaxOpGeneric;

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
                assert_eq!(results[0].0.as_long(), Some(10));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_max_op_generic_floats() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("max".to_string(), vec![MettaValue::Float(1.5), MettaValue::Float(2.5)]);
        let op = MaxOpGeneric;

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
                assert_eq!(results[0].0.as_float(), Some(2.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_max_op_generic_long_float() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("max".to_string(), vec![MettaValue::Long(3), MettaValue::Float(10.5)]);
        let op = MaxOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(3)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Float(10.5)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(10.5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_max_op_generic_float_long() {
        let factory = GcFactory::default();
        let mut state = GenericGroundedState::new("max".to_string(), vec![MettaValue::Float(10.5), MettaValue::Long(3)]);
        let op = MaxOpGeneric;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(0, vec![MettaValue::Float(10.5)]);
        state.step = 1;

        op.execute_step_generic(&mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(3)]);
        state.step = 2;

        let work = op.execute_step_generic(&mut state, &factory);
        match work {
            GenericGroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_float(), Some(10.5));
            }
            _ => panic!("Expected Done"),
        }
    }
}
