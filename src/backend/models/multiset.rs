//! Lock-free multiset (bag) for tracking atom multiplicities.
//!
//! This module provides an efficient multiset data structure for tracking
//! how many times each unique atom appears. It uses:
//! - Symbol interning for fast O(1) lookup via `AtomId`
//! - `DashMap` for lock-free concurrent access
//! - `AtomicUsize` counters for lock-free increment/decrement
//! - Persistent `im::HashMap` for O(1) fork/snapshot operations
//!
//! # Use Cases
//!
//! - **Rule multiplicities**: Track how many times each rule is defined
//! - **Atom deduplication**: Store unique atoms with counts instead of duplicates
//! - **Fork isolation**: Create lightweight copies with structural sharing
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::models::{AtomMultiset, SymbolTable, MettaValue};
//! use std::sync::Arc;
//!
//! let symbols = Arc::new(SymbolTable::new());
//! let multiset = AtomMultiset::new(Arc::clone(&symbols));
//!
//! let atom = MettaValue::Atom("foo".to_string());
//! multiset.insert(&atom);  // count = 1
//! multiset.insert(&atom);  // count = 2
//!
//! assert_eq!(multiset.count(&atom), 2);
//! assert_eq!(multiset.total(), 2);
//!
//! multiset.remove(&atom);  // count = 1
//! assert_eq!(multiset.count(&atom), 1);
//! ```

use dashmap::DashMap;
use xxhash_rust::xxh3::Xxh3Builder;
use im::HashMap as ImHashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use super::atom_id::{AtomId, SymbolTable};
use super::MettaValue;

/// A lock-free multiset (bag) for tracking atom multiplicities.
///
/// This is the mutable, concurrent version optimized for concurrent access.
/// For fork/copy-on-write operations, use `snapshot()` to get an immutable view.
#[derive(Debug)]
pub struct AtomMultiset {
    /// Shared symbol table for interning atoms
    symbols: Arc<SymbolTable>,

    /// AtomId → count mapping (lock-free via DashMap + AtomicUsize)
    counts: DashMap<AtomId, AtomicUsize, Xxh3Builder>,

    /// Total count across all atoms (for fast `total()` queries)
    total: AtomicUsize,
}

impl AtomMultiset {
    /// Create a new empty multiset with the given symbol table.
    pub fn new(symbols: Arc<SymbolTable>) -> Self {
        Self {
            symbols,
            counts: DashMap::with_hasher(Xxh3Builder::new()),
            total: AtomicUsize::new(0),
        }
    }

    /// Create a new multiset with pre-allocated capacity.
    pub fn with_capacity(symbols: Arc<SymbolTable>, capacity: usize) -> Self {
        Self {
            symbols,
            counts: DashMap::with_capacity_and_hasher(capacity, Xxh3Builder::new()),
            total: AtomicUsize::new(0),
        }
    }

    /// Get a reference to the shared symbol table.
    #[inline]
    pub fn symbols(&self) -> &Arc<SymbolTable> {
        &self.symbols
    }

    /// Insert an atom into the multiset, incrementing its count.
    ///
    /// Returns the new count after insertion.
    ///
    /// # Performance
    /// - O(1) if atom is already interned
    /// - O(1) amortized if atom needs interning
    pub fn insert(&self, value: &MettaValue) -> usize {
        let id = self.symbols.intern(value);
        self.insert_id(id)
    }

    /// Insert by AtomId (useful when you already have the ID).
    #[inline]
    pub fn insert_id(&self, id: AtomId) -> usize {
        self.total.fetch_add(1, Ordering::Relaxed);

        self.counts
            .entry(id)
            .or_insert_with(|| AtomicUsize::new(0))
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }

    /// Insert an atom with a specific count (bulk insert).
    ///
    /// Returns the new total count for this atom.
    pub fn insert_n(&self, value: &MettaValue, n: usize) -> usize {
        if n == 0 {
            return self.count(value);
        }

        let id = self.symbols.intern(value);
        self.total.fetch_add(n, Ordering::Relaxed);

        self.counts
            .entry(id)
            .or_insert_with(|| AtomicUsize::new(0))
            .fetch_add(n, Ordering::Relaxed)
            + n
    }

    /// Remove one occurrence of an atom from the multiset.
    ///
    /// Returns the new count after removal, or `None` if the atom wasn't present.
    ///
    /// # Note
    /// When count reaches 0, the entry is removed from the map to save memory.
    pub fn remove(&self, value: &MettaValue) -> Option<usize> {
        let id = self.symbols.get(value)?;
        self.remove_id(id)
    }

    /// Remove by AtomId.
    pub fn remove_id(&self, id: AtomId) -> Option<usize> {
        // Use entry API to handle the atomic decrement and potential removal
        let mut removed = false;
        let mut new_count = 0;

        self.counts.entry(id).and_modify(|count| {
            let old = count.load(Ordering::Relaxed);
            if old > 0 {
                new_count = old - 1;
                count.store(new_count, Ordering::Relaxed);
                removed = true;
            }
        });

        if removed {
            self.total.fetch_sub(1, Ordering::Relaxed);

            // Clean up zero-count entries to save memory
            if new_count == 0 {
                self.counts.remove(&id);
            }

            Some(new_count)
        } else {
            None
        }
    }

    /// Remove all occurrences of an atom.
    ///
    /// Returns the count that was removed, or `None` if not present.
    pub fn remove_all(&self, value: &MettaValue) -> Option<usize> {
        let id = self.symbols.get(value)?;

        if let Some((_, count)) = self.counts.remove(&id) {
            let old_count = count.load(Ordering::Relaxed);
            if old_count > 0 {
                self.total.fetch_sub(old_count, Ordering::Relaxed);
                return Some(old_count);
            }
        }
        None
    }

    /// Get the count of an atom in the multiset.
    ///
    /// Returns 0 if the atom is not present.
    #[inline]
    pub fn count(&self, value: &MettaValue) -> usize {
        if let Some(id) = self.symbols.get(value) {
            self.count_id(id)
        } else {
            0
        }
    }

    /// Get count by AtomId.
    #[inline]
    pub fn count_id(&self, id: AtomId) -> usize {
        self.counts
            .get(&id)
            .map(|c| c.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Check if the atom is present in the multiset.
    #[inline]
    pub fn contains(&self, value: &MettaValue) -> bool {
        self.count(value) > 0
    }

    /// Get the total number of items (sum of all counts).
    #[inline]
    pub fn total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }

    /// Get the number of distinct atoms.
    #[inline]
    pub fn distinct_count(&self) -> usize {
        self.counts.len()
    }

    /// Check if the multiset is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total() == 0
    }

    /// Clear all entries from the multiset.
    pub fn clear(&self) {
        self.counts.clear();
        self.total.store(0, Ordering::Relaxed);
    }

    /// Create an immutable snapshot for fork/copy-on-write operations.
    ///
    /// The snapshot uses structural sharing via persistent data structures,
    /// making cloning O(1) and memory-efficient.
    pub fn snapshot(&self) -> AtomMultisetSnapshot {
        let mut counts = ImHashMap::new();
        for entry in self.counts.iter() {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                counts.insert(*entry.key(), count);
            }
        }

        AtomMultisetSnapshot {
            symbols: Arc::clone(&self.symbols),
            counts,
            total: self.total.load(Ordering::Relaxed),
        }
    }

    /// Iterate over all (MettaValue, count) pairs.
    ///
    /// Note: Materializes values from the symbol table.
    pub fn iter(&self) -> impl Iterator<Item = (MettaValue, usize)> + '_ {
        self.counts.iter().filter_map(|entry| {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                let value = self.symbols.resolve(*entry.key());
                Some((value, count))
            } else {
                None
            }
        })
    }

    /// Iterate over all (AtomId, count) pairs without materializing values.
    pub fn iter_ids(&self) -> impl Iterator<Item = (AtomId, usize)> + '_ {
        self.counts.iter().filter_map(|entry| {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                Some((*entry.key(), count))
            } else {
                None
            }
        })
    }

    /// Expand all atoms according to their multiplicities.
    ///
    /// Returns each atom repeated according to its count.
    /// This is useful for `collapse()` operations that need the full list.
    pub fn expand(&self) -> Vec<MettaValue> {
        let total = self.total();
        let mut result = Vec::with_capacity(total);

        for entry in self.counts.iter() {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                let value = self.symbols.resolve(*entry.key());
                for _ in 0..count {
                    result.push(value.clone());
                }
            }
        }

        result
    }

    /// Merge another multiset into this one.
    pub fn merge(&self, other: &AtomMultiset) {
        for entry in other.counts.iter() {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                // Get the value from other's symbol table and intern in ours
                let value = other.symbols.resolve(*entry.key());
                self.insert_n(&value, count);
            }
        }
    }

    /// Merge from a snapshot.
    pub fn merge_snapshot(&self, snapshot: &AtomMultisetSnapshot) {
        for (id, count) in snapshot.counts.iter() {
            if *count > 0 {
                let value = snapshot.symbols.resolve(*id);
                self.insert_n(&value, *count);
            }
        }
    }
}

impl Clone for AtomMultiset {
    fn clone(&self) -> Self {
        let new_multiset = AtomMultiset::new(Arc::clone(&self.symbols));
        for entry in self.counts.iter() {
            let count = entry.value().load(Ordering::Relaxed);
            if count > 0 {
                new_multiset
                    .counts
                    .insert(*entry.key(), AtomicUsize::new(count));
            }
        }
        new_multiset
            .total
            .store(self.total.load(Ordering::Relaxed), Ordering::Relaxed);
        new_multiset
    }
}

/// An immutable snapshot of an AtomMultiset using persistent data structures.
///
/// Snapshots provide:
/// - O(1) cloning via structural sharing
/// - Immutable semantics for fork isolation
/// - Memory-efficient storage of modifications
///
/// Use `to_mutable()` to convert back to a mutable `AtomMultiset`.
#[derive(Debug, Clone)]
pub struct AtomMultisetSnapshot {
    /// Shared symbol table
    symbols: Arc<SymbolTable>,

    /// Persistent hash map with structural sharing
    counts: ImHashMap<AtomId, usize>,

    /// Cached total count
    total: usize,
}

impl AtomMultisetSnapshot {
    /// Create an empty snapshot with the given symbol table.
    pub fn new(symbols: Arc<SymbolTable>) -> Self {
        Self {
            symbols,
            counts: ImHashMap::new(),
            total: 0,
        }
    }

    /// Get a reference to the shared symbol table.
    #[inline]
    pub fn symbols(&self) -> &Arc<SymbolTable> {
        &self.symbols
    }

    /// Insert an atom, returning a new snapshot with the updated count.
    ///
    /// This is an immutable operation - the original snapshot is unchanged.
    #[must_use]
    pub fn insert(&self, value: &MettaValue) -> Self {
        let id = self.symbols.intern(value);
        let new_count = self.counts.get(&id).unwrap_or(&0) + 1;

        Self {
            symbols: Arc::clone(&self.symbols),
            counts: self.counts.update(id, new_count),
            total: self.total + 1,
        }
    }

    /// Insert with a specific count.
    #[must_use]
    pub fn insert_n(&self, value: &MettaValue, n: usize) -> Self {
        if n == 0 {
            return self.clone();
        }

        let id = self.symbols.intern(value);
        let new_count = self.counts.get(&id).unwrap_or(&0) + n;

        Self {
            symbols: Arc::clone(&self.symbols),
            counts: self.counts.update(id, new_count),
            total: self.total + n,
        }
    }

    /// Remove one occurrence of an atom.
    ///
    /// Returns `None` if the atom wasn't present.
    pub fn remove(&self, value: &MettaValue) -> Option<Self> {
        let id = self.symbols.get(value)?;
        let old_count = *self.counts.get(&id)?;

        if old_count == 0 {
            return None;
        }

        let new_count = old_count - 1;
        let new_counts = if new_count == 0 {
            self.counts.without(&id)
        } else {
            self.counts.update(id, new_count)
        };

        Some(Self {
            symbols: Arc::clone(&self.symbols),
            counts: new_counts,
            total: self.total - 1,
        })
    }

    /// Remove all occurrences of an atom.
    pub fn remove_all(&self, value: &MettaValue) -> Option<Self> {
        let id = self.symbols.get(value)?;
        let old_count = *self.counts.get(&id)?;

        if old_count == 0 {
            return None;
        }

        Some(Self {
            symbols: Arc::clone(&self.symbols),
            counts: self.counts.without(&id),
            total: self.total - old_count,
        })
    }

    /// Get the count of an atom.
    #[inline]
    pub fn count(&self, value: &MettaValue) -> usize {
        if let Some(id) = self.symbols.get(value) {
            self.count_id(id)
        } else {
            0
        }
    }

    /// Get count by AtomId.
    #[inline]
    pub fn count_id(&self, id: AtomId) -> usize {
        *self.counts.get(&id).unwrap_or(&0)
    }

    /// Check if the atom is present.
    #[inline]
    pub fn contains(&self, value: &MettaValue) -> bool {
        self.count(value) > 0
    }

    /// Get the total count.
    #[inline]
    pub fn total(&self) -> usize {
        self.total
    }

    /// Get the number of distinct atoms.
    #[inline]
    pub fn distinct_count(&self) -> usize {
        self.counts.len()
    }

    /// Check if empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Convert to a mutable AtomMultiset.
    pub fn to_mutable(&self) -> AtomMultiset {
        let multiset = AtomMultiset::new(Arc::clone(&self.symbols));
        for (id, count) in self.counts.iter() {
            if *count > 0 {
                multiset.counts.insert(*id, AtomicUsize::new(*count));
            }
        }
        multiset.total.store(self.total, Ordering::Relaxed);
        multiset
    }

    /// Iterate over (MettaValue, count) pairs.
    pub fn iter(&self) -> impl Iterator<Item = (MettaValue, usize)> + '_ {
        self.counts.iter().filter_map(|(id, count)| {
            if *count > 0 {
                let value = self.symbols.resolve(*id);
                Some((value, *count))
            } else {
                None
            }
        })
    }

    /// Expand all atoms according to their multiplicities.
    pub fn expand(&self) -> Vec<MettaValue> {
        let mut result = Vec::with_capacity(self.total);

        for (id, count) in self.counts.iter() {
            if *count > 0 {
                let value = self.symbols.resolve(*id);
                for _ in 0..*count {
                    result.push(value.clone());
                }
            }
        }

        result
    }

    /// Merge two snapshots.
    #[must_use]
    pub fn merge(&self, other: &AtomMultisetSnapshot) -> Self {
        let mut counts = self.counts.clone();
        let mut total = self.total;

        for (id, count) in other.counts.iter() {
            if *count > 0 {
                // Re-intern the value in case symbol tables differ
                let value = other.symbols.resolve(*id);
                let new_id = self.symbols.intern(&value);
                let new_count = counts.get(&new_id).unwrap_or(&0) + count;
                counts = counts.update(new_id, new_count);
                total += count;
            }
        }

        Self {
            symbols: Arc::clone(&self.symbols),
            counts,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Arc<SymbolTable> {
        Arc::new(SymbolTable::new())
    }

    #[test]
    fn test_insert_and_count() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let atom = MettaValue::Atom("foo".to_string());

        assert_eq!(multiset.count(&atom), 0);
        assert!(!multiset.contains(&atom));

        assert_eq!(multiset.insert(&atom), 1);
        assert_eq!(multiset.count(&atom), 1);
        assert!(multiset.contains(&atom));

        assert_eq!(multiset.insert(&atom), 2);
        assert_eq!(multiset.count(&atom), 2);

        assert_eq!(multiset.total(), 2);
        assert_eq!(multiset.distinct_count(), 1);
    }

    #[test]
    fn test_remove() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let atom = MettaValue::Atom("foo".to_string());

        multiset.insert(&atom);
        multiset.insert(&atom);
        assert_eq!(multiset.count(&atom), 2);

        assert_eq!(multiset.remove(&atom), Some(1));
        assert_eq!(multiset.count(&atom), 1);

        assert_eq!(multiset.remove(&atom), Some(0));
        assert_eq!(multiset.count(&atom), 0);
        assert!(!multiset.contains(&atom));

        // Remove from empty returns None
        assert_eq!(multiset.remove(&atom), None);
    }

    #[test]
    fn test_remove_all() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let atom = MettaValue::Atom("foo".to_string());

        multiset.insert_n(&atom, 5);
        assert_eq!(multiset.count(&atom), 5);

        assert_eq!(multiset.remove_all(&atom), Some(5));
        assert_eq!(multiset.count(&atom), 0);
        assert_eq!(multiset.total(), 0);
    }

    #[test]
    fn test_multiple_atoms() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let a = MettaValue::Atom("a".to_string());
        let b = MettaValue::Atom("b".to_string());
        let c = MettaValue::Atom("c".to_string());

        multiset.insert(&a);
        multiset.insert(&a);
        multiset.insert(&b);
        multiset.insert_n(&c, 3);

        assert_eq!(multiset.count(&a), 2);
        assert_eq!(multiset.count(&b), 1);
        assert_eq!(multiset.count(&c), 3);
        assert_eq!(multiset.total(), 6);
        assert_eq!(multiset.distinct_count(), 3);
    }

    #[test]
    fn test_expand() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let a = MettaValue::Atom("a".to_string());
        let b = MettaValue::Atom("b".to_string());

        multiset.insert_n(&a, 2);
        multiset.insert(&b);

        let expanded = multiset.expand();
        assert_eq!(expanded.len(), 3);

        // Count occurrences
        let a_count = expanded.iter().filter(|v| *v == &a).count();
        let b_count = expanded.iter().filter(|v| *v == &b).count();
        assert_eq!(a_count, 2);
        assert_eq!(b_count, 1);
    }

    #[test]
    fn test_snapshot_immutability() {
        let symbols = setup();
        let multiset = AtomMultiset::new(Arc::clone(&symbols));

        let a = MettaValue::Atom("a".to_string());
        multiset.insert_n(&a, 2);

        let snapshot = multiset.snapshot();

        // Modify original
        multiset.insert(&a);
        assert_eq!(multiset.count(&a), 3);

        // Snapshot unchanged
        assert_eq!(snapshot.count(&a), 2);
    }

    #[test]
    fn test_snapshot_operations() {
        let symbols = setup();
        let snapshot = AtomMultisetSnapshot::new(symbols);

        let a = MettaValue::Atom("a".to_string());

        let s1 = snapshot.insert(&a);
        assert_eq!(s1.count(&a), 1);
        assert_eq!(snapshot.count(&a), 0); // Original unchanged

        let s2 = s1.insert(&a);
        assert_eq!(s2.count(&a), 2);
        assert_eq!(s1.count(&a), 1); // s1 unchanged

        let s3 = s2.remove(&a).unwrap();
        assert_eq!(s3.count(&a), 1);
        assert_eq!(s2.count(&a), 2); // s2 unchanged
    }

    #[test]
    fn test_snapshot_to_mutable() {
        let symbols = setup();
        let multiset = AtomMultiset::new(Arc::clone(&symbols));

        let a = MettaValue::Atom("a".to_string());
        multiset.insert_n(&a, 5);

        let snapshot = multiset.snapshot();
        let mutable = snapshot.to_mutable();

        assert_eq!(mutable.count(&a), 5);
        mutable.insert(&a);
        assert_eq!(mutable.count(&a), 6);
        assert_eq!(snapshot.count(&a), 5); // Snapshot unchanged
    }

    #[test]
    fn test_merge() {
        let symbols = setup();

        let m1 = AtomMultiset::new(Arc::clone(&symbols));
        let m2 = AtomMultiset::new(Arc::clone(&symbols));

        let a = MettaValue::Atom("a".to_string());
        let b = MettaValue::Atom("b".to_string());

        m1.insert_n(&a, 2);
        m2.insert(&a);
        m2.insert_n(&b, 3);

        m1.merge(&m2);

        assert_eq!(m1.count(&a), 3); // 2 + 1
        assert_eq!(m1.count(&b), 3);
        assert_eq!(m1.total(), 6);
    }

    #[test]
    fn test_clear() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let a = MettaValue::Atom("a".to_string());
        multiset.insert_n(&a, 10);
        assert_eq!(multiset.total(), 10);

        multiset.clear();
        assert!(multiset.is_empty());
        assert_eq!(multiset.total(), 0);
        assert_eq!(multiset.distinct_count(), 0);
    }

    #[test]
    fn test_iter() {
        let symbols = setup();
        let multiset = AtomMultiset::new(symbols);

        let a = MettaValue::Atom("a".to_string());
        let b = MettaValue::Long(42);

        multiset.insert_n(&a, 2);
        multiset.insert(&b);

        let items: Vec<_> = multiset.iter().collect();
        assert_eq!(items.len(), 2);

        let total: usize = items.iter().map(|(_, c)| c).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn test_concurrent_insert() {
        use std::thread;

        let symbols = Arc::new(SymbolTable::new());
        let multiset = Arc::new(AtomMultiset::new(symbols));

        let num_threads = 8;
        let inserts_per_thread = 1000;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let multiset = Arc::clone(&multiset);
                thread::spawn(move || {
                    let atom = MettaValue::Atom("shared".to_string());
                    for _ in 0..inserts_per_thread {
                        multiset.insert(&atom);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let atom = MettaValue::Atom("shared".to_string());
        assert_eq!(
            multiset.count(&atom),
            num_threads * inserts_per_thread as usize
        );
    }
}
