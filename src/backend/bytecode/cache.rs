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

use gxhash::GxHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{LazyLock, RwLock};

use lru::LruCache;

use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::models::MettaValue;
use std::sync::Arc;

/// Statistics for bytecode cache monitoring
#[derive(Debug, Default, Clone)]
pub struct BytecodeCacheStats {
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
static CAN_COMPILE_CACHE: LazyLock<RwLock<LruCache<u64, bool>>> = LazyLock::new(|| {
    let size = get_can_compile_cache_size();
    RwLock::new(LruCache::new(size))
});

/// Global cache for compiled bytecode chunks
static BYTECODE_CACHE: LazyLock<RwLock<LruCache<u64, Arc<BytecodeChunk>>>> = LazyLock::new(|| {
    let size = get_bytecode_cache_size();
    RwLock::new(LruCache::new(size))
});

/// Global statistics
static CACHE_STATS: LazyLock<RwLock<BytecodeCacheStats>> =
    LazyLock::new(|| RwLock::new(BytecodeCacheStats::default()));

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
/// GxHasher allocation overhead. Falls back to SIMD-accelerated gxhash for
/// complex types (SExpr, Atom, String, etc.).
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

    match expr {
        MettaValue::Long(n) => {
            // FxHash-style mixing with type-specific seed
            let x = (*n as u64)
                .wrapping_add(LONG_SEED)
                .wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        MettaValue::Bool(b) => {
            // Distinct well-distributed values for true/false with type seed
            if *b {
                BOOL_SEED.wrapping_mul(GOLDEN_RATIO)
            } else {
                BOOL_SEED
            }
        }
        MettaValue::Nil => NIL_HASH,
        MettaValue::Float(f) => {
            // Use bit representation with type-specific seed and mixing
            let bits = f.to_bits();
            let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
            x ^ (x >> 32)
        }
        _ => {
            // Full GxHasher for complex types (SExpr, Atom, String, etc.)
            let mut hasher = GxHasher::with_seed(0);
            expr.hash(&mut hasher);
            hasher.finish()
        }
    }
}

/// Check can_compile cache, returning cached result if available
#[inline]
pub fn get_cached_can_compile(hash: u64) -> Option<bool> {
    // Use peek() + read lock for faster lookups (doesn't update LRU order)
    let cache = CAN_COMPILE_CACHE
        .read()
        .expect("can_compile cache lock poisoned");
    cache.peek(&hash).copied()
}

/// Store can_compile result in cache
#[inline]
pub fn cache_can_compile(hash: u64, compilable: bool) {
    let mut cache = CAN_COMPILE_CACHE
        .write()
        .expect("can_compile cache lock poisoned");
    cache.put(hash, compilable);
}

/// Check bytecode cache, returning compiled chunk if available
#[inline]
pub fn get_cached_bytecode(hash: u64) -> Option<Arc<BytecodeChunk>> {
    // Use peek() + read lock for faster lookups (doesn't update LRU order)
    let cache = BYTECODE_CACHE.read().expect("bytecode cache lock poisoned");
    cache.peek(&hash).cloned()
}

/// Store compiled bytecode chunk in cache
#[inline]
pub fn cache_bytecode(hash: u64, chunk: Arc<BytecodeChunk>) {
    let mut cache = BYTECODE_CACHE
        .write()
        .expect("bytecode cache lock poisoned");
    cache.put(hash, chunk);
}

/// Get current cache statistics
pub fn get_stats() -> BytecodeCacheStats {
    CACHE_STATS.read().expect("stats lock poisoned").clone()
}

/// Clear all caches (mainly for testing)
pub fn clear_caches() {
    if let Ok(mut cache) = CAN_COMPILE_CACHE.write() {
        cache.clear();
    }
    if let Ok(mut cache) = BYTECODE_CACHE.write() {
        cache.clear();
    }
    if let Ok(mut stats) = CACHE_STATS.write() {
        *stats = BytecodeCacheStats::default();
    }
}

/// Get current cache sizes (for diagnostics)
pub fn cache_sizes() -> (usize, usize) {
    let can_compile_size = CAN_COMPILE_CACHE.read().map(|c| c.len()).unwrap_or(0);
    let bytecode_size = BYTECODE_CACHE.read().map(|c| c.len()).unwrap_or(0);
    (can_compile_size, bytecode_size)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        use crate::backend::bytecode::chunk::ChunkBuilder;
        use crate::backend::bytecode::Opcode;

        clear_caches();
        let hash = 67890u64;

        // Miss
        assert!(get_cached_bytecode(hash).is_none());

        // Create a simple chunk
        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::PushNil);
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

        // Nil hashing
        let h_nil1 = hash_metta_value(&MettaValue::Nil);
        let h_nil2 = hash_metta_value(&MettaValue::Nil);
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
        let h_nil = hash_metta_value(&MettaValue::Nil);
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
