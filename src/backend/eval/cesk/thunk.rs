//! STG-Style Thunks with Blackholing for Cycle Detection
//!
//! Thunks represent suspended computations that are evaluated at most once.
//! After evaluation, the thunk is updated to hold the result (memoized).
//! Re-entry during evaluation (blackhole) indicates an infinite loop.
//!
//! ## Thunk States
//!
//! ```text
//! ┌────────────┐    eval()    ┌────────────┐   complete    ┌────────────┐
//! │  Suspended  │ ──────────> │  Blackhole  │ ──────────> │  Evaluated  │
//! │  (unevalated│             │  (in-flight)│             │  (cached)   │
//! │   expr+env) │             │             │             │             │
//! └────────────┘             └────────────┘             └────────────┘
//!                              │    ↑
//!                              │    │ re-entry = CYCLE
//!                              └────┘ (panic / return error)
//! ```
//!
//! ## Use Cases
//!
//! - **EvalWithBindings memoization**: Template+bindings pairs evaluated once
//! - **Infinite recursion detection**: Blackhole on re-entry → error instead of hang
//! - **Lazy evaluation**: Thunks defer computation until result is needed
//!
//! ## Integration
//!
//! Thunks are stored in a thread-local table keyed by content hash of
//! the suspended expression. The trampoline checks for existing thunks
//! before evaluating EvalWithBindings work items.

use std::cell::RefCell;
use std::collections::HashMap;

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

// ============================================================================
// Thunk State
// ============================================================================

/// The state of a thunk (suspended computation).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThunkState {
    /// Computation has not yet been started.
    Suspended,

    /// Computation is in progress. Re-entry means infinite recursion.
    Blackhole,

    /// Computation is complete. Results are cached.
    Evaluated,

    /// Computation encountered an error.
    Error,
}

// ============================================================================
// Thunk
// ============================================================================

/// A suspended computation with memoization and cycle detection.
///
/// Once evaluated, the result is cached for O(1) subsequent access.
/// Re-entry during evaluation (blackhole state) signals infinite recursion.
#[derive(Debug, Clone)]
pub struct Thunk<V: MettaValueTrait> {
    /// Current state.
    pub state: ThunkState,

    /// Cached results (populated when state transitions to Evaluated).
    pub results: SmallVec<[V; 2]>,

    /// Number of times this thunk has been accessed.
    pub access_count: u32,

    /// Mutation epoch (thread-local) when this thunk was evaluated.
    /// Used to detect stale entries after this thread's space mutations.
    pub mutation_epoch: u64,

    /// #309/#266: process-global space-mutation epoch when this thunk was
    /// evaluated. A cross-thread mutation of the SHARED atom-space bumps it, so
    /// a stale thunk result is rejected on lookup by any worker (parity with
    /// `TableEntry::space_epoch`; the thread-local `mutation_epoch` alone cannot
    /// observe a sibling worker's `add-atom`).
    pub space_epoch: u64,

    /// Scope generation when this thunk was evaluated.
    /// Used for cache isolation between nondeterministic branches.
    pub scope_gen: u64,
}

impl<V: MettaValueTrait> Thunk<V> {
    /// Create a new suspended thunk, tagged with the current scope generation.
    fn new_suspended() -> Self {
        Self {
            state: ThunkState::Suspended,
            results: SmallVec::new(),
            access_count: 0,
            mutation_epoch: 0,
            space_epoch: 0,
            scope_gen: crate::backend::eval::trampoline::dispatch_hints::cache_generation(),
        }
    }
}

// ============================================================================
// Thunk Table
// ============================================================================

/// Result of looking up a thunk.
#[derive(Debug)]
pub enum ThunkLookup<V: MettaValueTrait> {
    /// No thunk exists for this expression. Caller should evaluate.
    Absent,

    /// Thunk exists but hasn't been evaluated yet. Now marked as Blackhole.
    /// Caller should evaluate and then call `update()`.
    Suspended,

    /// Thunk is being evaluated (re-entry). Infinite recursion detected.
    Blackhole,

    /// Thunk has been evaluated. Return cached results.
    Evaluated(SmallVec<[V; 2]>),

    /// Thunk evaluation previously errored.
    Error,
}

/// Thread-local thunk table for memoizing suspended computations.
///
/// Keyed by content hash of the expression. Provides evaluate-once semantics
/// with automatic cycle detection via blackholing.
#[derive(Debug)]
pub struct ThunkTable<V: MettaValueTrait> {
    /// Hash → thunk mapping.
    entries: HashMap<u64, Thunk<V>>,

    /// Total blackhole detections (cycle count).
    pub total_cycles: u64,

    /// Total cache hits (evaluated thunk reuse).
    pub total_hits: u64,
}

impl<V: MettaValueTrait + Clone + 'static> ThunkTable<V> {
    /// Create a new thunk table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(256),
            total_cycles: 0,
            total_hits: 0,
        }
    }

    fn shade_values<I>(values: I)
    where
        V: 'static,
        I: IntoIterator<Item = V>,
    {
        let mut roots = Vec::new();
        for value in values {
            let any = &value as &dyn std::any::Any;
            if let Some(root) = any.downcast_ref::<MettaValue>() {
                roots.push(root.clone());
            }
        }
        crate::backend::eval::cesk::index_heap::index_gc::satb_shade_evicted_roots(roots);
    }

    fn shade_thunk(thunk: Thunk<V>)
    where
        V: 'static,
    {
        Self::shade_values(thunk.results);
    }

    fn insert_thunk_with_satb(&mut self, expr_hash: u64, thunk: Thunk<V>)
    where
        V: 'static,
    {
        crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
            |satb_active| {
                let old = self.entries.insert(expr_hash, thunk);
                if satb_active {
                    if let Some(old) = old {
                        Self::shade_thunk(old);
                    }
                }
            },
        );
    }

    fn remove_thunk_with_satb(&mut self, expr_hash: u64)
    where
        V: 'static,
    {
        crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
            |satb_active| {
                let old = self.entries.remove(&expr_hash);
                if satb_active {
                    if let Some(old) = old {
                        Self::shade_thunk(old);
                    }
                }
            },
        );
    }

    fn clear_entries_with_satb(&mut self)
    where
        V: 'static,
    {
        crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
            |satb_active| {
                if satb_active {
                    let roots: Vec<V> = self
                        .entries
                        .values()
                        .flat_map(|thunk| thunk.results.iter().cloned())
                        .collect();
                    Self::shade_values(roots);
                }
                self.entries.clear();
            },
        );
    }

    /// Look up or create a thunk for the given expression hash.
    ///
    /// State transitions:
    /// - Absent → creates Suspended entry, returns `Absent` (caller evaluates)
    /// - Suspended → transitions to Blackhole, returns `Suspended` (caller evaluates)
    /// - Blackhole → returns `Blackhole` (cycle detected!)
    /// - Evaluated → returns cached results
    /// - Error → returns `Error`
    pub fn lookup(&mut self, expr_hash: u64) -> ThunkLookup<V> {
        if let Some(thunk) = self.entries.get_mut(&expr_hash) {
            // Check scope visibility first — entries from sibling branches
            // must not be visible regardless of their state. Without this,
            // leftover Suspended/Blackhole thunks from branch N would corrupt
            // branch N+1's evaluation by falsely detecting cycles.
            if !crate::backend::eval::trampoline::dispatch_hints::is_scope_visible(thunk.scope_gen)
            {
                self.remove_thunk_with_satb(expr_hash);
                self.insert_thunk_with_satb(expr_hash, Thunk::new_suspended());
                return ThunkLookup::Absent;
            }
            thunk.access_count += 1;
            match thunk.state {
                ThunkState::Suspended => {
                    thunk.state = ThunkState::Blackhole;
                    ThunkLookup::Suspended
                }
                ThunkState::Blackhole => {
                    self.total_cycles += 1;
                    ThunkLookup::Blackhole
                }
                ThunkState::Evaluated => {
                    // Check mutation epoch — stale entries from before a
                    // space mutation must not be returned.
                    let current_epoch =
                        crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
                    let current_space_epoch =
                        crate::backend::eval::trampoline::dispatch_hints::space_mutation_epoch();
                    if thunk.mutation_epoch != current_epoch
                        || thunk.space_epoch != current_space_epoch
                    {
                        // Stale — evict and treat as new. The space-epoch check
                        // (#309/#266) rejects a thunk whose result predates a
                        // cross-thread shared-space mutation.
                        self.remove_thunk_with_satb(expr_hash);
                        self.insert_thunk_with_satb(expr_hash, Thunk::new_suspended());
                        return ThunkLookup::Absent;
                    }
                    self.total_hits += 1;
                    ThunkLookup::Evaluated(thunk.results.clone())
                }
                ThunkState::Error => ThunkLookup::Error,
            }
        } else {
            // #309/#266 root cause #1 (THUNK channel): no LOCAL entry, but this
            // thunk_hash is seeded as actively-evaluating on the forking thread's
            // lineage (a parent Blackhole captured at fanout, domain-tagged). A
            // fanned-out worker re-entering the parent's in-flight thunk must CUT
            // exactly as the parent would inline — NOT insert a fresh Suspended
            // and re-derive (the runaway). Return Blackhole WITHOUT inserting, so
            // no stale Suspended is left to falsely cut a later genuine local
            // re-use. The probe short-circuits on an empty seed (FANOUT=0 /
            // non-worker path stays byte-identical).
            if crate::backend::eval::cesk::tabling::is_actively_evaluating(thunk_seed_key(
                expr_hash,
            )) {
                self.total_cycles += 1;
                return ThunkLookup::Blackhole;
            }
            self.insert_thunk_with_satb(expr_hash, Thunk::new_suspended());
            ThunkLookup::Absent
        }
    }

    /// Update a thunk with evaluation results.
    ///
    /// Transitions from Blackhole to Evaluated. Must be called after
    /// successful evaluation.
    pub fn update(&mut self, expr_hash: u64, results: SmallVec<[V; 2]>) {
        if let Some(thunk) = self.entries.get_mut(&expr_hash) {
            crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
                |satb_active| {
                    if satb_active {
                        Self::shade_values(thunk.results.iter().cloned());
                    }
                    thunk.state = ThunkState::Evaluated;
                    thunk.results = results;
                    thunk.mutation_epoch =
                        crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
                    thunk.space_epoch =
                        crate::backend::eval::trampoline::dispatch_hints::space_mutation_epoch();
                    thunk.scope_gen =
                        crate::backend::eval::trampoline::dispatch_hints::cache_generation();
                },
            );
        }
    }

    /// Mark a thunk as errored.
    pub fn mark_error(&mut self, expr_hash: u64) {
        if let Some(thunk) = self.entries.get_mut(&expr_hash) {
            thunk.state = ThunkState::Error;
        }
    }

    /// Remove a thunk (e.g., after scope exit or invalidation).
    pub fn remove(&mut self, expr_hash: u64) {
        self.remove_thunk_with_satb(expr_hash);
    }

    /// Check if a thunk is in blackhole state (being evaluated).
    #[inline]
    pub fn is_blackhole(&self, expr_hash: u64) -> bool {
        self.entries
            .get(&expr_hash)
            .map_or(false, |t| t.state == ThunkState::Blackhole)
    }

    /// #309/#266 root cause #2 (thunk channel): is ANY thunk in blackhole state
    /// (a `CompleteThunk` derivation in flight on this thread)? Used to skip the
    /// thunk-table clear on a GC-rendezvous resume mid-thunk-derivation —
    /// `ACTIVE_EVAL_SET` tracks only subgoals, so the subgoal guard does NOT cover
    /// the thunk channel; clearing a live blackhole drops the fence that the thunk
    /// was protecting, dropping an inference layer.
    #[inline]
    pub fn has_blackhole(&self) -> bool {
        self.entries
            .values()
            .any(|t| t.state == ThunkState::Blackhole)
    }

    /// #309/#266 root cause #1 (THUNK channel): push the DOMAIN-TAGGED hashes of
    /// every Blackhole thunk (a `CompleteThunk` derivation in flight) into `out`
    /// (dedup'd). These are unioned into `snapshot_active_hashes()` at a parallel
    /// dispatch so a fanned-out worker inherits them in `SEEDED_ACTIVE_SET` and
    /// cuts a cross-thread thunk re-entry — the thunk analog of the subgoal seed.
    /// The `thunk_seed_key` tag keeps these disjoint from subgoal hashes in the
    /// shared seed set (red-team H1).
    pub fn push_blackhole_seed_keys(&self, out: &mut SmallVec<[u64; 8]>) {
        for (&h, thunk) in self.entries.iter() {
            if thunk.state == ThunkState::Blackhole {
                let key = thunk_seed_key(h);
                if !out.contains(&key) {
                    out.push(key);
                }
            }
        }
    }

    /// Return the number of thunks in the table.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all thunks.
    pub fn clear(&mut self) {
        self.clear_entries_with_satb();
        self.total_cycles = 0;
        self.total_hits = 0;
    }

    /// Collect all cached values as GC roots.
    pub fn collect_roots(&self, out: &mut Vec<V>) {
        for thunk in self.entries.values() {
            out.extend(thunk.results.iter().cloned());
        }
    }

    /// Invalidate all thunks (after space mutation).
    pub fn invalidate_all(&mut self) {
        self.clear_entries_with_satb();
    }
}

impl<V: MettaValueTrait + Clone + 'static> Default for ThunkTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

thread_local! {
    static THREAD_THUNKS: RefCell<ThunkTable<MettaValue>> = RefCell::new(ThunkTable::new());
    /// Dirty flag: only clear the table when entries have been added since last clear.
    static THUNK_DIRTY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Access the thread-local thunk table.
#[inline]
pub fn with_thunk_table<R>(f: impl FnOnce(&mut ThunkTable<MettaValue>) -> R) -> R {
    THUNK_DIRTY.with(|d| d.set(true));
    THREAD_THUNKS.with(|cell| {
        let mut table = cell.borrow_mut();
        f(&mut table)
    })
}

/// Collect GC roots from the thread-local thunk table.
///
/// Cached evaluation results in the thunk table hold MettaValue references
/// that must survive GC mark-sweep cycles. Without this, GC can free values
/// that are only reachable through cached thunk results, causing
/// use-after-poison when those results are later retrieved and serialized.
pub fn collect_thunk_roots(out: &mut Vec<MettaValue>) {
    THREAD_THUNKS.with(|cell| {
        let table = cell.borrow();
        for thunk in table.entries.values() {
            out.extend(thunk.results.iter().cloned());
        }
    });
}

/// #309/#266 root cause #2 (thunk channel): read-only check (does NOT set
/// THUNK_DIRTY) for whether THIS thread has an in-flight thunk derivation (a
/// blackhole entry). The GC-resume clear must skip `clear_thunk_table` while this
/// holds, independently of the subgoal active set — a blackholed thunk is not in
/// ACTIVE_EVAL_SET, so the subgoal guard (`active_eval_set_is_empty`) does not
/// cover it.
#[inline]
pub fn thunk_table_has_blackhole() -> bool {
    THREAD_THUNKS.with(|cell| cell.borrow().has_blackhole())
}

/// #309/#266 thunk-channel seed domain tag — XORed into a thunk hash to form its
/// seed key, keeping thunk seed keys disjoint from subgoal hashes in the shared
/// `SEEDED_ACTIVE_SET` (red-team H1). Applied symmetrically at the producer
/// (`push_blackhole_seed_keys`) and the consumer (`lookup`'s Absent-path probe).
/// Value = ASCII "THNKSEED".
pub(crate) const THUNK_SEED_DOMAIN: u64 = 0x5448_4e4b_5345_4544;

/// Domain-tag a thunk hash into its cross-thread seed key (see [`THUNK_SEED_DOMAIN`]).
#[inline]
pub(crate) fn thunk_seed_key(thunk_hash: u64) -> u64 {
    thunk_hash ^ THUNK_SEED_DOMAIN
}

/// #309/#266 root cause #1 (THUNK channel): read-only (does NOT set THUNK_DIRTY)
/// collect of this thread's in-flight thunk Blackhole seed keys, unioned into
/// `snapshot_active_hashes()` so a fanned-out worker inherits them and cuts a
/// cross-thread thunk re-entry. Mirrors the subgoal seed for the thunk channel.
#[inline]
pub fn collect_blackhole_hashes(out: &mut SmallVec<[u64; 8]>) {
    THREAD_THUNKS.with(|cell| cell.borrow().push_blackhole_seed_keys(out));
}

/// Clear the thread-local thunk table.
/// Skips the clear if no entries have been added since the last clear.
#[inline]
pub fn clear_thunk_table() {
    THUNK_DIRTY.with(|d| {
        if d.get() {
            THREAD_THUNKS.with(|cell| {
                cell.borrow_mut().clear();
            });
            d.set(false);
        }
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValueFactory};

    fn f() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

    fn make_long(n: i64) -> MettaValue {
        f().long(n)
    }

    #[test]
    fn test_empty_table() {
        let table = ThunkTable::<MettaValue>::new();
        assert!(table.is_empty());
    }

    #[test]
    fn test_absent_creates_suspended() {
        let mut table = ThunkTable::<MettaValue>::new();

        match table.lookup(42) {
            ThunkLookup::Absent => {} // Expected
            other => panic!("Expected Absent, got {:?}", other),
        }
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_suspended_to_blackhole() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(42); // Creates Suspended

        // Second lookup: Suspended → Blackhole
        match table.lookup(42) {
            ThunkLookup::Suspended => {} // Expected — transitions to Blackhole
            other => panic!("Expected Suspended, got {:?}", other),
        }
        assert!(table.is_blackhole(42));
    }

    #[test]
    fn test_blackhole_detection() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(42); // Absent → Suspended
        table.lookup(42); // Suspended → Blackhole

        // Third lookup: Blackhole → cycle detected
        match table.lookup(42) {
            ThunkLookup::Blackhole => {} // Expected — cycle!
            other => panic!("Expected Blackhole, got {:?}", other),
        }
        assert_eq!(table.total_cycles, 1);
    }

    #[test]
    fn test_evaluate_and_cache() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(42); // Absent
        table.lookup(42); // Suspended → Blackhole

        // Complete evaluation
        table.update(42, smallvec::smallvec![make_long(99)]);

        // Subsequent lookup: Evaluated
        match table.lookup(42) {
            ThunkLookup::Evaluated(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_long(), Some(99));
            }
            other => panic!("Expected Evaluated, got {:?}", other),
        }
        assert_eq!(table.total_hits, 1);
    }

    #[test]
    fn test_mark_error() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(42); // Absent
        table.lookup(42); // Suspended → Blackhole
        table.mark_error(42);

        match table.lookup(42) {
            ThunkLookup::Error => {}
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_remove() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(42);
        table.lookup(42);
        table.update(42, smallvec::smallvec![make_long(1)]);

        table.remove(42);
        assert!(table.is_empty());

        // Fresh lookup after removal
        match table.lookup(42) {
            ThunkLookup::Absent => {}
            other => panic!("Expected Absent after remove, got {:?}", other),
        }
    }

    #[test]
    fn test_collect_roots() {
        let mut table = ThunkTable::<MettaValue>::new();

        table.lookup(1);
        table.lookup(1);
        table.update(1, smallvec::smallvec![make_long(10), make_long(20)]);

        table.lookup(2);
        table.lookup(2);
        table.update(2, smallvec::smallvec![make_long(30)]);

        let mut roots = Vec::new();
        table.collect_roots(&mut roots);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_clear() {
        let mut table = ThunkTable::<MettaValue>::new();
        table.lookup(1);
        table.lookup(1);
        table.update(1, smallvec::smallvec![make_long(1)]);

        table.clear();
        assert!(table.is_empty());
        assert_eq!(table.total_hits, 0);
    }

    #[test]
    fn test_thread_local() {
        with_thunk_table(|table| {
            table.clear();
            table.lookup(42);
            table.lookup(42);
            table.update(42, smallvec::smallvec![make_long(99)]);
        });

        with_thunk_table(|table| {
            match table.lookup(42) {
                ThunkLookup::Evaluated(r) => assert_eq!(r[0].as_long(), Some(99)),
                other => panic!("Expected Evaluated, got {:?}", other),
            }
            table.clear();
        });
    }
}
