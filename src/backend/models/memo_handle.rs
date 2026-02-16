//! Memoization table for MeTTa expressions.
//!
//! Provides explicit, user-controlled memoization for expensive computations.
//! Memo tables cache evaluation results indexed by expression hash.
//!
//! ## Type-Agnostic Storage
//!
//! Memos are stored as serialized bytes, enabling:
//! - Storage of any value type implementing `MettaValueTrait::serialize()`
//! - Retrieval into any value type via `MettaValueFactory::deserialize()`
//! - Zero deep copies between value types - just serialization/deserialization

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;

use xxhash_rust::xxh3::xxh3_64;

use super::metta_value_trait::{MettaValueTrait, MettaValueFactory};
use super::{GcFactory, MettaValue};

/// Global counter for unique memo IDs
static NEXT_MEMO_ID: AtomicU64 = AtomicU64::new(1);

/// A memoization table handle.
///
/// MemoHandle provides O(1) lookup for previously evaluated expressions.
/// It stores results indexed by expression hash, enabling significant
/// speedups for repeated computations.
///
/// # Design
/// - Uses expression hash as cache key (not structural equality)
/// - Supports optional LRU eviction with configurable max size
/// - Thread-safe via RwLock for concurrent access
/// - Lock-free hit/miss statistics via AtomicU64 (no write lock needed for reads)
/// - Type-agnostic storage via byte serialization
#[derive(Debug)]
pub struct MemoHandle {
    /// Unique identifier for this memo table
    pub id: u64,
    /// Optional name for debugging/display
    pub name: String,
    /// Shared mutable state (cache only)
    inner: Arc<RwLock<MemoInner>>,
    /// Cache hit counter (lock-free atomic)
    hits: AtomicU64,
    /// Cache miss counter (lock-free atomic)
    misses: AtomicU64,
}

/// Internal memoization state (cache only; counters are external atomics)
#[derive(Debug)]
struct MemoInner {
    /// Cache: expression_hash -> cached results (stored as bytes)
    cache: HashMap<u64, MemoEntry>,
    /// Maximum cache size (0 = unlimited)
    max_size: usize,
    /// LRU order tracking (most recent at end)
    /// Only populated when max_size > 0
    lru_order: Vec<u64>,
}

/// A single cache entry (stores serialized bytes)
#[derive(Debug, Clone)]
struct MemoEntry {
    /// Cached evaluation results (serialized bytes)
    results_bytes: Vec<Vec<u8>>,
}

impl MemoHandle {
    /// Create a new memo table with no size limit
    pub fn new(name: String) -> Self {
        Self::with_max_size(name, 0)
    }

    /// Create a new memo table with LRU eviction at max_size
    pub fn with_max_size(name: String, max_size: usize) -> Self {
        let id = NEXT_MEMO_ID.fetch_add(1, Ordering::SeqCst);
        MemoHandle {
            id,
            name,
            inner: Arc::new(RwLock::new(MemoInner {
                cache: HashMap::new(),
                max_size,
                lru_order: Vec::new(),
            })),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Compute hash from serialized bytes
    #[inline]
    fn hash_bytes(bytes: &[u8]) -> u64 {
        xxh3_64(bytes)
    }

    /// Look up cached results for an expression (generic version using serialization).
    ///
    /// This version doesn't require `Hash` - it serializes the expression to bytes
    /// and hashes the bytes. This enables use with any `MettaValueTrait` implementation.
    ///
    /// Returns Some(results) if cached, None if miss.
    /// Updates hit/miss counters via lock-free atomics.
    pub fn lookup_generic<V: MettaValueTrait, F: MettaValueFactory<V>>(
        &self,
        expr: &V,
        factory: &F,
    ) -> Option<Vec<V>> {
        let hash = Self::hash_bytes(&expr.serialize());

        // Fast path: check cache with read lock, update atomics without lock
        {
            let inner = self.inner.read();
            if let Some(entry) = inner.cache.get(&hash) {
                // Deserialize results using the provided factory
                let results: Vec<V> = entry
                    .results_bytes
                    .iter()
                    .filter_map(|bytes| factory.deserialize(bytes).ok().map(|(v, _)| v))
                    .collect();

                // Update LRU requires write lock - defer to slow path if needed
                if inner.max_size > 0 {
                    drop(inner);
                    // Slow path: update LRU order with write lock
                    let mut inner = self.inner.write();
                    if let Some(pos) = inner.lru_order.iter().position(|&h| h == hash) {
                        inner.lru_order.remove(pos);
                        inner.lru_order.push(hash);
                    }
                }
                self.hits.fetch_add(1, Ordering::Relaxed);
                return Some(results);
            }
        }

        // Cache miss - only atomic increment needed
        self.misses.fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Look up cached results for an expression (legacy MettaValue version).
    pub fn lookup(&self, expr: &MettaValue) -> Option<Vec<MettaValue>> {
        self.lookup_generic(expr, &GcFactory::default())
    }

    /// Store evaluation results for an expression (generic version using serialization).
    ///
    /// This version doesn't require `Hash` - it serializes the expression to bytes
    /// and hashes the bytes. This enables use with any `MettaValueTrait` implementation.
    ///
    /// If max_size is set and exceeded, evicts LRU entry.
    pub fn store_generic<V: MettaValueTrait>(&self, expr: &V, results: &[V]) {
        let hash = Self::hash_bytes(&expr.serialize());
        let mut inner = self.inner.write();

        // Check if we need to evict (before inserting)
        if inner.max_size > 0 && inner.cache.len() >= inner.max_size {
            // Evict LRU entry
            if let Some(lru_hash) = inner.lru_order.first().copied() {
                inner.cache.remove(&lru_hash);
                inner.lru_order.remove(0);
            }
        }

        // Serialize the results to bytes
        let results_bytes: Vec<Vec<u8>> = results.iter().map(|r| r.serialize()).collect();

        // Insert new entry
        inner.cache.insert(
            hash,
            MemoEntry {
                results_bytes,
            },
        );

        // Update LRU order
        if inner.max_size > 0 {
            inner.lru_order.push(hash);
        }
    }

    /// Store evaluation results for an expression (legacy MettaValue version).
    ///
    /// If max_size is set and exceeded, evicts LRU entry.
    /// The `first_only` parameter is retained for API compatibility but no longer stored.
    pub fn store(&self, expr: &MettaValue, results: Vec<MettaValue>, _first_only: bool) {
        self.store_generic(expr, &results)
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.cache.clear();
        inner.lru_order.clear();
        // Note: don't reset hit/miss counters - those are cumulative stats
    }

    /// Get statistics about this memo table
    ///
    /// Returns (hits, misses, current_size, max_size)
    pub fn stats(&self) -> (u64, u64, usize, usize) {
        let inner = self.inner.read();
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        (hits, misses, inner.cache.len(), inner.max_size)
    }

    /// Get the hit rate as a percentage (0.0 - 100.0)
    pub fn hit_rate(&self) -> f64 {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        let total = hits + misses;
        if total == 0 {
            0.0
        } else {
            (hits as f64 / total as f64) * 100.0
        }
    }
}

impl Clone for MemoHandle {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            name: self.name.clone(),
            inner: Arc::clone(&self.inner),
            hits: AtomicU64::new(self.hits.load(Ordering::Relaxed)),
            misses: AtomicU64::new(self.misses.load(Ordering::Relaxed)),
        }
    }
}

// Implement PartialEq based on id (identity comparison)
impl PartialEq for MemoHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for MemoHandle {}

// Implement Hash based on id
impl Hash for MemoHandle {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memo_basic() {
        let memo = MemoHandle::new("test".to_string());

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Long(42),
        ]);

        // Initially not cached
        assert!(memo.lookup(&expr).is_none());

        // Store result
        let results = vec![MettaValue::Long(84)];
        memo.store(&expr, results.clone(), false);

        // Now cached
        let cached = memo.lookup(&expr);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap(), results);

        // Stats should show 1 miss, 1 hit
        let (hits, misses, size, _) = memo.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
        assert_eq!(size, 1);
    }

    #[test]
    fn test_memo_lru_eviction() {
        let memo = MemoHandle::with_max_size("lru-test".to_string(), 2);

        let expr1 = MettaValue::Long(1);
        let expr2 = MettaValue::Long(2);
        let expr3 = MettaValue::Long(3);

        memo.store(&expr1, vec![MettaValue::Long(10)], false);
        memo.store(&expr2, vec![MettaValue::Long(20)], false);

        // Both should be cached
        assert!(memo.lookup(&expr1).is_some());
        assert!(memo.lookup(&expr2).is_some());

        // Adding third should evict LRU
        // After lookups above: expr1 was accessed, then expr2 was accessed
        // So expr2 is MRU (most recent) and expr1 is LRU (least recent)
        memo.store(&expr3, vec![MettaValue::Long(30)], false);

        // expr1 should be evicted (was LRU after both lookups)
        assert!(memo.lookup(&expr1).is_none()); // evicted - was LRU
        assert!(memo.lookup(&expr2).is_some()); // kept - was MRU before expr3
        assert!(memo.lookup(&expr3).is_some());
    }

    #[test]
    fn test_memo_clear() {
        let memo = MemoHandle::new("clear-test".to_string());

        let expr = MettaValue::Long(42);
        memo.store(&expr, vec![MettaValue::Bool(true)], false);
        assert!(memo.lookup(&expr).is_some());

        memo.clear();

        // Should be gone after clear
        assert!(memo.lookup(&expr).is_none());
    }

    #[test]
    fn test_memo_hit_rate() {
        let memo = MemoHandle::new("hit-rate-test".to_string());

        let expr = MettaValue::Long(1);
        memo.store(&expr, vec![MettaValue::Long(1)], false);

        // 1 hit
        memo.lookup(&expr);
        // 1 miss
        memo.lookup(&MettaValue::Long(999));

        let rate = memo.hit_rate();
        assert!((rate - 50.0).abs() < 0.01);
    }
}
