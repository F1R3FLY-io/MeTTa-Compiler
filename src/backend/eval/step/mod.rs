//! Step-based Evaluation
//!
//! This module contains the functions and types for performing single evaluation
//! steps in the trampoline-based evaluator.

mod types;
mod grounded;
mod sexpr_step;

pub use types::{EvalStep, ProcessedSExpr};
pub use grounded::evaluate_grounded_args;
pub use sexpr_step::eval_sexpr_step;

use std::sync::Arc;

use tracing::{trace, warn};

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::trampoline::MAX_EVAL_DEPTH;
use super::{eval_conjunction, friendly_value_repr};

/// Perform a single step of evaluation.
/// Returns either a final result or indicates more work is needed.
pub fn eval_step(value: MettaValue, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_step", ?value, depth);

    // Check depth limit
    if depth > MAX_EVAL_DEPTH {
        warn!(
            depth = depth,
            max_depth = MAX_EVAL_DEPTH,
            "Maximum evaluation depth exceeded - possible infinite recursion or combinatorial explosion"
        );

        return EvalStep::Done((
            vec![MettaValue::Error(
                format!(
                    "Maximum evaluation depth ({}) exceeded. Possible causes:\n\
                     - Infinite recursion: check for missing base case in recursive rules\n\
                     - Combinatorial explosion: rule produces too many branches\n\
                     Hint: Use (function ...) and (return ...) for tail-recursive evaluation",
                    MAX_EVAL_DEPTH
                ),
                Arc::new(value),
            )],
            env,
        ));
    }

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
        MettaValue::Conjunction(goals) => EvalStep::Done(eval_conjunction(goals, env, depth)),
    }
}
