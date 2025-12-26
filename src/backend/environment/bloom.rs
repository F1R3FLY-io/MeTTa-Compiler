//! Bloom filter for (head_symbol, arity) pairs.
//!
//! Enables O(1) rejection in `match_space()` when the pattern's (head, arity)
//! definitely doesn't exist in the space.

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

    /// Check if the filter needs rebuilding due to accumulated deletions.
    #[allow(dead_code)]
    pub fn needs_rebuild(&self) -> bool {
        self.num_deletions > self.num_insertions / 4
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

    /// Compute two hash values for double hashing.
    #[inline]
    fn hash_pair(head: &[u8], arity: u8) -> (usize, usize) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        head.hash(&mut hasher);
        arity.hash(&mut hasher);
        let h = hasher.finish();
        (h as usize, (h >> 32) as usize)
    }
}
