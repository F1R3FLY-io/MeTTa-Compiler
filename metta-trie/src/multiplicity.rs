//! Multiplicity counter for atom bag semantics.
//!
//! Tracks how many times an atom has been added to a space.
//! Implements `Lattice` and `DistributiveLattice` for algebraic trie operations.
//!
//! ## Lattice Semantics
//!
//! - `pjoin`: Additive — `Multiplicity(a + b)`
//! - `pmeet`: Minimum — `Multiplicity(min(a, b))`
//! - `psubtract`: Saturating subtraction — `None` if result ≤ 0

use crate::algebra::{AlgebraicResult, DistributiveLattice, Lattice, COUNTER_IDENT, SELF_IDENT};

/// Multiplicity counter with additive Lattice semantics.
///
/// Used for tracking atom counts in MeTTa HE semantics where multiple
/// additions of the same atom increase its multiplicity.
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
    /// Join is ADDITIVE: adds multiplicities together.
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
            AlgebraicResult::None
        } else {
            AlgebraicResult::Element(Multiplicity(self.0 - other.0))
        }
    }
}

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

        match a.pjoin(&b) {
            AlgebraicResult::Element(result) => assert_eq!(result.count(), 8),
            _ => panic!("Expected Element result from pjoin"),
        }
    }

    #[test]
    fn test_multiplicity_lattice_meet() {
        let a = Multiplicity::new(5);
        let b = Multiplicity::new(3);

        match a.pmeet(&b) {
            AlgebraicResult::Identity(COUNTER_IDENT) => {}
            other => panic!("Expected Identity(COUNTER_IDENT), got {other:?}"),
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
            AlgebraicResult::None => {}
            other => panic!("Expected None, got {other:?}"),
        }

        // 3 - 3 = 0 (should be None/removed)
        match b.psubtract(&b) {
            AlgebraicResult::None => {}
            other => panic!("Expected None, got {other:?}"),
        }
    }
}
