//! Traits for grounded operations.
//!
//! Defines the core traits that grounded operations can implement to work
//! with any value type implementing `MettaValueTrait`:
//!
//! - `GroundedOperationTCO<V>` - Tail-call optimized variant
//!
//! ## Design
//!
//! By parameterizing over the value type `V`, operations can work with both
//! heap-allocated (`MettaValue`) and arena-allocated (`MettaValue`) types without
//! conversion at boundaries.
//!
//! ## Implementation Pattern
//!
//! Operations use `MettaValueTrait` methods instead of pattern matching on
//! `MettaValueInner`:
//!
//! ```ignore
//! // Before (concrete type):
//! match (a.inner(), b.inner()) {
//!     (MettaValueInner::Long(x), MettaValueInner::Long(y)) => {
//!         results.push((MettaValue::Long(x + y), None));
//!     }
//! }
//!
//! // After (generic type):
//! if let (Some(x), Some(y)) = (a.as_long(), b.as_long()) {
//!     results.push((factory.long(x + y), None));
//! }
//! ```

use super::state::{GroundedState, GroundedWork};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// TCO-compatible trait for grounded operations.
///
/// This trait is parameterized over the value type `V`,
/// enabling operations to work with both heap and arena allocation strategies.
///
/// Operations are implemented as state machines:
/// - Step 0: Validate args, request first argument evaluation
/// - Step 1: Process first arg results, request second argument (or compute)
/// - Step 2+: Continue until `GroundedWork::Done` or `GroundedWork::Error`
///
/// ## Factory Parameter
///
/// The `execute_step` method takes a factory parameter for constructing
/// result values. This allows the operation to create values in the correct type
/// without knowing the concrete implementation.
///
/// ## Example
///
/// ```ignore
/// impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for AddOp {
///     fn name(&self) -> &str { "+" }
///
///     fn execute_step<F: MettaValueFactory<V>>(
///         &self,
///         state: &mut GroundedState<V>,
///         factory: &F,
///     ) -> GroundedWork<V> {
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
///                 // Compute result using trait methods
///                 let a = state.get_arg(0).unwrap();
///                 let b = state.get_arg(1).unwrap();
///                 let mut results = Vec::new();
///                 for av in a {
///                     for bv in b {
///                         if let (Some(x), Some(y)) = (av.as_long(), bv.as_long()) {
///                             results.push((factory.long(x + y), None));
///                         }
///                     }
///                 }
///                 GroundedWork::Done(results)
///             }
///             _ => unreachable!()
///         }
///     }
/// }
/// ```
pub trait GroundedOperationTCO<V: MettaValueTrait + Clone>: Send + Sync {
    /// The name of this operation (e.g., "+", "-", "and")
    fn name(&self) -> &str;

    /// Execute one step of the operation using generic types.
    ///
    /// Called initially with `state.step == 0` and empty `state.evaluated_args`.
    /// Called again after each `EvalArg` request with the results added.
    ///
    /// # Arguments
    /// * `state` - Mutable state that persists across steps
    /// * `factory` - Factory for constructing result values
    ///
    /// # Returns
    /// * `Done(results)` - Operation complete, return these values
    /// * `EvalArg { arg_idx, state }` - Evaluate argument at index, then call again
    /// * `Error(e)` - Operation failed with error
    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V>;
}

#[cfg(test)]
mod tests {
    // Tests will be added when we implement concrete generic operations
}
