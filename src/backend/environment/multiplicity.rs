//! Value-based multiplicity tracking for MeTTa HE semantics.
//!
//! Atoms map directly to their multiplicities via MettaTrie:
//! `TrieKey sequence → (expression, Multiplicity(count))`
//!
//! # Data Layout
//!
//! ```text
//! btm: MettaTrie<V, Multiplicity>
//! ─────────────────────────────────
//! TrieKey path → (V, Multiplicity(count))   [single entry - atom + count unified]
//! ```
//!
//! # Benefits
//!
//! - Single entry per atom with CoW structural sharing (O(1) clone)
//! - No serialization/deserialization overhead (direct TrieKey decomposition)
//! - Direct `iter()` for all entries
//! - Lattice operations for efficient batch merges (join, meet, subtract)
//! - No arity-64 limit (arbitrary arity supported natively)

use metta_trie::{MettaTrie, Multiplicity, TrieKey};

// Re-export from metta_trie for convenience

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   MettaTrie-based Multiplicity Operations                               *-=

/// Get multiplicity for an atom by TrieKey path.
/// Returns 0 if the path does not exist.
#[inline]
pub fn trie_get_multiplicity<V: Clone>(btm: &MettaTrie<V, Multiplicity>, keys: &[TrieKey]) -> u64 {
    btm.get_at(keys).map(|m| m.count()).unwrap_or(0)
}

/// Add atom with multiplicity 1, or increment if exists.
/// Stores the original expression at the leaf for later retrieval.
#[inline]
pub fn trie_add_atom<V: Clone>(btm: &mut MettaTrie<V, Multiplicity>, keys: &[TrieKey], expr: V) {
    match btm.get_at(keys) {
        Some(m) => {
            let new_count = Multiplicity::new(m.count().saturating_add(1));
            btm.insert_at(keys, expr, new_count);
        }
        None => {
            btm.insert_at(keys, expr, Multiplicity::new(1));
        }
    }
}

/// Decrement multiplicity. Entry is auto-removed when count reaches 0.
/// Returns the new count (0 if removed).
#[inline]
pub fn trie_remove_atom<V: Clone>(btm: &mut MettaTrie<V, Multiplicity>, keys: &[TrieKey]) -> u64 {
    match btm.get_at(keys) {
        Some(m) if m.count() > 1 => {
            let new_count = m.count() - 1;
            // We need to get the existing expr to re-insert
            let expr = btm.get_entry_at(keys).expect("just checked").0.clone();
            btm.insert_at(keys, expr, Multiplicity::new(new_count));
            new_count
        }
        Some(_) => {
            // Count is 1, remove the entry entirely
            btm.remove_at(keys);
            0
        }
        None => 0,
    }
}

/// Set multiplicity to a specific value.
/// If count is 0, removes the entry entirely.
#[inline]
pub fn trie_set_multiplicity<V: Clone>(
    btm: &mut MettaTrie<V, Multiplicity>,
    keys: &[TrieKey],
    expr: V,
    count: u64,
) {
    if count == 0 {
        btm.remove_at(keys);
    } else {
        btm.insert_at(keys, expr, Multiplicity::new(count));
    }
}

/// Increment multiplicity (legacy-compatible API).
/// Returns the new multiplicity count after incrementing.
#[inline]
pub fn trie_increment_multiplicity<V: Clone>(
    btm: &mut MettaTrie<V, Multiplicity>,
    keys: &[TrieKey],
    expr: V,
) -> u64 {
    trie_add_atom(btm, keys, expr);
    trie_get_multiplicity(btm, keys)
}

/// Decrement multiplicity (legacy-compatible API).
/// Returns the new multiplicity count after decrementing (0 if fully removed).
#[inline]
pub fn trie_decrement_multiplicity<V: Clone>(
    btm: &mut MettaTrie<V, Multiplicity>,
    keys: &[TrieKey],
) -> u64 {
    trie_remove_atom(btm, keys)
}

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   PathMap compatibility re-exports (for Lattice trait impls)             *-=

// The Multiplicity type and its Lattice/DistributiveLattice impls are defined
// in metta_trie::multiplicity. The pathmap Lattice impls are no longer needed
// since we've migrated away from PathMap.

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   Tests                                                                 *-=

#[cfg(test)]
mod tests {
    use super::*;
    use metta_trie::Lattice;

    #[test]
    fn test_multiplicity_basic() {
        let m = Multiplicity::new(42);
        assert_eq!(m.count(), 42);
        assert!(!m.is_zero());

        let zero = Multiplicity::new(0);
        assert!(zero.is_zero());
    }

    #[test]
    fn test_multiplicity_lattice_join() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // Join should ADD: 5 + 3 = 8
        match a.pjoin(&b) {
            metta_trie::AlgebraicResult::Element(result) => assert_eq!(result.count(), 8),
            _ => panic!("Expected Element result from pjoin"),
        }
    }

    #[test]
    fn test_multiplicity_lattice_meet() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // Meet should return minimum
        match a.pmeet(&b) {
            metta_trie::AlgebraicResult::Identity(metta_trie::COUNTER_IDENT) => {} // b is smaller
            _ => panic!("Expected Identity(COUNTER_IDENT) from pmeet"),
        }
    }

    #[test]
    fn test_multiplicity_lattice_subtract() {
        use metta_trie::DistributiveLattice;
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // 5 - 3 = 2
        match a.psubtract(&b) {
            metta_trie::AlgebraicResult::Element(result) => assert_eq!(result.count(), 2),
            _ => panic!("Expected Element result from psubtract"),
        }

        // 3 - 5 = 0 (should be None/removed)
        match b.psubtract(&a) {
            metta_trie::AlgebraicResult::None => {} // Correct - entry should be removed
            _ => panic!("Expected None from psubtract when result would be <= 0"),
        }

        // 3 - 3 = 0 (should be None/removed)
        match b.psubtract(&b) {
            metta_trie::AlgebraicResult::None => {} // Correct - entry should be removed
            _ => panic!("Expected None from psubtract when result would be 0"),
        }
    }

    #[test]
    fn test_trie_get_multiplicity() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("foo")];

        // Initially no entry
        assert_eq!(trie_get_multiplicity(&btm, &keys), 0);

        // Add entry
        btm.insert_at(&keys, "foo".to_string(), Multiplicity::new(42));
        assert_eq!(trie_get_multiplicity(&btm, &keys), 42);
    }

    #[test]
    fn test_trie_add_remove_atom() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("foo")];

        // Add atom (0 -> 1)
        trie_add_atom(&mut btm, &keys, "foo".to_string());
        assert_eq!(trie_get_multiplicity(&btm, &keys), 1);

        // Add again (1 -> 2)
        trie_add_atom(&mut btm, &keys, "foo".to_string());
        assert_eq!(trie_get_multiplicity(&btm, &keys), 2);

        // Add again (2 -> 3)
        trie_add_atom(&mut btm, &keys, "foo".to_string());
        assert_eq!(trie_get_multiplicity(&btm, &keys), 3);

        // Remove (3 -> 2)
        assert_eq!(trie_remove_atom(&mut btm, &keys), 2);

        // Remove (2 -> 1)
        assert_eq!(trie_remove_atom(&mut btm, &keys), 1);

        // Remove (1 -> 0, entry removed)
        assert_eq!(trie_remove_atom(&mut btm, &keys), 0);
        assert_eq!(trie_get_multiplicity(&btm, &keys), 0);

        // Remove when already 0
        assert_eq!(trie_remove_atom(&mut btm, &keys), 0);
    }

    #[test]
    fn test_trie_multiple_atoms() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys1 = vec![TrieKey::Atom("foo")];
        let keys2 = vec![TrieKey::Atom("bar")];
        let keys3 = vec![TrieKey::Atom("baz")];

        // Add different atoms
        trie_add_atom(&mut btm, &keys1, "foo".to_string());
        trie_add_atom(&mut btm, &keys2, "bar".to_string());
        trie_add_atom(&mut btm, &keys3, "baz".to_string());

        // Check each tracked independently
        assert_eq!(trie_get_multiplicity(&btm, &keys1), 1);
        assert_eq!(trie_get_multiplicity(&btm, &keys2), 1);
        assert_eq!(trie_get_multiplicity(&btm, &keys3), 1);

        // Add keys1 multiple times
        trie_add_atom(&mut btm, &keys1, "foo".to_string());
        trie_add_atom(&mut btm, &keys1, "foo".to_string());

        // Others unaffected
        assert_eq!(trie_get_multiplicity(&btm, &keys1), 3);
        assert_eq!(trie_get_multiplicity(&btm, &keys2), 1);
        assert_eq!(trie_get_multiplicity(&btm, &keys3), 1);
    }

    #[test]
    fn test_trie_set_multiplicity() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("foo")];

        // Set to specific value
        trie_set_multiplicity(&mut btm, &keys, "foo".to_string(), 42);
        assert_eq!(trie_get_multiplicity(&btm, &keys), 42);

        // Set to different value
        trie_set_multiplicity(&mut btm, &keys, "foo".to_string(), 100);
        assert_eq!(trie_get_multiplicity(&btm, &keys), 100);

        // Set to 0 (removes entry)
        trie_set_multiplicity(&mut btm, &keys, "foo".to_string(), 0);
        assert_eq!(trie_get_multiplicity(&btm, &keys), 0);
    }

    #[test]
    fn test_trie_fork_isolation() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("foo")];

        // Set up initial multiplicity
        trie_add_atom(&mut btm, &keys, "foo".to_string());
        assert_eq!(trie_get_multiplicity(&btm, &keys), 1);

        // Fork via clone (O(1) via Arc CoW)
        let mut forked = btm.clone();

        // Increment in forked
        trie_add_atom(&mut forked, &keys, "foo".to_string());

        // Isolation: original unchanged, forked incremented
        assert_eq!(trie_get_multiplicity(&btm, &keys), 1);
        assert_eq!(trie_get_multiplicity(&forked, &keys), 2);
    }

    #[test]
    fn test_trie_large_counts() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("big")];

        // Increment to a large number
        for i in 1..=1000 {
            trie_add_atom(&mut btm, &keys, "big".to_string());
            assert_eq!(trie_get_multiplicity(&btm, &keys), i);
        }

        // Decrement back down
        for i in (0..1000).rev() {
            assert_eq!(trie_remove_atom(&mut btm, &keys), i);
        }
        assert_eq!(trie_get_multiplicity(&btm, &keys), 0);
    }

    #[test]
    fn test_trie_legacy_api_compatibility() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys = vec![TrieKey::Atom("test")];

        // Test legacy increment/decrement
        assert_eq!(trie_increment_multiplicity(&mut btm, &keys, "test".to_string()), 1);
        assert_eq!(trie_increment_multiplicity(&mut btm, &keys, "test".to_string()), 2);
        assert_eq!(trie_increment_multiplicity(&mut btm, &keys, "test".to_string()), 3);

        assert_eq!(trie_decrement_multiplicity(&mut btm, &keys), 2);
        assert_eq!(trie_decrement_multiplicity(&mut btm, &keys), 1);
        assert_eq!(trie_decrement_multiplicity(&mut btm, &keys), 0);
    }

    #[test]
    fn test_trie_iteration_with_multiplicity() {
        let mut btm: MettaTrie<String, Multiplicity> = MettaTrie::new();
        let keys1 = vec![TrieKey::Atom("foo")];
        let keys2 = vec![TrieKey::Atom("bar")];
        let keys3 = vec![TrieKey::Atom("baz")];

        // Add atoms with different multiplicities
        for _ in 0..3 {
            trie_add_atom(&mut btm, &keys1, "foo".to_string());
        }
        trie_add_atom(&mut btm, &keys2, "bar".to_string());
        for _ in 0..5 {
            trie_add_atom(&mut btm, &keys3, "baz".to_string());
        }

        // Iterate and collect (expr, count) pairs
        let mut results: Vec<(String, u64)> = Vec::new();
        for (expr, mult) in btm.iter() {
            results.push((expr.clone(), mult.count()));
        }

        // Should have exactly 3 entries
        assert_eq!(results.len(), 3);

        // Verify counts
        for (expr, count) in &results {
            match expr.as_str() {
                "foo" => assert_eq!(*count, 3),
                "bar" => assert_eq!(*count, 1),
                "baz" => assert_eq!(*count, 5),
                other => panic!("Unexpected expr: {}", other),
            }
        }
    }
}
