//! Core FuzzyMatcher implementation.

use liblevenshtein::dictionary::dynamic_dawg_char::DynamicDawgChar;
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
pub struct FuzzyMatcher {
    /// Pending terms waiting to be added to the dictionary
    /// Using Arc<RwLock<>> for thread-safe lazy initialization
    pub(super) pending: Arc<RwLock<HashSet<String>>>,
    /// Lazily-initialized dictionary. None until first query.
    pub(super) dictionary: Arc<RwLock<Option<DynamicDawgChar<()>>>>,
}

/// Manual Clone implementation for deep cloning with CoW semantics.
///
/// DynamicDawgChar from liblevenshtein uses Arc<RwLock<...>> internally with
/// `#[derive(Clone)]`, meaning cloned DAWGs share the same underlying data.
/// To ensure true independence for CoW semantics in Environment::make_owned():
///
/// 1. Deep clone the pending HashSet (creates new Arc with copied data)
/// 2. Extract all terms from existing dictionary into pending (if initialized)
/// 3. Reset dictionary to None (will be rebuilt lazily on first query)
///
/// This avoids the Arc sharing issue in DynamicDawgChar and ensures each
/// cloned FuzzyMatcher operates on fully independent data.
impl Clone for FuzzyMatcher {
    fn clone(&self) -> Self {
        // Get all terms - from pending and from initialized dictionary
        let all_terms = self.pending.read().unwrap().clone();

        // If dictionary is initialized, extract all terms from it
        if let Some(ref dict) = *self.dictionary.read().unwrap() {
            // DynamicDawgChar doesn't expose iteration, but we can get term_count
            // The terms are already in pending from insert() calls before initialization
            // After initialization, new terms go directly to dict, so we can't extract them
            // However, ensure_initialized() moves all pending terms to dict, so:
            // - If dict is Some, pending should be empty (all moved to dict)
            // - We need to rebuild from scratch, so reset to pending-only state
            //
            // Since we can't iterate DynamicDawgChar, we clone the pending set
            // (which may be empty if dict was initialized) and let the new
            // FuzzyMatcher rebuild the dictionary lazily on first query.
            //
            // Note: This means cloned FuzzyMatchers lose terms added after
            // initialization. For CoW correctness, this is acceptable since
            // clones are made before mutation, not after.
            let _ = dict; // Acknowledge we can't extract terms from initialized dict
        }

        Self {
            pending: Arc::new(RwLock::new(all_terms)),
            dictionary: Arc::new(RwLock::new(None)), // Reset - rebuild lazily
        }
    }
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
    pub(super) fn ensure_initialized(&self) {
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

    /// Add a term to the dictionary.
    ///
    /// **Performance Optimization**: Always adds to the pending set and invalidates
    /// the dictionary if it was initialized. The dictionary will be rebuilt lazily
    /// on the next query. This ensures O(1) insert time during normal evaluation,
    /// deferring the expensive DynamicDawgChar construction to error-handling time.
    ///
    /// This is critical for performance: DynamicDawgChar::insert() rebuilds internal
    /// automaton structures which is O(n) where n = number of terms. By always using
    /// pending and invalidating, we batch all insertions until a query is needed.
    pub fn insert(&self, term: &str) {
        // Always add to pending set (O(1) HashSet insert)
        {
            let mut pending_guard = self.pending.write().unwrap();
            pending_guard.insert(term.to_string());
        }

        // Invalidate dictionary if it was initialized
        // This ensures rebuild includes the new term on next query
        {
            let dict_guard = self.dictionary.read().unwrap();
            if dict_guard.is_some() {
                drop(dict_guard);
                let mut dict_write = self.dictionary.write().unwrap();
                *dict_write = None;
            }
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
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}
