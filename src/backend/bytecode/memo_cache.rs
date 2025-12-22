//! Memoization Cache for Bytecode VM
//!
//! This module provides a thread-safe memoization cache for pure function calls.
//! The cache stores results keyed by (function_head, args_hash) pairs.
//!
//! # Design
//!
//! - LRU eviction when capacity is reached
//! - Thread-safe via RwLock
//! - Content-addressed via hash of function head and arguments
//! - Configurable maximum entries
//!
//! # Example
//!
//! ```ignore
//! let mut cache = MemoCache::new(1000);
//!
//! // Check cache
//! if let Some(result) = cache.get("factorial", &[MettaValue::Long(10)]) {
//!     return result;
//! }
//!
//! // Compute result...
//! let result = compute_factorial(10);
//!
//! // Cache it
//! cache.insert("factorial", &[MettaValue::Long(10)], result.clone());
//! ```

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::RwLock;

use crate::backend::models::MettaValue;

/// Key for memo cache entries
#[derive(Clone, Eq, PartialEq, Hash)]
struct MemoKey {
    /// Function head symbol
    func_head: String,
    /// Hash of arguments
    args_hash: u64,
}

impl MemoKey {
    /// Create a new memo key
    fn new(head: &str, args: &[MettaValue]) -> Self {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        for arg in args {
            // MettaValue implements Hash trait directly
            arg.hash(&mut hasher);
        }
        MemoKey {
            func_head: head.to_string(),
            args_hash: hasher.finish(),
        }
    }
}

/// Entry in the memo cache
#[derive(Clone, Debug)]
struct MemoEntry {
    /// Cached result
    result: MettaValue,
    /// Access count for LRU
    access_count: u64,
}

/// Memoization cache for pure function calls
pub struct MemoCache {
    /// Cache storage
    cache: RwLock<HashMap<MemoKey, MemoEntry>>,
    /// Maximum number of entries
    max_entries: usize,
    /// Global access counter for LRU
    access_counter: RwLock<u64>,
    /// Hit count for statistics
    hits: RwLock<u64>,
    /// Miss count for statistics
    misses: RwLock<u64>,
}

impl std::fmt::Debug for MemoCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache = self.cache.read().unwrap();
        let hits = *self.hits.read().unwrap();
        let misses = *self.misses.read().unwrap();
        f.debug_struct("MemoCache")
            .field("entries", &cache.len())
            .field("max_entries", &self.max_entries)
            .field("hits", &hits)
            .field("misses", &misses)
            .finish()
    }
}

impl Default for MemoCache {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl MemoCache {
    /// Create a new memo cache with specified capacity
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(HashMap::with_capacity(max_entries)),
            max_entries,
            access_counter: RwLock::new(0),
            hits: RwLock::new(0),
            misses: RwLock::new(0),
        }
    }

    /// Look up a cached result
    pub fn get(&self, head: &str, args: &[MettaValue]) -> Option<MettaValue> {
        let key = MemoKey::new(head, args);

        let mut cache = self.cache.write().unwrap();
        if let Some(entry) = cache.get_mut(&key) {
            // Update access count for LRU
            let mut counter = self.access_counter.write().unwrap();
            *counter += 1;
            entry.access_count = *counter;

            *self.hits.write().unwrap() += 1;
            Some(entry.result.clone())
        } else {
            *self.misses.write().unwrap() += 1;
            None
        }
    }

    /// Insert a result into the cache
    pub fn insert(&self, head: &str, args: &[MettaValue], result: MettaValue) {
        let key = MemoKey::new(head, args);

        let mut cache = self.cache.write().unwrap();

        // Evict if at capacity
        if cache.len() >= self.max_entries && !cache.contains_key(&key) {
            self.evict_lru(&mut cache);
        }

        let mut counter = self.access_counter.write().unwrap();
        *counter += 1;

        cache.insert(
            key,
            MemoEntry {
                result,
                access_count: *counter,
            },
        );
    }

    /// Evict least recently used entries
    fn evict_lru(&self, cache: &mut HashMap<MemoKey, MemoEntry>) {
        // Evict ~25% of entries
        let to_evict = (self.max_entries / 4).max(1);

        // Find entries with lowest access counts
        let mut entries: Vec<_> = cache.iter().map(|(k, v)| (k.clone(), v.access_count)).collect();
        entries.sort_by_key(|(_, count)| *count);

        for (key, _) in entries.into_iter().take(to_evict) {
            cache.remove(&key);
        }
    }

    /// Clear the cache
    pub fn clear(&self) {
        let mut cache = self.cache.write().unwrap();
        cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap();
        let hits = *self.hits.read().unwrap();
        let misses = *self.misses.read().unwrap();

        CacheStats {
            entries: cache.len(),
            max_entries: self.max_entries,
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                hits as f64 / (hits + misses) as f64
            } else {
                0.0
            },
        }
    }

    /// Get the number of entries in the cache
    pub fn len(&self) -> usize {
        self.cache.read().unwrap().len()
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.cache.read().unwrap().is_empty()
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Current number of entries
    pub entries: usize,
    /// Maximum entries allowed
    pub max_entries: usize,
    /// Number of cache hits
    pub hits: u64,
    /// Number of cache misses
    pub misses: u64,
    /// Hit rate (0.0 - 1.0)
    pub hit_rate: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memo_cache_basic() {
        let cache = MemoCache::new(100);

        // Miss on first lookup
        assert!(cache.get("foo", &[MettaValue::Long(42)]).is_none());

        // Insert
        cache.insert("foo", &[MettaValue::Long(42)], MettaValue::Long(84));

        // Hit on second lookup
        let result = cache.get("foo", &[MettaValue::Long(42)]);
        assert_eq!(result, Some(MettaValue::Long(84)));
    }

    #[test]
    fn test_memo_cache_different_args() {
        let cache = MemoCache::new(100);

        cache.insert("double", &[MettaValue::Long(5)], MettaValue::Long(10));
        cache.insert("double", &[MettaValue::Long(7)], MettaValue::Long(14));

        assert_eq!(
            cache.get("double", &[MettaValue::Long(5)]),
            Some(MettaValue::Long(10))
        );
        assert_eq!(
            cache.get("double", &[MettaValue::Long(7)]),
            Some(MettaValue::Long(14))
        );
        assert!(cache.get("double", &[MettaValue::Long(9)]).is_none());
    }

    #[test]
    fn test_memo_cache_eviction() {
        let cache = MemoCache::new(4);

        // Fill cache
        for i in 0..4 {
            cache.insert("f", &[MettaValue::Long(i)], MettaValue::Long(i * 2));
        }

        assert_eq!(cache.len(), 4);

        // Add one more - should trigger eviction
        cache.insert("f", &[MettaValue::Long(10)], MettaValue::Long(20));

        // Should be under capacity
        assert!(cache.len() <= 4);

        // New entry should be present
        assert_eq!(
            cache.get("f", &[MettaValue::Long(10)]),
            Some(MettaValue::Long(20))
        );
    }

    #[test]
    fn test_memo_cache_stats() {
        let cache = MemoCache::new(100);

        cache.insert("f", &[MettaValue::Long(1)], MettaValue::Long(1));

        // Miss
        cache.get("f", &[MettaValue::Long(2)]);
        // Hit
        cache.get("f", &[MettaValue::Long(1)]);
        // Hit
        cache.get("f", &[MettaValue::Long(1)]);

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 0.666).abs() < 0.01);
    }

    #[test]
    fn test_memo_cache_complex_args() {
        let cache = MemoCache::new(100);

        let args = vec![
            MettaValue::SExpr(vec![MettaValue::sym("a"), MettaValue::Long(1)]),
            MettaValue::String("test".to_string()),
        ];

        cache.insert("complex", &args, MettaValue::Bool(true));

        assert_eq!(
            cache.get("complex", &args),
            Some(MettaValue::Bool(true))
        );
    }
}
