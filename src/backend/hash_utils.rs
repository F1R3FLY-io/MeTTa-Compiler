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
    fn finish(&self) -> u64 { self.0 }
    #[inline(always)]
    fn write_u64(&mut self, i: u64) { self.0 = i; }
    #[inline(always)]
    fn write(&mut self, _bytes: &[u8]) { unreachable!("IdentityU64Hasher only accepts u64") }
}

#[derive(Clone, Default)]
pub(crate) struct IdentityU64BuildHasher;

impl BuildHasher for IdentityU64BuildHasher {
    type Hasher = IdentityU64Hasher;
    #[inline(always)]
    fn build_hasher(&self) -> IdentityU64Hasher { IdentityU64Hasher(0) }
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
    fn finish(&self) -> u64 { self.0 }
    #[inline(always)]
    fn write_usize(&mut self, i: usize) {
        // Fibonacci hashing: multiply by golden ratio constant, then use
        // upper bits (which have maximal entropy after multiplication).
        self.0 = (i as u64).wrapping_mul(0x517cc1b727220a95);
    }
    #[inline(always)]
    fn write(&mut self, _bytes: &[u8]) { unreachable!("PtrHasher only accepts usize") }
}

#[derive(Clone, Default)]
pub(crate) struct PtrBuildHasher;

impl BuildHasher for PtrBuildHasher {
    type Hasher = PtrHasher;
    #[inline(always)]
    fn build_hasher(&self) -> PtrHasher { PtrHasher(0) }
}

/// HashSet for slab pointers using Fibonacci hashing instead of SipHash.
pub(crate) type PtrHashSet = std::collections::HashSet<*const u8, PtrBuildHasher>;
