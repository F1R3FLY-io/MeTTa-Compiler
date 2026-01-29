//! Step-based Evaluation
//!
//! This module contains the functions and types for performing single evaluation
//! steps in the trampoline-based evaluator.

mod grounded;
mod sexpr_step;
mod types;

pub use grounded::find_grounded_arg_indices;
pub use sexpr_step::eval_sexpr_step;
pub use types::{EvalStep, MemoOpType, ProcessedSExpr};

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

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
pub fn eval_step(value: MettaValue, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_step", ?value, depth);

    match value {
        // Errors propagate immediately
        MettaValue::Error(_, _) => EvalStep::Done((vec![value], env)),

        // Atoms: check special tokens first, then tokenizer, then evaluate to themselves
        // This enables HE-compatible bind! semantics where tokens are replaced during evaluation
        MettaValue::Atom(ref name) => {
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
        MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Type(_)
        | MettaValue::Space(_)
        | MettaValue::State(_)
        | MettaValue::Unit
        | MettaValue::Memo(_) => EvalStep::Done((vec![value], env)),

        // Empty sentinel - gets filtered out at result collection
        MettaValue::Empty => EvalStep::Done((vec![], env)),

        // S-expressions need special handling
        MettaValue::SExpr(items) => eval_sexpr_step(items, env, depth),

        // For conjunctions, evaluate goals left-to-right with binding threading
        MettaValue::Conjunction(goals) => eval_conjunction_step(goals, env, depth),
    }
}
