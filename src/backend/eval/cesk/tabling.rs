//! Subgoal Tabling for PLN Derivation Loop (Prolog-style)
//!
//! Subgoal tabling memoizes the results of intermediate subgoals during
//! nondeterministic evaluation, avoiding redundant re-derivation. This is
//! critical for PLN (Probabilistic Logic Networks) where the same intermediate
//! derivation may be attempted thousands of times during backward chaining.
//!
//! ## Design
//!
//! The tabling system implements **linear tabling** (XSB Prolog-inspired):
//!
//! 1. **First call** to a tabled subgoal → evaluate normally, store results
//! 2. **Subsequent calls** to the same subgoal → return cached results
//! 3. **Recursive calls** (cycle detection) → return current partial results
//!    (avoids infinite loops in recursive rules)
//!
//! ## Subgoal Identity
//!
//! Two subgoals are considered identical if their content hash matches.
//! Content hashing uses the MettaValue `hash_value()` method, which produces
//! a stable u64 hash based on the expression's structure.
//!
//! ## Table Entry Lifecycle
//!
//! ```text
//! ┌──────────┐     eval()     ┌───────────┐    complete     ┌───────────┐
//! │  Absent   │ ──────────> │  Active    │ ──────────> │  Complete  │
//! │           │              │  (cycling  │              │  (cached)  │
//! │           │              │   partial) │              │            │
//! └──────────┘              └───────────┘              └───────────┘
//!                              │    ↑
//!                              │    │ re-entry
//!                              └────┘ (return partial results)
//! ```
//!
//! ## Integration
//!
//! The tabling system is consulted at two points in the trampoline:
//!
//! 1. **Before evaluation**: `table.lookup(expr_hash)` — returns cached results
//!    if the subgoal is already complete, or partial results if active (cycle)
//! 2. **After evaluation**: `table.complete(expr_hash, results)` — stores results
//!    and transitions the entry from Active to Complete
//!
//! ## Thread Safety
//!
//! The table is thread-local (one per evaluation thread). Parallel branches
//! have independent tables. Cross-thread sharing is not needed because
//! parallel branches evaluate independent subgoals.
//!
//! ## Capacity
//!
//! Default capacity: 1024 entries. Auto-grows via HashMap. Entries are evicted
//! based on LRU order when capacity is exceeded (future enhancement).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

// ============================================================================
// Eval Generation Counter
// ============================================================================

thread_local! {
    /// Monotonic counter incremented on each trampoline entry.
    /// Used to distinguish true cycles (same generation, deeper depth)
    /// from sibling branch re-entries (different generation or same depth).
    static EVAL_GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// Get the current eval generation (thread-local).
pub fn current_eval_generation() -> u64 {
    EVAL_GENERATION.with(|g| g.get())
}

/// Increment and return the new eval generation (called on trampoline entry).
pub fn increment_eval_generation() -> u64 {
    EVAL_GENERATION.with(|g| {
        let new = g.get().wrapping_add(1);
        g.set(new);
        new
    })
}

// ============================================================================
// Table Entry
// ============================================================================

/// State of a tabled subgoal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TableEntryState {
    /// Subgoal is currently being evaluated (first call or recursive re-entry).
    /// Partial results may be available if the subgoal has yielded some results
    /// before a cycle was detected.
    Active,

    /// Subgoal evaluation is complete. Results are final and cached.
    Complete,
}

/// A single entry in the subgoal table.
#[derive(Debug, Clone)]
pub struct TableEntry<V: MettaValueTrait> {
    /// Current state of this subgoal.
    pub state: TableEntryState,

    /// Cached results (partial while Active, final when Complete).
    pub results: SmallVec<[V; 2]>,

    /// Number of times this entry was looked up (for diagnostics).
    pub hit_count: u32,

    /// Evaluation depth at which this subgoal was first tabled.
    /// Used for depth-aware cycle detection: true cycles re-enter
    /// at GREATER depth within the same generation.
    pub origin_depth: u32,

    /// Eval generation when this entry was created.
    /// Together with origin_depth, distinguishes true cycles
    /// (same generation + deeper) from sibling branch re-entries
    /// (different generation or equal/lesser depth).
    pub origin_generation: u64,

    /// Mutation epoch when this entry was created/completed.
    pub mutation_epoch: u64,

    /// Position in the continuation stack where `CompleteSubgoal` was pushed.
    /// Used for stack-validated cycle detection: before returning Cycle,
    /// the caller checks if `CompleteSubgoal` is still at this position.
    /// If not (fired or displaced), the Active entry is stale.
    pub cont_stack_pos: u32,
}

impl<V: MettaValueTrait> TableEntry<V> {
    fn new_active(depth: u32, epoch: u64, generation: u64, cont_stack_pos: u32) -> Self {
        Self {
            state: TableEntryState::Active,
            results: SmallVec::new(),
            hit_count: 0,
            origin_depth: depth,
            origin_generation: generation,
            mutation_epoch: epoch,
            cont_stack_pos,
        }
    }
}

// ============================================================================
// Subgoal Table
// ============================================================================

/// Lookup result from the subgoal table.
#[derive(Debug)]
pub enum TableLookup<V: MettaValueTrait> {
    /// Subgoal not in table — first call. Caller should evaluate and then
    /// call `complete()` with results.
    Absent,

    /// Subgoal is complete — return cached results directly.
    Complete(SmallVec<[V; 2]>),

    /// Subgoal is active (re-entered during evaluation — cycle detected).
    /// Returns any partial results accumulated so far.
    /// The caller should use these as the result and NOT re-evaluate.
    Cycle(SmallVec<[V; 2]>),

    /// Subgoal is Active but from a different eval context (sibling branch
    /// or different trampoline generation). NOT a true cycle — caller should
    /// evaluate normally WITHOUT tabling this occurrence.
    Bypass,

    /// Subgoal is Active from the same generation at greater depth.
    /// MIGHT be a true cycle, but needs stack validation: caller must check
    /// that `CompleteSubgoal` for this hash is still on the continuation
    /// stack at `cont_stack_pos`. If yes → true cycle (use partial results).
    /// If no → stale Active entry (abandon and re-evaluate).
    PossibleCycle {
        partial_results: SmallVec<[V; 2]>,
        cont_stack_pos: u32,
    },
}

/// Thread-local subgoal table for memoizing intermediate derivation results.
///
/// Keyed by content hash of the expression. Values are `TableEntry` with
/// state tracking (Active/Complete) and cached results.
#[derive(Debug)]
pub struct SubgoalTable<V: MettaValueTrait> {
    /// Hash → entry mapping.
    entries: HashMap<u64, TableEntry<V>>,

    /// Total cache hits (for diagnostics).
    total_hits: u64,

    /// Total cache misses (for diagnostics).
    total_misses: u64,

    /// Total cycles detected (for diagnostics).
    total_cycles: u64,
}

impl<V: MettaValueTrait + Clone> SubgoalTable<V> {
    /// Create a new subgoal table with default capacity (1024).
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    /// Create a new subgoal table with the given capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            total_hits: 0,
            total_misses: 0,
            total_cycles: 0,
        }
    }

    /// Look up a subgoal by its content hash.
    ///
    /// Returns:
    /// - `Absent` if not tabled — caller should evaluate and call `complete()`
    /// - `Complete(results)` if already evaluated — use cached results
    /// - `Cycle(partial)` if currently being evaluated — cycle detected
    ///
    /// On `Absent`, automatically creates an `Active` entry to detect future cycles.
    pub fn lookup(&mut self, expr_hash: u64, depth: u32, cont_stack_pos: u32) -> TableLookup<V> {
        let current_epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        let generation = current_eval_generation();

        if let Some(entry) = self.entries.get_mut(&expr_hash) {
            // Stale epoch: a mutation occurred since this entry was tabled.
            if entry.mutation_epoch != current_epoch {
                match entry.state {
                    TableEntryState::Complete => {
                        self.entries.remove(&expr_hash);
                        self.total_misses += 1;
                        self.entries.insert(expr_hash, TableEntry::new_active(depth, current_epoch, generation, cont_stack_pos));
                        return TableLookup::Absent;
                    }
                    TableEntryState::Active => {
                        if entry.origin_generation == generation && depth > entry.origin_depth {
                            // Might be a true cycle — let caller validate via stack position.
                            return TableLookup::PossibleCycle {
                                partial_results: entry.results.clone(),
                                cont_stack_pos: entry.cont_stack_pos,
                            };
                        }
                        self.entries.remove(&expr_hash);
                        self.total_misses += 1;
                        self.entries.insert(expr_hash, TableEntry::new_active(depth, current_epoch, generation, cont_stack_pos));
                        return TableLookup::Absent;
                    }
                }
            }

            entry.hit_count += 1;
            match entry.state {
                TableEntryState::Complete => {
                    self.total_hits += 1;
                    TableLookup::Complete(entry.results.clone())
                }
                TableEntryState::Active => {
                    if entry.origin_generation == generation && depth > entry.origin_depth {
                        // Might be a true cycle — let caller validate via stack position.
                        TableLookup::PossibleCycle {
                            partial_results: entry.results.clone(),
                            cont_stack_pos: entry.cont_stack_pos,
                        }
                    } else {
                        TableLookup::Bypass
                    }
                }
            }
        } else {
            self.total_misses += 1;
            self.entries.insert(expr_hash, TableEntry::new_active(depth, current_epoch, generation, cont_stack_pos));
            TableLookup::Absent
        }
    }

    /// Complete a subgoal, transitioning it from Active to Complete.
    ///
    /// Stores the final results. Future lookups will return `Complete`.
    ///
    /// # Panics
    ///
    /// Debug-asserts that the entry exists and is Active.
    pub fn complete(&mut self, expr_hash: u64, results: SmallVec<[V; 2]>) {
        if let Some(entry) = self.entries.get_mut(&expr_hash) {
            debug_assert_eq!(
                entry.state,
                TableEntryState::Active,
                "complete() called on non-Active entry"
            );
            entry.state = TableEntryState::Complete;
            entry.results = results;
        }
    }

    /// Add partial results to an Active entry.
    ///
    /// Called when a subgoal yields intermediate results before completing.
    /// These partial results are returned on cycle re-entry.
    pub fn add_partial_results(&mut self, expr_hash: u64, partial: &[V]) {
        if let Some(entry) = self.entries.get_mut(&expr_hash) {
            if entry.state == TableEntryState::Active {
                entry.results.extend(partial.iter().cloned());
            }
        }
    }

    /// Abandon an Active entry (evaluation failed or was cancelled).
    ///
    /// Removes the entry so future lookups return `Absent`.
    pub fn abandon(&mut self, expr_hash: u64) {
        if let Some(entry) = self.entries.get(&expr_hash) {
            if entry.state == TableEntryState::Active {
                self.entries.remove(&expr_hash);
            }
        }
    }

    /// Check if a subgoal is currently being evaluated (Active).
    #[inline]
    pub fn is_active(&self, expr_hash: u64) -> bool {
        self.entries
            .get(&expr_hash)
            .map_or(false, |e| e.state == TableEntryState::Active)
    }

    /// Check if a subgoal has completed results.
    #[inline]
    pub fn is_complete(&self, expr_hash: u64) -> bool {
        self.entries
            .get(&expr_hash)
            .map_or(false, |e| e.state == TableEntryState::Complete)
    }

    /// Return the number of entries in the table.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if the table is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear all entries, retaining allocated capacity.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_hits = 0;
        self.total_misses = 0;
        self.total_cycles = 0;
    }

    /// Return diagnostic statistics.
    pub fn stats(&self) -> TableStats {
        let complete_count = self.entries.values()
            .filter(|e| e.state == TableEntryState::Complete)
            .count();
        let active_count = self.entries.values()
            .filter(|e| e.state == TableEntryState::Active)
            .count();

        TableStats {
            total_entries: self.entries.len(),
            complete_entries: complete_count,
            active_entries: active_count,
            total_hits: self.total_hits,
            total_misses: self.total_misses,
            total_cycles: self.total_cycles,
        }
    }

    /// Collect all cached values as GC roots.
    pub fn collect_roots(&self, out: &mut Vec<V>) {
        for entry in self.entries.values() {
            out.extend(entry.results.iter().cloned());
        }
    }

    /// Invalidate entries that may be affected by space mutations.
    ///
    /// Called when `add-atom` or `remove-atom` modifies the atomspace,
    /// as cached derivation results may no longer be valid.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }
}

impl<V: MettaValueTrait + Clone> Default for SubgoalTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic statistics for the subgoal table.
#[derive(Debug, Clone)]
pub struct TableStats {
    pub total_entries: usize,
    pub complete_entries: usize,
    pub active_entries: usize,
    pub total_hits: u64,
    pub total_misses: u64,
    pub total_cycles: u64,
}

impl std::fmt::Display for TableStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SubgoalTable: {} entries ({} complete, {} active), {} hits, {} misses, {} cycles",
            self.total_entries,
            self.complete_entries,
            self.active_entries,
            self.total_hits,
            self.total_misses,
            self.total_cycles,
        )
    }
}

// ============================================================================
// Thread-Local Table Access
// ============================================================================

thread_local! {
    /// Thread-local subgoal table for the tree-walker evaluation.
    static THREAD_TABLE: RefCell<SubgoalTable<MettaValue>> = RefCell::new(SubgoalTable::new());
    /// Dirty flag: only clear the table when entries have been added since last clear.
    static SUBGOAL_DIRTY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Access the thread-local subgoal table.
#[inline]
pub fn with_subgoal_table<R>(f: impl FnOnce(&mut SubgoalTable<MettaValue>) -> R) -> R {
    SUBGOAL_DIRTY.with(|d| d.set(true));
    THREAD_TABLE.with(|cell| {
        let mut table = cell.borrow_mut();
        f(&mut table)
    })
}

/// Clear the thread-local subgoal table.
///
/// Called between top-level evaluations or after space mutations.
/// Skips the clear if no entries have been added since the last clear.
#[inline]
pub fn clear_subgoal_table() {
    SUBGOAL_DIRTY.with(|d| {
        if d.get() {
            THREAD_TABLE.with(|cell| {
                cell.borrow_mut().clear();
            });
            d.set(false);
        }
    });
}

/// Collect GC roots from the thread-local subgoal table.
///
/// Cached evaluation results in the subgoal table hold MettaValue references
/// that must survive GC mark-sweep cycles. Without this, GC can free values
/// that are only reachable through cached tabling results, causing
/// use-after-poison when those results are later retrieved and serialized.
pub fn collect_subgoal_roots(out: &mut Vec<MettaValue>) {
    THREAD_TABLE.with(|cell| {
        let table = cell.borrow();
        for entry in table.entries.values() {
            out.extend(entry.results.iter().copied());
        }
    });
}

/// Invalidate the thread-local subgoal table after space mutation.
#[inline]
pub fn invalidate_subgoal_table() {
    THREAD_TABLE.with(|cell| {
        cell.borrow_mut().invalidate_all();
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValueFactory, global_factory};

    fn f() -> crate::backend::models::GcFactory {
        global_factory()
    }

    fn make_long(n: i64) -> MettaValue {
        f().long(n)
    }

    fn hash_expr(expr: &MettaValue) -> u64 {
        expr.hash_value()
    }

    #[test]
    fn test_empty_table() {
        let table = SubgoalTable::<MettaValue>::new();
        assert!(table.is_empty());
        assert_eq!(table.len(), 0);
    }

    #[test]
    fn test_lookup_absent() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let expr = f().sexpr(vec![f().atom("f"), make_long(1)]);
        let hash = hash_expr(&expr);

        match table.lookup(hash, 0, 0) {
            TableLookup::Absent => {} // Expected
            other => panic!("Expected Absent, got {:?}", other),
        }

        // Entry should now be Active
        assert!(table.is_active(hash));
        assert_eq!(table.len(), 1);
    }

    #[test]
    fn test_lookup_cycle() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let expr = f().sexpr(vec![f().atom("f"), make_long(1)]);
        let hash = hash_expr(&expr);

        // First lookup: Absent (creates Active entry)
        table.lookup(hash, 0, 0);

        // Second lookup at deeper depth: PossibleCycle (needs stack validation)
        match table.lookup(hash, 1, 0) {
            TableLookup::PossibleCycle { partial_results, .. } => {
                assert!(partial_results.is_empty()); // No partial results yet
            }
            other => panic!("Expected PossibleCycle, got {:?}", other),
        }
    }

    #[test]
    fn test_complete_and_hit() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let expr = f().sexpr(vec![f().atom("f"), make_long(1)]);
        let hash = hash_expr(&expr);

        // First lookup: Absent
        table.lookup(hash, 0, 0);

        // Complete with results
        let results = smallvec::smallvec![make_long(42)];
        table.complete(hash, results);
        assert!(table.is_complete(hash));

        // Subsequent lookup: Complete (cache hit)
        match table.lookup(hash, 0, 0) {
            TableLookup::Complete(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_long(), Some(42));
            }
            other => panic!("Expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_partial_results_in_cycle() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let hash = 12345u64;

        // First lookup: Absent
        table.lookup(hash, 0, 0);

        // Add partial results while Active
        table.add_partial_results(hash, &[make_long(1), make_long(2)]);

        // Re-entry at deeper depth should return PossibleCycle with partial results
        match table.lookup(hash, 1, 0) {
            TableLookup::PossibleCycle { partial_results, .. } => {
                assert_eq!(partial_results.len(), 2);
                assert_eq!(partial_results[0].as_long(), Some(1));
                assert_eq!(partial_results[1].as_long(), Some(2));
            }
            other => panic!("Expected PossibleCycle with partial results, got {:?}", other),
        }
    }

    #[test]
    fn test_abandon() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let hash = 99999u64;

        table.lookup(hash, 0, 0);
        assert!(table.is_active(hash));

        table.abandon(hash);
        assert!(!table.is_active(hash));
        assert_eq!(table.len(), 0);

        // Next lookup should be Absent again
        match table.lookup(hash, 0, 0) {
            TableLookup::Absent => {}
            other => panic!("Expected Absent after abandon, got {:?}", other),
        }
    }

    #[test]
    fn test_multiple_subgoals() {
        let mut table = SubgoalTable::<MettaValue>::new();

        let hash1 = 111u64;
        let hash2 = 222u64;
        let hash3 = 333u64;

        table.lookup(hash1, 0, 0);
        table.complete(hash1, smallvec::smallvec![make_long(1)]);

        table.lookup(hash2, 0, 0);
        table.complete(hash2, smallvec::smallvec![make_long(2), make_long(3)]);

        table.lookup(hash3, 0, 0); // Still active

        assert!(table.is_complete(hash1));
        assert!(table.is_complete(hash2));
        assert!(table.is_active(hash3));
        assert_eq!(table.len(), 3);
    }

    #[test]
    fn test_stats() {
        let mut table = SubgoalTable::<MettaValue>::new();

        table.lookup(1, 0, 0); // miss
        table.complete(1, smallvec::smallvec![make_long(1)]);
        table.lookup(1, 0, 0); // hit
        table.lookup(1, 0, 0); // hit

        table.lookup(2, 0, 0); // miss
        table.lookup(2, 1, 0); // possible cycle (stack validation needed)

        let stats = table.stats();
        assert_eq!(stats.total_entries, 2);
        assert_eq!(stats.complete_entries, 1);
        assert_eq!(stats.active_entries, 1);
        assert_eq!(stats.total_hits, 2);
        assert_eq!(stats.total_misses, 2);
        // PossibleCycle doesn't increment total_cycles (caller decides after validation)
        assert_eq!(stats.total_cycles, 0);
    }

    #[test]
    fn test_clear() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.lookup(1, 0, 0);
        table.complete(1, smallvec::smallvec![make_long(1)]);

        table.clear();
        assert!(table.is_empty());
        let stats = table.stats();
        assert_eq!(stats.total_hits, 0);
    }

    #[test]
    fn test_invalidate_all() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.lookup(1, 0, 0);
        table.complete(1, smallvec::smallvec![make_long(1)]);
        table.lookup(2, 0, 0);

        table.invalidate_all();
        assert!(table.is_empty());
    }

    #[test]
    fn test_collect_roots() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.lookup(1, 0, 0);
        table.add_partial_results(1, &[make_long(10)]);
        table.lookup(2, 0, 0);
        table.complete(2, smallvec::smallvec![make_long(20), make_long(30)]);

        let mut roots = Vec::new();
        table.collect_roots(&mut roots);
        assert_eq!(roots.len(), 3); // 1 partial + 2 complete
    }

    #[test]
    fn test_thread_local_access() {
        with_subgoal_table(|table| {
            table.clear();
            table.lookup(42, 0, 0);
            table.complete(42, smallvec::smallvec![make_long(99)]);
        });

        with_subgoal_table(|table| {
            match table.lookup(42, 0, 0) {
                TableLookup::Complete(results) => {
                    assert_eq!(results[0].as_long(), Some(99));
                }
                other => panic!("Expected Complete, got {:?}", other),
            }
            table.clear();
        });
    }

    #[test]
    fn test_stats_display() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.lookup(1, 0, 0);
        table.complete(1, smallvec::smallvec![make_long(1)]);

        let stats = table.stats();
        let display = format!("{}", stats);
        assert!(display.contains("1 complete"));
        assert!(display.contains("1 misses"));
    }
}
