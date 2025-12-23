//! Fuzzy matching suggestions for Environment.
//!
//! Provides "Did you mean?" functionality for undefined symbols using Levenshtein distance.

use super::Environment;
use crate::backend::fuzzy_match::SmartSuggestion;

impl Environment {
    /// Get fuzzy suggestions for a potentially misspelled symbol
    ///
    /// Returns a list of (symbol, distance) pairs sorted by Levenshtein distance.
    ///
    /// # Arguments
    /// - `query`: The symbol to find matches for (e.g., "fibonaci")
    /// - `max_distance`: Maximum edit distance (typically 1-2)
    ///
    /// # Example
    /// ```ignore
    /// let suggestions = env.suggest_similar_symbols("fibonaci", 2);
    /// // Returns: [("fibonacci", 1)]
    /// ```
    pub fn suggest_similar_symbols(&self, query: &str, max_distance: usize) -> Vec<(String, usize)> {
        self.shared
            .fuzzy_matcher
            .read()
            .expect("fuzzy_matcher lock poisoned")
            .suggest(query, max_distance)
    }

    /// Generate a "Did you mean?" error message for an undefined symbol
    ///
    /// Returns None if no suggestions are found within max_distance.
    ///
    /// # Arguments
    /// - `symbol`: The undefined symbol
    /// - `max_distance`: Maximum edit distance (default: 2)
    ///
    /// # Example
    /// ```ignore
    /// if let Some(msg) = env.did_you_mean("fibonaci", 2) {
    ///     eprintln!("Error: Undefined symbol 'fibonaci'. {}", msg);
    /// }
    /// // Prints: "Error: Undefined symbol 'fibonaci'. Did you mean: fibonacci?"
    /// ```
    pub fn did_you_mean(&self, symbol: &str, max_distance: usize) -> Option<String> {
        self.shared
            .fuzzy_matcher
            .read()
            .expect("fuzzy_matcher lock poisoned")
            .did_you_mean(symbol, max_distance, 3)
    }

    /// Get a smart "Did you mean?" suggestion with sophisticated heuristics
    ///
    /// Unlike `did_you_mean`, this method applies heuristics to avoid false positives:
    /// - Rejects suggestions for short words (< 4 chars for distance 1)
    /// - Detects data constructor patterns (PascalCase, hyphenated names)
    /// - Considers relative edit distance (distance/length ratio)
    /// - Returns confidence level for appropriate error/warning handling
    ///
    /// # Returns
    /// - `Some(SmartSuggestion)` with message and confidence level
    /// - `None` if no appropriate suggestion is found
    ///
    /// # Example
    /// ```ignore
    /// if let Some(suggestion) = env.smart_did_you_mean("fibonaci", 2) {
    ///     match suggestion.confidence {
    ///         SuggestionConfidence::High => eprintln!("Warning: {}", suggestion.message),
    ///         SuggestionConfidence::Low => eprintln!("Note: {}", suggestion.message),
    ///         SuggestionConfidence::None => {} // Don't show anything
    ///     }
    /// }
    /// ```
    pub fn smart_did_you_mean(&self, symbol: &str, max_distance: usize) -> Option<SmartSuggestion> {
        self.shared
            .fuzzy_matcher
            .read()
            .expect("fuzzy_matcher lock poisoned")
            .smart_did_you_mean(symbol, max_distance, 3)
    }
}
