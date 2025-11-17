//! Fuzzy string matching for "Did you mean?" suggestions.
//!
//! This module provides fuzzy matching capabilities using liblevenshtein's
//! Levenshtein automata for efficient approximate string matching.

use liblevenshtein::dictionary::pathmap::PathMapDictionary;
use liblevenshtein::dictionary::Dictionary; // Trait for contains()
use liblevenshtein::transducer::{Candidate, Transducer};
use std::sync::{Arc, Mutex};

/// Fuzzy matcher for symbol suggestions using Levenshtein distance.
///
/// Uses PathMapDictionary as the backend, which is compatible with
/// MeTTaTron's existing PathMap usage for MORK.
#[derive(Clone)]
pub struct FuzzyMatcher {
    dictionary: Arc<Mutex<PathMapDictionary<()>>>,
}

impl FuzzyMatcher {
    /// Create a new empty fuzzy matcher
    pub fn new() -> Self {
        Self {
            dictionary: Arc::new(Mutex::new(PathMapDictionary::new())),
        }
    }

    /// Create a fuzzy matcher from an iterator of terms
    pub fn from_terms<I, S>(terms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            dictionary: Arc::new(Mutex::new(PathMapDictionary::from_terms(terms))),
        }
    }

    /// Add a term to the dictionary
    pub fn insert(&self, term: &str) {
        let dict = self.dictionary.lock().unwrap();
        dict.insert(term);
    }

    /// Remove a term from the dictionary
    pub fn remove(&self, term: &str) -> bool {
        let dict = self.dictionary.lock().unwrap();
        dict.remove(term)
    }

    /// Check if a term exists in the dictionary
    pub fn contains(&self, term: &str) -> bool {
        let dict = self.dictionary.lock().unwrap();
        dict.contains(term)
    }

    /// Find similar terms within the given edit distance.
    ///
    /// Returns a vector of (term, distance) pairs sorted by distance.
    ///
    /// # Arguments
    /// - `query`: The term to find matches for
    /// - `max_distance`: Maximum Levenshtein distance (typically 1-2)
    ///
    /// # Example
    /// ```ignore
    /// let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);
    /// let suggestions = matcher.suggest("fibonaci", 2);
    /// // Returns: [("fibonacci", 1)]
    /// ```
    pub fn suggest(&self, query: &str, max_distance: usize) -> Vec<(String, usize)> {
        let dict = self.dictionary.lock().unwrap();
        let dict_clone = dict.clone(); // Clone to release lock before query
        drop(dict); // Explicitly drop to release lock

        // Use Transposition algorithm to catch common typos (e.g., "teh" -> "the")
        let transducer = Transducer::with_transposition(dict_clone);

        let mut results: Vec<(String, usize)> = transducer
            .query_with_distance(query, max_distance)
            .map(|candidate: Candidate| (candidate.term, candidate.distance))
            .collect();

        // Sort by distance (closest matches first), then alphabetically
        results.sort_by(|a, b| {
            a.1.cmp(&b.1) // Sort by distance first
                .then_with(|| a.0.cmp(&b.0)) // Then alphabetically
        });

        results
    }

    /// Find the closest match for a term (minimum edit distance).
    ///
    /// Returns None if no match is found within max_distance.
    ///
    /// # Example
    /// ```ignore
    /// let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);
    /// let closest = matcher.closest_match("fibonaci", 2);
    /// // Returns: Some(("fibonacci", 1))
    /// ```
    pub fn closest_match(&self, query: &str, max_distance: usize) -> Option<(String, usize)> {
        self.suggest(query, max_distance).into_iter().next()
    }

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

        let suggestion_list: Vec<String> = suggestions
            .into_iter()
            .take(max_suggestions)
            .map(|(term, _)| term)
            .collect();

        if suggestion_list.len() == 1 {
            Some(format!("Did you mean: {}?", suggestion_list[0]))
        } else {
            Some(format!(
                "Did you mean one of: {}?",
                suggestion_list.join(", ")
            ))
        }
    }

    /// Get the number of terms in the dictionary
    pub fn len(&self) -> usize {
        let dict = self.dictionary.lock().unwrap();
        dict.term_count()
    }

    /// Check if the dictionary is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_fuzzy_matching() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

        // Exact match (distance 0)
        assert!(matcher.contains("fibonacci"));

        // Single character substitution (distance 1)
        let suggestions = matcher.suggest("fibonaci", 2);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].0, "fibonacci");
        assert_eq!(suggestions[0].1, 1);
    }

    #[test]
    fn test_transposition_typos() {
        let matcher = FuzzyMatcher::from_terms(vec!["test", "testing"]);

        // Transposition: "tset" -> "test"
        let suggestions = matcher.suggest("tset", 1);
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].0, "test");
    }

    #[test]
    fn test_multiple_suggestions() {
        let matcher =
            FuzzyMatcher::from_terms(vec!["fibonacci", "fib", "fibonacci-fast", "factorial"]);

        // Should find multiple similar matches
        let suggestions = matcher.suggest("fibonaci", 2);
        assert!(!suggestions.is_empty());
        assert_eq!(suggestions[0].0, "fibonacci"); // Closest match first
    }

    #[test]
    fn test_closest_match() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

        let closest = matcher.closest_match("fibonaci", 2);
        assert!(closest.is_some());
        let (term, distance) = closest.unwrap();
        assert_eq!(term, "fibonacci");
        assert_eq!(distance, 1);
    }

    #[test]
    fn test_did_you_mean_single() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

        let msg = matcher.did_you_mean("fibonaci", 2, 3);
        assert_eq!(msg, Some("Did you mean: fibonacci?".to_string()));
    }

    #[test]
    fn test_did_you_mean_multiple() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "fib", "fib-fast"]);

        // "fob" -> "fib" has distance 1 (substitute o->i)
        let suggestions = matcher.suggest("fob", 1);
        // Should find at least "fib"
        assert!(!suggestions.is_empty(), "Expected at least one suggestion");

        let msg = matcher.did_you_mean("fob", 1, 3);
        assert!(msg.is_some());
        // If we only found one match, it will say "Did you mean: X?"
        // If we found multiple, it will say "Did you mean one of: X, Y?"
        let msg_str = msg.unwrap();
        assert!(
            msg_str.starts_with("Did you mean:") || msg_str.starts_with("Did you mean one of:"),
            "Unexpected message format: {}",
            msg_str
        );
    }

    #[test]
    fn test_did_you_mean_no_match() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

        let msg = matcher.did_you_mean("xyz", 1, 3);
        assert_eq!(msg, None);
    }

    #[test]
    fn test_insert_and_remove() {
        let matcher = FuzzyMatcher::new();
        assert_eq!(matcher.len(), 0);

        matcher.insert("test");
        assert_eq!(matcher.len(), 1);
        assert!(matcher.contains("test"));

        let removed = matcher.remove("test");
        assert!(removed);
        assert_eq!(matcher.len(), 0);
    }

    #[test]
    fn test_empty_dictionary() {
        let matcher = FuzzyMatcher::new();
        assert!(matcher.is_empty());

        let suggestions = matcher.suggest("anything", 2);
        assert!(suggestions.is_empty());
    }
}
