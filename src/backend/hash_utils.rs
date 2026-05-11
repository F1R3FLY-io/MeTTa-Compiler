//! Shared hasher utilities for eliminating double-hashing in caches.
//!
//! Two hasher types for different key kinds:
//!
//! - **IdentityU64Hasher**: Pure pass-through for u64 keys that are already
//!   well-distributed hashes (e.g. xxh3 output). Avoids SipHash (~50 cycles)
//!   re-hashing overhead.
//!
//! - **PtrHasher**: Fibonacci mixing for usize/pointer keys with alignment bias.
//!   Relocated from `gc_allocator.rs` for central importability.

use std::hash::{BuildHasher, Hasher};

// ============================================================================
// Identity hasher for pre-computed u64 content hashes (xxh3 output)
// ============================================================================

pub(crate) struct IdentityU64Hasher(u64);

impl Hasher for IdentityU64Hasher {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline(always)]
    fn write_u64(&mut self, i: u64) {
        self.0 = i;
    }
    #[inline(always)]
    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("IdentityU64Hasher only accepts u64")
    }
}

#[derive(Clone, Default)]
pub(crate) struct IdentityU64BuildHasher;

impl BuildHasher for IdentityU64BuildHasher {
    type Hasher = IdentityU64Hasher;
    #[inline(always)]
    fn build_hasher(&self) -> IdentityU64Hasher {
        IdentityU64Hasher(0)
    }
}

// ============================================================================
// Fibonacci hasher for pointer/usize keys
// ============================================================================

/// Fast identity-like hasher for slab-allocated pointers.
/// Uses Fibonacci hashing to spread aligned pointers across hash table buckets.
/// SipHash (~50 cycles/hash) is overkill for pointer keys; this costs ~3 cycles.
pub(crate) struct PtrHasher(u64);

impl Hasher for PtrHasher {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        // Fibonacci hashing: multiply by golden ratio constant, then use
        // upper bits (which have maximal entropy after multiplication).
        self.0 = (i as u64).wrapping_mul(0x517cc1b727220a95);
    }
    #[inline(always)]
    fn write(&mut self, _bytes: &[u8]) {
        unreachable!("PtrHasher only accepts usize")
    }
}

#[derive(Clone, Default)]
pub(crate) struct PtrBuildHasher;

impl BuildHasher for PtrBuildHasher {
    type Hasher = PtrHasher;
    #[inline(always)]
    fn build_hasher(&self) -> PtrHasher {
        PtrHasher(0)
    }
}

/// HashSet for slab pointers using Fibonacci hashing instead of SipHash.
pub(crate) type PtrHashSet = std::collections::HashSet<*const u8, PtrBuildHasher>;

// ============================================================================
// FxHash-style hasher for byte-slice / Vec<u8> keys
// ============================================================================

/// Fast multiply-rotate hasher for byte-sequence keys (symbol names, short strings).
///
/// Initialization cost: 1 u64 zero (vs. Xxh3's 256-byte internal state).
/// Quality is sufficient for HashMap bucketing of short symbol names (3-30 bytes).
/// Uses the FNV-1a-style structure with a better mixing constant (from `rustc-hash`).
pub(crate) struct FxHasher(u64);

const FX_SEED: u64 = 0x517cc1b727220a95; // same golden-ratio constant used elsewhere

impl Hasher for FxHasher {
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Process 8 bytes at a time for speed, then handle remainder.
        let mut h = self.0;
        let chunks = bytes.chunks_exact(8);
        let remainder = chunks.remainder();
        for chunk in chunks {
            let word = u64::from_le_bytes(chunk.try_into().expect("chunk is 8 bytes"));
            h = (h.rotate_left(5) ^ word).wrapping_mul(FX_SEED);
        }
        for &b in remainder {
            h = (h.rotate_left(5) ^ b as u64).wrapping_mul(FX_SEED);
        }
        self.0 = h;
    }

    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        self.0 = (self.0.rotate_left(5) ^ i as u64).wrapping_mul(FX_SEED);
    }

    #[inline(always)]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(FX_SEED);
    }
}

#[derive(Clone, Default)]
pub(crate) struct FxBuildHasher;

impl BuildHasher for FxBuildHasher {
    type Hasher = FxHasher;
    #[inline(always)]
    fn build_hasher(&self) -> FxHasher {
        FxHasher(0)
    }
}
