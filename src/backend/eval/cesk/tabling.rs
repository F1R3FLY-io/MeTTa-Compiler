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
use std::sync::Arc;

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

    /// #309/#266 cross-thread fixpoint seed (root cause #1). Hashes that were
    /// active on the FORKING thread's lineage when a parallel branch was
    /// dispatched. `ACTIVE_EVAL_SET` is thread-local, so a fanned-out worker —
    /// which starts a FRESH trampoline with an empty active set — would NOT see a
    /// parent-active subgoal S as a cycle, miss the fixpoint cut, and re-derive a
    /// divergent/smaller bag that `CompleteSubgoal` then tables (a later identical
    /// S is served the smaller bag -> a clean SUBSET of results drops). Seeding
    /// the worker with the parent's active hashes makes `is_actively_evaluating`
    /// return true for S, so the worker cuts to the EMPTY fixpoint exactly as the
    /// parent would inline. SEPARATE from `ACTIVE_EVAL_SET` and NOT refcounted by
    /// `mark`/`unmark` (the bytecode VM + JIT also `mark`/`unmark` `ACTIVE_EVAL_SET`;
    /// entangling the seed with their refcount would underflow it). Maintained
    /// solely by `SeedActiveScope` (RAII, refcounted only for nested re-entrancy).
    static SEEDED_ACTIVE_SET: RefCell<HashMap<u64, u32>> = RefCell::new(HashMap::new());
}

/// Check if an expression hash is currently being evaluated (on the call stack).
///
/// Consults this thread's own `ACTIVE_EVAL_SET` marks AND the `SEEDED_ACTIVE_SET`
/// (#309/#266: subgoals active on the FORKING thread's lineage when this worker
/// was dispatched), so a fanned-out worker detects a parent-active cycle and cuts
/// to the fixpoint EMPTY. The seed probe short-circuits on an empty seed map, so
/// the FANOUT=0 / non-worker path stays byte-identical.
// #309 frisbee-drop A/B (off by default -> byte-identical): when METTATRON_DISABLE_SEED is
// set, is_actively_evaluating IGNORES the cross-thread SEEDED_ACTIVE_SET (only this thread's
// own ACTIVE_EVAL_SET marks count). Tests whether the seed's over-cut of non-recursive
// subgoals (e.g. the constant `(kb)` -> empty -> cascade) is the frisbee-drop root.
fn seed_disabled() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var("METTATRON_DISABLE_SEED").is_ok())
}

#[inline]
pub fn is_actively_evaluating(expr_hash: u64) -> bool {
    ACTIVE_EVAL_SET.with(|set| set.borrow().get(&expr_hash).copied().unwrap_or(0) > 0)
        || (!seed_disabled()
            && SEEDED_ACTIVE_SET.with(|set| {
                let s = set.borrow();
                !s.is_empty() && s.get(&expr_hash).copied().unwrap_or(0) > 0
            }))
}

/// #309/#266 frisbee-drop: true iff `expr_hash` is active SOLELY via the cross-thread
/// `SEEDED_ACTIVE_SET` (and NOT this thread's own `ACTIVE_EVAL_SET`). A cut triggered only
/// by the seed is TENTATIVE — the seed cuts the FIRST worker-occurrence of a parent-active
/// subgoal, which is correct for a genuine cross-thread recursion but WRONG for a
/// non-recursive one (the constant `(kb)` → EMPTY → cascade). The caller bumps
/// `note_seed_cut()` so the enclosing subgoal's `CompleteSubgoal` skips tabling the
/// transient result. A real same-thread recursion (own-active) is NOT seed-only and its
/// fixpoint-EMPTY base IS correct to table.
#[inline]
pub fn is_seed_only_active(expr_hash: u64) -> bool {
    if seed_disabled() {
        return false;
    }
    let own = ACTIVE_EVAL_SET.with(|set| set.borrow().get(&expr_hash).copied().unwrap_or(0) > 0);
    if own {
        return false;
    }
    SEEDED_ACTIVE_SET.with(|set| {
        let s = set.borrow();
        !s.is_empty() && s.get(&expr_hash).copied().unwrap_or(0) > 0
    })
}

thread_local! {
    /// #309/#266 frisbee-drop: monotonic per-thread count of SEED-cuts (a seed-only
    /// `is_actively_evaluating` hit that returned the fixpoint EMPTY). A subgoal whose
    /// derivation window [push, fire] spans a bump produced a seed-TENTATIVE result and is
    /// not tabled (see `Continuation::CompleteSubgoal.start_seed_cut`). The trampoline is
    /// sequential per thread, so the delta over a subgoal's window counts exactly the
    /// seed-cuts in ITS subtree.
    static SEED_CUT_COUNTER: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Read this thread's seed-cut counter (recorded into `CompleteSubgoal` at push).
#[inline]
pub fn seed_cut_count() -> u64 {
    SEED_CUT_COUNTER.with(|c| c.get())
}

/// Record that a seed-only cut just fired on this thread.
#[inline]
pub fn note_seed_cut() {
    SEED_CUT_COUNTER.with(|c| c.set(c.get().wrapping_add(1)));
}

// #309/#266 PRECISE SEEDING (complete fix): the cross-thread seed must carry ONLY genuinely
// RECURSIVE subgoals (a fixpoint head re-entering itself), NOT non-recursive constants like
// `(kb)` — seeding the latter makes a worker cut its FIRST `(kb)` to EMPTY (the frisbee-drop
// root). Recursion is detected DYNAMICALLY (no static rule analysis; survives add-atom):
// HEAD_ACTIVE_SET counts how many subgoals of each HEAD are on this thread's stack; a
// subgoal pushed while its head is ALREADY active is a recursive occurrence and is recorded
// in RECURSIVE_ACTIVE_SET, which is what `snapshot_active_hashes` seeds (under the gate).
thread_local! {
    /// head_hash -> count of subgoals with that head currently active on this thread.
    static HEAD_ACTIVE_SET: RefCell<HashMap<u64, u32>> = RefCell::new(HashMap::with_capacity(32));
    /// subgoal_hash -> count, for subgoals that are RECURSIVE occurrences (their head was
    /// already active when they were pushed). Seeded into workers; non-recursive constants
    /// (`(kb)`) never appear here.
    static RECURSIVE_ACTIVE_SET: RefCell<HashMap<u64, u32>> =
        RefCell::new(HashMap::with_capacity(32));
}

/// Companion to `mark_eval_active` at the trampoline subgoal-push: track head-recursion so
/// `snapshot_active_hashes` can seed ONLY recursive subgoals. `head_hash` identifies the
/// HEAD symbol (same for `(H a)` and `(H b)`), so re-entering `H` is detected.
#[inline]
pub fn mark_recursive_active(expr_hash: u64, head_hash: u64) {
    let was_active = HEAD_ACTIVE_SET.with(|h| {
        let mut m = h.borrow_mut();
        let c = m.entry(head_hash).or_insert(0);
        let was = *c;
        *c += 1;
        was >= 1
    });
    if was_active {
        RECURSIVE_ACTIVE_SET.with(|r| *r.borrow_mut().entry(expr_hash).or_insert(0) += 1);
    }
}

/// Companion to `unmark_eval_active`. Balanced with `mark_recursive_active`.
#[inline]
pub fn unmark_recursive_active(expr_hash: u64, head_hash: u64) {
    HEAD_ACTIVE_SET.with(|h| {
        let mut m = h.borrow_mut();
        if let Some(c) = m.get_mut(&head_hash) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                m.remove(&head_hash);
            }
        }
    });
    RECURSIVE_ACTIVE_SET.with(|r| {
        let mut m = r.borrow_mut();
        if let Some(c) = m.get_mut(&expr_hash) {
            *c = c.saturating_sub(1);
            if *c == 0 {
                m.remove(&expr_hash);
            }
        }
    });
}

/// #309/#266 root cause #2: is THIS thread mid-derivation (some subgoal marked
/// active, i.e. a `CompleteSubgoal` frame is pending)? `mark_eval_active` is
/// called exactly when `CompleteSubgoal` is pushed and `unmark` when it fires, so
/// a non-empty `ACTIVE_EVAL_SET` is equivalent to "a `CompleteSubgoal` is pending
/// on this thread's continuation stack". Used to SKIP clearing the fixpoint memos
/// on a GC-rendezvous RESUME — clearing them mid-derivation drops the in-flight
/// cycle-cut marks and tables a corrupted (empty) result under the still-pending
/// outer `CompleteSubgoal`. Checks only the thread's OWN marks, not the
/// cross-thread seed (`SEEDED_ACTIVE_SET`).
#[inline]
pub fn active_eval_set_is_empty() -> bool {
    ACTIVE_EVAL_SET.with(|set| set.borrow().is_empty())
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

/// Clear the active evaluation set. Does NOT clear `SEEDED_ACTIVE_SET`
/// (#309/#266): the cross-thread seed MUST persist for the seeded worker's whole
/// life so its recursive re-entries keep cutting to the fixpoint EMPTY. This is
/// called by `clear_subgoal_table`, which the GC-rendezvous resume runs
/// MID-derivation; clearing the seed there drops it and re-opens root cause #1
/// (the runtime symptom: seeded workers whose seed is wiped by a concurrent GC
/// resume still runaway/drop). The seed is balanced solely by `SeedActiveScope`
/// (RAII enter/drop, which runs even on a panic-unwind), so no belt-and-suspenders
/// clear is needed here.
#[inline]
pub fn clear_active_eval_set() {
    ACTIVE_EVAL_SET.with(|set| set.borrow_mut().clear());
    // #309/#266 precise seeding: the head-recursion tracking parallels ACTIVE_EVAL_SET, so
    // clear it in lockstep (same call sites, same balance discipline).
    HEAD_ACTIVE_SET.with(|set| set.borrow_mut().clear());
    RECURSIVE_ACTIVE_SET.with(|set| set.borrow_mut().clear());
}

/// #309/#266 root cause #1: snapshot the hashes active on THIS (forking) thread's
/// lineage, for seeding into fanned-out workers. Returns the UNION of
/// `ACTIVE_EVAL_SET` keys (this thread's own marks) AND `SEEDED_ACTIVE_SET` keys
/// (a seed this thread itself inherited — so nested-within-nested fanout carries
/// the seed transitively, closing the hole one level deeper; red-team R5).
/// `None` when both are empty (no active subgoal -> no seed -> zero overhead).
#[inline]
pub fn snapshot_active_hashes() -> Option<Arc<SmallVec<[u64; 8]>>> {
    let mut hashes: SmallVec<[u64; 8]> = SmallVec::new();
    // #309/#266 PRECISE SEEDING: seed ONLY genuinely RECURSIVE subgoals (a fixpoint head
    // that re-enters itself), tracked in RECURSIVE_ACTIVE_SET. Seeding a NON-recursive
    // subgoal made a worker cut its first occurrence to the fixpoint EMPTY — correct for a
    // real recursion, but WRONG for a constant like `(kb)` (→ empty → cascade → frisbee-drop).
    // Real PLN fixpoint recursions (PLN.Derive) still seed/cut, so root-cause-#1 stays fixed.
    RECURSIVE_ACTIVE_SET.with(|set| {
        for (&h, &c) in set.borrow().iter() {
            if c > 0 {
                hashes.push(h);
            }
        }
    });
    SEEDED_ACTIVE_SET.with(|set| {
        for (&h, &c) in set.borrow().iter() {
            if c > 0 && !hashes.contains(&h) {
                hashes.push(h);
            }
        }
    });
    // #309/#266 M4 (thunk cross-thread seed) REVERTED 2026-06-17 — unioning this
    // thread's Blackhole thunk hashes into the worker seed was proven net-negative
    // (it miscut shared in-flight dependencies; see thunk.rs `lookup` Absent-
    // branch for the root cause). Only the SUBGOAL channel (ACTIVE_EVAL_SET +
    // SEEDED_ACTIVE_SET) is seeded. Reverted line was:
    //   crate::backend::eval::cesk::thunk::collect_blackhole_hashes(&mut hashes);
    if hashes.is_empty() {
        None
    } else {
        Some(Arc::new(hashes))
    }
}

/// #309/#266 root cause #1: RAII scope that seeds the worker's `SEEDED_ACTIVE_SET`
/// with the forking thread's active hashes (captured by `snapshot_active_hashes`),
/// so a cross-thread recursive re-entry of a parent-active subgoal is detected as
/// a cycle and cut to the fixpoint EMPTY — byte-identical to the single-threaded
/// inline cut. Mirrors `WorkerCaptureScope`. Refcounted set semantics make a
/// nested `SeedActiveScope` (a worker that itself fans out and is re-entered)
/// re-entrancy-safe; `Drop` removes exactly the entries this scope inserted.
pub struct SeedActiveScope {
    seeded: SmallVec<[u64; 8]>,
    // !Send: manipulates thread-locals and must not cross threads.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl SeedActiveScope {
    /// Seed the current thread's `SEEDED_ACTIVE_SET` with `hint`. No-op when
    /// `hint` is `None` (the common, non-fanned-out case) — byte-identical.
    #[inline]
    pub fn enter(hint: Option<Arc<SmallVec<[u64; 8]>>>) -> Self {
        let mut seeded: SmallVec<[u64; 8]> = SmallVec::new();
        if let Some(hashes) = hint {
            SEEDED_ACTIVE_SET.with(|set| {
                let mut map = set.borrow_mut();
                for &h in hashes.iter() {
                    *map.entry(h).or_insert(0) += 1;
                    seeded.push(h);
                }
            });
        }
        SeedActiveScope {
            seeded,
            _not_send: std::marker::PhantomData,
        }
    }
}

impl Drop for SeedActiveScope {
    #[inline]
    fn drop(&mut self) {
        if self.seeded.is_empty() {
            return;
        }
        SEEDED_ACTIVE_SET.with(|set| {
            let mut map = set.borrow_mut();
            for &h in self.seeded.iter() {
                if let Some(c) = map.get_mut(&h) {
                    *c -= 1;
                    if *c == 0 {
                        map.remove(&h);
                    }
                }
            }
        });
    }
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
///
/// **IMPORTANT**: This is NOT a bug — storing `(V, GenericBindings<V>)` pairs
/// would cause within-query cross-caller contamination. Caller A with
/// `carrying_bindings = {$who=a}` and Caller B with `{$who=b}` hitting the
/// same cached subgoal must each reconstitute THEIR OWN `$who` on retrieval.
/// Storing bindings would leak Caller A's `$who=a` into Caller B's result —
/// a ghost that `query_generation` cannot prevent (it only isolates across
/// top-level `!`). Phase 3.2-G attempted pair-storage and was reverted
/// (commit 2e669c0) for this reason.
///
/// See `tests/ghost_branch_regression.rs::within_query_cache_isolation_contract`
/// for the regression guard.
#[derive(Debug, Clone)]
pub struct TableEntry<V: MettaValueTrait + Clone> {
    /// Cached result values (final).
    pub results: SmallVec<[V; 2]>,

    /// Number of times this entry was looked up (diagnostics).
    pub hit_count: u32,

    /// Mutation epoch (thread-local) when this entry was created.
    pub mutation_epoch: u64,

    /// Process-global space-mutation epoch (#309/#266) when this entry was
    /// created. A cross-thread `add-atom` to the shared atom-space bumps it, so
    /// a stale (especially negative/empty) tabled subgoal result is rejected on
    /// lookup by ANY worker — closing the parallel under-production hole where a
    /// sibling's derived fact never invalidated this thread's tabled empty.
    pub space_epoch: u64,

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

impl<V: MettaValueTrait + Clone + 'static> SubgoalTable<V> {
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

    fn shade_entry(entry: TableEntry<V>)
    where
        V: 'static,
    {
        Self::shade_values(entry.results);
    }

    fn insert_entry_with_satb(&mut self, expr_hash: u64, entry: TableEntry<V>)
    where
        V: 'static,
    {
        crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
            |satb_active| {
                let old = self.entries.insert(expr_hash, entry);
                if satb_active {
                    if let Some(old) = old {
                        Self::shade_entry(old);
                    }
                }
            },
        );
    }

    fn remove_entry_with_satb(&mut self, expr_hash: u64)
    where
        V: 'static,
    {
        crate::backend::eval::cesk::index_heap::index_gc::with_satb_deletion_barrier(
            |satb_active| {
                let old = self.entries.remove(&expr_hash);
                if satb_active {
                    if let Some(old) = old {
                        Self::shade_entry(old);
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
                        .flat_map(|entry| entry.results.iter().cloned())
                        .collect();
                    Self::shade_values(roots);
                }
                self.entries.clear();
            },
        );
    }

    /// Look up a cached result by expression hash.
    ///
    /// Returns `Complete(results)` on cache hit, `Absent` on miss.
    /// Stale entries (mutation epoch mismatch) are evicted.
    pub fn lookup(&mut self, expr_hash: u64) -> TableLookup<V> {
        // #309/#266 kill-switch: `METTATRON_DISABLE_EVAL_CACHES=1` also disables
        // the SubgoalTable memo (the flag historically covered only EVAL_MEMO +
        // MATCH_RESULT_CACHE, leaving this — the dominant recursive-PLN cache —
        // live). Cycle detection (the active-eval set, upstream of this lookup)
        // is unaffected, so termination is preserved; this only forces
        // recomputation of completed subgoals.
        if crate::backend::eval::trampoline::dispatch_hints::eval_caches_disabled() {
            self.total_misses += 1;
            return TableLookup::Absent;
        }
        let current_epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        let current_space_epoch =
            crate::backend::eval::trampoline::dispatch_hints::space_mutation_epoch();

        if let Some(entry) = self.entries.get_mut(&expr_hash) {
            if entry.mutation_epoch != current_epoch || entry.space_epoch != current_space_epoch {
                // Stale — evict. Thread-local `mutation_epoch` mismatch (this
                // thread's own side effect) OR #309/#266 cross-thread
                // `space_epoch` mismatch: a sibling worker mutated the SHARED
                // atom-space after this entry was tabled, so a tabled result
                // (including a negative/empty one) computed before that fact
                // existed must not be served.
                self.remove_entry_with_satb(expr_hash);
                self.total_misses += 1;
                return TableLookup::Absent;
            }
            if !crate::backend::eval::trampoline::dispatch_hints::is_scope_visible(entry.scope_gen)
            {
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
    pub fn complete(&mut self, expr_hash: u64, results: SmallVec<[V; 2]>) {
        let epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        let space_epoch =
            crate::backend::eval::trampoline::dispatch_hints::space_mutation_epoch();
        let gen = crate::backend::eval::trampoline::dispatch_hints::cache_generation();
        self.insert_entry_with_satb(
            expr_hash,
            TableEntry {
                results,
                hit_count: 0,
                mutation_epoch: epoch,
                space_epoch,
                scope_gen: gen,
            },
        );
    }

    /// Remove a cached entry (e.g., for selective invalidation).
    pub fn remove_entry(&mut self, expr_hash: u64) {
        self.remove_entry_with_satb(expr_hash);
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
        self.clear_entries_with_satb();
        self.total_hits = 0;
        self.total_misses = 0;
    }

    pub fn invalidate_all(&mut self) {
        self.clear_entries_with_satb();
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
    use crate::backend::models::{global_factory, MettaValueFactory};

    fn f() -> crate::backend::models::ActiveFactory {
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
