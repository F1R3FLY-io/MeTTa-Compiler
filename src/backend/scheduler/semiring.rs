//! Semiring types for the scheduler automaton.
//!
//! Extracted from `mettail-rust/prattail/src/automata/semiring.rs` (~300 LOC).
//! Provides the `Semiring` trait and concrete weight types needed for the
//! WFST/WPDS scheduler:
//!
//! - `TropicalWeight`: `(ℝ⁺∪{∞}, min, +, ∞, 0)` — lowest cost = highest priority
//! - `CountingWeight`: `(ℕ, +, ×, 0, 1)` — parallelism degree
//! - `ProductWeight<S1,S2>`: dual metric (cost + parallelism)
//!
//! The `Semiring` trait interface is preserved for future direct dependency on
//! prattail if needed.
//!
//! ## References
//!
//! - Mohri (2009), "Weighted automata algorithms"
//! - Reps, Lal & Kidd (2007), "Program analysis using weighted pushdown systems"

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};

// ══════════════════════════════════════════════════════════════════════════════
// Semiring trait
// ══════════════════════════════════════════════════════════════════════════════

/// A semiring `(K, ⊕, ⊗, 0̄, 1̄)` where `⊕` combines parallel paths and `⊗`
/// sequences path segments.
///
/// Properties required:
/// - `(K, ⊕, 0̄)` is a commutative monoid
/// - `(K, ⊗, 1̄)` is a monoid
/// - `⊗` distributes over `⊕`
/// - `0̄ ⊗ a = a ⊗ 0̄ = 0̄` (zero annihilates)
pub trait Semiring: Clone + Copy + fmt::Debug + PartialEq + Send + Sync + 'static {
    /// Additive identity (0̄). For tropical: `+∞` (unreachable).
    fn zero() -> Self;
    /// Multiplicative identity (1̄). For tropical: `0.0` (zero cost).
    fn one() -> Self;
    /// Semiring addition (⊕): combines parallel paths. For tropical: `min(a, b)`.
    fn plus(&self, other: &Self) -> Self;
    /// Semiring multiplication (⊗): sequences path segments. For tropical: `a + b`.
    fn times(&self, other: &Self) -> Self;
    /// Whether this is the additive identity.
    fn is_zero(&self) -> bool;
    /// Whether this is the multiplicative identity.
    fn is_one(&self) -> bool;
    /// Approximate equality for floating-point convergence checks.
    fn approx_eq(&self, other: &Self, epsilon: f64) -> bool;
}

/// Marker trait: `is_zero()` is O(1) and reliable.
pub trait DetectableZero: Semiring {}

/// Marker trait: `a ⊕ a = a` for all `a` (idempotent addition).
///
/// Guarantees fixed-point convergence in iterative algorithms.
pub trait IdempotentSemiring: Semiring {}

// ══════════════════════════════════════════════════════════════════════════════
// TropicalWeight
// ══════════════════════════════════════════════════════════════════════════════

/// Tropical semiring weight: `(ℝ⁺ ∪ {+∞}, min, +, +∞, 0.0)`.
///
/// - `⊕ = min`: selects the best (lowest-cost) alternative
/// - `⊗ = +`: accumulates costs along a path
/// - `0̄ = +∞`: unreachable (identity for min)
/// - `1̄ = 0.0`: zero cost (identity for addition)
///
/// Lower weight = higher priority. Used as the cost component in the
/// scheduler's `ProductWeight<TropicalWeight, CountingWeight>`.
#[derive(Clone, Copy)]
pub struct TropicalWeight(pub f64);

impl TropicalWeight {
    /// Create a new tropical weight.
    #[inline]
    pub const fn new(value: f64) -> Self {
        TropicalWeight(value)
    }

    /// Get the underlying `f64` value.
    #[inline]
    pub const fn value(self) -> f64 {
        self.0
    }

    /// Positive infinity (unreachable / zero element).
    #[inline]
    pub const fn infinity() -> Self {
        TropicalWeight(f64::INFINITY)
    }

    /// Whether this weight is infinite (unreachable).
    #[inline]
    pub fn is_infinite(self) -> bool {
        self.0.is_infinite()
    }
}

impl Semiring for TropicalWeight {
    #[inline]
    fn zero() -> Self {
        TropicalWeight::infinity()
    }

    #[inline]
    fn one() -> Self {
        TropicalWeight(0.0)
    }

    #[inline]
    fn plus(&self, other: &Self) -> Self {
        TropicalWeight(self.0.min(other.0))
    }

    #[inline]
    fn times(&self, other: &Self) -> Self {
        TropicalWeight(self.0 + other.0)
    }

    #[inline]
    fn is_zero(&self) -> bool {
        self.0.is_infinite() && self.0.is_sign_positive()
    }

    #[inline]
    fn is_one(&self) -> bool {
        self.0 == 0.0
    }

    fn approx_eq(&self, other: &Self, epsilon: f64) -> bool {
        if self.is_zero() && other.is_zero() {
            true
        } else if self.is_zero() || other.is_zero() {
            false
        } else {
            (self.0 - other.0).abs() <= epsilon
        }
    }
}

impl fmt::Debug for TropicalWeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            write!(f, "TropicalWeight(∞)")
        } else {
            write!(f, "TropicalWeight({:.2})", self.0)
        }
    }
}

impl fmt::Display for TropicalWeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_zero() {
            write!(f, "∞")
        } else {
            write!(f, "{:.2}", self.0)
        }
    }
}

impl PartialEq for TropicalWeight {
    fn eq(&self, other: &Self) -> bool {
        self.0.total_cmp(&other.0) == Ordering::Equal
    }
}

impl Eq for TropicalWeight {}

impl PartialOrd for TropicalWeight {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TropicalWeight {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Hash for TropicalWeight {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl Default for TropicalWeight {
    fn default() -> Self {
        Self::one()
    }
}

impl DetectableZero for TropicalWeight {}
impl IdempotentSemiring for TropicalWeight {}

// ══════════════════════════════════════════════════════════════════════════════
// CountingWeight
// ══════════════════════════════════════════════════════════════════════════════

/// Counting semiring: `(ℕ, +, ×, 0, 1)`.
///
/// Counts the number of derivations / parallel branches:
/// - `⊕ = +`: total alternatives (saturating)
/// - `⊗ = ×`: combinatorial product of sequential choices (saturating)
/// - `0̄ = 0`: no paths
/// - `1̄ = 1`: one path
///
/// Used as the parallelism component in `ProductWeight<TropicalWeight, CountingWeight>`.
/// The count indicates how many independent branches exist for wavefront scheduling.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CountingWeight(pub u64);

impl CountingWeight {
    /// Create a counting weight with the given path count.
    #[inline]
    pub const fn new(count: u64) -> Self {
        CountingWeight(count)
    }

    /// Get the path count.
    #[inline]
    pub const fn count(self) -> u64 {
        self.0
    }
}

impl Semiring for CountingWeight {
    #[inline]
    fn zero() -> Self {
        CountingWeight(0)
    }

    #[inline]
    fn one() -> Self {
        CountingWeight(1)
    }

    #[inline]
    fn plus(&self, other: &Self) -> Self {
        CountingWeight(self.0.saturating_add(other.0))
    }

    #[inline]
    fn times(&self, other: &Self) -> Self {
        CountingWeight(self.0.saturating_mul(other.0))
    }

    #[inline]
    fn is_zero(&self) -> bool {
        self.0 == 0
    }

    #[inline]
    fn is_one(&self) -> bool {
        self.0 == 1
    }

    fn approx_eq(&self, other: &Self, _epsilon: f64) -> bool {
        self.0 == other.0
    }
}

impl fmt::Debug for CountingWeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CountingWeight({})", self.0)
    }
}

impl fmt::Display for CountingWeight {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for CountingWeight {
    fn default() -> Self {
        Self::one()
    }
}

impl PartialOrd for CountingWeight {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CountingWeight {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl DetectableZero for CountingWeight {}
// CountingWeight is NOT idempotent: plus(3, 3) = 6 ≠ 3

// ══════════════════════════════════════════════════════════════════════════════
// ProductWeight
// ══════════════════════════════════════════════════════════════════════════════

/// Product semiring: component-wise operations over two semirings.
///
/// `ProductWeight<TropicalWeight, CountingWeight>` provides dual metrics:
/// - Left (tropical): execution cost — lower is better
/// - Right (counting): parallelism degree — number of independent branches
///
/// The `⊕` (plus) selects the minimum-cost alternative while summing branch
/// counts. The `⊗` (times) accumulates costs and multiplies branch counts
/// along sequential paths.
#[derive(Clone, Copy)]
pub struct ProductWeight<S1: Semiring, S2: Semiring> {
    /// First component weight (cost).
    pub left: S1,
    /// Second component weight (parallelism).
    pub right: S2,
}

impl<S1: Semiring, S2: Semiring> ProductWeight<S1, S2> {
    /// Create a product weight from two components.
    #[inline]
    pub const fn new(left: S1, right: S2) -> Self {
        ProductWeight { left, right }
    }
}

impl<S1: Semiring + Eq + Hash + fmt::Display, S2: Semiring + Eq + Hash + fmt::Display> Semiring
    for ProductWeight<S1, S2>
{
    #[inline]
    fn zero() -> Self {
        ProductWeight {
            left: S1::zero(),
            right: S2::zero(),
        }
    }

    #[inline]
    fn one() -> Self {
        ProductWeight {
            left: S1::one(),
            right: S2::one(),
        }
    }

    #[inline]
    fn plus(&self, other: &Self) -> Self {
        ProductWeight {
            left: self.left.plus(&other.left),
            right: self.right.plus(&other.right),
        }
    }

    #[inline]
    fn times(&self, other: &Self) -> Self {
        ProductWeight {
            left: self.left.times(&other.left),
            right: self.right.times(&other.right),
        }
    }

    #[inline]
    fn is_zero(&self) -> bool {
        self.left.is_zero() || self.right.is_zero()
    }

    #[inline]
    fn is_one(&self) -> bool {
        self.left.is_one() && self.right.is_one()
    }

    fn approx_eq(&self, other: &Self, epsilon: f64) -> bool {
        self.left.approx_eq(&other.left, epsilon)
            && self.right.approx_eq(&other.right, epsilon)
    }
}

impl<S1: Semiring + PartialEq, S2: Semiring + PartialEq> PartialEq for ProductWeight<S1, S2> {
    fn eq(&self, other: &Self) -> bool {
        self.left == other.left && self.right == other.right
    }
}

impl<S1: Semiring + Eq, S2: Semiring + Eq> Eq for ProductWeight<S1, S2> {}

impl<S1: Semiring + Ord, S2: Semiring + Ord> PartialOrd for ProductWeight<S1, S2> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Lexicographic ordering: compare left (cost) first, then right (parallelism).
impl<S1: Semiring + Ord, S2: Semiring + Ord> Ord for ProductWeight<S1, S2> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.left
            .cmp(&other.left)
            .then_with(|| self.right.cmp(&other.right))
    }
}

impl<S1: Semiring + Hash, S2: Semiring + Hash> Hash for ProductWeight<S1, S2> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.left.hash(state);
        self.right.hash(state);
    }
}

impl<S1: Semiring + fmt::Display, S2: Semiring + fmt::Display> fmt::Display
    for ProductWeight<S1, S2>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.left, self.right)
    }
}

impl<S1: Semiring + fmt::Display, S2: Semiring + fmt::Display> fmt::Debug
    for ProductWeight<S1, S2>
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ProductWeight({}, {})", self.left, self.right)
    }
}

impl<S1: Semiring, S2: Semiring> Default for ProductWeight<S1, S2> {
    fn default() -> Self {
        ProductWeight {
            left: S1::one(),
            right: S2::one(),
        }
    }
}

impl<S1: DetectableZero, S2: DetectableZero> DetectableZero for ProductWeight<S1, S2> where
    ProductWeight<S1, S2>: Semiring
{
}

impl<S1: IdempotentSemiring, S2: IdempotentSemiring> IdempotentSemiring
    for ProductWeight<S1, S2>
where
    ProductWeight<S1, S2>: Semiring,
{
}

/// Convenience type alias for the scheduler's dual-metric weight.
///
/// - Left (tropical): execution cost (lower = schedule first)
/// - Right (counting): parallelism degree (number of independent branches)
pub type SchedulerWeight = ProductWeight<TropicalWeight, CountingWeight>;

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tropical_semiring_laws() {
        let a = TropicalWeight::new(3.0);
        let b = TropicalWeight::new(5.0);
        let zero = TropicalWeight::zero();
        let one = TropicalWeight::one();

        // plus = min
        assert_eq!(a.plus(&b), TropicalWeight::new(3.0));
        // times = +
        assert_eq!(a.times(&b), TropicalWeight::new(8.0));
        // zero identity for plus
        assert_eq!(a.plus(&zero), a);
        // one identity for times
        assert_eq!(a.times(&one), a);
        // zero annihilates times
        assert!(a.times(&zero).is_zero());
    }

    #[test]
    fn test_counting_semiring_laws() {
        let a = CountingWeight::new(3);
        let b = CountingWeight::new(5);
        let zero = CountingWeight::zero();
        let one = CountingWeight::one();

        // plus = add
        assert_eq!(a.plus(&b), CountingWeight::new(8));
        // times = mul
        assert_eq!(a.times(&b), CountingWeight::new(15));
        // zero identity for plus
        assert_eq!(a.plus(&zero), a);
        // one identity for times
        assert_eq!(a.times(&one), a);
        // zero annihilates
        assert!(a.times(&zero).is_zero());
    }

    #[test]
    fn test_counting_saturating() {
        let big = CountingWeight::new(u64::MAX);
        let two = CountingWeight::new(2);
        assert_eq!(big.plus(&two), CountingWeight::new(u64::MAX));
        assert_eq!(big.times(&two), CountingWeight::new(u64::MAX));
    }

    #[test]
    fn test_product_weight() {
        let a = SchedulerWeight::new(TropicalWeight::new(3.0), CountingWeight::new(2));
        let b = SchedulerWeight::new(TropicalWeight::new(5.0), CountingWeight::new(3));

        // plus: min cost, sum counts
        let sum = a.plus(&b);
        assert_eq!(sum.left, TropicalWeight::new(3.0));
        assert_eq!(sum.right, CountingWeight::new(5));

        // times: add costs, mul counts
        let prod = a.times(&b);
        assert_eq!(prod.left, TropicalWeight::new(8.0));
        assert_eq!(prod.right, CountingWeight::new(6));
    }

    #[test]
    fn test_product_zero_one() {
        let zero = SchedulerWeight::zero();
        let one = SchedulerWeight::one();
        let a = SchedulerWeight::new(TropicalWeight::new(3.0), CountingWeight::new(2));

        assert!(zero.is_zero());
        assert!(one.is_one());
        assert_eq!(a.plus(&zero).left, a.left);
        assert_eq!(a.times(&one), a);
    }

    #[test]
    fn test_tropical_approx_eq() {
        let a = TropicalWeight::new(3.0);
        let b = TropicalWeight::new(3.001);
        assert!(a.approx_eq(&b, 0.01));
        assert!(!a.approx_eq(&b, 0.0001));
    }
}
