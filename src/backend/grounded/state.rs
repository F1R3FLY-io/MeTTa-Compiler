//! State types for TCO grounded operations.
//!
//! Provides the state machine types for tail-call optimized grounded operations:
//! - `GroundedWork` - Enum describing what work needs to be done
//! - `GroundedState` - State saved between operation steps

use std::collections::HashMap;
use std::sync::Arc;

use super::{Bindings, ExecError, MettaValue};

/// Work returned by grounded operations for TCO.
///
/// Instead of calling `eval_fn` internally, operations return this enum
/// to request argument evaluation. The trampoline processes the request
/// and calls the operation back with results.
#[derive(Debug, Clone)]
pub enum GroundedWork {
    /// Operation complete - return these results
    Done(Vec<(MettaValue, Option<Bindings>)>),

    /// Need to evaluate an argument before continuing
    EvalArg {
        /// Which argument index to evaluate (0-based)
        arg_idx: usize,
        /// Operation state to restore when resuming
        state: GroundedState,
    },

    /// Error during execution
    Error(ExecError),
}

/// State saved between grounded operation steps.
///
/// This struct is passed to `execute_step` and contains all state needed
/// to resume the operation after argument evaluation completes.
#[derive(Debug, Clone)]
pub struct GroundedState {
    /// Operation name (to look up the operation again)
    pub op_name: String,
    /// Original unevaluated arguments (Arc-wrapped for O(1) clone)
    pub args: Arc<Vec<MettaValue>>,
    /// Results from previously evaluated arguments: arg_idx -> Vec<MettaValue>
    pub evaluated_args: HashMap<usize, Vec<MettaValue>>,
    /// Current step in the operation's state machine
    pub step: usize,
    /// Accumulated results so far (for short-circuit ops like `and`/`or`)
    pub accumulated_results: Vec<(MettaValue, Option<Bindings>)>,
}

impl GroundedState {
    /// Create a new state for starting an operation
    pub fn new(op_name: String, args: Vec<MettaValue>) -> Self {
        GroundedState {
            op_name,
            args: Arc::new(args),
            evaluated_args: HashMap::new(),
            step: 0,
            accumulated_results: Vec::new(),
        }
    }

    /// Create a new state from pre-wrapped Arc args (avoids re-wrapping)
    pub fn from_arc(op_name: String, args: Arc<Vec<MettaValue>>) -> Self {
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
    pub fn args(&self) -> &[MettaValue] {
        &self.args
    }

    /// Get evaluated arg results, or None if not yet evaluated
    pub fn get_arg(&self, idx: usize) -> Option<&Vec<MettaValue>> {
        self.evaluated_args.get(&idx)
    }

    /// Set evaluated arg results
    pub fn set_arg(&mut self, idx: usize, results: Vec<MettaValue>) {
        self.evaluated_args.insert(idx, results);
    }
}
