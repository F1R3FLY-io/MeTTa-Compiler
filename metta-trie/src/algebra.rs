//! Algebraic Lattice Traits for MettaTrie
//!
//! Defines `AlgebraicResult`, `Lattice`, and `DistributiveLattice` traits that
//! govern how values are combined during trie algebraic operations (join, meet,
//! subtract, restrict).
//!
//! These are equivalent to PathMap's `ring` module traits but defined independently
//! to avoid the PathMap dependency.
//!
//! ## Design
//!
//! `AlgebraicResult<V>` avoids unnecessary cloning by indicating when the result
//! is one of the inputs unchanged:
//!
//! - `None` — elements annihilate, entry should be removed from the trie
//! - `Identity(SELF_IDENT)` — result is `self`, no clone needed
//! - `Identity(COUNTER_IDENT)` — result is `other`, no clone needed
//! - `Element(V)` — new computed value

/// Bitmask indicating the result is `self` (the left operand).
pub const SELF_IDENT: u64 = 0x1;

/// Bitmask indicating the result is `other` (the right operand).
pub const COUNTER_IDENT: u64 = 0x2;

/// Result of combining two lattice elements.
///
/// Avoids unnecessary cloning by indicating when the result is one of the
/// inputs unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlgebraicResult<V> {
    /// Elements annihilate — the entry should be removed from the trie.
    None,

    /// The result is one of the inputs, identified by bitmask:
    /// - `SELF_IDENT` (0x1): result is `self`
    /// - `COUNTER_IDENT` (0x2): result is `other`
    Identity(u64),

    /// A new computed value.
    Element(V),
}

/// Status returned by in-place algebraic operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgebraicStatus {
    /// No changes were made.
    Unchanged,
    /// Changes were made.
    Modified,
}

/// Lattice with join (union) and meet (intersection) operations.
///
/// # Implementor Contract
///
/// - `pjoin` must be commutative: `a.pjoin(&b)` ≡ `b.pjoin(&a)` (semantically)
/// - `pmeet` must be commutative: `a.pmeet(&b)` ≡ `b.pmeet(&a)` (semantically)
/// - `pjoin` is associative: `a.pjoin(&b).pjoin(&c)` ≡ `a.pjoin(&b.pjoin(&c))`
/// - Absorption: `a.pjoin(&a.pmeet(&b))` ≡ `a`
pub trait Lattice: Clone + Sized {
    /// Join (union). Combines two values into their least upper bound.
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self>;

    /// Meet (intersection). Combines two values into their greatest lower bound.
    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self>;
}

/// Extends `Lattice` with subtraction (set difference).
///
/// # Implementor Contract
///
/// - `psubtract` removes the contribution of `other` from `self`
/// - Returns `None` when the subtraction fully annihilates the value
pub trait DistributiveLattice: Lattice {
    /// Subtract `other` from `self`.
    ///
    /// Returns `None` if the result would be zero/empty (entry should be removed).
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simple counter type for testing Lattice operations.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct Count(u64);

    impl Lattice for Count {
        fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
            AlgebraicResult::Element(Count(self.0.saturating_add(other.0)))
        }

        fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
            let min = self.0.min(other.0);
            if min == self.0 {
                AlgebraicResult::Identity(SELF_IDENT)
            } else {
                AlgebraicResult::Identity(COUNTER_IDENT)
            }
        }
    }

    impl DistributiveLattice for Count {
        fn psubtract(&self, other: &Self) -> AlgebraicResult<Self> {
            if other.0 >= self.0 {
                AlgebraicResult::None
            } else {
                AlgebraicResult::Element(Count(self.0 - other.0))
            }
        }
    }

    #[test]
    fn test_count_join() {
        let a = Count(5);
        let b = Count(3);
        match a.pjoin(&b) {
            AlgebraicResult::Element(result) => assert_eq!(result, Count(8)),
            _ => panic!("Expected Element"),
        }
    }

    #[test]
    fn test_count_meet() {
        let a = Count(5);
        let b = Count(3);
        match a.pmeet(&b) {
            AlgebraicResult::Identity(COUNTER_IDENT) => {} // b is smaller
            other => panic!("Expected Identity(COUNTER_IDENT), got {other:?}"),
        }
    }

    #[test]
    fn test_count_subtract() {
        let a = Count(5);
        let b = Count(3);
        match a.psubtract(&b) {
            AlgebraicResult::Element(result) => assert_eq!(result, Count(2)),
            _ => panic!("Expected Element"),
        }
        match b.psubtract(&a) {
            AlgebraicResult::None => {} // annihilated
            other => panic!("Expected None, got {other:?}"),
        }
    }
}
