//! Rule matching for MeTTa evaluation.
//!
//! This module handles finding and matching user-defined rules against expressions.
//! It supports both MORK-accelerated O(k) matching and fallback O(n) iteration.

use std::sync::Arc;
use tracing::trace;

use mork_expr::Expr;

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, MettaValue, Rule};
use crate::backend::mork_convert::{metta_to_mork_bytes, mork_bindings_to_metta, ConversionContext};

use super::helpers::{get_head_symbol, pattern_specificity};
use super::pattern::pattern_match;

/// Find ALL rules in the environment that match the given expression
/// Returns Vec<(rhs, bindings)> with all matching rules
/// RHS is Arc-wrapped for O(1) cloning
///
/// This function supports MeTTa's non-deterministic semantics where multiple rules
/// can match the same expression and all results should be returned.
pub fn try_match_all_rules(expr: &MettaValue, env: &Environment) -> Vec<(Arc<MettaValue>, Bindings)> {
    // Try MORK's query_multi first for O(k) matching where k = number of matching rules
    // Falls back to iterative O(n) matching if query_multi fails (e.g., arity >= 64)
    let query_multi_results = try_match_all_rules_query_multi(expr, env);
    if !query_multi_results.is_empty() {
        return query_multi_results;
    }

    // Fall back to iteration-based approach
    try_match_all_rules_iterative(expr, env)
}

/// Try pattern matching using MORK's query_multi to find ALL matching rules (O(k) where k = matching rules)
/// RHS is Arc-wrapped for O(1) cloning
pub fn try_match_all_rules_query_multi(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(Arc<MettaValue>, Bindings)> {
    trace!(target: "mettatron::backend::eval::try_match_all_rules_query_multi", ?expr);
    // Create a pattern that queries for rules: (= <expr-pattern> $rhs)
    // This will find all rules where the LHS matches our expression

    let space = env.create_space();

    // IMPORTANT: Use shared ConversionContext for pattern building AND binding conversion
    // This ensures variable names are properly registered and can be looked up later
    // FIX: Previously ctx was empty, causing mork_bindings_to_metta to fail
    let mut ctx = ConversionContext::new();

    // Build pattern as MettaValue: (= <expr> $rhs)
    let rhs_var = MettaValue::Atom("$rhs".to_string());
    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("=".to_string()),
        expr.clone(),
        rhs_var,
    ]);

    // Convert to MORK bytes using metta_to_mork_bytes with shared context
    // This registers the $rhs variable (and any variables in expr) in ctx
    let pattern_bytes = match metta_to_mork_bytes(&pattern, &space, &mut ctx) {
        Ok(bytes) => bytes,
        Err(_) => return Vec::new(), // Fallback if conversion fails
    };
    trace!(
        target: "mettatron::backend::eval::try_match_all_rules_query_multi",
        pattern_bytes_len = ?pattern_bytes.len(), "Convert expression to MORK format"
    );

    let pattern_expr = Expr {
        ptr: pattern_bytes.as_ptr().cast_mut(),
    };

    // Collect ALL matches using query_multi
    // Note: All matches from query_multi will have the same LHS pattern (since we're querying for it)
    // Therefore, they all have the same LHS specificity and we should return all of them
    let mut matches: Vec<(Arc<MettaValue>, Bindings)> = Vec::new();

    mork::space::Space::query_multi(&space.btm, pattern_expr, |result, _matched_expr| {
        if let Err(bindings) = result {
            // Convert MORK bindings to our format
            // The ctx now has variable names registered from metta_to_mork_bytes
            if let Ok(our_bindings) = mork_bindings_to_metta(&bindings, &ctx, &space) {
                // Extract the RHS from bindings - variable name is "rhs" (without $)
                if let Some((_, rhs)) = our_bindings
                    .iter()
                    .find(|(name, _)| name.as_str() == "rhs")
                {
                    matches.push((Arc::new(rhs.clone()), our_bindings));
                }
            }
        }
        true // Continue searching for ALL matches
    });
    trace!(
        target: "mettatron::backend::eval::try_match_all_rules_query_multi",
        matches_ctr = ?matches.len(), "Collected matches"
    );

    matches
    // space will be dropped automatically here
}

/// Optimized: Try pattern matching using indexed lookup to find ALL matching rules
/// Uses O(1) index lookup instead of O(n) iteration
/// Complexity: O(k) where k = rules with matching head symbol (typically k << n)
/// RHS is Arc-wrapped for O(1) cloning
pub fn try_match_all_rules_iterative(
    expr: &MettaValue,
    env: &Environment,
) -> Vec<(Arc<MettaValue>, Bindings)> {
    trace!(target: "mettatron::backend::eval::try_match_all_rules_iterative", ?expr);
    // Extract head symbol and arity for indexed lookup
    let matching_rules = if let Some(head) = get_head_symbol(expr) {
        let arity = expr.get_arity();
        // O(1) indexed lookup instead of O(n) iteration
        env.get_matching_rules(head, arity)
    } else {
        // For expressions without head symbol, check wildcard rules only
        // This is still O(k_wildcards) instead of O(n_total)
        env.get_matching_rules("", 0) // Empty head will return only wildcards
    };

    // Sort rules by specificity (more specific first)
    let mut sorted_rules = matching_rules;
    sorted_rules.sort_by_key(|rule| pattern_specificity(&rule.lhs));
    trace!(target: "mettatron::backend::eval::try_match_all_rules_iterative", ?sorted_rules);

    // Collect ALL matching rules, tracking LHS specificity
    // Keep Arc<MettaValue> from Rule struct for O(1) cloning
    let mut matches: Vec<(Arc<MettaValue>, Bindings, usize, Rule)> = Vec::new();
    for rule in sorted_rules {
        if let Some(bindings) = pattern_match(&rule.lhs, expr) {
            let lhs_specificity = pattern_specificity(&rule.lhs);
            // Use Arc::clone for O(1) cloning instead of deep copy
            matches.push((rule.rhs_arc(), bindings, lhs_specificity, rule));
        }
    }

    // Find the best (lowest) specificity
    if let Some(best_spec) = matches.iter().map(|(_, _, spec, _)| *spec).min() {
        // Filter to only matches with the best specificity
        let best_matches: Vec<_> = matches
            .into_iter()
            .filter(|(_, _, spec, _)| *spec == best_spec)
            .collect();

        // Duplicate results based on rule count
        let mut final_matches = Vec::new();
        for (rhs, bindings, _, rule) in best_matches {
            let count = env.get_rule_count(&rule);
            for _ in 0..count {
                // Arc::clone is O(1) - just increments reference count
                final_matches.push((Arc::clone(&rhs), bindings.clone()));
            }
        }

        trace!(target: "mettatron::backend::eval::try_match_all_rules_iterative", ?final_matches);
        final_matches
    } else {
        Vec::new()
    }
}
