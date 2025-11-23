// Term Store: Bidirectional mapping between MettaValue and u64 IDs
//
// This module provides term interning for MettaValue instances, enabling:
// 1. Deduplication: Identical terms get the same ID
// 2. ACT compatibility: MettaValue encoded as u64 for PathMap ACT format
// 3. Memory efficiency: Terms stored once, referenced by ID

use crate::backend::models::MettaValue;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

/// Term store with bidirectional mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermStore {
    /// Map from term to ID (for interning)
    term_to_id: HashMap<MettaValue, u64>,
    /// Map from ID to term (for resolution)
    id_to_term: HashMap<u64, MettaValue>,
    /// Next available ID
    next_id: u64,
}

impl TermStore {
    /// Create a new empty term store
    pub fn new() -> Self {
        TermStore {
            term_to_id: HashMap::new(),
            id_to_term: HashMap::new(),
            next_id: 1, // Start at 1 (reserve 0 for special purposes)
        }
    }

    /// Intern a term, returning its ID
    /// If the term already exists, returns its existing ID (deduplication)
    /// Otherwise, assigns a new ID and stores the term
    pub fn intern(&mut self, term: MettaValue) -> u64 {
        // Check if term already exists
        if let Some(&id) = self.term_to_id.get(&term) {
            return id;
        }

        // Assign new ID
        let id = self.next_id;
        self.next_id += 1;

        // Store bidirectional mapping
        self.term_to_id.insert(term.clone(), id);
        self.id_to_term.insert(id, term);

        id
    }

    /// Resolve an ID to its corresponding term
    /// Returns None if ID not found
    pub fn resolve(&self, id: u64) -> Option<&MettaValue> {
        self.id_to_term.get(&id)
    }

    /// Get the number of unique terms stored
    pub fn len(&self) -> usize {
        self.id_to_term.len()
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.id_to_term.is_empty()
    }

    /// Get the ID for a term if it exists (without interning)
    pub fn get_id(&self, term: &MettaValue) -> Option<u64> {
        self.term_to_id.get(term).copied()
    }

    /// Clear all entries
    pub fn clear(&mut self) {
        self.term_to_id.clear();
        self.id_to_term.clear();
        self.next_id = 1;
    }

    /// Get statistics about the term store
    pub fn stats(&self) -> TermStoreStats {
        TermStoreStats {
            num_terms: self.len(),
            next_id: self.next_id,
        }
    }
}

impl Default for TermStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about term store usage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TermStoreStats {
    pub num_terms: usize,
    pub next_id: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intern_deduplication() {
        let mut store = TermStore::new();

        let term1 = MettaValue::Atom("hello".to_string());
        let term2 = MettaValue::Atom("hello".to_string());
        let term3 = MettaValue::Atom("world".to_string());

        // Same term should get same ID
        let id1 = store.intern(term1.clone());
        let id2 = store.intern(term2);
        assert_eq!(id1, id2);

        // Different term should get different ID
        let id3 = store.intern(term3);
        assert_ne!(id1, id3);

        // Only 2 unique terms stored
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn test_resolve() {
        let mut store = TermStore::new();

        let term = MettaValue::Long(42);
        let id = store.intern(term.clone());

        // Resolve should return the original term
        assert_eq!(store.resolve(id), Some(&term));

        // Non-existent ID should return None
        assert_eq!(store.resolve(999), None);
    }

    #[test]
    fn test_get_id() {
        let mut store = TermStore::new();

        let term = MettaValue::Bool(true);
        let id = store.intern(term.clone());

        // get_id should find existing term
        assert_eq!(store.get_id(&term), Some(id));

        // get_id should not intern new terms
        let new_term = MettaValue::Bool(false);
        assert_eq!(store.get_id(&new_term), None);
        assert_eq!(store.len(), 1); // Still only 1 term
    }

    #[test]
    fn test_clear() {
        let mut store = TermStore::new();

        store.intern(MettaValue::Atom("test".to_string()));
        assert_eq!(store.len(), 1);

        store.clear();
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());
    }

    #[test]
    fn test_stats() {
        let mut store = TermStore::new();

        store.intern(MettaValue::Long(1));
        store.intern(MettaValue::Long(2));
        store.intern(MettaValue::Long(1)); // Duplicate

        let stats = store.stats();
        assert_eq!(stats.num_terms, 2); // Only 2 unique terms
        assert_eq!(stats.next_id, 3); // Next ID is 3 (1 and 2 used)
    }
}
