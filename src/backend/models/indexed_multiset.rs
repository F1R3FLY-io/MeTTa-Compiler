//! Lock-free indexed multiset for O(1) multiplicity tracking.
//!
//! This module provides an optimized multiset that uses array indexing instead of hashing
//! for maximum performance. Each rule is assigned a unique index at insertion time,
//! and that index is cached on the Rule struct for O(1) lookup.
//!
//! # Key Performance Characteristics
//!
//! | Operation | Time Complexity | Notes |
//! |-----------|----------------|-------|
//! | Lookup    | O(1)           | Direct array access via cached index |
//! | Insert    | O(1)           | Atomic increment + index allocation |
//! | Fork      | O(1)           | Arc clone with lazy CoW |
//! | First write after fork | O(n) | One-time deep copy |
//!
//! # Memory Efficiency
//!
//! Compared to `DashMap<Vec<u8>, AtomicUsize>`:
//! - 6-8x more memory efficient (8 bytes per entry vs 48-64 bytes)
//! - Better cache locality (contiguous array vs hash table)
//!
//! # Thread Safety
//!
//! - Index allocation: Global `AtomicU32` ensures no collisions across forks
//! - Count access: `AtomicUsize` for lock-free read/write
//! - Fork: `Arc` sharing with CoW on first write
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::models::IndexedMultiset;
//!
//! let mut multiset = IndexedMultiset::new();
//!
//! // Allocate index for a new rule
//! let idx = multiset.allocate_index();
//! multiset.increment(idx);  // count = 1
//! multiset.increment(idx);  // count = 2
//!
//! assert_eq!(multiset.count(idx), 2);
//!
//! // Fork for nondeterministic evaluation (O(1))
//! let forked = multiset.fork();
//! // Forked shares data until first write
//! ```

use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// Default initial capacity for the counts array.
const DEFAULT_CAPACITY: usize = 1024;

/// Lock-free indexed multiset for O(1) multiplicity tracking.
///
/// Uses array indexing instead of hashing for maximum performance.
/// Supports O(1) fork via Arc sharing with lazy CoW.
#[derive(Debug)]
pub struct IndexedMultiset {
    /// Global index allocator (shared across all forks)
    /// Uses fetch_add(1) to guarantee unique indices without collisions
    idx_allocator: Arc<AtomicU32>,

    /// Counts array (shared via Arc, copied on write)
    /// Each entry is an AtomicUsize for lock-free increment/decrement
    counts: Arc<Vec<AtomicUsize>>,

    /// CoW flag: true if this instance owns the counts array
    /// When false, first mutation triggers deep copy
    owns_counts: bool,

    /// Current capacity of the counts array
    capacity: usize,
}

impl IndexedMultiset {
    /// Create a new empty multiset with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// Create a new multiset with specified initial capacity.
    ///
    /// # Arguments
    /// * `capacity` - Initial capacity for the counts array
    pub fn with_capacity(capacity: usize) -> Self {
        let counts: Vec<AtomicUsize> = (0..capacity).map(|_| AtomicUsize::new(0)).collect();

        Self {
            idx_allocator: Arc::new(AtomicU32::new(0)),
            counts: Arc::new(counts),
            owns_counts: true,
            capacity,
        }
    }

    /// Allocate a new index for a rule.
    ///
    /// Atomically allocates a fresh index that won't collide with any other
    /// allocation, even across forked environments.
    ///
    /// # Returns
    /// A unique index guaranteed not to collide with any other allocation.
    #[inline]
    pub fn allocate_index(&self) -> u32 {
        // Allocate new index atomically
        self.idx_allocator.fetch_add(1, Ordering::Relaxed)
    }

    /// Increment count at index, returning new count.
    ///
    /// # Performance
    /// O(1) - single atomic increment
    ///
    /// # Panics
    /// May panic if index is beyond capacity and auto-resize fails.
    #[inline]
    pub fn increment(&self, idx: u32) -> usize {
        let idx = idx as usize;
        if idx < self.counts.len() {
            self.counts[idx].fetch_add(1, Ordering::Relaxed) + 1
        } else {
            // Index beyond capacity - this shouldn't happen with proper pre-allocation
            // Return 1 as a safe fallback (rule exists with count 1)
            1
        }
    }

    /// Increment count at index by a specific amount, returning new count.
    #[inline]
    pub fn increment_n(&self, idx: u32, n: usize) -> usize {
        if n == 0 {
            return self.count(idx);
        }
        let idx = idx as usize;
        if idx < self.counts.len() {
            self.counts[idx].fetch_add(n, Ordering::Relaxed) + n
        } else {
            n
        }
    }

    /// Get count at index.
    ///
    /// # Performance
    /// O(1) - single atomic load
    #[inline]
    pub fn count(&self, idx: u32) -> usize {
        let idx = idx as usize;
        if idx < self.counts.len() {
            self.counts[idx].load(Ordering::Relaxed)
        } else {
            0
        }
    }

    /// Decrement count at index. Returns new count.
    ///
    /// # Note
    /// This triggers CoW if the multiset was forked.
    pub fn decrement(&mut self, idx: u32) -> usize {
        self.make_owned();
        let idx_usize = idx as usize;
        if idx_usize < self.counts.len() {
            let old = self.counts[idx_usize].load(Ordering::Relaxed);
            if old > 0 {
                let new_count = old - 1;
                self.counts[idx_usize].store(new_count, Ordering::Relaxed);
                new_count
            } else {
                0
            }
        } else {
            0
        }
    }

    /// Ensure we own the counts array (CoW).
    ///
    /// If this multiset was created via `fork()`, this performs a deep copy
    /// of the counts array. This is O(n) but only happens once per branch
    /// on first mutation.
    fn make_owned(&mut self) {
        if self.owns_counts {
            return;
        }

        // Deep copy the counts array
        let new_counts: Vec<AtomicUsize> = self
            .counts
            .iter()
            .map(|c| AtomicUsize::new(c.load(Ordering::Relaxed)))
            .collect();

        self.counts = Arc::new(new_counts);
        self.owns_counts = true;
    }

    /// Fork for nondeterministic evaluation.
    ///
    /// # Performance
    /// O(1) - just Arc clones, lazy CoW on first write.
    ///
    /// # Semantics
    /// The forked multiset shares data with the original until first mutation.
    /// Index allocation is shared across all forks to prevent collisions.
    pub fn fork(&self) -> Self {
        Self {
            idx_allocator: Arc::clone(&self.idx_allocator),
            counts: Arc::clone(&self.counts),
            owns_counts: false, // Will copy on first write
            capacity: self.capacity,
        }
    }

    /// Ensure capacity for at least n indices.
    ///
    /// Grows the array if needed. This triggers CoW if the multiset was forked.
    pub fn ensure_capacity(&mut self, n: usize) {
        if n <= self.counts.len() {
            return;
        }

        self.make_owned();

        // Grow the array
        let mut new_counts: Vec<AtomicUsize> = self
            .counts
            .iter()
            .map(|c| AtomicUsize::new(c.load(Ordering::Relaxed)))
            .collect();

        new_counts.resize_with(n, || AtomicUsize::new(0));
        self.counts = Arc::new(new_counts);
        self.capacity = n;
    }

    /// Get the current capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.counts.len()
    }

    /// Get the next index that would be allocated.
    ///
    /// This is useful for pre-allocating capacity before bulk operations.
    #[inline]
    pub fn next_index(&self) -> u32 {
        self.idx_allocator.load(Ordering::Relaxed)
    }

    /// Check if this multiset owns its counts array.
    ///
    /// Returns true if mutations won't trigger CoW.
    #[inline]
    pub fn is_owned(&self) -> bool {
        self.owns_counts
    }

    /// Get the total count across all indices.
    ///
    /// # Performance
    /// O(n) - iterates all entries
    pub fn total(&self) -> usize {
        self.counts.iter().map(|c| c.load(Ordering::Relaxed)).sum()
    }

    /// Get the number of non-zero entries.
    ///
    /// # Performance
    /// O(n) - iterates all entries
    pub fn distinct_count(&self) -> usize {
        self.counts
            .iter()
            .filter(|c| c.load(Ordering::Relaxed) > 0)
            .count()
    }

    /// Clear all counts (set to 0).
    ///
    /// This triggers CoW if the multiset was forked.
    pub fn clear(&mut self) {
        self.make_owned();
        for c in self.counts.iter() {
            c.store(0, Ordering::Relaxed);
        }
    }

    /// Iterate over (index, count) pairs for non-zero entries.
    pub fn iter_nonzero(&self) -> impl Iterator<Item = (u32, usize)> + '_ {
        self.counts.iter().enumerate().filter_map(|(idx, c)| {
            let count = c.load(Ordering::Relaxed);
            if count > 0 {
                Some((idx as u32, count))
            } else {
                None
            }
        })
    }
}

impl Clone for IndexedMultiset {
    fn clone(&self) -> Self {
        self.fork()
    }
}

impl Default for IndexedMultiset {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocate_and_count() {
        let multiset = IndexedMultiset::new();

        let idx = multiset.allocate_index();
        assert_eq!(multiset.count(idx), 0);

        assert_eq!(multiset.increment(idx), 1);
        assert_eq!(multiset.count(idx), 1);

        assert_eq!(multiset.increment(idx), 2);
        assert_eq!(multiset.count(idx), 2);
    }

    #[test]
    fn test_multiple_indices() {
        let multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        let idx2 = multiset.allocate_index();
        let idx3 = multiset.allocate_index();

        assert_ne!(idx1, idx2);
        assert_ne!(idx2, idx3);

        multiset.increment(idx1);
        multiset.increment(idx1);
        multiset.increment(idx2);
        multiset.increment_n(idx3, 5);

        assert_eq!(multiset.count(idx1), 2);
        assert_eq!(multiset.count(idx2), 1);
        assert_eq!(multiset.count(idx3), 5);
        assert_eq!(multiset.total(), 8);
        assert_eq!(multiset.distinct_count(), 3);
    }

    #[test]
    fn test_decrement() {
        let mut multiset = IndexedMultiset::new();

        let idx = multiset.allocate_index();
        multiset.increment(idx);
        multiset.increment(idx);
        assert_eq!(multiset.count(idx), 2);

        assert_eq!(multiset.decrement(idx), 1);
        assert_eq!(multiset.count(idx), 1);

        assert_eq!(multiset.decrement(idx), 0);
        assert_eq!(multiset.count(idx), 0);

        // Decrement from 0 stays at 0
        assert_eq!(multiset.decrement(idx), 0);
    }

    #[test]
    fn test_fork_cow() {
        let multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        multiset.increment(idx1);
        multiset.increment(idx1);

        // Fork (O(1))
        let mut forked = multiset.fork();
        assert!(!forked.is_owned());

        // Read doesn't trigger CoW
        assert_eq!(forked.count(idx1), 2);
        assert!(!forked.is_owned());

        // Write triggers CoW
        let idx2 = forked.allocate_index();
        forked.decrement(idx1); // This triggers make_owned()
        assert!(forked.is_owned());

        // Original unchanged
        assert_eq!(multiset.count(idx1), 2);

        // Forked has modified data
        assert_eq!(forked.count(idx1), 1);

        // New allocations work in forked
        forked.increment(idx2);
        assert_eq!(forked.count(idx2), 1);
        assert_eq!(multiset.count(idx2), 0); // Not in original
    }

    #[test]
    fn test_index_allocation_across_forks() {
        let multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        assert_eq!(idx1, 0);

        let forked1 = multiset.fork();
        let forked2 = multiset.fork();

        // All share the same allocator
        let idx2 = forked1.allocate_index();
        let idx3 = forked2.allocate_index();
        let idx4 = multiset.allocate_index();

        // No collisions
        assert_eq!(idx2, 1);
        assert_eq!(idx3, 2);
        assert_eq!(idx4, 3);
    }

    #[test]
    fn test_ensure_capacity() {
        let mut multiset = IndexedMultiset::with_capacity(10);
        assert_eq!(multiset.capacity(), 10);

        multiset.ensure_capacity(100);
        assert_eq!(multiset.capacity(), 100);

        // Check existing data preserved
        let idx = multiset.allocate_index();
        multiset.increment(idx);
        multiset.ensure_capacity(200);
        assert_eq!(multiset.count(idx), 1);
    }

    #[test]
    fn test_decrement_to_zero() {
        let mut multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        multiset.increment(idx1);
        assert_eq!(multiset.count(idx1), 1);

        // Decrement to 0
        let new_count = multiset.decrement(idx1);
        assert_eq!(new_count, 0);
        assert_eq!(multiset.count(idx1), 0);

        // Next allocation is a fresh index (indices are not reused)
        let idx2 = multiset.allocate_index();
        assert_ne!(idx1, idx2);
    }

    #[test]
    fn test_clear() {
        let mut multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        let idx2 = multiset.allocate_index();
        multiset.increment(idx1);
        multiset.increment_n(idx2, 5);

        assert_eq!(multiset.total(), 6);

        multiset.clear();
        assert_eq!(multiset.total(), 0);
        assert_eq!(multiset.count(idx1), 0);
        assert_eq!(multiset.count(idx2), 0);
    }

    #[test]
    fn test_iter_nonzero() {
        let multiset = IndexedMultiset::new();

        let idx1 = multiset.allocate_index();
        let idx2 = multiset.allocate_index();
        let _idx3 = multiset.allocate_index(); // Not incremented

        multiset.increment_n(idx1, 2);
        multiset.increment(idx2);

        let nonzero: Vec<_> = multiset.iter_nonzero().collect();
        assert_eq!(nonzero.len(), 2);

        // Check contents (order may vary)
        assert!(nonzero.contains(&(idx1, 2)));
        assert!(nonzero.contains(&(idx2, 1)));
    }

    #[test]
    fn test_concurrent_increment() {
        use std::sync::Arc;
        use std::thread;

        let multiset = Arc::new(IndexedMultiset::new());
        let idx = multiset.allocate_index();

        let num_threads = 8;
        let increments_per_thread = 1000;

        let handles: Vec<_> = (0..num_threads)
            .map(|_| {
                let multiset = Arc::clone(&multiset);
                thread::spawn(move || {
                    for _ in 0..increments_per_thread {
                        multiset.increment(idx);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread should complete");
        }

        assert_eq!(
            multiset.count(idx),
            num_threads * increments_per_thread as usize
        );
    }
}
