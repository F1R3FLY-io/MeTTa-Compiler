//! Efficient multiplicity tracking with suffix-replacement via WriteZipper.
//!
//! This module provides O(prefix_len + 8) increment/decrement by using WriteZipper's
//! public API to replace just the 8-byte count suffix, avoiding full path reconstruction.
//!
//! # Algorithm
//!
//! To update count from N to N+1:
//! 1. Read current count (if any) using read zipper
//! 2. Navigate to prefix using write zipper
//! 3. If old count exists: remove old suffix branches
//! 4. Descend to new count suffix and set value
//!
//! # Data Layout
//!
//! ```text
//! btm: PathMap<()>
//! ───────────────────────────────────
//! atom_bytes → ()                                [atom - for MORK iteration]
//! [0x03, 0xC1, 'M', atom_bytes, count_u64] → ()  [multiplicity entry]
//! ```
//!
//! # Space Efficiency
//!
//! Uses a 3-byte prefix (`[0x03, 0xC1, 'M']`) which encodes as a valid MORK expression:
//! - 0x03 = Arity(3) - a 3-element list
//! - 0xC1 = SymbolSize(1) - 1-byte symbol
//! - 'M' = The symbol "M"
//!
//! This saves 11 bytes per unique atom compared to a full "multiplicity" prefix.

use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperWriting};
use pathmap::PathMap;

/// Fixed prefix: [0x03, 0xC1, 'M'] - 3 bytes for space efficiency.
///
/// This encodes as a valid MORK expression (M atom count) which won't conflict
/// with actual atoms since:
/// - Real atoms don't start with Arity(3)
/// - The 'M' symbol uniquely identifies multiplicity entries
const MULTIPLICITY_PREFIX: &[u8] = &[0x03, 0xC1, b'M'];

/// Size of the count suffix in bytes (u64 big-endian).
const COUNT_SIZE: usize = 8;

/// Build full multiplicity path: prefix ++ atom_bytes ++ count_u64_be
///
/// # Arguments
/// * `atom_bytes` - The MORK-encoded bytes of the atom
/// * `count` - The multiplicity count to encode
///
/// # Returns
/// A Vec containing the full path bytes
#[inline]
pub fn build_multiplicity_path(atom_bytes: &[u8], count: u64) -> Vec<u8> {
    let mut path = Vec::with_capacity(MULTIPLICITY_PREFIX.len() + atom_bytes.len() + COUNT_SIZE);
    path.extend_from_slice(MULTIPLICITY_PREFIX);
    path.extend_from_slice(atom_bytes);
    path.extend_from_slice(&count.to_be_bytes());
    path
}

/// Build multiplicity prefix (without count suffix): prefix ++ atom_bytes
///
/// # Arguments
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// A Vec containing the prefix bytes (ready to append count)
#[inline]
pub fn build_multiplicity_prefix(atom_bytes: &[u8]) -> Vec<u8> {
    let mut prefix = Vec::with_capacity(MULTIPLICITY_PREFIX.len() + atom_bytes.len());
    prefix.extend_from_slice(MULTIPLICITY_PREFIX);
    prefix.extend_from_slice(atom_bytes);
    prefix
}

/// Extract count from last 8 bytes of path.
///
/// # Arguments
/// * `full_path` - The complete path including prefix and count
/// * `prefix_len` - Length of the prefix (MULTIPLICITY_PREFIX + atom_bytes)
///
/// # Returns
/// The decoded u64 count, or 0 if the path is too short
#[inline]
pub fn extract_count_from_path(full_path: &[u8], prefix_len: usize) -> u64 {
    if full_path.len() < prefix_len + COUNT_SIZE {
        return 0;
    }
    let count_bytes = &full_path[full_path.len() - COUNT_SIZE..];
    u64::from_be_bytes(count_bytes.try_into().unwrap_or([0; 8]))
}

/// Check if path is a multiplicity entry (starts with MULTIPLICITY_PREFIX).
///
/// This is an O(1) check used to filter out multiplicity entries during
/// atom iteration in pattern matching.
///
/// # Arguments
/// * `path` - The path bytes to check
///
/// # Returns
/// `true` if the path is a multiplicity entry, `false` otherwise
#[inline]
pub fn is_multiplicity_entry(path: &[u8]) -> bool {
    path.starts_with(MULTIPLICITY_PREFIX)
}

/// Get multiplicity for an atom - O(prefix_len + atom_len).
///
/// Navigates to the multiplicity prefix and finds the count by looking
/// at the first value below that prefix.
///
/// # Arguments
/// * `btm` - The PathMap containing atoms and multiplicities
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// The multiplicity count, or 0 if no entry exists
pub fn get_multiplicity(btm: &PathMap<()>, atom_bytes: &[u8]) -> u64 {
    let prefix = build_multiplicity_prefix(atom_bytes);

    // Use root zipper and descend manually for more predictable behavior
    let mut rz = btm.read_zipper();
    rz.descend_to(&prefix);

    // Find the first (and only) value below this prefix.
    // to_next_val() iterates in DFS order from current position.
    if rz.to_next_val() {
        let path = rz.path();
        // Verify we're still under our prefix and have exactly 8 count bytes
        if path.starts_with(&prefix) && path.len() == prefix.len() + COUNT_SIZE {
            extract_count_from_path(path, prefix.len())
        } else {
            0
        }
    } else {
        0
    }
}

/// Get multiplicity reusing external buffer (zero-allocation hot path).
///
/// This is the optimized version of `get_multiplicity()` that avoids allocating
/// a new Vec for each lookup. The caller provides a reusable buffer.
///
/// # Arguments
/// * `btm` - The PathMap containing atoms and multiplicities
/// * `atom_bytes` - The MORK-encoded bytes of the atom
/// * `prefix_buf` - A reusable buffer for building the prefix (will be cleared and reused)
///
/// # Returns
/// The multiplicity count, or 0 if no entry exists
///
/// # Performance
/// Eliminates ~1000 Vec allocations per match_space() call compared to `get_multiplicity()`.
#[inline]
pub fn get_multiplicity_with_buffer(
    btm: &PathMap<()>,
    atom_bytes: &[u8],
    prefix_buf: &mut Vec<u8>,
) -> u64 {
    // Clear and reuse buffer
    prefix_buf.clear();
    prefix_buf.extend_from_slice(MULTIPLICITY_PREFIX);
    prefix_buf.extend_from_slice(atom_bytes);

    let mut rz = btm.read_zipper();
    rz.descend_to(prefix_buf.as_slice());

    if rz.to_next_val() {
        let path = rz.path();
        // Verify we're still under our prefix and have exactly 8 count bytes
        if path.starts_with(prefix_buf) && path.len() == prefix_buf.len() + COUNT_SIZE {
            extract_count_from_path(path, prefix_buf.len())
        } else {
            0
        }
    } else {
        0
    }
}

/// Multiplicity lookup helper optimized for batch lookups.
///
/// Pre-allocates a buffer and provides a zero-allocation `get()` method
/// for looking up multiplicities in hot loops (e.g., match_space iteration).
///
/// # Example
/// ```ignore
/// let mut lookup = MultiplicityLookup::new();
/// for atom_bytes in atoms.iter() {
///     let count = lookup.get(&btm, atom_bytes);
///     // ... use count
/// }
/// ```
///
/// # Performance
/// - First lookup: Same as `get_multiplicity()` (single allocation for buffer)
/// - Subsequent lookups: Zero allocation (buffer is reused)
/// - Eliminates ~1000 Vec allocations per match_space() call
pub struct MultiplicityLookup {
    prefix_buf: Vec<u8>,
}

impl MultiplicityLookup {
    /// Create a new multiplicity lookup helper with pre-allocated buffer.
    #[inline]
    pub fn new() -> Self {
        Self {
            prefix_buf: Vec::with_capacity(256),
        }
    }

    /// Get multiplicity for an atom using the pre-allocated buffer.
    ///
    /// # Arguments
    /// * `btm` - The PathMap containing atoms and multiplicities
    /// * `atom_bytes` - The MORK-encoded bytes of the atom
    ///
    /// # Returns
    /// The multiplicity count, or 0 if no entry exists
    #[inline]
    pub fn get(&mut self, btm: &PathMap<()>, atom_bytes: &[u8]) -> u64 {
        get_multiplicity_with_buffer(btm, atom_bytes, &mut self.prefix_buf)
    }
}

impl Default for MultiplicityLookup {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// OPTIMIZED ITERATION: Iterate from multiplicity prefix for O(1) count access
// ============================================================================

/// Get the multiplicity prefix for direct iteration.
///
/// This allows callers to iterate directly from the multiplicity subtrie,
/// extracting both atom bytes and counts in a single pass without separate lookups.
///
/// # Returns
/// The 3-byte multiplicity prefix `[0x03, 0xC1, 'M']`
#[inline]
pub const fn multiplicity_prefix() -> &'static [u8] {
    MULTIPLICITY_PREFIX
}

/// Extract atom bytes from a multiplicity entry path.
///
/// Given a full multiplicity path `[prefix, atom_bytes, count_u64]`, extracts just
/// the atom_bytes portion.
///
/// # Arguments
/// * `full_path` - The complete multiplicity entry path
///
/// # Returns
/// * `Some(&[u8])` - The atom bytes if this is a valid multiplicity entry
/// * `None` - If the path is too short or not a multiplicity entry
///
/// # Performance
/// O(1) - just pointer arithmetic, no allocation
#[inline]
pub fn extract_atom_bytes(full_path: &[u8]) -> Option<&[u8]> {
    // Verify this is a multiplicity entry and has enough bytes
    if full_path.len() <= MULTIPLICITY_PREFIX.len() + COUNT_SIZE {
        return None;
    }
    if !full_path.starts_with(MULTIPLICITY_PREFIX) {
        return None;
    }

    // Atom bytes are between the prefix and the count suffix
    let atom_start = MULTIPLICITY_PREFIX.len();
    let atom_end = full_path.len() - COUNT_SIZE;
    Some(&full_path[atom_start..atom_end])
}

/// Extract atom bytes and count from a multiplicity entry path.
///
/// Given a full multiplicity path `[prefix, atom_bytes, count_u64]`, extracts both
/// the atom_bytes and the count in a single operation.
///
/// # Arguments
/// * `full_path` - The complete multiplicity entry path
///
/// # Returns
/// * `Some((&[u8], u64))` - Tuple of (atom_bytes, count) if valid
/// * `None` - If the path is invalid or too short
///
/// # Performance
/// O(1) - pointer arithmetic and 8-byte read, no allocation
///
/// # Example
/// ```ignore
/// // Iterate multiplicity entries with O(1) count access
/// let mut rz = btm.read_zipper();
/// rz.descend_to(multiplicity_prefix());
/// while rz.to_next_val() {
///     if let Some((atom_bytes, count)) = extract_atom_and_count(rz.path()) {
///         // Use atom_bytes and count directly - no separate lookup needed!
///     }
/// }
/// ```
#[inline]
pub fn extract_atom_and_count(full_path: &[u8]) -> Option<(&[u8], u64)> {
    // Verify this is a multiplicity entry and has enough bytes
    if full_path.len() <= MULTIPLICITY_PREFIX.len() + COUNT_SIZE {
        return None;
    }
    if !full_path.starts_with(MULTIPLICITY_PREFIX) {
        return None;
    }

    // Atom bytes are between the prefix and the count suffix
    let atom_start = MULTIPLICITY_PREFIX.len();
    let atom_end = full_path.len() - COUNT_SIZE;
    let atom_bytes = &full_path[atom_start..atom_end];

    // Extract count from last 8 bytes
    let count_bytes = &full_path[atom_end..];
    let count = u64::from_be_bytes(count_bytes.try_into().unwrap_or([0; 8]));

    Some((atom_bytes, count))
}

// ============================================================================
// SUFFIX-REPLACEMENT IMPLEMENTATION
// Uses WriteZipper's public API for efficient count updates
// ============================================================================

/// Increment multiplicity using suffix-replacement - O(prefix_len + atom_len + 8).
///
/// For existing entries, only the 8-byte count suffix is replaced,
/// not the entire path. This is the key optimization: after navigating
/// to the atom's multiplicity prefix, the actual count update is O(8).
///
/// # Algorithm
///
/// 1. Read current count using read zipper
/// 2. Get write zipper and navigate to prefix
/// 3. If old count exists: remove_branches to clear old suffix
/// 4. Descend to new count suffix and set_val
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// The new multiplicity count after incrementing
pub fn increment_multiplicity(btm: &mut PathMap<()>, atom_bytes: &[u8]) -> u64 {
    // Step 1: Read current count
    let old_count = get_multiplicity(btm, atom_bytes);
    let new_count = old_count + 1;

    // Step 2: Get write zipper and navigate to prefix
    let prefix = build_multiplicity_prefix(atom_bytes);
    let mut wz = btm.write_zipper();
    wz.descend_to(&prefix);

    // Step 3: If old entry exists, remove the old count suffix
    if old_count > 0 {
        // We're at the prefix, remove all branches below (the 8-byte count suffix)
        wz.remove_branches(false); // Don't prune - we're adding new suffix
    }

    // Step 4: Descend to new count suffix and set value
    let new_count_bytes = new_count.to_be_bytes();
    wz.descend_to(&new_count_bytes);
    wz.set_val(());

    new_count
}

/// Decrement multiplicity using suffix-replacement - O(prefix_len + atom_len + 8).
///
/// For existing entries, only the 8-byte count suffix is replaced.
/// Returns 0 if the atom is fully removed.
///
/// # Algorithm
///
/// 1. Read current count using get_multiplicity
/// 2. If count <= 1: remove entire entry with pruning
/// 3. Otherwise: remove old suffix, add new suffix with count-1
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// The new multiplicity count after decrementing (0 if fully removed)
pub fn decrement_multiplicity(btm: &mut PathMap<()>, atom_bytes: &[u8]) -> u64 {
    // Step 1: Read current count
    let old_count = get_multiplicity(btm, atom_bytes);

    if old_count == 0 {
        return 0; // No entry to decrement
    }

    // Step 2: Get write zipper and navigate to prefix
    let prefix = build_multiplicity_prefix(atom_bytes);
    let mut wz = btm.write_zipper();
    wz.descend_to(&prefix);

    if old_count <= 1 {
        // Remove entirely - remove all branches below prefix with pruning
        wz.remove_branches(true);
        return 0;
    }

    // Step 3: Remove old suffix branches
    wz.remove_branches(false); // Don't prune - we're adding new suffix

    // Step 4: Descend to new count suffix and set value
    let new_count = old_count - 1;
    let new_count_bytes = new_count.to_be_bytes();
    wz.descend_to(&new_count_bytes);
    wz.set_val(());

    new_count
}

// ============================================================================
// FALLBACK: Full path remove+insert (for comparison benchmarking)
// ============================================================================

/// Increment using full path reconstruction (for benchmark comparison).
///
/// This is the naive approach: remove the old entry entirely and insert
/// a new one with the updated count. This requires O(full_path_len) × 2
/// operations instead of O(8) for suffix replacement.
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// The new multiplicity count after incrementing
#[allow(dead_code)]
pub fn increment_multiplicity_full_path(btm: &mut PathMap<()>, atom_bytes: &[u8]) -> u64 {
    // Get old count
    let old_count = get_multiplicity(btm, atom_bytes);
    let new_count = old_count + 1;

    // Remove old entry if exists
    if old_count > 0 {
        let old_path = build_multiplicity_path(atom_bytes, old_count);
        btm.remove(&old_path);
    }

    // Insert new entry
    let new_path = build_multiplicity_path(atom_bytes, new_count);
    btm.insert(&new_path, ());

    new_count
}

/// Decrement using full path reconstruction (for benchmark comparison).
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `atom_bytes` - The MORK-encoded bytes of the atom
///
/// # Returns
/// The new multiplicity count after decrementing (0 if fully removed)
#[allow(dead_code)]
pub fn decrement_multiplicity_full_path(btm: &mut PathMap<()>, atom_bytes: &[u8]) -> u64 {
    // Get old count
    let old_count = get_multiplicity(btm, atom_bytes);

    if old_count == 0 {
        return 0;
    }

    // Remove old entry
    let old_path = build_multiplicity_path(atom_bytes, old_count);
    btm.remove(&old_path);

    if old_count <= 1 {
        return 0;
    }

    // Insert new entry with decremented count
    let new_count = old_count - 1;
    let new_path = build_multiplicity_path(atom_bytes, new_count);
    btm.insert(&new_path, ());

    new_count
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_multiplicity_path() {
        let atom_bytes = vec![0xC4, b't', b'e', b's', b't'];
        let path = build_multiplicity_path(&atom_bytes, 42);

        // Should start with MULTIPLICITY_PREFIX
        assert!(path.starts_with(MULTIPLICITY_PREFIX));

        // Should have correct length: prefix(3) + atom(5) + count(8) = 16
        assert_eq!(path.len(), MULTIPLICITY_PREFIX.len() + atom_bytes.len() + COUNT_SIZE);

        // Should be able to extract the count
        let prefix = build_multiplicity_prefix(&atom_bytes);
        let extracted = extract_count_from_path(&path, prefix.len());
        assert_eq!(extracted, 42);
    }

    #[test]
    fn test_is_multiplicity_entry() {
        let atom_bytes = vec![0xC3, b'f', b'o', b'o'];
        let mult_path = build_multiplicity_path(&atom_bytes, 5);

        assert!(is_multiplicity_entry(&mult_path));
        assert!(!is_multiplicity_entry(&atom_bytes));

        // Edge cases
        assert!(!is_multiplicity_entry(&[]));
        assert!(!is_multiplicity_entry(&[0x03]));
        assert!(!is_multiplicity_entry(&[0x03, 0xC1]));
    }

    #[test]
    fn test_increment_decrement_basic() {
        let mut btm: PathMap<()> = PathMap::new();
        let atom_bytes = vec![0xC3, b'f', b'o', b'o'];

        // Initially no entry
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 0);

        // Increment from 0 to 1
        assert_eq!(increment_multiplicity(&mut btm, &atom_bytes), 1);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1);

        // Increment from 1 to 2
        assert_eq!(increment_multiplicity(&mut btm, &atom_bytes), 2);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 2);

        // Increment from 2 to 3
        assert_eq!(increment_multiplicity(&mut btm, &atom_bytes), 3);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 3);

        // Decrement from 3 to 2
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 2);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 2);

        // Decrement from 2 to 1
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 1);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1);

        // Decrement from 1 to 0 (removal)
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 0);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 0);

        // Decrement when already 0 should stay at 0
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 0);
    }

    #[test]
    fn test_multiple_atoms() {
        let mut btm: PathMap<()> = PathMap::new();
        let atom1 = vec![0xC3, b'f', b'o', b'o'];
        let atom2 = vec![0xC3, b'b', b'a', b'r'];
        let atom3 = vec![0xC3, b'b', b'a', b'z'];

        // Increment different atoms
        assert_eq!(increment_multiplicity(&mut btm, &atom1), 1);
        assert_eq!(increment_multiplicity(&mut btm, &atom2), 1);
        assert_eq!(increment_multiplicity(&mut btm, &atom3), 1);

        // Check each is tracked independently
        assert_eq!(get_multiplicity(&btm, &atom1), 1);
        assert_eq!(get_multiplicity(&btm, &atom2), 1);
        assert_eq!(get_multiplicity(&btm, &atom3), 1);

        // Increment atom1 multiple times
        assert_eq!(increment_multiplicity(&mut btm, &atom1), 2);
        assert_eq!(increment_multiplicity(&mut btm, &atom1), 3);

        // Others should be unaffected
        assert_eq!(get_multiplicity(&btm, &atom1), 3);
        assert_eq!(get_multiplicity(&btm, &atom2), 1);
        assert_eq!(get_multiplicity(&btm, &atom3), 1);
    }

    #[test]
    fn test_large_counts() {
        let mut btm: PathMap<()> = PathMap::new();
        let atom_bytes = vec![0xC3, b'b', b'i', b'g'];

        // Increment to a large number
        for i in 1..=1000 {
            assert_eq!(increment_multiplicity(&mut btm, &atom_bytes), i);
        }
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1000);

        // Decrement back down
        for i in (1..1000).rev() {
            assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), i);
        }
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 0);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 0);
    }

    #[test]
    fn test_fork_isolation() {
        let mut btm: PathMap<()> = PathMap::new();
        let atom_bytes = vec![0xC3, b'f', b'o', b'o'];

        // Set up initial multiplicity
        increment_multiplicity(&mut btm, &atom_bytes);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1);

        // Fork via clone (O(1) via Arc CoW)
        let mut forked = btm.clone();

        // Increment in forked
        increment_multiplicity(&mut forked, &atom_bytes);

        // Isolation: original unchanged, forked incremented
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1);
        assert_eq!(get_multiplicity(&forked, &atom_bytes), 2);

        // Further modifications are isolated
        increment_multiplicity(&mut btm, &atom_bytes);
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 2);
        assert_eq!(get_multiplicity(&forked, &atom_bytes), 2);
    }

    #[test]
    fn test_full_path_equivalence() {
        // Verify that suffix-replacement and full-path give same results
        let mut btm1: PathMap<()> = PathMap::new();
        let mut btm2: PathMap<()> = PathMap::new();
        let atom_bytes = vec![0xC4, b't', b'e', b's', b't'];

        // Increment both
        for _ in 0..10 {
            increment_multiplicity(&mut btm1, &atom_bytes);
            increment_multiplicity_full_path(&mut btm2, &atom_bytes);
        }

        assert_eq!(get_multiplicity(&btm1, &atom_bytes), 10);
        assert_eq!(get_multiplicity(&btm2, &atom_bytes), 10);

        // Decrement both
        for _ in 0..5 {
            decrement_multiplicity(&mut btm1, &atom_bytes);
            decrement_multiplicity_full_path(&mut btm2, &atom_bytes);
        }

        assert_eq!(get_multiplicity(&btm1, &atom_bytes), 5);
        assert_eq!(get_multiplicity(&btm2, &atom_bytes), 5);
    }

    #[test]
    fn test_atom_coexistence() {
        // Test that atom entries and multiplicity entries can coexist
        let mut btm: PathMap<()> = PathMap::new();
        let atom_bytes = vec![0xC3, b'f', b'o', b'o'];

        // Insert the atom itself
        btm.insert(&atom_bytes, ());

        // Increment multiplicity
        assert_eq!(increment_multiplicity(&mut btm, &atom_bytes), 1);

        // Both should exist
        assert!(btm.contains(&atom_bytes));
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 1);

        // Decrement multiplicity to 0
        assert_eq!(decrement_multiplicity(&mut btm, &atom_bytes), 0);

        // Atom should still exist, multiplicity should be 0
        assert!(btm.contains(&atom_bytes));
        assert_eq!(get_multiplicity(&btm, &atom_bytes), 0);
    }

    #[test]
    fn test_extract_count_edge_cases() {
        let prefix_len = 10;

        // Path too short
        assert_eq!(extract_count_from_path(&[0u8; 5], prefix_len), 0);

        // Exactly prefix length (no count)
        assert_eq!(extract_count_from_path(&[0u8; 10], prefix_len), 0);

        // Prefix + partial count
        assert_eq!(extract_count_from_path(&[0u8; 14], prefix_len), 0);

        // Correct length with count = 1
        let mut path = vec![0u8; prefix_len];
        path.extend_from_slice(&1u64.to_be_bytes());
        assert_eq!(extract_count_from_path(&path, prefix_len), 1);

        // Correct length with max count
        let mut path = vec![0u8; prefix_len];
        path.extend_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(extract_count_from_path(&path, prefix_len), u64::MAX);
    }

    #[test]
    fn test_multiplicity_prefix_constant() {
        // Verify the constant is accessible and correct
        assert_eq!(multiplicity_prefix(), &[0x03, 0xC1, b'M']);
        assert_eq!(multiplicity_prefix().len(), 3);
    }

    #[test]
    fn test_extract_atom_bytes() {
        let atom_bytes = vec![0xC4, b't', b'e', b's', b't'];
        let full_path = build_multiplicity_path(&atom_bytes, 42);

        // Should extract the atom bytes correctly
        let extracted = extract_atom_bytes(&full_path);
        assert!(extracted.is_some());
        assert_eq!(extracted.unwrap(), atom_bytes.as_slice());

        // Edge cases: path too short
        assert!(extract_atom_bytes(&[]).is_none());
        assert!(extract_atom_bytes(&[0x03, 0xC1, b'M']).is_none()); // Just prefix
        assert!(extract_atom_bytes(&[0x03, 0xC1, b'M', 0x00]).is_none()); // Prefix + 1 byte < 8

        // Edge case: not a multiplicity entry
        assert!(extract_atom_bytes(&[0xC4, b't', b'e', b's', b't']).is_none());
    }

    #[test]
    fn test_extract_atom_and_count() {
        let atom_bytes = vec![0xC4, b't', b'e', b's', b't'];
        let count = 12345u64;
        let full_path = build_multiplicity_path(&atom_bytes, count);

        // Should extract both correctly
        let extracted = extract_atom_and_count(&full_path);
        assert!(extracted.is_some());
        let (atoms, cnt) = extracted.unwrap();
        assert_eq!(atoms, atom_bytes.as_slice());
        assert_eq!(cnt, count);

        // Edge cases
        assert!(extract_atom_and_count(&[]).is_none());
        assert!(extract_atom_and_count(&atom_bytes).is_none());
    }

    #[test]
    fn test_iterate_from_multiplicity_prefix() {
        use pathmap::zipper::ZipperIteration;

        let mut btm: PathMap<()> = PathMap::new();
        let atom1 = vec![0xC3, b'f', b'o', b'o'];
        let atom2 = vec![0xC3, b'b', b'a', b'r'];
        let atom3 = vec![0xC3, b'b', b'a', b'z'];

        // Add atoms with different multiplicities
        for _ in 0..3 {
            increment_multiplicity(&mut btm, &atom1);
        }
        for _ in 0..1 {
            increment_multiplicity(&mut btm, &atom2);
        }
        for _ in 0..5 {
            increment_multiplicity(&mut btm, &atom3);
        }

        // Iterate from multiplicity prefix and collect (atom, count) pairs
        let mut results: Vec<(Vec<u8>, u64)> = Vec::new();
        let mut rz = btm.read_zipper();
        rz.descend_to(multiplicity_prefix());

        while rz.to_next_val() {
            if let Some((atom_bytes, count)) = extract_atom_and_count(rz.path()) {
                results.push((atom_bytes.to_vec(), count));
            }
        }

        // Should have exactly 3 entries (one per unique atom)
        assert_eq!(results.len(), 3);

        // Verify each atom has correct count
        for (atom, count) in &results {
            if atom == &atom1 {
                assert_eq!(*count, 3, "atom1 should have count 3");
            } else if atom == &atom2 {
                assert_eq!(*count, 1, "atom2 should have count 1");
            } else if atom == &atom3 {
                assert_eq!(*count, 5, "atom3 should have count 5");
            } else {
                panic!("Unexpected atom in results: {:?}", atom);
            }
        }
    }

    #[test]
    fn test_iteration_vs_lookup_equivalence() {
        // Verify that iterating from prefix gives same results as individual lookups
        let mut btm: PathMap<()> = PathMap::new();

        // Create several atoms with various multiplicities
        let atoms: Vec<Vec<u8>> = vec![
            vec![0xC3, b'a', b'a', b'a'],
            vec![0xC3, b'b', b'b', b'b'],
            vec![0xC3, b'c', b'c', b'c'],
            vec![0xC4, b't', b'e', b's', b't'],
            vec![0xC5, b'h', b'e', b'l', b'l', b'o'],
        ];

        let counts: Vec<u64> = vec![1, 5, 10, 100, 1000];

        for (atom, &count) in atoms.iter().zip(counts.iter()) {
            for _ in 0..count {
                increment_multiplicity(&mut btm, atom);
            }
        }

        // Verify via individual lookups
        for (atom, &expected_count) in atoms.iter().zip(counts.iter()) {
            assert_eq!(
                get_multiplicity(&btm, atom),
                expected_count,
                "Individual lookup failed for {:?}",
                atom
            );
        }

        // Verify via iteration from prefix
        use pathmap::zipper::ZipperIteration;
        let mut rz = btm.read_zipper();
        rz.descend_to(multiplicity_prefix());

        let mut iter_count = 0;
        while rz.to_next_val() {
            if let Some((atom_bytes, count)) = extract_atom_and_count(rz.path()) {
                // Find this atom in our list and verify count
                let found = atoms.iter().zip(counts.iter()).find(|(a, _)| a.as_slice() == atom_bytes);
                assert!(found.is_some(), "Unexpected atom from iteration: {:?}", atom_bytes);
                let (_, &expected) = found.unwrap();
                assert_eq!(count, expected, "Count mismatch for {:?}", atom_bytes);
                iter_count += 1;
            }
        }

        assert_eq!(iter_count, atoms.len(), "Iteration should visit all atoms");
    }
}
