//! Memoization table for MeTTa expressions.
//!
//! Provides explicit, user-controlled memoization for expensive computations.
//! Memo tables cache evaluation results indexed by expression hash.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use super::MettaValue;

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
/// - Tracks hit/miss statistics for performance analysis
#[derive(Debug, Clone)]
pub struct MemoHandle {
    /// Unique identifier for this memo table
    pub id: u64,
    /// Optional name for debugging/display
    pub name: String,
    /// Shared mutable state
    inner: Arc<RwLock<MemoInner>>,
}

/// Internal memoization state
#[derive(Debug)]
struct MemoInner {
    /// Cache: expression_hash -> cached results
    cache: HashMap<u64, MemoEntry>,
    /// Cache hit counter
    hits: u64,
    /// Cache miss counter
    misses: u64,
    /// Maximum cache size (0 = unlimited)
    max_size: usize,
    /// LRU order tracking (most recent at end)
    /// Only populated when max_size > 0
    lru_order: Vec<u64>,
}

/// A single cache entry
#[derive(Debug, Clone)]
struct MemoEntry {
    /// The original expression (for debugging/verification)
    #[allow(dead_code)]
    expression: MettaValue,
    /// Cached evaluation results
    results: Vec<MettaValue>,
    /// Whether this was a first-only cache (memo-first vs memo)
    first_only: bool,
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
                hits: 0,
                misses: 0,
                max_size,
                lru_order: Vec::new(),
            })),
        }
    }

    /// Compute hash for a MettaValue expression
    #[inline]
    fn hash_expression(expr: &MettaValue) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        expr.hash(&mut hasher);
        hasher.finish()
    }

    /// Look up cached results for an expression
    ///
    /// Returns Some(results) if cached, None if miss.
    /// Updates hit/miss counters.
    pub fn lookup(&self, expr: &MettaValue) -> Option<Vec<MettaValue>> {
        let hash = Self::hash_expression(expr);
        let mut inner = self.inner.write().unwrap();

        // Check if entry exists and get results if so
        let result = inner.cache.get(&hash).map(|entry| entry.results.clone());

        if result.is_some() {
            inner.hits += 1;

            // Update LRU order if tracking
            if inner.max_size > 0 {
                if let Some(pos) = inner.lru_order.iter().position(|&h| h == hash) {
                    inner.lru_order.remove(pos);
                    inner.lru_order.push(hash);
                }
            }
        } else {
            inner.misses += 1;
        }

        result
    }

    /// Store evaluation results for an expression
    ///
    /// If max_size is set and exceeded, evicts LRU entry.
    pub fn store(&self, expr: &MettaValue, results: Vec<MettaValue>, first_only: bool) {
        let hash = Self::hash_expression(expr);
        let mut inner = self.inner.write().unwrap();

        // Check if we need to evict (before inserting)
        if inner.max_size > 0 && inner.cache.len() >= inner.max_size {
            // Evict LRU entry
            if let Some(lru_hash) = inner.lru_order.first().copied() {
                inner.cache.remove(&lru_hash);
                inner.lru_order.remove(0);
            }
        }

        // Insert new entry
        inner.cache.insert(
            hash,
            MemoEntry {
                expression: expr.clone(),
                results,
                first_only,
            },
        );

        // Update LRU order
        if inner.max_size > 0 {
            inner.lru_order.push(hash);
        }
    }

    /// Clear all cached entries
    pub fn clear(&self) {
        let mut inner = self.inner.write().unwrap();
        inner.cache.clear();
        inner.lru_order.clear();
        // Note: don't reset hit/miss counters - those are cumulative stats
    }

    /// Get statistics about this memo table
    ///
    /// Returns (hits, misses, current_size, max_size)
    pub fn stats(&self) -> (u64, u64, usize, usize) {
        let inner = self.inner.read().unwrap();
        (
            inner.hits,
            inner.misses,
            inner.cache.len(),
            inner.max_size,
        )
    }

    /// Get the hit rate as a percentage (0.0 - 100.0)
    pub fn hit_rate(&self) -> f64 {
        let inner = self.inner.read().unwrap();
        let total = inner.hits + inner.misses;
        if total == 0 {
            0.0
        } else {
            (inner.hits as f64 / total as f64) * 100.0
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
