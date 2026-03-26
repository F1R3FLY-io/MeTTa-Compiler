//! Coroutine-Based Lazy Nondeterminism (Phase 5.1, Lua VM-inspired)
//!
//! Branches as coroutines that yield results one at a time. Consumers with
//! known demand (e.g., `first-result`, `best-candidate`, `if-reducible`)
//! can stop early without evaluating remaining branches.
//!
//! ## Demand Levels
//!
//! - `All`: Evaluate all branches (current behavior, default)
//! - `Exactly(n)`: Stop after n results
//! - `AtLeast(n)`: Need at least n, but accept more
//!
//! When `demand != All`, a `BranchCoroutine` wraps the remaining
//! unevaluated branches and yields results incrementally.

use smallvec::SmallVec;

use crate::backend::models::{GenericBindings, MettaValueTrait};

// ============================================================================
// Demand
// ============================================================================

/// How many results the consumer needs from nondeterministic branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Demand {
    /// Need all results (current behavior).
    All,
    /// Need exactly N results, then stop.
    Exactly(usize),
    /// Need at least N results.
    AtLeast(usize),
}

impl Demand {
    /// Check if the demand is satisfied by the given count.
    #[inline]
    pub fn is_satisfied(&self, count: usize) -> bool {
        match self {
            Demand::All => false, // Never satisfied until exhausted
            Demand::Exactly(n) => count >= *n,
            Demand::AtLeast(n) => count >= *n,
        }
    }

    /// Check if this is the default (all) demand.
    #[inline]
    pub fn is_all(&self) -> bool {
        matches!(self, Demand::All)
    }
}

impl Default for Demand {
    fn default() -> Self { Demand::All }
}

// ============================================================================
// Branch Coroutine
// ============================================================================

/// A suspended nondeterministic branch producer.
///
/// Wraps remaining unevaluated `(rhs, bindings)` pairs and yields results
/// one at a time. The consumer can stop early when demand is satisfied.
#[derive(Debug)]
pub struct BranchCoroutine<V: MettaValueTrait> {
    /// Remaining unevaluated (rhs, bindings) pairs.
    remaining: Vec<(V, GenericBindings<V>)>,
    /// Results yielded so far.
    yielded: SmallVec<[V; 2]>,
    /// Consumer demand level.
    demand: Demand,
    /// Current index into remaining.
    cursor: usize,
}

/// Result of advancing a coroutine.
#[derive(Debug)]
pub enum CoroutineStep<V: MettaValueTrait> {
    /// One result is available for the consumer.
    Yield(V),
    /// All branches exhausted or demand satisfied.
    Done(SmallVec<[V; 2]>),
}

impl<V: MettaValueTrait + Clone> BranchCoroutine<V> {
    /// Create a new coroutine from a list of unevaluated branches.
    pub fn new(branches: Vec<(V, GenericBindings<V>)>, demand: Demand) -> Self {
        Self {
            remaining: branches,
            yielded: SmallVec::new(),
            demand,
            cursor: 0,
        }
    }

    /// Get the next unevaluated branch, if any and demand not yet satisfied.
    ///
    /// Returns `Some((rhs, bindings))` for the next branch to evaluate,
    /// or `None` if demand is satisfied or branches are exhausted.
    pub fn next_branch(&mut self) -> Option<(V, GenericBindings<V>)> {
        if self.demand.is_satisfied(self.yielded.len()) {
            return None;
        }
        if self.cursor >= self.remaining.len() {
            return None;
        }
        let (rhs, bindings) = self.remaining[self.cursor].clone();
        self.cursor += 1;
        Some((rhs, bindings))
    }

    /// Record a result from an evaluated branch.
    pub fn record_result(&mut self, result: V) {
        self.yielded.push(result);
    }

    /// Record multiple results.
    pub fn record_results(&mut self, results: impl IntoIterator<Item = V>) {
        self.yielded.extend(results);
    }

    /// Check if demand is satisfied.
    #[inline]
    pub fn is_satisfied(&self) -> bool {
        self.demand.is_satisfied(self.yielded.len())
    }

    /// Check if all branches are exhausted.
    #[inline]
    pub fn is_exhausted(&self) -> bool {
        self.cursor >= self.remaining.len()
    }

    /// Check if done (satisfied or exhausted).
    #[inline]
    pub fn is_done(&self) -> bool {
        self.is_satisfied() || self.is_exhausted()
    }

    /// Take the collected results.
    pub fn take_results(self) -> SmallVec<[V; 2]> {
        self.yielded
    }

    /// Collect all V values reachable from this coroutine for GC root tracing.
    ///
    /// Includes remaining unevaluated RHS templates + their bindings (from cursor
    /// onward) and already-yielded results. Without this, a GC safepoint during
    /// lazy rule matching would miss these values, causing use-after-free when the
    /// coroutine yields the next branch and MORK serialization dereferences freed
    /// slab pointers.
    pub fn collect_values(&self, out: &mut Vec<V>) {
        for (rhs, bindings) in &self.remaining[self.cursor..] {
            out.push(rhs.clone());
            for (_name, val) in bindings.iter() {
                out.push(val.clone());
            }
        }
        out.extend(self.yielded.iter().cloned());
    }

    /// Number of results yielded so far.
    #[inline]
    pub fn result_count(&self) -> usize {
        self.yielded.len()
    }

    /// Number of remaining unevaluated branches.
    #[inline]
    pub fn remaining_count(&self) -> usize {
        self.remaining.len() - self.cursor
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValue, MettaValueFactory, global_factory};

    fn f() -> crate::backend::models::GcFactory { global_factory() }

    #[test]
    fn test_demand_all() {
        let d = Demand::All;
        assert!(!d.is_satisfied(0));
        assert!(!d.is_satisfied(100));
        assert!(d.is_all());
    }

    #[test]
    fn test_demand_exactly() {
        let d = Demand::Exactly(3);
        assert!(!d.is_satisfied(0));
        assert!(!d.is_satisfied(2));
        assert!(d.is_satisfied(3));
        assert!(d.is_satisfied(5));
    }

    #[test]
    fn test_coroutine_basic() {
        let branches = vec![
            (f().long(1), GenericBindings::Empty),
            (f().long(2), GenericBindings::Empty),
            (f().long(3), GenericBindings::Empty),
        ];
        let mut coro = BranchCoroutine::new(branches, Demand::All);

        assert!(!coro.is_done());
        assert_eq!(coro.remaining_count(), 3);

        // Consume all branches
        while let Some((rhs, _)) = coro.next_branch() {
            coro.record_result(rhs);
        }

        assert!(coro.is_done());
        assert_eq!(coro.result_count(), 3);
    }

    #[test]
    fn test_coroutine_demand_exactly_1() {
        let branches = vec![
            (f().long(1), GenericBindings::Empty),
            (f().long(2), GenericBindings::Empty),
            (f().long(3), GenericBindings::Empty),
        ];
        let mut coro = BranchCoroutine::new(branches, Demand::Exactly(1));

        // Get first branch
        let (rhs, _) = coro.next_branch().expect("should have branch");
        coro.record_result(rhs);

        // Demand satisfied — next_branch returns None
        assert!(coro.is_satisfied());
        assert!(coro.next_branch().is_none());

        let results = coro.take_results();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(1));
    }

    #[test]
    fn test_coroutine_demand_at_least_2() {
        let branches = vec![
            (f().long(10), GenericBindings::Empty),
            (f().long(20), GenericBindings::Empty),
            (f().long(30), GenericBindings::Empty),
        ];
        let mut coro = BranchCoroutine::new(branches, Demand::AtLeast(2));

        // Get first two branches
        for _ in 0..2 {
            let (rhs, _) = coro.next_branch().expect("should have branch");
            coro.record_result(rhs);
        }

        assert!(coro.is_satisfied());
        assert_eq!(coro.result_count(), 2);
    }

    #[test]
    fn test_coroutine_empty_branches() {
        let branches: Vec<(MettaValue, GenericBindings<MettaValue>)> = vec![];
        let mut coro = BranchCoroutine::new(branches, Demand::All);

        assert!(coro.is_exhausted());
        assert!(coro.next_branch().is_none());
    }
}
