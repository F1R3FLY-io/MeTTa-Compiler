//! Bloom filter for (head_symbol, arity) pairs.
//!
//! Enables O(1) rejection in `match_space()` when the pattern's (head, arity)
//! definitely doesn't exist in the space.
//!
//! # Performance
//!
//! Uses xxh3 (SIMD-accelerated) instead of SipHash for 3-5× faster hashing.
//! This reduces bloom filter overhead from ~27% to ~5-10% of total CPU time.

use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};

use xxhash_rust::xxh3::Xxh3;

/// Bloom filter for (head_symbol, arity) pairs.
///
/// Enables O(1) rejection in `match_space()` when the pattern's (head, arity)
/// definitely doesn't exist in the space. Uses Kirsch-Mitzenmacher double hashing
/// with k=3 hash functions for ~1% false positive rate at 10 bits per entry.
///
/// # Design Notes
/// - False positives allowed (may iterate when no match exists)
/// - No false negatives (never skips when match does exist)
/// - Doesn't support deletion; uses lazy rebuild when staleness threshold exceeded
/// - Uses xxh3 (SIMD-accelerated) for 3-5× faster hashing than SipHash
#[derive(Clone)]
pub(crate) struct HeadArityBloomFilter {
    bits: Vec<u64>,
    num_bits: usize,
    num_insertions: usize,
    num_deletions: usize,
}

impl HeadArityBloomFilter {
    /// Create a new bloom filter sized for expected_entries.
    /// Uses 10 bits per entry for ~1% false positive rate.
    pub fn new(expected_entries: usize) -> Self {
        let num_bits = (expected_entries * 10).max(1024);
        let num_words = (num_bits + 63) / 64;
        Self {
            bits: vec![0; num_words],
            num_bits,
            num_insertions: 0,
            num_deletions: 0,
        }
    }

    /// Insert a (head, arity) pair into the bloom filter.
    #[inline]
    pub fn insert(&mut self, head: &[u8], arity: u8) {
        let (h1, h2) = Self::hash_pair(head, arity);
        for i in 0usize..3 {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            self.bits[idx / 64] |= 1 << (idx % 64);
        }
        self.num_insertions += 1;
    }

    /// Check if a (head, arity) pair may exist in the filter.
    /// Returns false only if the pair definitely doesn't exist.
    #[inline]
    pub fn may_contain(&self, head: &[u8], arity: u8) -> bool {
        let (h1, h2) = Self::hash_pair(head, arity);
        (0usize..3).all(|i| {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            self.bits[idx / 64] & (1 << (idx % 64)) != 0
        })
    }

    /// Note that a deletion occurred (for lazy rebuild tracking).
    pub fn note_deletion(&mut self) {
        self.num_deletions += 1;
    }

    /// Clear the filter and reset counters.
    pub fn clear(&mut self) {
        self.bits.fill(0);
        self.num_insertions = 0;
        self.num_deletions = 0;
    }

    /// Compute two hash values for double hashing using xxh3 (SIMD-accelerated).
    ///
    /// xxh3 provides 3-5× faster hashing than SipHash (DefaultHasher) by using
    /// SIMD instructions (SSE2/AVX2 on x86_64, NEON on ARM). This reduces bloom filter
    /// overhead from ~27% to ~5-10% of total CPU time in match_space().
    ///
    /// Uses a thread-local cache to skip xxh3 recomputation for repeated lookups
    /// with the same atom string (stable slab-allocated pointers).
    #[inline]
    fn hash_pair(head: &[u8], arity: u8) -> (usize, usize) {
        // Compute hash directly — xxh3 is SIMD-accelerated and fast enough
        // without caching. The previous pointer-based cache (`BloomHashCache`)
        // used `head.as_ptr() as usize` as the cache key, which is subject to
        // the ABA pointer reuse problem: when a String is dropped and a new one
        // allocated at the same address, the cache returns a stale hash computed
        // from different content. This caused false negatives in may_contain(),
        // making match_rules_native() miss valid rules.
        let mut hasher = Xxh3::with_seed(0);
        head.hash(&mut hasher);
        arity.hash(&mut hasher);
        let h = hasher.finish();
        (h as usize, (h >> 32) as usize)
    }
}

/// Bloom filter for atom names that have type declarations.
///
/// Enables O(1) rejection in `get_type()`/`get_types_generic()` when an atom
/// name definitely has no type declared. Avoids HashMap lookup and MORK trie
/// traversal for the common case of untyped atoms.
///
/// # Design Notes
/// - Only tracks atom names (no arity needed for type lookups)
/// - False positives allowed (may check HashMap when no type exists)
/// - No false negatives (never skips when type does exist)
/// - Uses xxh3 (SIMD-accelerated) for fast hashing
#[derive(Clone)]
pub(crate) struct TypeBloomFilter {
    bits: Vec<u64>,
    num_bits: usize,
    num_insertions: usize,
    num_deletions: usize,
}

impl TypeBloomFilter {
    /// Create a new type bloom filter sized for expected_entries.
    /// Uses 10 bits per entry for ~1% false positive rate.
    pub fn new(expected_entries: usize) -> Self {
        let num_bits = (expected_entries * 10).max(512);
        let num_words = (num_bits + 63) / 64;
        Self {
            bits: vec![0; num_words],
            num_bits,
            num_insertions: 0,
            num_deletions: 0,
        }
    }

    /// Insert an atom name into the type bloom filter.
    #[inline]
    pub fn insert(&mut self, name: &[u8]) {
        let (h1, h2) = Self::hash_name(name);
        for i in 0usize..3 {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            self.bits[idx / 64] |= 1 << (idx % 64);
        }
        self.num_insertions += 1;
    }

    /// Check if an atom name may have a type declaration.
    /// Returns false only if the name definitely has no type.
    #[inline]
    pub fn may_have_type(&self, name: &[u8]) -> bool {
        let (h1, h2) = Self::hash_name(name);
        (0usize..3).all(|i| {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            self.bits[idx / 64] & (1 << (idx % 64)) != 0
        })
    }

    /// Note that a type deletion occurred (for lazy rebuild tracking).
    pub fn note_deletion(&mut self) {
        self.num_deletions += 1;
    }

    /// Compute two hash values for double hashing using xxh3 (SIMD-accelerated).
    ///
    /// Uses a thread-local cache to skip xxh3 recomputation for repeated lookups.
    #[inline]
    fn hash_name(name: &[u8]) -> (usize, usize) {
        // Compute hash directly — no pointer-based caching (ABA-unsafe).
        // See HeadArityBloomFilter::hash_pair for rationale.
        let mut hasher = Xxh3::with_seed(0x7470); // seed = "tp" (type)
        name.hash(&mut hasher);
        let h = hasher.finish();
        (h as usize, (h >> 32) as usize)
    }
}

/// Lock-free bloom filter using atomic bit arrays (Phase 10.1).
///
/// All operations are wait-free:
/// - Reads via `AtomicU64::load(Relaxed)` — zero synchronization.
/// - Writes via `AtomicU64::fetch_or(Relaxed, mask)` — lock-free CAS insertion.
///
/// Used for O(1) rejection of inferred function type lookups. False positives
/// are harmless (fall through to DashMap lookup). No false negatives.
///
/// Uses Kirsch-Mitzenmacher double hashing with k=3 hash functions and xxh3
/// (SIMD-accelerated) for consistent hashing with the other bloom filters.
pub(crate) struct AtomicBloomFilter {
    bits: Box<[AtomicU64]>,
    num_bits: usize,
}

impl AtomicBloomFilter {
    /// Number of hash functions (k=3 for ~1% FPR at 10 bits/entry).
    const NUM_HASHES: usize = 3;

    /// Create a new atomic bloom filter sized for expected_entries.
    /// Uses 10 bits per entry for ~1% false positive rate.
    pub fn new(expected_entries: usize) -> Self {
        let num_bits = (expected_entries * 10).max(512);
        let num_words = (num_bits + 63) / 64;
        let bits: Vec<AtomicU64> = (0..num_words).map(|_| AtomicU64::new(0)).collect();
        Self {
            bits: bits.into_boxed_slice(),
            num_bits,
        }
    }

    /// Lock-free insertion via fetch_or.
    #[inline]
    pub fn insert(&self, key: &[u8]) {
        let (h1, h2) = Self::hash_key(key);
        for i in 0..Self::NUM_HASHES {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            let word = idx / 64;
            let bit = idx % 64;
            self.bits[word].fetch_or(1u64 << bit, Ordering::Relaxed);
        }
    }

    /// Lock-free query via load.
    #[inline]
    pub fn may_contain(&self, key: &[u8]) -> bool {
        let (h1, h2) = Self::hash_key(key);
        (0..Self::NUM_HASHES).all(|i| {
            let idx = (h1.wrapping_add(i.wrapping_mul(h2))) % self.num_bits;
            let word = idx / 64;
            let bit = idx % 64;
            self.bits[word].load(Ordering::Relaxed) & (1u64 << bit) != 0
        })
    }

    /// Clear all bits in the filter (zeroing all atomic words).
    ///
    /// Used to invalidate the normal-form memoization bloom filter when
    /// new rules are added (Phase 9.5). Uses `Relaxed` ordering since
    /// this is called from `add_rule()` which is O(N) during loading
    /// and never during concurrent evaluation.
    pub fn clear(&self) {
        for word in self.bits.iter() {
            word.store(0, Ordering::Relaxed);
        }
    }

    /// Snapshot for fork: clone all atomic words into a new filter.
    pub fn snapshot(&self) -> Self {
        let bits: Vec<AtomicU64> = self
            .bits
            .iter()
            .map(|w| AtomicU64::new(w.load(Ordering::Relaxed)))
            .collect();
        Self {
            bits: bits.into_boxed_slice(),
            num_bits: self.num_bits,
        }
    }

    /// Bitwise OR merge from another filter (for union).
    /// Both filters must have the same size.
    pub fn merge_from(&self, other: &Self) {
        debug_assert_eq!(self.bits.len(), other.bits.len());
        for (dst, src) in self.bits.iter().zip(other.bits.iter()) {
            dst.fetch_or(src.load(Ordering::Relaxed), Ordering::Relaxed);
        }
    }

    /// Compute two hash values for double hashing using xxh3 (SIMD-accelerated).
    /// Uses a different seed (0x6966 = "if" for inferred) to avoid collisions
    /// with the TypeBloomFilter.
    ///
    /// Uses a thread-local cache to skip xxh3 recomputation for repeated lookups.
    #[inline]
    fn hash_key(key: &[u8]) -> (usize, usize) {
        // Compute hash directly — no pointer-based caching (ABA-unsafe).
        // See HeadArityBloomFilter::hash_pair for rationale.
        let mut hasher = Xxh3::with_seed(0x6966); // seed = "if" (inferred function)
        key.hash(&mut hasher);
        let h = hasher.finish();
        (h as usize, (h >> 32) as usize)
    }
}
