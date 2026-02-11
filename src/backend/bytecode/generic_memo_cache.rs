//! Generic Memoization Cache for Bytecode VM
//!
//! This module provides a thread-safe memoization cache that works with any
//! value type implementing `MettaValueTrait`. It mirrors `MemoCache` but uses
//! `v.hash_value()` instead of requiring `Hash` trait bounds.
//!
//! # Design
//!
//! - LRU eviction when capacity is reached
//! - Lock-free reads and writes via DashMap and atomics
//! - Content-addressed via `MettaValueTrait::hash_value()`
//! - Configurable maximum entries

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use xxhash_rust::xxh3::Xxh3;

use dashmap::DashMap;

use crate::backend::models::metta_value_trait::MettaValueTrait;

/// Key for generic memo cache entries
#[derive(Clone, Eq, PartialEq, Hash)]
struct GenericMemoKey {
    /// Function head symbol
    func_head: String,
    /// Hash of arguments (computed via MettaValueTrait::hash_value)
    args_hash: u64,
}

impl GenericMemoKey {
    /// Create a new memo key from head symbol and generic arguments.
    fn new<V: MettaValueTrait>(head: &str, args: &[V]) -> Self {
        // Collect all hash values into a buffer and hash them together
        let hash_values: Vec<u64> = args.iter().map(|arg| arg.hash_value()).collect();
        let mut h = Xxh3::new();
        hash_values.hash(&mut h);
        GenericMemoKey {
            func_head: head.to_string(),
            args_hash: h.finish(),
        }
    }
}

/// Entry in the generic memo cache
#[derive(Clone)]
struct GenericMemoEntry<V> {
    /// Cached result
    result: V,
    /// Access count for LRU
    access_count: u64,
}

/// Generic memoization cache for pure function calls.
///
/// Uses DashMap for lock-free concurrent access and AtomicU64 for counters.
/// Works with any value type implementing `MettaValueTrait`.
pub struct GenericMemoCache<V: MettaValueTrait + Clone + Send + Sync + 'static> {
    /// Cache storage (lock-free concurrent HashMap)
    cache: DashMap<GenericMemoKey, GenericMemoEntry<V>>,
    /// Maximum number of entries
    max_entries: usize,
    /// Global access counter for LRU (atomic, lock-free)
    access_counter: AtomicU64,
    /// Hit count for statistics (atomic, lock-free)
    hits: AtomicU64,
    /// Miss count for statistics (atomic, lock-free)
    misses: AtomicU64,
}

impl<V: MettaValueTrait + Clone + Send + Sync + 'static> std::fmt::Debug for GenericMemoCache<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericMemoCache")
            .field("entries", &self.cache.len())
            .field("max_entries", &self.max_entries)
            .field("hits", &self.hits.load(Ordering::Relaxed))
            .field("misses", &self.misses.load(Ordering::Relaxed))
            .finish()
    }
}

impl<V: MettaValueTrait + Clone + Send + Sync + 'static> Default for GenericMemoCache<V> {
    fn default() -> Self {
        Self::new(1000)
    }
}

impl<V: MettaValueTrait + Clone + Send + Sync + 'static> GenericMemoCache<V> {
    /// Create a new generic memo cache with specified capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: DashMap::with_capacity(max_entries),
            max_entries,
            access_counter: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Look up a cached result.
    pub fn get(&self, head: &str, args: &[V]) -> Option<V> {
        let key = GenericMemoKey::new(head, args);

        if let Some(mut entry) = self.cache.get_mut(&key) {
            let new_count = self.access_counter.fetch_add(1, Ordering::Relaxed) + 1;
            entry.access_count = new_count;
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.result.clone())
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a result into the cache.
    pub fn insert(&self, head: &str, args: &[V], result: V) {
        let key = GenericMemoKey::new(head, args);

        // Evict if at capacity
        if self.cache.len() >= self.max_entries && !self.cache.contains_key(&key) {
            self.evict_lru();
        }

        let new_count = self.access_counter.fetch_add(1, Ordering::Relaxed) + 1;
        self.cache.insert(
            key,
            GenericMemoEntry {
                result,
                access_count: new_count,
            },
        );
    }

    /// Evict least recently used entries (~25%).
    fn evict_lru(&self) {
        let to_evict = (self.max_entries / 4).max(1);
        let mut entries: Vec<_> = self
            .cache
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().access_count))
            .collect();
        entries.sort_by_key(|(_, count)| *count);
        for (key, _) in entries.into_iter().take(to_evict) {
            self.cache.remove(&key);
        }
    }

    /// Clear the cache.
    pub fn clear(&self) {
        self.cache.clear();
    }

    /// Get cache statistics.
    pub fn stats(&self) -> GenericCacheStats {
        let hits = self.hits.load(Ordering::Relaxed);
        let misses = self.misses.load(Ordering::Relaxed);
        GenericCacheStats {
            entries: self.cache.len(),
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

    /// Get the number of entries in the cache.
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    /// Check if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

/// Generic cache statistics
#[derive(Debug, Clone)]
pub struct GenericCacheStats {
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
    use crate::backend::models::{GcFactory, MettaValue, MettaValueFactory};

    #[test]
    fn test_generic_memo_cache_basic() {
        let cache = GenericMemoCache::new(100);
        let factory = GcFactory::default();

        // Miss on first lookup
        assert!(cache.get("foo", &[factory.long(42)]).is_none());

        // Insert
        cache.insert("foo", &[factory.long(42)], factory.long(84));

        // Hit on second lookup
        let result = cache.get("foo", &[factory.long(42)]);
        assert_eq!(result, Some(factory.long(84)));
    }

    #[test]
    fn test_generic_memo_cache_different_args() {
        let cache: GenericMemoCache<MettaValue> = GenericMemoCache::new(100);
        let factory = GcFactory::default();

        cache.insert("double", &[factory.long(5)], factory.long(10));
        cache.insert("double", &[factory.long(7)], factory.long(14));

        assert_eq!(cache.get("double", &[factory.long(5)]), Some(factory.long(10)));
        assert_eq!(cache.get("double", &[factory.long(7)]), Some(factory.long(14)));
        assert!(cache.get("double", &[factory.long(9)]).is_none());
    }

    #[test]
    fn test_generic_memo_cache_eviction() {
        let cache: GenericMemoCache<MettaValue> = GenericMemoCache::new(4);
        let factory = GcFactory::default();

        for i in 0..4 {
            cache.insert("f", &[factory.long(i)], factory.long(i * 2));
        }
        assert_eq!(cache.len(), 4);

        cache.insert("f", &[factory.long(10)], factory.long(20));
        assert!(cache.len() <= 4);
        assert_eq!(cache.get("f", &[factory.long(10)]), Some(factory.long(20)));
    }

    #[test]
    fn test_generic_memo_cache_stats() {
        let cache: GenericMemoCache<MettaValue> = GenericMemoCache::new(100);
        let factory = GcFactory::default();

        cache.insert("f", &[factory.long(1)], factory.long(1));
        cache.get("f", &[factory.long(2)]); // miss
        cache.get("f", &[factory.long(1)]); // hit
        cache.get("f", &[factory.long(1)]); // hit

        let stats = cache.stats();
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
        assert!((stats.hit_rate - 0.666).abs() < 0.01);
    }
}
