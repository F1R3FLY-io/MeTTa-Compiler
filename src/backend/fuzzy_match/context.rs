//! Context-aware suggestion methods for FuzzyMatcher.

use crate::backend::builtin_signatures::{get_arg_types, get_signature, TypeExpr};
use crate::backend::Environment;

use super::helpers::{
    are_prefixes_compatible, compute_suggestion_confidence, is_likely_data_constructor,
    type_matches, validate_type_vars,
};
use super::matcher::FuzzyMatcher;
use super::types::{SmartSuggestion, SuggestionConfidence, SuggestionContext};

impl FuzzyMatcher {
    /// Context-aware smart suggestion with structural, type, and arity validation.
    ///
    /// This method implements the three pillars of smart recommendations:
    ///
    /// 1. **Arity Compatibility**: Expression arity must match candidate's min/max
    /// 2. **Type Compatibility**: Argument types must match expected types
    /// 3. **Context Compatibility**: Position-aware prefix suggestions
    ///
    /// # Arguments
    /// - `query`: The unknown symbol to find matches for
    /// - `max_distance`: Maximum Levenshtein distance
    /// - `context`: Context information about where the symbol appears
    ///
    /// # Example
    /// ```ignore
    /// // (lit p) - 1 arg, should NOT suggest 'let' (needs 3 args)
    /// let ctx = SuggestionContext::for_head(&expr, &env);
    /// let suggestion = matcher.smart_suggest_with_context("lit", 2, &ctx);
    /// assert!(suggestion.is_none()); // Filtered by arity
    /// ```
    pub fn smart_suggest_with_context(
        &self,
        query: &str,
        max_distance: usize,
        context: &SuggestionContext,
    ) -> Option<SmartSuggestion> {
        // 1. Check for context-specific prefix recommendations
        if let Some(suggestion) = self.check_prefix_context(query, context) {
            return Some(suggestion);
        }

        // 2. Check if query looks like an intentional data constructor
        if is_likely_data_constructor(query) {
            return None;
        }

        // 3. Get raw fuzzy matches
        let suggestions = self.suggest(query, max_distance);
        if suggestions.is_empty() {
            return None;
        }

        let query_len = query.chars().count();
        let arity = context.arity();

        // 4. Filter by all three pillars
        let filtered: Vec<(String, usize, SuggestionConfidence)> = suggestions
            .into_iter()
            .filter(|(_, distance)| *distance > 0) // No exact matches
            .filter_map(|(term, distance)| {
                // Pillar 1: Check prefix type compatibility
                if !are_prefixes_compatible(query, &term) {
                    return None;
                }

                // Pillar 2: Check arity compatibility (for built-ins)
                if !self.is_arity_compatible(&term, arity) {
                    return None;
                }

                // Pillar 3: Check type compatibility (for built-ins)
                if !self.is_type_compatible(&term, context) {
                    return None;
                }

                let confidence = compute_suggestion_confidence(query, &term, distance, query_len);
                match confidence {
                    SuggestionConfidence::None => None,
                    conf => Some((term, distance, conf)),
                }
            })
            .take(3) // Limit suggestions
            .collect();

        if filtered.is_empty() {
            return None;
        }

        // Determine overall confidence (highest among suggestions)
        let overall_confidence = filtered
            .iter()
            .map(|(_, _, conf)| *conf)
            .max()
            .unwrap_or(SuggestionConfidence::None);

        let terms: Vec<String> = filtered.into_iter().map(|(t, _, _)| t).collect();

        let message = if terms.len() == 1 {
            format!("Did you mean: {}?", terms[0])
        } else {
            format!("Did you mean one of: {}?", terms.join(", "))
        };

        Some(SmartSuggestion {
            message,
            confidence: overall_confidence,
            suggestions: terms,
        })
    }

    /// Check if a candidate's arity is compatible with the expression's arity.
    ///
    /// For built-ins, the expression arity must fall within [min_arity, max_arity].
    /// For non-builtins, always returns true (no signature to check against).
    pub(super) fn is_arity_compatible(&self, candidate: &str, arity: usize) -> bool {
        let Some(sig) = get_signature(candidate) else {
            return true; // Non-builtins pass (no signature to check)
        };

        arity >= sig.min_arity && arity <= sig.max_arity
    }

    /// Check if argument types are compatible with a candidate's type signature.
    ///
    /// Uses simple structural type matching. For built-ins with known signatures,
    /// checks each argument position against the expected type.
    pub(super) fn is_type_compatible(&self, candidate: &str, ctx: &SuggestionContext) -> bool {
        let Some(sig) = get_signature(candidate) else {
            return true; // Non-builtins pass
        };

        let Some(arg_types) = get_arg_types(&sig.type_sig) else {
            return true; // Non-arrow signatures pass
        };

        let args = if ctx.expr.len() > 1 {
            &ctx.expr[1..]
        } else {
            return true; // No args to check
        };

        // Check each argument against expected type
        for (i, expected_type) in arg_types.iter().enumerate() {
            if i >= args.len() {
                break;
            }
            if !type_matches(&args[i], expected_type, ctx.env) {
                return false;
            }
        }

        // Also validate type variable consistency (e.g., if branches must match)
        validate_type_vars(args, arg_types, ctx.env)
    }

    /// Check for context-specific prefix suggestions.
    ///
    /// For example, in `(match self ...)`, if `self` appears at position 1,
    /// suggest `&self` because match expects a space reference.
    pub(super) fn check_prefix_context(
        &self,
        query: &str,
        ctx: &SuggestionContext,
    ) -> Option<SmartSuggestion> {
        // Only check if we have a parent head and we're in an argument position
        let parent = ctx.parent_head?;

        // Check if this position expects a Space type
        if let Some(sig) = get_signature(parent) {
            if let Some(arg_types) = get_arg_types(&sig.type_sig) {
                // Position in signature (0-indexed in context, but args start at index 1)
                let sig_pos = ctx.position.saturating_sub(1);
                if let Some(TypeExpr::Space) = arg_types.get(sig_pos) {
                    // This position expects a Space
                    if !query.starts_with('&') && !query.starts_with('$') {
                        // Suggest adding & prefix
                        let suggested = format!("&{}", query);
                        return Some(SmartSuggestion {
                            message: format!(
                                "Did you mean: {}? ({} expects a space reference at position {})",
                                suggested,
                                parent,
                                ctx.position
                            ),
                            confidence: SuggestionConfidence::High,
                            suggestions: vec![suggested],
                        });
                    }
                }
            }
        }

        None
    }
}
