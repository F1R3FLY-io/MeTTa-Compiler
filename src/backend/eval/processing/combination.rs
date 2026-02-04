//! Single Combination Processing
//!
//! This module handles the processing of a single combination in the fast path
//! for deterministic evaluation.

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::helpers::needs_special_form_redispatch;
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
    unified_env: HeapEnvironment,
    depth: usize,
) -> ProcessedSExpr {
    // Check if this is a grounded operation or special form
    if let Some(first) = evaled_items.first() {
        if let MettaValueInner::Atom(op) = first.inner() {
            // First, check for grounded operations (arithmetic, etc.)
            if let Some(result) = try_eval_builtin(op, &evaled_items[1..]) {
                return ProcessedSExpr::Done((vec![result], unified_env));
            }

            // Re-dispatch special forms through eval_sexpr_step.
            // This ensures map-atom, if, let, etc. get proper handling after
            // their arguments have been evaluated via Cartesian product.
            if needs_special_form_redispatch(op) {
                return ProcessedSExpr::RedispatchSExpr {
                    items: evaled_items,
                    env: unified_env,
                    depth,
                };
            }
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
    let result = handle_no_rule_match(evaled_items, &sexpr, &mut env, depth);
    ProcessedSExpr::Done((vec![result], env))
}
