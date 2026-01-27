//! Value-based multiplicity tracking for MeTTa HE semantics.
//!
//! Atoms map directly to their multiplicities: `mork_bytes → Multiplicity(count)`
//! No suffix encoding. Uses direct zipper operations for efficiency.
//!
//! # Data Layout
//!
//! ```text
//! btm: PathMap<Multiplicity>
//! ─────────────────────────────────
//! mork_bytes → Multiplicity(count)   [single entry - atom + count unified]
//! ```
//!
//! # Benefits over dual-entry approach
//!
//! - Single entry per atom (50% storage reduction)
//! - No suffix encoding/decoding overhead
//! - No iteration filtering - just iterate all entries
//! - Direct `rz.val()` read during iteration (no separate lookup)
//! - Lattice operations for efficient batch merges

use pathmap::ring::{AlgebraicResult, DistributiveLattice, Lattice, COUNTER_IDENT, SELF_IDENT};
use pathmap::PathMap;
use pathmap::zipper::{Zipper, ZipperMoving, ZipperValues, ZipperWriting};

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   `Multiplicity` Type                                                   *-=

/// Multiplicity counter with additive Lattice semantics.
///
/// Used for tracking atom counts in MeTTa HE semantics where multiple
/// additions of the same atom increase its multiplicity.
///
/// # Lattice Semantics (per MORK/PathMap multiplicities spec)
/// - `pjoin`: Additive - returns `Multiplicity(self.0 + other.0)`
/// - `pmeet`: Minimum - returns `Multiplicity(min(self.0, other.0))`
/// - `psubtract`: Saturating subtraction - returns `None` if result is 0
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Multiplicity(pub u64);

impl Multiplicity {
    /// Create a new Multiplicity with the given count.
    #[inline]
    pub const fn new(count: u64) -> Self {
        Self(count)
    }

    /// Get the count value.
    #[inline]
    pub const fn count(&self) -> u64 {
        self.0
    }

    /// Check if the count is zero.
    #[inline]
    pub const fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl Lattice for Multiplicity {
    /// Join is ADDITIVE per MORK spec: adds multiplicities together.
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Element(Multiplicity(self.0.saturating_add(other.0)))
    }

    /// Meet returns minimum (intersection semantics).
    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        let min = self.0.min(other.0);
        if min == self.0 {
            AlgebraicResult::Identity(SELF_IDENT)
        } else {
            AlgebraicResult::Identity(COUNTER_IDENT)
        }
    }
}

impl DistributiveLattice for Multiplicity {
    /// Subtract with removal semantics.
    /// Returns `None` if result would be 0 (entry should be removed).
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self> {
        if other.0 >= self.0 {
            AlgebraicResult::None // Entry should be removed
        } else {
            AlgebraicResult::Element(Multiplicity(self.0 - other.0))
        }
    }
}

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   Multiplicity Operations                                               *-=

/// Get multiplicity for an atom directly from the value.
/// Returns 0 if the path doesn't exist.
#[inline]
pub fn get_multiplicity(btm: &PathMap<Multiplicity>, path: &[u8]) -> u64 {
    let mut rz = btm.read_zipper();
    rz.descend_to(path);
    if rz.path_exists() && rz.is_val() {
        rz.val().map(|m| m.count()).unwrap_or(0)
    } else {
        0
    }
}

/// Add atom with multiplicity 1, or increment if exists.
/// Uses write zipper for single-path efficiency (no intermediate PathMap allocation).
#[inline]
pub fn add_atom(btm: &mut PathMap<Multiplicity>, path: &[u8]) {
    let mut wz = btm.write_zipper();
    wz.descend_to(path);
    let new_val = match wz.val() {
        Some(m) => Multiplicity::new(m.count().saturating_add(1)),
        None => Multiplicity::new(1),
    };
    wz.set_val(new_val);
}

/// Decrement multiplicity. Entry is auto-removed when count reaches 0.
/// Returns the new count (0 if removed).
#[inline]
pub fn remove_atom(btm: &mut PathMap<Multiplicity>, path: &[u8]) -> u64 {
    let mut wz = btm.write_zipper();
    wz.descend_to(path);
    if !wz.path_exists() || !wz.is_val() {
        return 0; // Path doesn't exist or has no value
    }

    match wz.val() {
        Some(m) if m.count() > 1 => {
            let new_count = m.count() - 1;
            wz.set_val(Multiplicity::new(new_count));
            new_count
        }
        Some(_) => {
            // Count is 1, remove the entry entirely with pruning
            wz.remove_val(true); // true = prune empty nodes
            0
        }
        None => 0,
    }
}

/// Set multiplicity to a specific value.
/// If count is 0, removes the entry entirely.
#[inline]
pub fn set_multiplicity(btm: &mut PathMap<Multiplicity>, path: &[u8], count: u64) {
    if count == 0 {
        btm.remove(path);
    } else {
        let mut wz = btm.write_zipper();
        wz.descend_to(path);
        wz.set_val(Multiplicity::new(count));
    }
}

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   Legacy Compatibility                                                  *-=

/// Increment multiplicity (legacy API - calls add_atom internally).
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `path` - The path bytes of the atom
///
/// # Returns
/// The new multiplicity count after incrementing
#[inline]
pub fn increment_multiplicity(btm: &mut PathMap<Multiplicity>, path: &[u8]) -> u64 {
    add_atom(btm, path);
    get_multiplicity(btm, path)
}

/// Decrement multiplicity (legacy API - calls remove_atom internally).
///
/// # Arguments
/// * `btm` - The PathMap to update (mutable)
/// * `path` - The path bytes of the atom
///
/// # Returns
/// The new multiplicity count after decrementing (0 if fully removed)
#[inline]
pub fn decrement_multiplicity(btm: &mut PathMap<Multiplicity>, path: &[u8]) -> u64 {
    remove_atom(btm, path)
}

// =-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-==-**-=
// =-*   Tests                                                                 *-=

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiplicity_basic() {
        let m = Multiplicity::new(42);
        assert_eq!(m.count(), 42);
        assert!(!m.is_zero());

        let zero = Multiplicity::new(0);
        assert!(zero.is_zero());
    }

    #[test]
    fn test_multiplicity_lattice_join() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // Join should ADD: 5 + 3 = 8
        match a.pjoin(&b) {
            AlgebraicResult::Element(result) => assert_eq!(result.count(), 8),
            _ => panic!("Expected Element result from pjoin"),
        }
    }

    #[test]
    fn test_multiplicity_lattice_meet() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // Meet should return minimum
        match a.pmeet(&b) {
            AlgebraicResult::Identity(COUNTER_IDENT) => {} // b is smaller
            _ => panic!("Expected Identity(COUNTER_IDENT) from pmeet"),
        }
    }

    #[test]
    fn test_multiplicity_lattice_subtract() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        // 5 - 3 = 2
        match a.psubtract(&b) {
            AlgebraicResult::Element(result) => assert_eq!(result.count(), 2),
            _ => panic!("Expected Element result from psubtract"),
        }

        // 3 - 5 = 0 (should be None/removed)
        match b.psubtract(&a) {
            AlgebraicResult::None => {} // Correct - entry should be removed
            _ => panic!("Expected None from psubtract when result would be <= 0"),
        }

        // 3 - 3 = 0 (should be None/removed)
        match b.psubtract(&b) {
            AlgebraicResult::None => {} // Correct - entry should be removed
            _ => panic!("Expected None from psubtract when result would be 0"),
        }
    }

    #[test]
    fn test_get_multiplicity() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b'f', b'o', b'o'];

        // Initially no entry
        assert_eq!(get_multiplicity(&btm, &path), 0);

        // Add entry
        btm.insert(&path, Multiplicity::new(42));
        assert_eq!(get_multiplicity(&btm, &path), 42);
    }

    #[test]
    fn test_add_remove_atom() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b'f', b'o', b'o'];

        // Add atom (0 -> 1)
        add_atom(&mut btm, &path);
        assert_eq!(get_multiplicity(&btm, &path), 1);

        // Add again (1 -> 2)
        add_atom(&mut btm, &path);
        assert_eq!(get_multiplicity(&btm, &path), 2);

        // Add again (2 -> 3)
        add_atom(&mut btm, &path);
        assert_eq!(get_multiplicity(&btm, &path), 3);

        // Remove (3 -> 2)
        assert_eq!(remove_atom(&mut btm, &path), 2);

        // Remove (2 -> 1)
        assert_eq!(remove_atom(&mut btm, &path), 1);

        // Remove (1 -> 0, entry removed)
        assert_eq!(remove_atom(&mut btm, &path), 0);
        assert_eq!(get_multiplicity(&btm, &path), 0);

        // Remove when already 0
        assert_eq!(remove_atom(&mut btm, &path), 0);
    }

    #[test]
    fn test_multiple_atoms() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path1 = vec![0xC3, b'f', b'o', b'o'];
        let path2 = vec![0xC3, b'b', b'a', b'r'];
        let path3 = vec![0xC3, b'b', b'a', b'z'];

        // Add different atoms
        add_atom(&mut btm, &path1);
        add_atom(&mut btm, &path2);
        add_atom(&mut btm, &path3);

        // Check each tracked independently
        assert_eq!(get_multiplicity(&btm, &path1), 1);
        assert_eq!(get_multiplicity(&btm, &path2), 1);
        assert_eq!(get_multiplicity(&btm, &path3), 1);

        // Add path1 multiple times
        add_atom(&mut btm, &path1);
        add_atom(&mut btm, &path1);

        // Others unaffected
        assert_eq!(get_multiplicity(&btm, &path1), 3);
        assert_eq!(get_multiplicity(&btm, &path2), 1);
        assert_eq!(get_multiplicity(&btm, &path3), 1);
    }

    #[test]
    fn test_set_multiplicity() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b'f', b'o', b'o'];

        // Set to specific value
        set_multiplicity(&mut btm, &path, 42);
        assert_eq!(get_multiplicity(&btm, &path), 42);

        // Set to different value
        set_multiplicity(&mut btm, &path, 100);
        assert_eq!(get_multiplicity(&btm, &path), 100);

        // Set to 0 (removes entry)
        set_multiplicity(&mut btm, &path, 0);
        assert_eq!(get_multiplicity(&btm, &path), 0);
        assert!(!btm.contains(&path));
    }

    #[test]
    fn test_fork_isolation() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b'f', b'o', b'o'];

        // Set up initial multiplicity
        add_atom(&mut btm, &path);
        assert_eq!(get_multiplicity(&btm, &path), 1);

        // Fork via clone (O(1) via Arc CoW)
        let mut forked = btm.clone();

        // Increment in forked
        add_atom(&mut forked, &path);

        // Isolation: original unchanged, forked incremented
        assert_eq!(get_multiplicity(&btm, &path), 1);
        assert_eq!(get_multiplicity(&forked, &path), 2);
    }

    #[test]
    fn test_large_counts() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b'b', b'i', b'g'];

        // Increment to a large number
        for i in 1..=1000 {
            add_atom(&mut btm, &path);
            assert_eq!(get_multiplicity(&btm, &path), i);
        }

        // Decrement back down
        for i in (0..1000).rev() {
            assert_eq!(remove_atom(&mut btm, &path), i);
        }
        assert_eq!(get_multiplicity(&btm, &path), 0);
    }

    #[test]
    fn test_legacy_api_compatibility() {
        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path = vec![0xC3, b't', b'e', b's', b't'];

        // Test legacy increment/decrement
        assert_eq!(increment_multiplicity(&mut btm, &path), 1);
        assert_eq!(increment_multiplicity(&mut btm, &path), 2);
        assert_eq!(increment_multiplicity(&mut btm, &path), 3);

        assert_eq!(decrement_multiplicity(&mut btm, &path), 2);
        assert_eq!(decrement_multiplicity(&mut btm, &path), 1);
        assert_eq!(decrement_multiplicity(&mut btm, &path), 0);
    }

    #[test]
    fn test_iteration_with_multiplicity() {
        use pathmap::zipper::ZipperIteration;

        let mut btm: PathMap<Multiplicity> = PathMap::new();
        let path1 = vec![0xC3, b'f', b'o', b'o'];
        let path2 = vec![0xC3, b'b', b'a', b'r'];
        let path3 = vec![0xC3, b'b', b'a', b'z'];

        // Add atoms with different multiplicities
        for _ in 0..3 {
            add_atom(&mut btm, &path1);
        }
        add_atom(&mut btm, &path2);
        for _ in 0..5 {
            add_atom(&mut btm, &path3);
        }

        // Iterate and collect (path, count) pairs
        let mut results: Vec<(Vec<u8>, u64)> = Vec::new();
        let mut rz = btm.read_zipper();

        while rz.to_next_val() {
            let path = rz.path().to_vec();
            let count = rz.val().map(|m| m.count()).unwrap_or(0);
            results.push((path, count));
        }

        // Should have exactly 3 entries
        assert_eq!(results.len(), 3);

        // Verify counts
        for (path, count) in &results {
            if path == &path1 {
                assert_eq!(*count, 3);
            } else if path == &path2 {
                assert_eq!(*count, 1);
            } else if path == &path3 {
                assert_eq!(*count, 5);
            }
        }
    }
}
