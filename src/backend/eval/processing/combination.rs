//! Single Combination Processing
//!
//! This module handles the processing of a single combination in the fast path
//! for deterministic evaluation.

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::super::step::ProcessedSExpr;
use super::super::{try_eval_builtin, try_match_all_rules};
use super::no_match::handle_no_rule_match;

/// Process a single combination (fast path for deterministic evaluation).
/// This avoids creating a continuation when there's only one combination to process.
///
/// MeTTa HE semantics: After evaluating arguments, TRY RULE MATCHING AGAIN.
/// This is essential for patterns like (intensity (color)) where:
/// 1. (intensity (color)) doesn't match any rule (lazy)
/// 2. Evaluate args: (color) → [red, green, blue]
/// 3. (intensity red), (intensity green), (intensity blue) NOW match intensity rules
pub fn process_single_combination(
    evaled_items: Vec<MettaValue>,
    unified_env: Environment,
    depth: usize,
) -> ProcessedSExpr {
    // Check if this is a grounded operation
    if let Some(MettaValue::Atom(op)) = evaled_items.first() {
        if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
            return ProcessedSExpr::Done((vec![result], unified_env));
        }
    }

    // MeTTa HE semantics: After argument evaluation, try rule matching AGAIN.
    // The newly-evaluated arguments may now match rules that didn't match before.
    // Example: (intensity (color)) → (intensity red) → 100
    let sexpr = MettaValue::SExpr(evaled_items.clone());
    let all_matches = try_match_all_rules(&sexpr, &unified_env);

    if !all_matches.is_empty() {
        // Rules match with evaluated arguments - evaluate the rule RHS
        return ProcessedSExpr::EvalRuleMatches {
            matches: all_matches,
            env: unified_env,
            depth,
            base_results: vec![],
        };
    }

    // No rules matched even with evaluated arguments - this is a data constructor.
    // Check for typos and emit helpful warnings
    let mut env = unified_env;
    let result = handle_no_rule_match(evaled_items, &sexpr, &mut env);
    ProcessedSExpr::Done((vec![result], env))
}
