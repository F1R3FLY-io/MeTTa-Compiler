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
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::continuation_spine::{ContinuationAddr, SpineStore};
use crate::backend::models::{GenericBindings, MettaValue, MettaValueTrait};

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
    fn default() -> Self {
        Demand::All
    }
}

// ============================================================================
// CancelToken — cooperative cancellation for parallel-dispatch boundaries
// ============================================================================

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Marker payload thrown via `panic::resume_unwind` when a parallel-branch
/// worker observes that its demand has been satisfied by a sibling. Caught
/// at the worker closure boundary in `priority_scheduler::execute`; the
/// catch arm uses the marker to release the slot cleanly without surfacing
/// a panic.
#[derive(Debug, Clone, Copy)]
pub struct BranchCancelled;

/// Shared cancellation state for one `parallel_branch_eval` /
/// `parallel_collapse_eval` invocation. All workers spawned for the call
/// hold an `Arc<CancelToken>`. When any branch produces a result that
/// satisfies the demand, the token's `satisfied` flag flips; siblings
/// observe the flip cooperatively at the next safepoint and bail.
///
/// For `Demand::All`, `record_branch_result` is a no-op and `is_satisfied`
/// returns `false` forever — the token is allocated but cancellation never
/// fires. This keeps the codepath uniform without special-casing.
pub struct CancelToken {
    /// True once `non_empty_branches` has reached the demand threshold.
    satisfied: AtomicBool,
    /// Count of branches that produced at least one non-empty result.
    /// Empty branches don't count — they don't satisfy "if-not-empty"
    /// patterns like `(if (not (== ... ())) ...)`.
    non_empty_branches: AtomicU32,
    /// The demand level that determines when `satisfied` flips.
    demand: Demand,
}

impl CancelToken {
    #[inline]
    pub fn new(demand: Demand) -> Self {
        Self {
            satisfied: AtomicBool::new(false),
            non_empty_branches: AtomicU32::new(0),
            demand,
        }
    }

    /// Record that a branch produced `branch_result_count` results, of
    /// which any may have been non-empty (caller decides eligibility before
    /// calling). Increments the non-empty counter; if the demand threshold
    /// is reached, atomically flips `satisfied` from false→true.
    ///
    /// Returns `true` iff this call performed the false→true transition.
    /// Subsequent calls return `false`. Useful for trace emission.
    #[inline]
    pub fn record_non_empty_branch(&self) -> bool {
        // Demand::All never satisfies — short-circuit cheap path.
        if matches!(self.demand, Demand::All) {
            return false;
        }
        let prev = self.non_empty_branches.fetch_add(1, Ordering::AcqRel);
        let new_count = prev as usize + 1;
        if self.demand.is_satisfied(new_count) {
            // single-shot transition via compare_exchange
            self.satisfied
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        } else {
            false
        }
    }

    /// Cooperative observation point. Workers call this at safepoints; the
    /// parent wait loop calls it on every tick. Acquire ordering pairs with
    /// the Release in the `compare_exchange` flip in `record_non_empty_branch`.
    #[inline]
    pub fn is_satisfied(&self) -> bool {
        self.satisfied.load(Ordering::Acquire)
    }

    /// Manually flip the satisfied bit. Used by callers that want to
    /// short-circuit dispatch for reasons unrelated to result counts
    /// (e.g., outer cancellation propagation in nested parallelism).
    #[inline]
    pub fn cancel(&self) {
        self.satisfied.store(true, Ordering::Release);
    }

    /// The demand level this token enforces.
    #[inline]
    pub fn demand(&self) -> Demand {
        self.demand
    }

    /// Count of non-empty branches observed so far. Used for trace events.
    #[inline]
    pub fn non_empty_count(&self) -> u32 {
        self.non_empty_branches.load(Ordering::Acquire)
    }
}

impl std::fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CancelToken")
            .field("satisfied", &self.is_satisfied())
            .field("non_empty_count", &self.non_empty_count())
            .field("demand", &self.demand)
            .finish()
    }
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
// Store-addressed selective continuation spine
// ============================================================================

#[derive(Debug)]
struct StoredBranchCoroutineNode {
    remaining: Vec<(MettaValue, GenericBindings<MettaValue>)>,
    yielded: SmallVec<[MettaValue; 2]>,
    demand: Demand,
    cursor: usize,
}

impl StoredBranchCoroutineNode {
    fn new(branches: Vec<(MettaValue, GenericBindings<MettaValue>)>, demand: Demand) -> Self {
        Self {
            remaining: branches,
            yielded: SmallVec::new(),
            demand,
            cursor: 0,
        }
    }

    fn next_branch(&mut self) -> Option<(MettaValue, GenericBindings<MettaValue>)> {
        if self.demand.is_satisfied(self.yielded.len()) || self.cursor >= self.remaining.len() {
            return None;
        }
        let (rhs, bindings) = self.remaining[self.cursor].clone();
        self.cursor += 1;
        Some((rhs, bindings))
    }

    fn record_result(&mut self, result: MettaValue) {
        self.yielded.push(result);
    }

    fn is_satisfied(&self) -> bool {
        self.demand.is_satisfied(self.yielded.len())
    }

    fn is_exhausted(&self) -> bool {
        self.cursor >= self.remaining.len()
    }

    fn is_done(&self) -> bool {
        self.is_satisfied() || self.is_exhausted()
    }

    fn collect_values(&self, out: &mut Vec<MettaValue>) {
        for (rhs, bindings) in &self.remaining[self.cursor..] {
            out.push(*rhs);
            for (_name, val) in bindings.iter() {
                out.push(*val);
            }
        }
        out.extend(self.yielded.iter().copied());
    }

    fn take_results(self) -> SmallVec<[MettaValue; 2]> {
        self.yielded
    }

    fn result_count(&self) -> usize {
        self.yielded.len()
    }

    fn remaining_count(&self) -> usize {
        self.remaining.len() - self.cursor
    }
}

#[derive(Debug)]
enum ContinuationSpineNode {
    BranchCoroutine(StoredBranchCoroutineNode),
}

#[derive(Debug)]
struct BranchContinuationSpineStore {
    nodes: SpineStore<ContinuationSpineNode>,
}

impl BranchContinuationSpineStore {
    fn new() -> Self {
        Self {
            nodes: SpineStore::new(),
        }
    }

    fn alloc_branch_coroutine(
        &mut self,
        branches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        demand: Demand,
    ) -> ContinuationAddr {
        self.nodes.alloc(ContinuationSpineNode::BranchCoroutine(
            StoredBranchCoroutineNode::new(branches, demand),
        ))
    }

    fn branch_mut(&mut self, addr: ContinuationAddr) -> &mut StoredBranchCoroutineNode {
        match self.nodes.get_mut(addr) {
            Some(ContinuationSpineNode::BranchCoroutine(node)) => node,
            None => panic!("missing branch-coroutine continuation node at {:?}", addr),
        }
    }

    fn branch(&self, addr: ContinuationAddr) -> &StoredBranchCoroutineNode {
        match self.nodes.get(addr) {
            Some(ContinuationSpineNode::BranchCoroutine(node)) => node,
            None => panic!("missing branch-coroutine continuation node at {:?}", addr),
        }
    }

    fn remove_branch(&mut self, addr: ContinuationAddr) -> Option<StoredBranchCoroutineNode> {
        match self.nodes.remove(addr) {
            Some(ContinuationSpineNode::BranchCoroutine(node)) => Some(node),
            None => None,
        }
    }

    #[cfg(test)]
    fn contains(&self, addr: ContinuationAddr) -> bool {
        self.nodes.contains(addr)
    }
}

fn continuation_spine_store() -> &'static Mutex<BranchContinuationSpineStore> {
    static STORE: OnceLock<Mutex<BranchContinuationSpineStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(BranchContinuationSpineStore::new()))
}

fn lock_continuation_spine_store() -> MutexGuard<'static, BranchContinuationSpineStore> {
    match continuation_spine_store().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Store-backed branch coroutine used by the production trampoline
/// continuation. The handle is movable across Rust stack frames and names its
/// payload by [`ContinuationAddr`]; dropping the handle releases the spine node.
#[derive(Debug)]
pub struct StoredBranchCoroutine {
    addr: Option<ContinuationAddr>,
}

impl StoredBranchCoroutine {
    pub fn new(branches: Vec<(MettaValue, GenericBindings<MettaValue>)>, demand: Demand) -> Self {
        let addr = lock_continuation_spine_store().alloc_branch_coroutine(branches, demand);
        Self { addr: Some(addr) }
    }

    #[inline]
    pub fn addr(&self) -> ContinuationAddr {
        self.addr
            .expect("stored branch coroutine used after its spine node was taken")
    }

    pub fn next_branch(&mut self) -> Option<(MettaValue, GenericBindings<MettaValue>)> {
        lock_continuation_spine_store()
            .branch_mut(self.addr())
            .next_branch()
    }

    pub fn record_result(&mut self, result: MettaValue) {
        lock_continuation_spine_store()
            .branch_mut(self.addr())
            .record_result(result);
    }

    #[inline]
    pub fn is_done(&self) -> bool {
        lock_continuation_spine_store()
            .branch(self.addr())
            .is_done()
    }

    pub fn take_results(mut self) -> SmallVec<[MettaValue; 2]> {
        let addr = self
            .addr
            .take()
            .expect("stored branch coroutine results already taken");
        lock_continuation_spine_store()
            .remove_branch(addr)
            .expect("stored branch coroutine node missing at take_results")
            .take_results()
    }

    pub fn collect_values(&self, out: &mut Vec<MettaValue>) {
        lock_continuation_spine_store()
            .branch(self.addr())
            .collect_values(out);
    }

    #[inline]
    pub fn result_count(&self) -> usize {
        lock_continuation_spine_store()
            .branch(self.addr())
            .result_count()
    }

    #[inline]
    pub fn remaining_count(&self) -> usize {
        lock_continuation_spine_store()
            .branch(self.addr())
            .remaining_count()
    }
}

impl Drop for StoredBranchCoroutine {
    fn drop(&mut self) {
        if let Some(addr) = self.addr.take() {
            let _ = lock_continuation_spine_store().remove_branch(addr);
        }
    }
}

#[cfg(test)]
fn stored_node_is_live_for_tests(addr: ContinuationAddr) -> bool {
    lock_continuation_spine_store().contains(addr)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValue, MettaValueFactory};

    fn f() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

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

    #[test]
    fn test_stored_coroutine_roots_remaining_bindings_and_yielded_values() {
        let factory = f();
        let yielded = factory.atom("stored-yielded");
        let remaining_rhs = factory.atom("stored-remaining-rhs");
        let binding_value = factory.atom("stored-binding-value");
        let mut bindings = GenericBindings::new();
        bindings.insert("$x", binding_value);
        let branches = vec![
            (factory.atom("stored-first-rhs"), GenericBindings::new()),
            (remaining_rhs, bindings),
        ];

        let mut coro = StoredBranchCoroutine::new(branches, Demand::All);
        let addr = coro.addr();
        assert!(stored_node_is_live_for_tests(addr));

        let (_rhs, _bindings) = coro.next_branch().expect("first branch is present");
        coro.record_result(yielded);

        let mut roots = Vec::new();
        coro.collect_values(&mut roots);
        assert!(roots.iter().any(|v| v.inner_ptr() == yielded.inner_ptr()));
        assert!(roots
            .iter()
            .any(|v| v.inner_ptr() == remaining_rhs.inner_ptr()));
        assert!(roots
            .iter()
            .any(|v| v.inner_ptr() == binding_value.inner_ptr()));
    }

    #[test]
    fn test_stored_coroutine_drop_releases_spine_node() {
        let factory = f();
        let addr = {
            let coro = StoredBranchCoroutine::new(
                vec![(factory.atom("stored-drop-rhs"), GenericBindings::new())],
                Demand::All,
            );
            let addr = coro.addr();
            assert!(stored_node_is_live_for_tests(addr));
            addr
        };

        assert!(!stored_node_is_live_for_tests(addr));
    }
}
