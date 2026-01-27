//! Core FuzzyMatcher implementation.

use dashmap::DashSet;
use liblevenshtein::dictionary::dynamic_dawg_char::DynamicDawgChar;
use liblevenshtein::transducer::{Candidate, Transducer};
use std::sync::OnceLock;

/// Fuzzy matcher for symbol suggestions using Levenshtein distance.
///
/// Uses DynamicDawgChar as the backend, providing character-level Levenshtein
/// distances with proper Unicode semantics for multi-byte UTF-8 sequences.
///
/// **Lazy Initialization**: Terms are collected in a lock-free DashSet until
/// the first query (suggest/did_you_mean). Only then is the DynamicDawgChar
/// built. This defers the expensive Levenshtein automaton construction to
/// error-handling time, avoiding overhead during successful evaluation.
///
/// **Lock-Free Design**:
/// - `pending`: DashSet provides lock-free concurrent inserts
/// - `dictionary`: OnceLock provides safe one-time initialization
pub struct FuzzyMatcher {
    /// Pending terms waiting to be added to the dictionary (lock-free DashSet)
    pub(super) pending: DashSet<String>,
    /// Lazily-initialized dictionary (one-time init via OnceLock)
    pub(super) dictionary: OnceLock<DynamicDawgChar<()>>,
}

/// Manual Clone implementation for deep cloning with CoW semantics.
///
/// Creates a new FuzzyMatcher with:
/// 1. A deep clone of the pending DashSet
/// 2. An uninitialized OnceLock (dictionary will be rebuilt lazily on first query)
///
/// Note: Terms added after the source dictionary was initialized won't be included
/// in the clone's pending set, but this is acceptable for CoW semantics since
/// clones are made before mutation.
impl Clone for FuzzyMatcher {
    fn clone(&self) -> Self {
        // Collect all terms from pending DashSet
        let new_pending: DashSet<String> = DashSet::new();
        for term in self.pending.iter() {
            new_pending.insert(term.clone());
        }

        Self {
            pending: new_pending,
            dictionary: OnceLock::new(), // Reset - rebuild lazily
        }
    }
}

impl FuzzyMatcher {
    /// Create a new empty fuzzy matcher
    pub fn new() -> Self {
        Self {
            pending: DashSet::new(),
            dictionary: OnceLock::new(),
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
        let pending: DashSet<String> = DashSet::new();
        for s in terms {
            pending.insert(s.as_ref().to_string());
        }
        Self {
            pending,
            dictionary: OnceLock::new(),
        }
    }

    /// Ensure the dictionary is initialized from pending terms.
    /// Uses OnceLock::get_or_init for clean one-time initialization.
    fn get_dictionary(&self) -> &DynamicDawgChar<()> {
        self.dictionary.get_or_init(|| {
            let term_count = self.pending.len();

            // Create DAWG with bloom filter enabled for fast negative lookup rejection
            // Use f32::INFINITY for auto_minimize_threshold to disable auto-minimization
            // (we only build once and don't modify after)
            let bloom_capacity = if term_count > 0 {
                Some(term_count)
            } else {
                None
            };
            let dawg = DynamicDawgChar::with_config(f32::INFINITY, bloom_capacity);

            // Insert all terms (bloom filter is automatically populated)
            for term in self.pending.iter() {
                dawg.insert(&*term);
            }

            dawg
        })
    }

    /// Add a term to the dictionary.
    ///
    /// **Lock-Free Insert**: Adds directly to the DashSet with no locking overhead.
    /// Terms are accumulated until the first query triggers dictionary construction.
    ///
    /// **Note**: Terms added after the dictionary is initialized will still be
    /// stored in pending but won't be included in fuzzy matching (the dictionary
    /// is built once and cached). This is acceptable for the error-handling use
    /// case where terms are typically added during evaluation before any errors.
    pub fn insert(&self, term: &str) {
        self.pending.insert(term.to_string());
    }

    /// Remove a term from the pending set.
    ///
    /// Note: If the dictionary is already initialized, the term will still be
    /// in the dictionary until rebuilt. For the error-handling use case, this
    /// is acceptable.
    pub fn remove(&self, term: &str) -> bool {
        self.pending.remove(term).is_some()
    }

    /// Check if a term exists in the pending set.
    ///
    /// Note: This only checks the pending set, not the initialized dictionary.
    /// For accurate membership after initialization, use the dictionary directly.
    pub fn contains(&self, term: &str) -> bool {
        self.pending.contains(term)
    }

    /// Get the number of terms in the pending set.
    ///
    /// Note: After dictionary initialization, this still returns the pending count.
    pub fn len(&self) -> usize {
        if let Some(dict) = self.dictionary.get() {
            dict.term_count()
        } else {
            self.pending.len()
        }
    }

    /// Check if the matcher has no terms.
    pub fn is_empty(&self) -> bool {
        if let Some(dict) = self.dictionary.get() {
            dict.term_count() == 0
        } else {
            self.pending.is_empty()
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
        let dict = self.get_dictionary();

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
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}
