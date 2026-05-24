//! Meta / polymorphic grounded operations.
//!
//! Houses HE-aligned polymorphic helpers that don't fit the
//! arithmetic/comparison/logical/string buckets. Currently:
//!
//! - `id` — identity (HE: `(= (id $x) $x)` in stdlib.metta:263 with
//!   signature `(: id (-> $t $t))`). Empirically verified:
//!     `!(id 42)`        -> `[42]`
//!     `!(id "hello")`   -> `["hello"]`
//!     `!(id (a b c))`   -> `[(a b c)]`
//!     `!(id (g))` with `(= (g) 42)` -> `[42]`   (argument is evaluated first)
//!
//! Implemented here as a grounded op rather than a stdlib rule so we don't
//! need to inject MeTTa source on env init; this matches our existing pattern
//! for trivial pass-through ops and keeps the rule table free of polymorphic
//! shadow entries.

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::grounded::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// `(id $x) -> $x`.
///
/// Returns the evaluated argument unchanged. Equivalent to HE stdlib2's
/// `(= (id $x) $x)`. Errors propagate (the first error result is returned).
pub struct IdOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for IdOp {
    fn name(&self) -> &str {
        "id"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        _factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                // HE empirical: `!(id)` and `!(id 1 2)` both emit
                // `(Error <call> IncorrectNumberOfArguments)` — tagged atom,
                // not a string. Match exactly via `ExecError::Tagged`.
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // Forward every evaluation result unchanged (Cartesian-product
                // semantics for multi-valued args — `id` is the identity, so
                // the result set equals the input set).
                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    results.push((value.clone(), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for id operation", state.step),
        }
    }
}
