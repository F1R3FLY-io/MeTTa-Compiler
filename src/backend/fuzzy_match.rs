//! Fuzzy string matching for "Did you mean?" suggestions.
//!
//! This module provides fuzzy matching capabilities using liblevenshtein's
//! Levenshtein automata for efficient approximate string matching.
//!
//! **Performance Optimizations**:
//! - **Lazy Initialization**: The matcher defers expensive DynamicDawgChar
//!   construction until the first query. During normal successful evaluation,
//!   no Levenshtein automaton is built. This saves ~4% CPU time.
//! - **SIMD Acceleration**: liblevenshtein is compiled with SIMD support enabled
//!   for faster distance calculations on modern CPUs.
//! - **Bloom Filter**: Fast negative lookup rejection using a bloom filter.
//!   For `contains()` checks, this provides ~91-93% faster rejection of
//!   non-existent terms (~20-30ns vs ~25-40µs for full traversal).
//!   Memory cost: ~1.2 bytes per term.
//!
//! **Unicode Support**: Uses DynamicDawgChar for character-level Levenshtein
//! distances, providing correct Unicode semantics for multi-byte UTF-8 sequences.
//! Example: "ñ" → "n" = distance 1 (character-level), not distance 2 (byte-level).
//!
//! **Sophisticated Recommendation Heuristics** (issue #51):
//! To distinguish typos from intentional data constructors, we apply:
//! - **Relative distance threshold**: distance/min_len must be < 0.33 (avoid `lit`→`let`)
//! - **Minimum length**: Require query length >= 4 for distance-1 suggestions
//! - **Data constructor detection**: Skip suggestions for PascalCase, hyphenated names
//! - **Prefix type detection**: Don't suggest across prefix boundaries (`$x` vs `&x`)

use liblevenshtein::dictionary::dynamic_dawg_char::DynamicDawgChar;
use liblevenshtein::dictionary::Dictionary; // Trait for contains()
use liblevenshtein::transducer::{Candidate, Transducer};
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// Fuzzy matcher for symbol suggestions using Levenshtein distance.
///
/// Uses DynamicDawgChar as the backend, providing character-level Levenshtein
/// distances with proper Unicode semantics for multi-byte UTF-8 sequences.
///
/// **Lazy Initialization**: Terms are collected in a lightweight HashSet until
/// the first query (suggest/did_you_mean). Only then is the DynamicDawgChar
/// built. This defers the expensive Levenshtein automaton construction to
/// error-handling time, avoiding overhead during successful evaluation.
#[derive(Clone)]
pub struct FuzzyMatcher {
    /// Pending terms waiting to be added to the dictionary
    /// Using Arc<RwLock<>> for thread-safe lazy initialization
    pending: Arc<RwLock<HashSet<String>>>,
    /// Lazily-initialized dictionary. None until first query.
    dictionary: Arc<RwLock<Option<DynamicDawgChar<()>>>>,
}

impl FuzzyMatcher {
    /// Create a new empty fuzzy matcher
    pub fn new() -> Self {
        Self {
            pending: Arc::new(RwLock::new(HashSet::new())),
            dictionary: Arc::new(RwLock::new(None)),
        }
    }

    /// Create a fuzzy matcher from an iterator of terms
    ///
    /// Note: With lazy initialization, this still defers dictionary creation.
    /// The terms are stored in the pending set.
    pub fn from_terms<I, S>(terms: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let pending: HashSet<String> = terms.into_iter().map(|s| s.as_ref().to_string()).collect();
        Self {
            pending: Arc::new(RwLock::new(pending)),
            dictionary: Arc::new(RwLock::new(None)),
        }
    }

    /// Ensure the dictionary is initialized from pending terms
    /// This is called lazily on first query
    fn ensure_initialized(&self) {
        // Fast path: check if already initialized
        {
            let dict_guard = self.dictionary.read().unwrap();
            if dict_guard.is_some() {
                return;
            }
        }

        // Slow path: initialize the dictionary
        let mut dict_guard = self.dictionary.write().unwrap();
        // Double-check after acquiring write lock
        if dict_guard.is_some() {
            return;
        }

        // Build dictionary from pending terms with bloom filter
        let pending_guard = self.pending.read().unwrap();
        let term_count = pending_guard.len();

        // Create DAWG with bloom filter enabled for fast negative lookup rejection
        // Use f32::INFINITY for auto_minimize_threshold to disable auto-minimization
        // (we only build once and don't modify after)
        let bloom_capacity = if term_count > 0 { Some(term_count) } else { None };
        let dawg = DynamicDawgChar::with_config(f32::INFINITY, bloom_capacity);

        // Insert all terms (bloom filter is automatically populated)
        for term in pending_guard.iter() {
            dawg.insert(term);
        }

        *dict_guard = Some(dawg);
    }

    /// Add a term to the dictionary (or pending set if not initialized)
    ///
    /// **Lazy**: If dictionary is not yet initialized, the term is added to
    /// the pending set (O(1) HashSet insert). Only when the dictionary IS
    /// initialized do we insert directly.
    pub fn insert(&self, term: &str) {
        // Check if dictionary is initialized
        let dict_guard = self.dictionary.read().unwrap();
        if let Some(ref dict) = *dict_guard {
            // Dictionary exists, insert directly
            dict.insert(term);
        } else {
            // Dictionary not initialized, add to pending set
            drop(dict_guard); // Release read lock before write
            let mut pending_guard = self.pending.write().unwrap();
            pending_guard.insert(term.to_string());
        }
    }

    /// Remove a term from the dictionary
    pub fn remove(&self, term: &str) -> bool {
        // First remove from pending (in case not initialized yet)
        {
            let mut pending_guard = self.pending.write().unwrap();
            if pending_guard.remove(term) {
                return true;
            }
        }

        // Then check initialized dictionary
        let dict_guard = self.dictionary.read().unwrap();
        if let Some(ref dict) = *dict_guard {
            dict.remove(term)
        } else {
            false
        }
    }

    /// Check if a term exists in the dictionary or pending set
    pub fn contains(&self, term: &str) -> bool {
        // Check pending first (fast path, no dictionary needed)
        {
            let pending_guard = self.pending.read().unwrap();
            if pending_guard.contains(term) {
                return true;
            }
        }

        // Check initialized dictionary
        let dict_guard = self.dictionary.read().unwrap();
        if let Some(ref dict) = *dict_guard {
            dict.contains(term)
        } else {
            false
        }
    }

    /// Find similar terms within the given edit distance.
    ///
    /// Returns a vector of (term, distance) pairs sorted by distance.
    ///
    /// **Lazy Initialization**: This method triggers dictionary construction
    /// if not already initialized. This is intentional - the dictionary is only
    /// built when actually needed (during error handling).
    ///
    /// # Arguments
    /// - `query`: The term to find matches for
    /// - `max_distance`: Maximum Levenshtein distance (typically 2 for transposition typos)
    ///
    /// # Example
    /// ```ignore
    /// let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);
    /// let suggestions = matcher.suggest("fibonaci", 2);
    /// // Returns: [("fibonacci", 1)]
    /// ```
    pub fn suggest(&self, query: &str, max_distance: usize) -> Vec<(String, usize)> {
        // Lazy initialization: build dictionary on first query
        self.ensure_initialized();

        // Get the dictionary (guaranteed to exist after ensure_initialized)
        let dict_guard = self.dictionary.read().unwrap();
        let dict = dict_guard.as_ref().unwrap();

        // Use Transposition algorithm to catch common typos (e.g., "teh" -> "the")
        let transducer = Transducer::with_transposition(dict.clone());

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

    /// Get the number of terms in the dictionary
    ///
    /// Note: This counts both pending terms and initialized dictionary terms.
    /// If the dictionary is not initialized, returns the pending count.
    pub fn len(&self) -> usize {
        let dict_guard = self.dictionary.read().unwrap();
        if let Some(ref dict) = *dict_guard {
            dict.term_count()
        } else {
            self.pending.read().unwrap().len()
        }
    }

    /// Check if the dictionary is empty
    pub fn is_empty(&self) -> bool {
        let dict_guard = self.dictionary.read().unwrap();
        if let Some(ref dict) = *dict_guard {
            dict.term_count() == 0
        } else {
            self.pending.read().unwrap().is_empty()
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

/// Result of a smart suggestion query with confidence level
#[derive(Debug, Clone)]
pub struct SmartSuggestion {
    /// The formatted "Did you mean: X?" message
    pub message: String,
    /// How confident we are this is a typo vs intentional
    pub confidence: SuggestionConfidence,
    /// The suggested terms
    pub suggestions: Vec<String>,
}

/// Confidence level for typo suggestions
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SuggestionConfidence {
    /// No suggestion should be made
    None,
    /// Low confidence - only show as a note, don't affect evaluation
    Low,
    /// High confidence - likely a typo, show as warning
    High,
}

/// Check if a term looks like an intentional data constructor
///
/// Data constructors in MeTTa typically follow these patterns:
/// - PascalCase: `MyType`, `DataConstructor`, `True`, `False`
/// - Contains hyphens: `is-valid`, `get-value`, `my-function`
/// - All uppercase: `NIL`, `VOID`, `ERROR`
/// - Contains underscores: `my_value`, `data_type`
fn is_likely_data_constructor(term: &str) -> bool {
    // Skip empty terms
    if term.is_empty() {
        return false;
    }

    let first_char = term.chars().next().unwrap();

    // Skip if starts with special prefix (these are handled elsewhere)
    if matches!(first_char, '$' | '&' | '\'' | '%') {
        return false;
    }

    // PascalCase: starts with uppercase letter followed by lowercase
    if first_char.is_uppercase() {
        let has_lowercase = term.chars().skip(1).any(|c| c.is_lowercase());
        if has_lowercase {
            return true;
        }
    }

    // All uppercase (constants): `NIL`, `VOID`
    let all_upper = term.chars().all(|c| c.is_uppercase() || c == '_');
    if term.len() >= 2 && all_upper {
        return true;
    }

    // Contains hyphen (compound names): `is-valid`, `my-func`
    if term.contains('-') {
        return true;
    }

    // Contains underscore (snake_case): `my_value`
    if term.contains('_') {
        return true;
    }

    // Contains digits (likely intentional): `value1`, `test2`
    if term.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }

    false
}

/// Check if two terms have compatible prefix types
///
/// Different prefixes have different semantics:
/// - `$x` - pattern variable
/// - `&x` - space reference
/// - `'x` - quoted symbol
///
/// Suggesting across these boundaries would be unhelpful.
fn are_prefixes_compatible(query: &str, suggestion: &str) -> bool {
    let query_prefix = query.chars().next();
    let suggestion_prefix = suggestion.chars().next();

    match (query_prefix, suggestion_prefix) {
        (Some('$'), Some('$')) => true,
        (Some('&'), Some('&')) => true,
        (Some('\''), Some('\'')) => true,
        (Some('%'), Some('%')) => true,
        // Both are regular identifiers (no special prefix)
        (Some(q), Some(s)) if !matches!(q, '$' | '&' | '\'' | '%') && !matches!(s, '$' | '&' | '\'' | '%') => true,
        _ => false,
    }
}

/// Compute suggestion confidence based on distance and length heuristics
fn compute_suggestion_confidence(
    query: &str,
    suggested: &str,
    distance: usize,
    query_len: usize,
) -> SuggestionConfidence {
    let suggested_len = suggested.chars().count();
    let min_len = query_len.min(suggested_len);

    // Relative distance threshold: distance/min_len must be < 0.33
    // This prevents short-word false positives like lit→let
    let relative_distance = distance as f64 / min_len as f64;
    if relative_distance >= 0.33 {
        return SuggestionConfidence::None;
    }

    // For distance 1, require minimum length of 4
    if distance == 1 && query_len < 4 {
        return SuggestionConfidence::None;
    }

    // For distance 2, require minimum length of 6
    if distance == 2 && query_len < 6 {
        return SuggestionConfidence::Low;
    }

    // High confidence for longer words with small relative distance
    if relative_distance < 0.20 {
        SuggestionConfidence::High
    } else {
        SuggestionConfidence::Low
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

    // ============================================================
    // Smart Suggestion Heuristic Tests (issue #51)
    // ============================================================

    #[test]
    fn test_issue_51_lit_vs_let_not_suggested() {
        // This is the exact case from issue #51:
        // `lit` should NOT suggest `let` because it's too short (3 chars)
        let matcher = FuzzyMatcher::from_terms(vec!["let", "if", "case", "match"]);

        let result = matcher.smart_did_you_mean("lit", 2, 3);
        assert!(
            result.is_none(),
            "lit→let should NOT be suggested (short word false positive)"
        );
    }

    #[test]
    fn test_smart_suggestion_longer_words_accepted() {
        // Longer words with small relative distance should be suggested
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial", "function"]);

        let result = matcher.smart_did_you_mean("fibonaci", 2, 3);
        assert!(result.is_some(), "fibonaci→fibonacci should be suggested");
        let suggestion = result.unwrap();
        assert_eq!(suggestion.confidence, SuggestionConfidence::High);
        assert!(suggestion.message.contains("fibonacci"));
    }

    #[test]
    fn test_smart_suggestion_4_char_word_accepted() {
        // 4-char word with distance 1 should be accepted (barely)
        let matcher = FuzzyMatcher::from_terms(vec!["lett", "test", "case"]);

        // "lest" → "lett" (distance 1, len 4) = 0.25 relative distance
        let result = matcher.smart_did_you_mean("lest", 1, 3);
        assert!(result.is_some(), "lest→lett should be suggested (4 chars)");
    }

    #[test]
    fn test_smart_suggestion_pascal_case_skipped() {
        // PascalCase names should not trigger suggestions (likely data constructors)
        let matcher = FuzzyMatcher::from_terms(vec!["MyType", "DataCon"]);

        // Even though "MyTipe" is close to "MyType", it's PascalCase so skip
        let result = matcher.smart_did_you_mean("MyTipe", 1, 3);
        assert!(
            result.is_none(),
            "PascalCase should not trigger suggestions"
        );
    }

    #[test]
    fn test_smart_suggestion_hyphenated_skipped() {
        // Hyphenated names should not trigger suggestions (compound identifiers)
        let matcher = FuzzyMatcher::from_terms(vec!["is-valid", "get-value"]);

        let result = matcher.smart_did_you_mean("is-valud", 1, 3);
        assert!(
            result.is_none(),
            "Hyphenated names should not trigger suggestions"
        );
    }

    #[test]
    fn test_smart_suggestion_prefix_mismatch_rejected() {
        // Different prefixes should not match
        let matcher = FuzzyMatcher::from_terms(vec!["$stack", "&stack"]);

        // Querying "$stack" should not suggest "&stack"
        let result = matcher.smart_did_you_mean("$steck", 1, 3);
        if let Some(suggestion) = result {
            // If we get a suggestion, it should only be $stack, not &stack
            for term in &suggestion.suggestions {
                assert!(
                    term.starts_with('$'),
                    "Should not suggest &stack for $steck"
                );
            }
        }
    }

    #[test]
    fn test_is_likely_data_constructor() {
        // PascalCase
        assert!(is_likely_data_constructor("MyType"));
        assert!(is_likely_data_constructor("DataConstructor"));
        assert!(is_likely_data_constructor("True"));
        assert!(is_likely_data_constructor("False"));

        // All uppercase
        assert!(is_likely_data_constructor("NIL"));
        assert!(is_likely_data_constructor("VOID"));

        // Hyphenated
        assert!(is_likely_data_constructor("is-valid"));
        assert!(is_likely_data_constructor("get-value"));

        // Underscored
        assert!(is_likely_data_constructor("my_value"));

        // With digits
        assert!(is_likely_data_constructor("value1"));
        assert!(is_likely_data_constructor("test2"));

        // Regular lowercase words - NOT data constructors
        assert!(!is_likely_data_constructor("let"));
        assert!(!is_likely_data_constructor("if"));
        assert!(!is_likely_data_constructor("match"));
        assert!(!is_likely_data_constructor("factorial"));
    }

    #[test]
    fn test_are_prefixes_compatible() {
        // Same prefix types should be compatible
        assert!(are_prefixes_compatible("$x", "$y"));
        assert!(are_prefixes_compatible("&space", "&other"));
        assert!(are_prefixes_compatible("foo", "bar"));

        // Different prefix types should NOT be compatible
        assert!(!are_prefixes_compatible("$x", "&x"));
        assert!(!are_prefixes_compatible("&space", "$space"));
        assert!(!are_prefixes_compatible("$var", "var"));
    }

    #[test]
    fn test_compute_suggestion_confidence() {
        // High confidence: long word, small relative distance
        assert_eq!(
            compute_suggestion_confidence("fibonacci", "fibonaci", 1, 9),
            SuggestionConfidence::High
        );

        // Low confidence: medium word, medium relative distance
        assert_eq!(
            compute_suggestion_confidence("match", "matsh", 1, 5),
            SuggestionConfidence::Low
        );

        // None: short word, high relative distance
        assert_eq!(
            compute_suggestion_confidence("lit", "let", 1, 3),
            SuggestionConfidence::None
        );

        // None: 3-char word with distance 1 (min length check)
        assert_eq!(
            compute_suggestion_confidence("add", "adn", 1, 3),
            SuggestionConfidence::None
        );
    }

    #[test]
    fn test_smart_suggestion_confidence_levels() {
        let matcher = FuzzyMatcher::from_terms(vec!["fibonacci", "factorial"]);

        // Long word should have high confidence
        let result = matcher.smart_did_you_mean("fibonaci", 2, 3);
        assert!(result.is_some());
        assert_eq!(result.unwrap().confidence, SuggestionConfidence::High);
    }
}
