//! Step-based Evaluation
//!
//! This module contains the functions and types for performing single evaluation
//! steps in the trampoline-based evaluator.
//!
//! ## Generic Step Functions
//!
//! The `generic_step` and `generic_sexpr` modules provide generic versions of the
//! step evaluation functions that work with any value type implementing `MettaValueTrait`.

mod generic_sexpr;
mod generic_step;
mod generic_types;
mod grounded;
mod sexpr_step;
mod types;

#[allow(unused_imports)]
pub use generic_sexpr::eval_sexpr_step_generic;
pub use generic_step::eval_step_generic;
#[allow(unused_imports)]
pub use generic_types::{GenericEvalStep, GenericProcessedSExpr, HeapEvalStep, HeapProcessedSExpr};
#[allow(unused_imports)]
pub use grounded::{find_grounded_arg_indices, find_grounded_arg_indices_generic};
pub use sexpr_step::eval_sexpr_step;
pub use types::{EvalStep, MemoOpType, ProcessedSExpr};

use tracing::trace;

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::conjunction::eval_conjunction_step;

/// Perform a single step of evaluation.
/// Returns either a final result or indicates more work is needed.
///
/// Note: The `depth` parameter is retained for debugging/metrics but no longer
/// enforces a limit. In a trampoline-based evaluator, the Rust stack is bounded
/// by design (work items are heap-allocated), so depth limits are unnecessary
/// for preventing stack overflow. The previous MAX_EVAL_DEPTH check was incorrectly
/// tracking work item count rather than recursion depth, causing legitimate
/// iterative workloads (map-atom, filter-atom, foldl-atom over lists) to fail.
pub fn eval_step(value: MettaValue, env: HeapEnvironment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_step", ?value, depth);

    match value.inner() {
        // Errors propagate immediately
        MettaValueInner::Error(_, _) => EvalStep::Done((vec![value], env)),

        // Atoms: check special tokens first, then tokenizer, then evaluate to themselves
        // This enables HE-compatible bind! semantics where tokens are replaced during evaluation
        MettaValueInner::Atom(ref name) => {
            // Special handling for &self - evaluates to the current module's space
            // This is HE-compatible behavior where &self is a space reference
            if name == "&self" {
                let space_handle = env.self_space();
                return EvalStep::Done((vec![MettaValue::Space(space_handle)], env));
            }

            if let Some(bound_value) = env.lookup_token(name) {
                // Token was registered via bind! - return the bound value
                EvalStep::Done((vec![bound_value], env))
            } else {
                // No binding - atom evaluates to itself
                EvalStep::Done((vec![value], env))
            }
        }

        // Ground types evaluate to themselves
        MettaValueInner::Bool(_)
        | MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::String(_)
        | MettaValueInner::Nil
        | MettaValueInner::Type(_)
        | MettaValueInner::Space(_)
        | MettaValueInner::State(_)
        | MettaValueInner::Unit
        | MettaValueInner::Memo(_) => EvalStep::Done((vec![value], env)),

        // Empty sentinel - gets filtered out at result collection
        MettaValueInner::Empty => EvalStep::Done((vec![], env)),

        // S-expressions need special handling
        MettaValueInner::SExpr(items) => eval_sexpr_step(items.clone(), env, depth),

        // For conjunctions, evaluate goals left-to-right with binding threading
        MettaValueInner::Conjunction(goals) => eval_conjunction_step(goals.clone(), env, depth),
    }
}
