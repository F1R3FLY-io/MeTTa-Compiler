//! Bytecode compilation caching
//!
//! Provides caching to eliminate redundant work:
//!
//! ## Active Cache
//! - `can_compile` cache: Boolean results for compilability checks (safe to cache)
//!
//! ## Disabled Cache (kept for future reference)
//! - `bytecode` cache: Compiled Arc<BytecodeChunk> - DISABLED because expressions
//!   with the same structure can have different runtime values when variables are
//!   bound differently. Only safe for pure expressions without variables.
//!
//! Both caches use LRU eviction for bounded memory usage.

use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
#[cfg(feature = "track-stats")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use parking_lot::RwLock;
use xxhash_rust::xxh3::Xxh3;

use lru::LruCache;

use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::hash_utils::IdentityU64BuildHasher;
use crate::backend::models::{register_root_provider, MettaValue, RootProvider, ValueView};

/// Statistics for bytecode cache monitoring (lock-free atomics).
#[cfg(feature = "track-stats")]
#[derive(Debug)]
pub struct BytecodeCacheStats {
    /// can_compile cache hits
    pub can_compile_hits: AtomicU64,
    /// can_compile cache misses
    pub can_compile_misses: AtomicU64,
    /// bytecode cache hits
    pub bytecode_hits: AtomicU64,
    /// bytecode cache misses (compilations)
    pub bytecode_misses: AtomicU64,
}

#[cfg(feature = "track-stats")]
impl Default for BytecodeCacheStats {
    fn default() -> Self {
        Self {
            can_compile_hits: AtomicU64::new(0),
            can_compile_misses: AtomicU64::new(0),
            bytecode_hits: AtomicU64::new(0),
            bytecode_misses: AtomicU64::new(0),
        }
    }
}

#[cfg(feature = "track-stats")]
impl BytecodeCacheStats {
    /// Create a snapshot of current statistics.
    pub fn snapshot(&self) -> BytecodeCacheStatsSnapshot {
        BytecodeCacheStatsSnapshot {
            can_compile_hits: self.can_compile_hits.load(Ordering::Relaxed),
            can_compile_misses: self.can_compile_misses.load(Ordering::Relaxed),
            bytecode_hits: self.bytecode_hits.load(Ordering::Relaxed),
            bytecode_misses: self.bytecode_misses.load(Ordering::Relaxed),
        }
    }

    /// Reset all counters to zero.
    pub fn reset(&self) {
        self.can_compile_hits.store(0, Ordering::Relaxed);
        self.can_compile_misses.store(0, Ordering::Relaxed);
        self.bytecode_hits.store(0, Ordering::Relaxed);
        self.bytecode_misses.store(0, Ordering::Relaxed);
    }
}

/// Snapshot of bytecode cache statistics (plain u64 values).
#[derive(Debug, Default, Clone)]
pub struct BytecodeCacheStatsSnapshot {
    /// can_compile cache hits
    pub can_compile_hits: u64,
    /// can_compile cache misses
    pub can_compile_misses: u64,
    /// bytecode cache hits
    pub bytecode_hits: u64,
    /// bytecode cache misses (compilations)
    pub bytecode_misses: u64,
}

/// Global cache for can_compile results
static CAN_COMPILE_CACHE: LazyLock<RwLock<LruCache<u64, bool, IdentityU64BuildHasher>>> = LazyLock::new(|| {
    let size = get_can_compile_cache_size();
    RwLock::new(LruCache::with_hasher(size, IdentityU64BuildHasher))
});

/// Global cache for compiled bytecode chunks
static BYTECODE_CACHE: LazyLock<RwLock<LruCache<u64, Arc<BytecodeChunk>, IdentityU64BuildHasher>>> = LazyLock::new(|| {
    let size = get_bytecode_cache_size();
    RwLock::new(LruCache::with_hasher(size, IdentityU64BuildHasher))
});

/// Global statistics (lock-free atomics, no RwLock needed)
#[cfg(feature = "track-stats")]
static CACHE_STATS: LazyLock<BytecodeCacheStats> = LazyLock::new(BytecodeCacheStats::default);

fn get_can_compile_cache_size() -> NonZeroUsize {
    std::env::var("METTA_CAN_COMPILE_CACHE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .and_then(NonZeroUsize::new)
        .unwrap_or(NonZeroUsize::new(16384).expect("16384 is non-zero"))
}

fn get_bytecode_cache_size() -> NonZeroUsize {
    std::env::var("METTA_BYTECODE_CACHE_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .and_then(NonZeroUsize::new)
        .unwrap_or(NonZeroUsize::new(4096).expect("4096 is non-zero"))
}

/// Compute hash for a MettaValue
///
/// Uses fast inline hashing for primitives (Long, Bool, Nil, Float) to avoid
/// hasher allocation overhead. Falls back to xxHash3 for complex types
/// (SExpr, Atom, String, etc.) which provides SIMD-accelerated hashing with
/// excellent distribution.
#[inline]
pub fn hash_metta_value(expr: &MettaValue) -> u64 {
    // Fast path for primitives - avoids GxHasher allocation
    // Golden ratio constant for good hash distribution
    const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;

    // Type discriminant seeds to avoid collisions between different types
    // These are arbitrary primes chosen to be well-distributed
    const LONG_SEED: u64 = 0x517cc1b727220a95;
    const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
    const NIL_HASH: u64 = 0x6e696c5f_68617368; // "nil_hash" as bytes
    const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;

    match expr.view() {
        ValueView::Long(n) => {
            // FxHash-style mixing with type-specific seed
            let x = (n as u64)
                .wrapping_add(LONG_SEED)
                .wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        ValueView::Bool(b) => {
            // Distinct well-distributed values for true/false with type seed
            if b {
                BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
            } else {
                BOOL_SEED
            }
        }
        ValueView::Unit => NIL_HASH,
        ValueView::Float(f) => {
            // Use bit representation with type-specific seed and mixing
            let bits = f.to_bits();
            let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        _ => {
            // xxHash3 for complex types - SIMD-accelerated with excellent distribution
            // 3x faster than FxHash for typical expression sizes and much better
            // collision resistance. Safe SIMD (no buffer overflows like gxhash).
            let mut hasher = Xxh3::new();
            expr.hash(&mut hasher);
            hasher.finish()
        }
    }
}

/// Check can_compile cache, returning cached result if available
#[inline]
pub fn get_cached_can_compile(hash: u64) -> Option<bool> {
    // Use peek() + read lock for faster lookups (doesn't update LRU order)
    let cache = CAN_COMPILE_CACHE.read();
    cache.peek(&hash).copied()
}

/// Store can_compile result in cache
#[inline]
pub fn cache_can_compile(hash: u64, compilable: bool) {
    let mut cache = CAN_COMPILE_CACHE.write();
    cache.put(hash, compilable);
}

/// Check bytecode cache, returning compiled chunk if available
#[inline]
pub fn get_cached_bytecode(hash: u64) -> Option<Arc<BytecodeChunk>> {
    // Use peek() + read lock for faster lookups (doesn't update LRU order)
    let cache = BYTECODE_CACHE.read();
    cache.peek(&hash).cloned()
}

/// Store compiled bytecode chunk in cache
#[inline]
pub fn cache_bytecode(hash: u64, chunk: Arc<BytecodeChunk>) {
    ensure_bytecode_cache_roots_registered();
    let mut cache = BYTECODE_CACHE.write();
    cache.put(hash, chunk);
}

/// Get current cache statistics (lock-free snapshot)
#[cfg(feature = "track-stats")]
pub fn get_stats() -> BytecodeCacheStatsSnapshot {
    CACHE_STATS.snapshot()
}

/// Clear all caches (mainly for testing)
pub fn clear_caches() {
    CAN_COMPILE_CACHE.write().clear();
    BYTECODE_CACHE.write().clear();
    // Reset stats atomically (no lock needed)
    #[cfg(feature = "track-stats")]
    CACHE_STATS.reset();
}

/// Get current cache sizes (for diagnostics)
pub fn cache_sizes() -> (usize, usize) {
    let can_compile_size = CAN_COMPILE_CACHE.read().len();
    let bytecode_size = BYTECODE_CACHE.read().len();
    (can_compile_size, bytecode_size)
}

// =============================================================================
// GC Root Provider for BYTECODE_CACHE
// =============================================================================

/// GC root provider that exposes all MettaValue constants stored in cached
/// BytecodeChunks to the garbage collector's root set.
///
/// Without this, constants in cached bytecode chunks are invisible to GC and
/// may be freed while still reachable from the cache, causing use-after-free.
struct BytecodeCacheRoots;

impl RootProvider for BytecodeCacheRoots {
    fn collect_roots(&self, roots: &mut Vec<MettaValue>) {
        let cache = BYTECODE_CACHE.read();
        for (_, chunk) in cache.iter() {
            collect_chunk_constants(chunk, roots);
        }
    }
}

/// Recursively collect all MettaValue constants from a BytecodeChunk and its
/// sub-chunks.
pub fn collect_chunk_constants(chunk: &BytecodeChunk, roots: &mut Vec<MettaValue>) {
    roots.extend(chunk.constants().iter().copied());
    for sub in chunk.sub_chunks() {
        collect_chunk_constants(sub, roots);
    }
}

/// Keeps the Arc<dyn RootProvider> alive for the lifetime of the process so
/// the Weak reference in ROOT_REGISTRY remains valid.
static BYTECODE_CACHE_ROOT_PROVIDER: OnceLock<Arc<dyn RootProvider>> = OnceLock::new();

/// Ensure the bytecode cache is registered as a GC root provider.
///
/// Called lazily on first cache mutation (cache_bytecode). Idempotent —
/// OnceLock guarantees single initialization.
pub fn ensure_bytecode_cache_roots_registered() {
    BYTECODE_CACHE_ROOT_PROVIDER.get_or_init(|| {
        let provider = Arc::new(BytecodeCacheRoots) as Arc<dyn RootProvider>;
        register_root_provider(&provider);
        provider
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::bytecode::chunk::ChunkBuilder;
    use crate::backend::bytecode::Opcode;
    use std::sync::Mutex;

    /// Serializes the cache-cache tests so they don't race against each
    /// other on the shared `CAN_COMPILE_CACHE` / `BYTECODE_CACHE` statics.
    ///
    /// `cargo test` runs tests in parallel by default. Both
    /// `test_can_compile_cache` and `test_bytecode_cache` call
    /// `clear_caches()` which wipes BOTH caches; without this lock, one
    /// test can clear the other's data mid-test, causing a flaky failure.
    static CACHE_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_hash_stability() {
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let h1 = hash_metta_value(&expr);
        let h2 = hash_metta_value(&expr);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_different_exprs() {
        let expr1 = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(3),
        ]);
        let h1 = hash_metta_value(&expr1);
        let h2 = hash_metta_value(&expr2);
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_can_compile_cache() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_caches();
        let hash = 12345u64;

        // Miss
        assert!(get_cached_can_compile(hash).is_none());

        // Store
        cache_can_compile(hash, true);

        // Hit
        assert_eq!(get_cached_can_compile(hash), Some(true));
    }

    #[test]
    fn test_bytecode_cache() {
        let _guard = CACHE_TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        clear_caches();
        let hash = 67890u64;

        // Miss
        assert!(get_cached_bytecode(hash).is_none());

        // Create a simple chunk
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::PushUnit);
        builder.emit(Opcode::Return);
        let chunk = Arc::new(builder.build());

        // Store
        cache_bytecode(hash, Arc::clone(&chunk));

        // Hit
        let cached = get_cached_bytecode(hash);
        assert!(cached.is_some());
    }

    // Note: stats tracking removed from hot path for performance.
    // Stats can be re-enabled with a debug feature flag if needed.

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_fast_hash_primitives() {
        // Long hashing
        let h1 = hash_metta_value(&MettaValue::Long(42));
        let h2 = hash_metta_value(&MettaValue::Long(42));
        assert_eq!(h1, h2, "Long hash should be stable");

        let h3 = hash_metta_value(&MettaValue::Long(43));
        assert_ne!(h1, h3, "Different Longs should have different hashes");

        // Bool hashing
        let h_true1 = hash_metta_value(&MettaValue::Bool(true));
        let h_true2 = hash_metta_value(&MettaValue::Bool(true));
        let h_false = hash_metta_value(&MettaValue::Bool(false));
        assert_eq!(h_true1, h_true2, "Bool(true) hash should be stable");
        assert_ne!(h_true1, h_false, "Bool(true) and Bool(false) should differ");

        // Unit hashing
        let h_nil1 = hash_metta_value(&MettaValue::Unit());
        let h_nil2 = hash_metta_value(&MettaValue::Unit());
        assert_eq!(h_nil1, h_nil2, "Nil hash should be stable");

        // Float hashing
        let h_f1 = hash_metta_value(&MettaValue::Float(3.14));
        let h_f2 = hash_metta_value(&MettaValue::Float(3.14));
        let h_f3 = hash_metta_value(&MettaValue::Float(2.71));
        assert_eq!(h_f1, h_f2, "Float hash should be stable");
        assert_ne!(h_f1, h_f3, "Different Floats should have different hashes");
    }

    #[test]
    fn test_fast_hash_distinct_types() {
        // Different types should produce different hashes
        let h_long = hash_metta_value(&MettaValue::Long(0));
        let h_false = hash_metta_value(&MettaValue::Bool(false));
        let h_nil = hash_metta_value(&MettaValue::Unit());
        let h_float = hash_metta_value(&MettaValue::Float(0.0));

        // All should be distinct
        let hashes = [h_long, h_false, h_nil, h_float];
        for i in 0..hashes.len() {
            for j in i + 1..hashes.len() {
                assert_ne!(
                    hashes[i], hashes[j],
                    "Hash collision between types at indices {} and {}",
                    i, j
                );
            }
        }
    }
}
