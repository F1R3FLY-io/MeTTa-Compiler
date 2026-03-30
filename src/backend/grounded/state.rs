//! State types for TCO grounded operations.
//!
//! Provides state machine types for tail-call optimized grounded operations
//! that work with any value type implementing `MettaValueTrait`.
//!
//! - `GroundedWork<V>` - Enum describing what work needs to be done
//! - `GroundedState<V>` - State saved between operation steps
//!
//! ## Design
//!
//! These types are parameterized over the value type `V`, enabling grounded operations
//! to work with both heap-allocated (`MettaValue`) and arena-allocated (`MettaValue`)
//! types without conversion at boundaries.
//!
//! ## Zero-Conversion Pattern
//!
//! By using generic types, evaluated arguments are stored in their native representation:
//! - `MettaValue.clone()` = O(1) Arc increment
//! - `MettaValue.clone()` = O(1) pointer copy
//!
//! This eliminates the need for conversion between heap and arena types during
//! grounded operation execution.

use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::grounded::ExecError;
use crate::backend::models::{GenericBindings, MettaValueTrait};

/// Work returned by grounded operations for TCO.
///
/// Instead of calling `eval_fn` internally, operations return this enum
/// to request argument evaluation. The trampoline processes the request
/// and calls the operation back with results.
///
/// This version works with any value type implementing `MettaValueTrait`.
#[derive(Debug, Clone)]
pub enum GroundedWork<V: MettaValueTrait + Clone> {
    /// Operation complete - return these results
    Done(Vec<(V, Option<GenericBindings<V>>)>),

    /// Need to evaluate an argument before continuing
    EvalArg {
        /// Which argument index to evaluate (0-based)
        arg_idx: usize,
        /// Operation state to restore when resuming
        state: GroundedState<V>,
    },

    /// Error during execution
    Error(ExecError),
}

/// State saved between grounded operation steps.
///
/// This struct is passed to `execute_step` and contains all state needed
/// to resume the operation after argument evaluation completes.
///
/// Parameterized over `V: MettaValueTrait` to store values in their native type.
#[derive(Debug, Clone)]
pub struct GroundedState<V: MettaValueTrait + Clone> {
    /// Operation name (to look up the operation again)
    pub op_name: String,
    /// Original unevaluated arguments (Arc-wrapped for O(1) clone)
    pub args: Arc<Vec<V>>,
    /// Results from previously evaluated arguments: arg_idx -> Vec<V>
    pub evaluated_args: HashMap<usize, Vec<V>>,
    /// Current step in the operation's state machine
    pub step: usize,
    /// Accumulated results so far (for short-circuit ops like `and`/`or`)
    pub accumulated_results: Vec<(V, Option<GenericBindings<V>>)>,
}

impl<V: MettaValueTrait + Clone> GroundedState<V> {
    /// Create a new state for starting an operation
    pub fn new(op_name: String, args: Vec<V>) -> Self {
        GroundedState {
            op_name,
            args: Arc::new(args),
            evaluated_args: HashMap::new(),
            step: 0,
            accumulated_results: Vec::new(),
        }
    }

    /// Create a new state from pre-wrapped Arc args (avoids re-wrapping)
    pub fn from_arc(op_name: String, args: Arc<Vec<V>>) -> Self {
        GroundedState {
            op_name,
            args,
            evaluated_args: HashMap::new(),
            step: 0,
            accumulated_results: Vec::new(),
        }
    }

    /// Get the args slice
    #[inline]
    pub fn args(&self) -> &[V] {
        &self.args
    }

    /// Get evaluated arg results, or None if not yet evaluated
    pub fn get_arg(&self, idx: usize) -> Option<&Vec<V>> {
        self.evaluated_args.get(&idx)
    }

    /// Set evaluated arg results
    ///
    /// # Panics
    /// Panics if `idx` is suspiciously large (> 1000), which likely indicates
    /// an integer underflow bug rather than legitimate use.
    pub fn set_arg(&mut self, idx: usize, results: Vec<V>) {
        // DEFENSIVE ASSERTION: Catch bogus indices from underflow.
        debug_assert!(
            idx < 1000,
            "BUG: Suspicious arg index {} in set_arg (likely underflow). \
             op_name={}, step={}, args_len={}, evaluated_args_keys={:?}",
            idx,
            self.op_name,
            self.step,
            self.args.len(),
            self.evaluated_args.keys().collect::<Vec<_>>()
        );
        if idx >= 1000 {
            panic!(
                "BUG: Suspicious arg index {} in set_arg (likely underflow). \
                 op_name={}, step={}, args_len={}, evaluated_args_keys={:?}",
                idx,
                self.op_name,
                self.step,
                self.args.len(),
                self.evaluated_args.keys().collect::<Vec<_>>()
            );
        }
        self.evaluated_args.insert(idx, results);
    }
}

/// Check if any result in the vector is an error.
///
/// Version that works with any value type implementing `MettaValueTrait`.
pub fn find_error<V: MettaValueTrait>(results: &[V]) -> Option<&V> {
    results.iter().find(|v| v.is_error())
}

/// Get a friendly type name for error messages.
///
/// Version that uses `MettaValueTrait::friendly_type_name()`.
pub fn friendly_type_name<V: MettaValueTrait>(value: &V) -> &'static str {
    value.friendly_type_name()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_generic_grounded_state_new() {
        let args = vec![MettaValue::Long(1), MettaValue::Long(2)];
        let state: GroundedState<MettaValue> = GroundedState::new("+".to_string(), args);

        assert_eq!(state.op_name, "+");
        assert_eq!(state.args.len(), 2);
        assert_eq!(state.step, 0);
        assert!(state.evaluated_args.is_empty());
    }

    #[test]
    fn test_generic_grounded_state_set_get_arg() {
        let args = vec![MettaValue::Long(1), MettaValue::Long(2)];
        let mut state: GroundedState<MettaValue> = GroundedState::new("+".to_string(), args);

        // Initially no args evaluated
        assert!(state.get_arg(0).is_none());

        // Set evaluated arg
        state.set_arg(0, vec![MettaValue::Long(10)]);

        // Now we can get it
        let arg0 = state.get_arg(0).expect("should have arg 0");
        assert_eq!(arg0.len(), 1);
        assert_eq!(arg0[0].as_long(), Some(10));
    }

    #[test]
    fn test_find_error() {
        let values = vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
        ];
        assert!(find_error(&values).is_none());

        let values_with_error = vec![
            MettaValue::Long(1),
            MettaValue::Error("test error".to_string(), MettaValue::Unit()),
        ];
        let err = find_error(&values_with_error);
        assert!(err.is_some());
        assert!(err.unwrap().is_error());
    }
}
