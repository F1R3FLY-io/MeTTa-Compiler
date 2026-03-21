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
}

impl<V: MettaValueTrait> Thunk<V> {
    /// Create a new suspended thunk.
    fn new_suspended() -> Self {
        Self {
            state: ThunkState::Suspended,
            results: SmallVec::new(),
            access_count: 0,
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

impl<V: MettaValueTrait + Clone> ThunkTable<V> {
    /// Create a new thunk table.
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(256),
            total_cycles: 0,
            total_hits: 0,
        }
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
                    self.total_hits += 1;
                    ThunkLookup::Evaluated(thunk.results.clone())
                }
                ThunkState::Error => {
                    ThunkLookup::Error
                }
            }
        } else {
            self.entries.insert(expr_hash, Thunk::new_suspended());
            ThunkLookup::Absent
        }
    }

    /// Update a thunk with evaluation results.
    ///
    /// Transitions from Blackhole to Evaluated. Must be called after
    /// successful evaluation.
    pub fn update(&mut self, expr_hash: u64, results: SmallVec<[V; 2]>) {
        if let Some(thunk) = self.entries.get_mut(&expr_hash) {
            thunk.state = ThunkState::Evaluated;
            thunk.results = results;
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
        self.entries.remove(&expr_hash);
    }

    /// Check if a thunk is in blackhole state (being evaluated).
    #[inline]
    pub fn is_blackhole(&self, expr_hash: u64) -> bool {
        self.entries
            .get(&expr_hash)
            .map_or(false, |t| t.state == ThunkState::Blackhole)
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
        self.entries.clear();
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
        self.entries.clear();
    }
}

impl<V: MettaValueTrait + Clone> Default for ThunkTable<V> {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

thread_local! {
    static THREAD_THUNKS: RefCell<ThunkTable<MettaValue>> = RefCell::new(ThunkTable::new());
}

/// Access the thread-local thunk table.
#[inline]
pub fn with_thunk_table<R>(f: impl FnOnce(&mut ThunkTable<MettaValue>) -> R) -> R {
    THREAD_THUNKS.with(|cell| {
        let mut table = cell.borrow_mut();
        f(&mut table)
    })
}

/// Clear the thread-local thunk table.
#[inline]
pub fn clear_thunk_table() {
    THREAD_THUNKS.with(|cell| {
        cell.borrow_mut().clear();
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
