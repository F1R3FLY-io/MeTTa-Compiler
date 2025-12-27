//! Suggestion generation methods for FuzzyMatcher.

use super::helpers::{
    are_prefixes_compatible, compute_suggestion_confidence, is_likely_data_constructor,
};
use super::matcher::FuzzyMatcher;
use super::types::{SmartSuggestion, SuggestionConfidence};

impl FuzzyMatcher {
    /// Generate a "Did you mean?" error message suggestion.
    ///
    /// Returns None if no suggestions are found within max_distance.
    ///
    /// # Arguments
    /// - `query`: The misspelled term
    /// - `max_distance`: Maximum edit distance (default: 2)
    /// - `max_suggestions`: Maximum number of suggestions to return (default: 3)
    ///
    /// # Example
    /// ```ignore
    /// let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "fib"]);
    /// let msg = matcher.did_you_mean("fibonaci", 2, 3);
    /// // Returns: Some("Did you mean: fibonacci?")
    /// ```
    pub fn did_you_mean(
        &self,
        query: &str,
        max_distance: usize,
        max_suggestions: usize,
    ) -> Option<String> {
        let suggestions = self.suggest(query, max_distance);

        if suggestions.is_empty() {
            return None;
        }

        // Filter out exact matches (distance 0) - if the term already exists,
        // suggesting "Did you mean: X?" where X is exactly the query is unhelpful
        let suggestion_list: Vec<String> = suggestions
            .into_iter()
            .filter(|(_, distance)| *distance > 0)
            .take(max_suggestions)
            .map(|(term, _)| term)
            .collect();

        if suggestion_list.is_empty() {
            return None;
        }

        if suggestion_list.len() == 1 {
            Some(format!("Did you mean: {}?", suggestion_list[0]))
        } else {
            Some(format!(
                "Did you mean one of: {}?",
                suggestion_list.join(", ")
            ))
        }
    }

    /// Smart "Did you mean?" with sophisticated heuristics to avoid false positives.
    ///
    /// This method applies multiple heuristics to determine if a suggestion is
    /// likely a typo vs an intentional data constructor name (issue #51):
    ///
    /// 1. **Relative distance**: Rejects if distance/min_len > 0.33
    ///    - `lit` → `let` (distance 1, len 3): 1/3 = 0.33 → REJECTED
    ///    - `fibonacci` → `fibonaci` (distance 1, len 8): 1/8 = 0.125 → ACCEPTED
    ///
    /// 2. **Minimum length for short distances**: For distance 1, requires query >= 4 chars
    ///    - `lit` (3 chars) → no distance-1 suggestions
    ///    - `lett` (4 chars) → distance-1 suggestions allowed
    ///
    /// 3. **Data constructor patterns**: Skip suggestions for PascalCase, hyphenated names
    ///    - `MyType`, `DataConstructor`, `is-valid` → skip suggestions
    ///
    /// 4. **Prefix type mismatch**: Don't suggest across identifier prefix boundaries
    ///    - `$x` vs `&x` → different semantics, skip
    ///
    /// Returns `(Option<String>, SuggestionConfidence)` where confidence indicates
    /// whether this should be shown as an error, warning, or not at all.
    pub fn smart_did_you_mean(
        &self,
        query: &str,
        max_distance: usize,
        max_suggestions: usize,
    ) -> Option<SmartSuggestion> {
        // Check if query looks like an intentional data constructor
        if is_likely_data_constructor(query) {
            return None;
        }

        let suggestions = self.suggest(query, max_distance);
        if suggestions.is_empty() {
            return None;
        }

        // Filter suggestions using sophisticated heuristics
        let query_len = query.chars().count();
        let filtered: Vec<(String, usize, SuggestionConfidence)> = suggestions
            .into_iter()
            .filter(|(_, distance)| *distance > 0) // No exact matches
            .filter_map(|(term, distance)| {
                // Check prefix type compatibility
                if !are_prefixes_compatible(query, &term) {
                    return None;
                }

                let confidence = compute_suggestion_confidence(query, &term, distance, query_len);
                match confidence {
                    SuggestionConfidence::None => None,
                    conf => Some((term, distance, conf)),
                }
            })
            .take(max_suggestions)
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
}
