//! Generic Memoization Cache for Bytecode VM
//!
//! This module provides a thread-safe memoization cache that works with any
//! value type implementing `MettaValueTrait`. Uses `v.hash_value()` instead of
//! requiring `Hash` trait bounds. Includes GC root registration via a global
//! singleton for the `MettaValue` monomorphization.
//!
//! # Design
//!
//! - LRU eviction when capacity is reached
//! - Lock-free reads and writes via DashMap and atomics
//! - Content-addressed via `MettaValueTrait::hash_value()`
//! - Configurable maximum entries
//! - Global singleton for `MettaValue` with GC root registration

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};

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
    #[cfg(feature = "track-stats")]
    hits: AtomicU64,
    /// Miss count for statistics (atomic, lock-free)
    #[cfg(feature = "track-stats")]
    misses: AtomicU64,
}

impl<V: MettaValueTrait + Clone + Send + Sync + 'static> std::fmt::Debug for GenericMemoCache<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("GenericMemoCache");
        s.field("entries", &self.cache.len());
        s.field("max_entries", &self.max_entries);
        #[cfg(feature = "track-stats")]
        {
            s.field("hits", &self.hits.load(Ordering::Relaxed));
            s.field("misses", &self.misses.load(Ordering::Relaxed));
        }
        s.finish()
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
            #[cfg(feature = "track-stats")]
            hits: AtomicU64::new(0),
            #[cfg(feature = "track-stats")]
            misses: AtomicU64::new(0),
        }
    }

    /// Look up a cached result.
    pub fn get(&self, head: &str, args: &[V]) -> Option<V> {
        let key = GenericMemoKey::new(head, args);

        if let Some(mut entry) = self.cache.get_mut(&key) {
            let new_count = self.access_counter.fetch_add(1, Ordering::Relaxed) + 1;
            entry.access_count = new_count;
            #[cfg(feature = "track-stats")]
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry.result.clone())
        } else {
            #[cfg(feature = "track-stats")]
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
    #[cfg(feature = "track-stats")]
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

    /// Collect all cached result values into the provided vector.
    ///
    /// Used by the GC root provider to expose all cached MettaValue entries
    /// to the garbage collector's root set.
    pub fn collect_all_values(&self, out: &mut Vec<V>) {
        out.reserve(self.cache.len());
        for entry in self.cache.iter() {
            out.push(entry.value().result.clone());
        }
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

// =============================================================================
// Global Singleton Memo Cache + GC Root Provider
// =============================================================================

use crate::backend::models::gc_allocator::{register_root_provider, RootProvider};
use crate::backend::models::MettaValue;

/// Global singleton `GenericMemoCache<MettaValue>` shared across all VM instances.
///
/// Uses `LazyLock` for zero-cost lazy initialization. Cache size is configurable
/// via `METTA_MEMO_CACHE_SIZE` environment variable (default: 4096).
static GLOBAL_MEMO_CACHE: LazyLock<Arc<GenericMemoCache<MettaValue>>> = LazyLock::new(|| {
    let max_entries = std::env::var("METTA_MEMO_CACHE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4096);
    Arc::new(GenericMemoCache::new(max_entries))
});

/// GC root provider that exposes all MettaValue entries stored in the global
/// memo cache to the garbage collector's root set.
///
/// Without this, cached function results are invisible to GC and may be freed
/// while still reachable from the cache, causing use-after-free.
struct MemoCacheRoots;

impl RootProvider for MemoCacheRoots {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        GLOBAL_MEMO_CACHE.collect_all_values(roots);
    }
}

/// Keeps the `Arc<dyn RootProvider>` alive for the lifetime of the process so
/// the `Weak` reference in `ROOT_REGISTRY` remains valid.
static MEMO_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();

/// Ensure the global memo cache is registered as a GC root provider.
///
/// Called lazily on first access to the global cache. Idempotent —
/// `OnceLock` guarantees single initialization.
pub fn ensure_memo_cache_roots_registered() {
    MEMO_CACHE_ROOT_PROVIDER.get_or_init(|| {
        let provider = Arc::new(MemoCacheRoots) as Arc<dyn RootProvider>;
        register_root_provider(&provider);
        provider
    });
}

/// Get a reference to the global `GenericMemoCache<MettaValue>`.
///
/// On first call, this also registers the cache as a GC root provider
/// (idempotent). All subsequent calls return the same `Arc`.
pub fn global_memo_cache() -> &'static Arc<GenericMemoCache<MettaValue>> {
    ensure_memo_cache_roots_registered();
    &GLOBAL_MEMO_CACHE
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
    #[cfg(feature = "track-stats")]
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

    #[test]
    fn test_global_memo_cache_returns_same_arc() {
        let cache1 = global_memo_cache();
        let cache2 = global_memo_cache();
        assert!(Arc::ptr_eq(cache1, cache2), "global_memo_cache() must return the same Arc");
    }

    #[test]
    fn test_collect_all_values() {
        let cache: GenericMemoCache<MettaValue> = GenericMemoCache::new(100);
        let factory = GcFactory::default();

        cache.insert("a", &[factory.long(1)], factory.long(10));
        cache.insert("b", &[factory.long(2)], factory.long(20));
        cache.insert("c", &[factory.long(3)], factory.long(30));

        let mut roots = Vec::new();
        cache.collect_all_values(&mut roots);
        assert_eq!(roots.len(), 3);

        // Check that all results are present (order not guaranteed)
        let mut values: Vec<i64> = roots
            .iter()
            .map(|v| match v.inner() {
                crate::backend::models::MettaValueInner::Long(n) => *n,
                _ => panic!("expected Long"),
            })
            .collect();
        values.sort();
        assert_eq!(values, vec![10, 20, 30]);
    }

    #[test]
    fn test_ensure_memo_cache_roots_registered_is_idempotent() {
        // Should not panic when called multiple times
        ensure_memo_cache_roots_registered();
        ensure_memo_cache_roots_registered();
        ensure_memo_cache_roots_registered();
    }
}
