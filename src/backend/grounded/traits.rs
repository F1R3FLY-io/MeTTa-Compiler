//! Traits for grounded operations.
//!
//! Defines the core traits that grounded operations must implement:
//! - `GroundedOperation` - Standard lazy evaluation trait
//! - `GroundedOperationTCO` - Tail-call optimized variant

use super::{Environment, GroundedResult, GroundedState, GroundedWork, MettaValue};

/// Function type for evaluating MeTTa expressions
/// Used by grounded operations to evaluate their arguments when needed
pub type EvalFn = dyn Fn(MettaValue, Environment) -> (Vec<MettaValue>, Environment) + Send + Sync;

/// HE-compatible trait for grounded (built-in) operations.
///
/// Operations receive RAW (unevaluated) arguments and evaluate them internally
/// when concrete values are needed. This matches HE's lazy evaluation semantics.
///
/// # Implementing a Grounded Operation
///
/// ```ignore
/// struct AddOp;
///
/// impl GroundedOperation for AddOp {
///     fn name(&self) -> &str { "+" }
///
///     fn execute_raw(
///         &self,
///         args: &[MettaValue],
///         env: &Environment,
///         eval_fn: &EvalFn,
///     ) -> GroundedResult {
///         if args.len() != 2 {
///             return Err(ExecError::IncorrectArgument(
///                 format!("+ requires 2 arguments, got {}", args.len())
///             ));
///         }
///
///         // Evaluate arguments internally
///         let (a_results, _) = eval_fn(args[0].clone(), env.clone());
///         let (b_results, _) = eval_fn(args[1].clone(), env.clone());
///
///         // Compute Cartesian product of results
///         let mut results = Vec::new();
///         for a in &a_results {
///             for b in &b_results {
///                 if let (MettaValue::Long(x), MettaValue::Long(y)) = (a, b) {
///                     results.push((MettaValue::Long(x + y), None));
///                 } else {
///                     return Err(ExecError::NoReduce);
///                 }
///             }
///         }
///         Ok(results)
///     }
/// }
/// ```
pub trait GroundedOperation: Send + Sync {
    /// The name of this operation (e.g., "+", "-", "and")
    fn name(&self) -> &str;

    /// Execute the operation with unevaluated arguments.
    ///
    /// # Arguments
    /// * `args` - The unevaluated argument expressions
    /// * `env` - The current environment for evaluation
    /// * `eval_fn` - Function to evaluate sub-expressions when needed
    ///
    /// # Returns
    /// * `Ok(results)` - List of (value, optional_bindings) pairs
    /// * `Err(NoReduce)` - Operation not applicable, try other rules
    /// * `Err(...)` - Actual error during execution
    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult;
}

/// TCO-compatible trait for grounded operations.
///
/// Unlike `GroundedOperation`, this trait does NOT receive an `eval_fn`.
/// Instead, operations return `GroundedWork::EvalArg` to request argument
/// evaluation, and the trampoline handles it.
///
/// # State Machine Pattern
///
/// Operations are implemented as state machines:
/// - Step 0: Validate args, request first argument evaluation
/// - Step 1: Process first arg results, request second argument (or compute)
/// - Step 2+: Continue until `GroundedWork::Done` or `GroundedWork::Error`
///
/// # Example
///
/// ```ignore
/// impl GroundedOperationTCO for AddOpTCO {
///     fn name(&self) -> &str { "+" }
///
///     fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
///         match state.step {
///             0 => {
///                 if state.args.len() != 2 {
///                     return GroundedWork::Error(...);
///                 }
///                 state.step = 1;
///                 GroundedWork::EvalArg { arg_idx: 0, state: state.clone() }
///             }
///             1 => {
///                 state.step = 2;
///                 GroundedWork::EvalArg { arg_idx: 1, state: state.clone() }
///             }
///             2 => {
///                 // Compute result from evaluated args
///                 let a = state.get_arg(0).unwrap();
///                 let b = state.get_arg(1).unwrap();
///                 // ... compute Cartesian product ...
///                 GroundedWork::Done(results)
///             }
///             _ => unreachable!()
///         }
///     }
/// }
/// ```
pub trait GroundedOperationTCO: Send + Sync {
    /// The name of this operation (e.g., "+", "-", "and")
    fn name(&self) -> &str;

    /// Execute one step of the operation.
    ///
    /// Called initially with `state.step == 0` and empty `state.evaluated_args`.
    /// Called again after each `EvalArg` request with the results added.
    ///
    /// # Arguments
    /// * `state` - Mutable state that persists across steps
    ///
    /// # Returns
    /// * `Done(results)` - Operation complete, return these values
    /// * `EvalArg { arg_idx, state }` - Evaluate argument at index, then call again
    /// * `Error(e)` - Operation failed with error
    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork;
}
