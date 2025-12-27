//! S-Expression Result Collection Processing
//!
//! This module handles the processing of collected S-expression evaluation results,
//! including Cartesian product generation and rule matching.

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::step::ProcessedSExpr;
use super::super::{cartesian_product_lazy, CartesianProductResult};
use super::combination::process_single_combination;

/// Process collected S-expression evaluation results.
/// This handles Cartesian products, builtins, and rule matching.
/// Uses lazy Cartesian product for memory-efficient nondeterministic evaluation.
pub fn process_collected_sexpr(
    collected: Vec<EvalResult>,
    original_env: Environment,
    depth: usize,
) -> ProcessedSExpr {
    trace!(target: "mettatron::backend::eval::process_collected_sexpr", ?collected, depth);

    // Check for errors in sub-expression results
    for (results, new_env) in &collected {
        if let Some(first) = results.first() {
            if matches!(first, MettaValue::Error(_, _)) {
                return ProcessedSExpr::Done((vec![first.clone()], new_env.clone()));
            }
        }
    }

    // Split results and environments
    let (eval_results, envs): (Vec<_>, Vec<_>) = collected.into_iter().unzip();

    // Union all environments
    let mut unified_env = original_env;
    for e in envs {
        unified_env = unified_env.union(&e);
    }

    // Generate lazy Cartesian product of all sub-expression results
    match cartesian_product_lazy(eval_results) {
        CartesianProductResult::Empty => {
            // No combinations possible (empty result list)
            ProcessedSExpr::Done((vec![], unified_env))
        }
        CartesianProductResult::Single(evaled_items) => {
            // FAST PATH: Single combination (deterministic evaluation)
            // Process it directly without creating continuation
            // Convert SmallVec to Vec for downstream functions
            process_single_combination(evaled_items.into_vec(), unified_env, depth)
        }
        CartesianProductResult::Lazy(combinations) => {
            // LAZY PATH: Multiple combinations - process via continuation
            ProcessedSExpr::EvalCombinations {
                combinations,
                env: unified_env,
                depth,
            }
        }
    }
}
