//! No-Match Handler
//!
//! This module handles the case where no rule matches an s-expression,
//! providing helpful suggestions and implementing ADD mode.

#[cfg(feature = "fuzzy-suggestions")]
use tracing::trace;

use crate::backend::environment::Environment;
#[cfg(feature = "fuzzy-suggestions")]
use crate::backend::fuzzy_match::SuggestionConfidence;
use crate::backend::models::MettaValue;

#[cfg(feature = "fuzzy-suggestions")]
use super::super::suggest_special_form_with_context;

/// Handle the case where no rule matches an s-expression
///
/// When the `fuzzy-suggestions` feature is enabled, uses context-aware smart
/// suggestion heuristics (issue #51) to avoid false positives:
/// - **Arity filtering**: `(lit p)` won't suggest `let` (arity 1 != 3)
/// - **Type filtering**: `(match "hello" ...)` won't suggest match (String != Space)
/// - **Prefix compatibility**: `$steck` suggests `$stack`, not `&stack`
/// - Detects data constructor patterns (PascalCase, hyphenated names)
/// - Emits warnings (notes) instead of errors for suggestions
///
/// This allows intentional data constructors like `lit` to work without
/// triggering spurious "Did you mean: let?" errors.
///
/// **Performance Note**: Fuzzy suggestions are disabled by default because
/// they add 10-20% overhead (DynamicDawgChar query operations).
/// Enable with `--features fuzzy-suggestions` if you need typo detection.
#[cfg(feature = "fuzzy-suggestions")]
pub fn handle_no_rule_match(
    evaled_items: Vec<MettaValue>,
    sexpr: &MettaValue,
    unified_env: &mut Environment,
) -> MettaValue {
    // Check for likely typos before falling back to ADD mode
    // Only check atoms >= 3 chars to avoid false positives on short symbols
    // and reduce fuzzy matching overhead (which is O(n*m) for query)
    if let Some(MettaValue::Atom(head)) = evaled_items.first() {
        if head.len() >= 3 {
            // Check for misspelled special form using context-aware heuristics
            // The three-pillar validation filters out structurally incompatible suggestions
            if let Some(suggestion) =
                suggest_special_form_with_context(head, &evaled_items, unified_env)
            {
                trace!(
                    target: "mettatron::backend::eval::handle_no_rule_match",
                    head, ?suggestion, "Unknown special form"
                );
                // Always emit as a note/warning, never as an error
                // This allows the expression to continue evaluating in ADD mode
                match suggestion.confidence {
                    SuggestionConfidence::High => {
                        eprintln!(
                            "Warning: '{}' is not defined. {}",
                            head, suggestion.message
                        );
                    }
                    SuggestionConfidence::Low => {
                        eprintln!("Note: '{}' is not defined. {}", head, suggestion.message);
                    }
                    SuggestionConfidence::None => {
                        // No suggestion - don't print anything
                    }
                }
                // Fall through to ADD mode (don't return error)
            }

            // Check for misspelled rule head using smart heuristics
            // Use max_distance=1 to reduce false positives and improve performance
            if let Some(suggestion) = unified_env.smart_did_you_mean(head, 1) {
                trace!(
                    target: "mettatron::backend::eval::handle_no_rule_match",
                    head, ?suggestion, "No rule matches"
                );
                match suggestion.confidence {
                    SuggestionConfidence::High => {
                        eprintln!(
                            "Warning: No rule matches '{}'. {}",
                            head, suggestion.message
                        );
                    }
                    SuggestionConfidence::Low => {
                        eprintln!("Note: No rule matches '{}'. {}", head, suggestion.message);
                    }
                    SuggestionConfidence::None => {
                        // No suggestion - don't print anything
                    }
                }
                // Fall through to ADD mode (don't return error)
            }
        }
    }

    // ADD mode: add to space and return unreduced s-expression
    // In official MeTTa's default ADD mode, bare expressions are automatically added to &self
    unified_env.add_to_space(sexpr);
    sexpr.clone()
}

/// Handle the case where no rule matches an s-expression (fast path)
///
/// This is the optimized version without fuzzy suggestions.
/// For typo detection, enable the `fuzzy-suggestions` feature.
#[cfg(not(feature = "fuzzy-suggestions"))]
#[inline]
pub fn handle_no_rule_match(
    _evaled_items: Vec<MettaValue>,
    sexpr: &MettaValue,
    unified_env: &mut Environment,
) -> MettaValue {
    // ADD mode: add to space and return unreduced s-expression
    // In official MeTTa's default ADD mode, bare expressions are automatically added to &self
    unified_env.add_to_space(sexpr);
    sexpr.clone()
}
