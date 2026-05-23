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

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use std::collections::HashMap;
use std::sync::Arc;

use crate::backend::grounded::ExecError;
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

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

    /// Build the originating call form `(op_name arg0 arg1 ...)` from the
    /// operation's name and stored args. Used by the HE-aligned Error-atom
    /// shape `(Error <call> <detail>)` (spec §10.6, HE
    /// `metta/runner/stdlib/atom.rs` decons_atom emit pattern).
    pub fn call_form<F: MettaValueFactory<V>>(&self, factory: &F) -> V {
        let mut parts = Vec::with_capacity(1 + self.args.len());
        parts.push(factory.atom(&self.op_name));
        for arg in self.args.iter() {
            parts.push(arg.clone());
        }
        factory.sexpr(parts)
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

/// Build a HE-aligned `BadArgType` error atom for the case when a grounded
/// op's argument evaluated to an `Error` value (T04/046).
///
/// HE empirically returns `(Error (op args...) (BadArgType POS EXPECTED ErrorType))`
/// — see `hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs` Error
/// emission shape (spec §10.6). The position is 1-indexed.
///
/// Use this in arithmetic / comparison / logical ops after `find_error`
/// detects an Error-typed argument; produces the structured BadArgType
/// shape instead of forwarding the inner error verbatim.
pub fn error_to_bad_arg_type<V, F>(
    state: &GroundedState<V>,
    factory: &F,
    arg_idx: usize,
    expected_type: &str,
) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let call = state.call_form(factory);
    let detail = factory.sexpr(vec![
        factory.atom("BadArgType"),
        factory.long((arg_idx + 1) as i64),
        factory.atom(expected_type),
        factory.atom("ErrorType"),
    ]);
    factory.error( detail,call)
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
        let values = vec![MettaValue::Long(1), MettaValue::Long(2)];
        assert!(find_error(&values).is_none());

        let values_with_error = vec![
            MettaValue::Long(1),
            MettaValue::Error(MettaValue::Unit(), MettaValue::String("test error")),
        ];
        let err = find_error(&values_with_error);
        assert!(err.is_some());
        assert!(err.unwrap().is_error());
    }
}
