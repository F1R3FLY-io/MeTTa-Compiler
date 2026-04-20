//! Subgoal Tabling for PLN Derivation Loop (Prolog-style)
//!
//! Subgoal tabling memoizes the results of intermediate subgoals during
//! nondeterministic evaluation, avoiding redundant re-derivation.
//!
//! ## Design
//!
//! The system separates two concerns:
//!
//! 1. **Cycle detection**: A thread-local `HashMap<u64, u32>` (active evaluation
//!    set) tracks which expression hashes are currently on the evaluation call
//!    stack. An expression is in a true cycle if and only if its hash is already
//!    in the active set when encountered. This is O(1) and precise.
//!
//! 2. **Memoization**: The `SubgoalTable` caches Complete results. Once an
//!    expression is fully evaluated, its results are stored for future lookups.
//!
//! ## Call-Stack Tracking
//!
//! ```text
//! ┌──────────┐    mark_eval_active()    ┌──────────┐    unmark_eval_active()    ┌──────────┐
//! │  Absent   │ ──────────────────────> │  Active   │ ──────────────────────> │  Complete  │
//! │           │                          │ (in set)  │                          │ (in table) │
//! └──────────┘                          └──────────┘                          └──────────┘
//!                                         │    ↑
//!                                         │    │ re-entry (true cycle)
//!                                         └────┘ → return empty (fixpoint)
//! ```

use std::cell::RefCell;
use std::collections::HashMap;

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

// ============================================================================
// Active Evaluation Set (Call-Stack Tracking)
// ============================================================================

thread_local! {
    /// Reference-counted set of expression hashes currently being evaluated.
    /// An entry with count > 0 means the expression is on the call stack
    /// (a `CompleteSubgoal` continuation exists for it).
    ///
    /// - `mark_eval_active()`: increment count (CompleteSubgoal pushed)
    /// - `unmark_eval_active()`: decrement count (CompleteSubgoal fired)
    /// - `is_actively_evaluating()`: check count > 0 (cycle detection)
    static ACTIVE_EVAL_SET: RefCell<HashMap<u64, u32>> = RefCell::new(HashMap::with_capacity(64));
}

/// Check if an expression hash is currently being evaluated (on the call stack).
#[inline]
pub fn is_actively_evaluating(expr_hash: u64) -> bool {
    ACTIVE_EVAL_SET.with(|set| {
        set.borrow().get(&expr_hash).copied().unwrap_or(0) > 0
    })
}

/// Mark an expression as actively being evaluated.
/// Called when `CompleteSubgoal` is pushed onto the continuation stack.
#[inline]
pub fn mark_eval_active(expr_hash: u64) {
    ACTIVE_EVAL_SET.with(|set| {
        *set.borrow_mut().entry(expr_hash).or_insert(0) += 1;
    });
}

/// Unmark an expression as actively being evaluated.
/// Called when `CompleteSubgoal` fires (is consumed from the continuation stack).
#[inline]
pub fn unmark_eval_active(expr_hash: u64) {
    ACTIVE_EVAL_SET.with(|set| {
        let mut map = set.borrow_mut();
        if let Some(count) = map.get_mut(&expr_hash) {
            *count -= 1;
            if *count == 0 {
                map.remove(&expr_hash);
            }
        }
    });
}

/// Clear the active evaluation set.
#[inline]
pub fn clear_active_eval_set() {
    ACTIVE_EVAL_SET.with(|set| set.borrow_mut().clear());
}

// ============================================================================
// Table Entry (Complete results only)
// ============================================================================

/// A cached evaluation result in the subgoal table.
///
/// Cross-branch isolation is enforced by the `scope_gen` field (per-branch
/// cache generation watermark) plus top-level `query_generation` bumps at
/// each `!` boundary. The cache stores only values; bindings are
/// re-attached at retrieval time from the retrieving branch's
/// `carrying_bindings`. Sibling-branch contamination is prevented by
/// `is_scope_visible` on lookup.
#[derive(Debug, Clone)]
pub struct TableEntry<V: MettaValueTrait + Clone> {
    /// Cached result values (final).
    pub results: SmallVec<[V; 2]>,

    /// Number of times this entry was looked up (diagnostics).
    pub hit_count: u32,

    /// Mutation epoch when this entry was created.
    pub mutation_epoch: u64,

    /// Scope generation when this entry was created.
    /// Used for cache isolation between nondeterministic branches.
    pub scope_gen: u64,
}

// ============================================================================
// Subgoal Table (Complete cache)
// ============================================================================

/// Lookup result from the subgoal table.
#[derive(Debug)]
pub enum TableLookup<V: MettaValueTrait + Clone> {
    /// Expression not cached — caller should evaluate.
    Absent,

    /// Cached Complete results — return directly.
    Complete(SmallVec<[V; 2]>),
}

/// Thread-local subgoal table for memoizing completed evaluation results.
#[derive(Debug)]
pub struct SubgoalTable<V: MettaValueTrait> {
    entries: HashMap<u64, TableEntry<V>>,
    total_hits: u64,
    total_misses: u64,
}

impl<V: MettaValueTrait + Clone> SubgoalTable<V> {
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::with_capacity(capacity),
            total_hits: 0,
            total_misses: 0,
        }
    }

    /// Look up a cached result by expression hash.
    ///
    /// Returns `Complete(results)` on cache hit, `Absent` on miss.
    /// Stale entries (mutation epoch mismatch) are evicted.
    pub fn lookup(&mut self, expr_hash: u64) -> TableLookup<V> {
        let current_epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();

        if let Some(entry) = self.entries.get_mut(&expr_hash) {
            if entry.mutation_epoch != current_epoch {
                // Stale — evict (epoch mismatch)
                self.entries.remove(&expr_hash);
                self.total_misses += 1;
                return TableLookup::Absent;
            }
            if !crate::backend::eval::trampoline::dispatch_hints::is_scope_visible(entry.scope_gen) {
                // Entry from a sibling branch — not visible in current scope
                self.total_misses += 1;
                return TableLookup::Absent;
            }
            entry.hit_count += 1;
            self.total_hits += 1;
            TableLookup::Complete(entry.results.clone())
        } else {
            self.total_misses += 1;
            TableLookup::Absent
        }
    }

    /// Store completed values for an expression hash.
    ///
    /// Cross-branch isolation is enforced by scope_gen (per-branch
    /// watermark) and query_generation (per-`!` watermark) on lookup.
    pub fn complete(
        &mut self,
        expr_hash: u64,
        results: SmallVec<[V; 2]>,
    ) {
        let epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        let gen = crate::backend::eval::trampoline::dispatch_hints::cache_generation();
        self.entries.insert(expr_hash, TableEntry {
            results,
            hit_count: 0,
            mutation_epoch: epoch,
            scope_gen: gen,
        });
    }

    /// Remove a cached entry (e.g., for selective invalidation).
    pub fn remove_entry(&mut self, expr_hash: u64) {
        self.entries.remove(&expr_hash);
    }

    /// Check if a hash has a cached Complete result.
    pub fn is_complete(&self, expr_hash: u64) -> bool {
        self.entries.contains_key(&expr_hash)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.total_hits = 0;
        self.total_misses = 0;
    }

    pub fn invalidate_all(&mut self) {
        self.entries.clear();
    }

    /// Collect GC roots from cached results.
    pub fn collect_roots(&self, out: &mut Vec<V>) {
        for entry in self.entries.values() {
            out.extend(entry.results.iter().cloned());
        }
    }

    pub fn stats(&self) -> TableStats {
        TableStats {
            total_entries: self.entries.len(),
            total_hits: self.total_hits,
            total_misses: self.total_misses,
        }
    }
}

/// Diagnostic statistics.
#[derive(Debug, Clone)]
pub struct TableStats {
    pub total_entries: usize,
    pub total_hits: u64,
    pub total_misses: u64,
}

impl std::fmt::Display for TableStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SubgoalTable: {} entries, {} hits, {} misses",
            self.total_entries, self.total_hits, self.total_misses,
        )
    }
}

// ============================================================================
// Thread-Local Table Access
// ============================================================================

thread_local! {
    static THREAD_TABLE: RefCell<SubgoalTable<MettaValue>> = RefCell::new(SubgoalTable::new());
    static SUBGOAL_DIRTY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Access the thread-local subgoal table.
#[inline]
pub fn with_subgoal_table<R>(f: impl FnOnce(&mut SubgoalTable<MettaValue>) -> R) -> R {
    SUBGOAL_DIRTY.with(|d| d.set(true));
    THREAD_TABLE.with(|cell| f(&mut cell.borrow_mut()))
}

/// Clear the thread-local subgoal table and active evaluation set.
#[inline]
pub fn clear_subgoal_table() {
    SUBGOAL_DIRTY.with(|d| {
        if d.get() {
            THREAD_TABLE.with(|cell| cell.borrow_mut().clear());
            clear_active_eval_set();
            d.set(false);
        }
    });
}

/// Collect GC roots from the thread-local subgoal table.
pub fn collect_subgoal_roots(out: &mut Vec<MettaValue>) {
    THREAD_TABLE.with(|cell| {
        let table = cell.borrow();
        for entry in table.entries.values() {
            out.extend(entry.results.iter().copied());
        }
    });
}

/// Invalidate the thread-local subgoal table and active evaluation set.
#[inline]
pub fn invalidate_subgoal_table() {
    THREAD_TABLE.with(|cell| cell.borrow_mut().invalidate_all());
    clear_active_eval_set();
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

        match table.lookup(hash) {
            TableLookup::Absent => {} // Expected
            other => panic!("Expected Absent, got {:?}", other),
        }
    }

    #[test]
    fn test_complete_and_hit() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let hash = 12345u64;

        table.complete(hash, smallvec::smallvec![make_long(42)]);
        assert!(table.is_complete(hash));

        match table.lookup(hash) {
            TableLookup::Complete(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].as_long(), Some(42));
            }
            other => panic!("Expected Complete, got {:?}", other),
        }
    }

    #[test]
    fn test_remove_entry() {
        let mut table = SubgoalTable::<MettaValue>::new();
        let hash = 99999u64;

        table.complete(hash, smallvec::smallvec![make_long(1)]);
        assert!(table.is_complete(hash));

        table.remove_entry(hash);
        assert!(!table.is_complete(hash));
    }

    #[test]
    fn test_active_eval_set() {
        clear_active_eval_set();

        assert!(!is_actively_evaluating(42));

        mark_eval_active(42);
        assert!(is_actively_evaluating(42));

        unmark_eval_active(42);
        assert!(!is_actively_evaluating(42));
    }

    #[test]
    fn test_active_eval_set_refcount() {
        clear_active_eval_set();

        mark_eval_active(42);
        mark_eval_active(42); // Two CompleteSubgoal frames for same hash
        assert!(is_actively_evaluating(42));

        unmark_eval_active(42); // One still active
        assert!(is_actively_evaluating(42));

        unmark_eval_active(42); // Both done
        assert!(!is_actively_evaluating(42));
    }

    #[test]
    fn test_multiple_subgoals() {
        let mut table = SubgoalTable::<MettaValue>::new();

        table.complete(111, smallvec::smallvec![make_long(1)]);
        table.complete(222, smallvec::smallvec![make_long(2), make_long(3)]);

        assert!(table.is_complete(111));
        assert!(table.is_complete(222));
        assert_eq!(table.len(), 2);
    }

    #[test]
    fn test_stats() {
        let mut table = SubgoalTable::<MettaValue>::new();

        table.complete(1, smallvec::smallvec![make_long(1)]);
        table.lookup(1); // hit
        table.lookup(1); // hit
        table.lookup(2); // miss

        let stats = table.stats();
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.total_hits, 2);
        assert_eq!(stats.total_misses, 1);
    }

    #[test]
    fn test_clear() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.complete(1, smallvec::smallvec![make_long(1)]);

        table.clear();
        assert!(table.is_empty());
    }

    #[test]
    fn test_collect_roots() {
        let mut table = SubgoalTable::<MettaValue>::new();
        table.complete(1, smallvec::smallvec![make_long(10)]);
        table.complete(2, smallvec::smallvec![make_long(20), make_long(30)]);

        let mut roots = Vec::new();
        table.collect_roots(&mut roots);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_thread_local_access() {
        with_subgoal_table(|table| {
            table.clear();
            table.complete(42, smallvec::smallvec![make_long(99)]);
        });

        with_subgoal_table(|table| {
            match table.lookup(42) {
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
        table.complete(1, smallvec::smallvec![make_long(1)]);

        let stats = table.stats();
        let display = format!("{}", stats);
        assert!(display.contains("1 entries"));
    }
}
