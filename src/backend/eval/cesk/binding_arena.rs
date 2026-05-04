//! Per-Thread Binding Arena with Frame Stack and Shallow Backtracking
//!
//! This module provides a thread-local binding arena optimized for the pattern
//! matching hot path. It supports:
//!
//! - **Frame-based scoping**: push/pop frames for nested scopes (let, if, chain)
//! - **Shallow backtracking**: O(1) rollback when a rule match fails (WAM-inspired)
//! - **Arena allocation**: linear allocation with bulk deallocation on scope exit
//! - **Choice point snapshots**: O(1) save/restore for nondeterministic matching
//!
//! ## Design
//!
//! The arena uses a flat `Vec<BindingEntry>` with a frame stack tracking scope
//! boundaries. When a rule match fails, the arena truncates to the frame boundary
//! instead of deallocating individual entries. This eliminates allocation overhead
//! during the try-each-rule loop in `try_match_all_rules`.
//!
//! ## Integration
//!
//! The arena coexists with `GenericBindings<V>` — it's used for the matching phase
//! (trying rules), while `GenericBindings<V>` remains the storage format for
//! bindings that survive into continuations and work items.
//!
//! ## Memory Model
//!
//! ```text
//! ┌─────────────────────────────────────────────┐
//! │  entries: Vec<BindingEntry<V>>               │
//! │  ┌──────┬──────┬──────┬──────┬──────┬─────┐ │
//! │  │ $x=1 │ $y=2 │ $z=3 │ $a=4 │ $b=5 │ ... │ │
//! │  └──────┴──────┴──────┴──────┴──────┴─────┘ │
//! │  ^             ^             ^               │
//! │  frame[0]      frame[1]      frame[2]        │
//! │  start=0       start=2       start=4         │
//! │                                              │
//! │  frames: Vec<ArenaFrame>                     │
//! │  choice_points: Vec<ChoicePoint>             │
//! └─────────────────────────────────────────────┘
//! ```

use std::cell::RefCell;

use crate::backend::models::{GenericBindings, MettaValueTrait};

// ============================================================================
// Binding Entry
// ============================================================================

/// A single variable binding in the arena.
///
/// Uses `&'static str` for the variable name (interned in the slab allocator,
/// same as `GenericBindings`). The value is owned by the arena.
#[derive(Debug, Clone)]
pub struct BindingEntry<V: MettaValueTrait> {
    /// Variable name (e.g., "$x"). Interned with 'static lifetime.
    pub name: &'static str,
    /// Bound value.
    pub value: V,
}

// ============================================================================
// Arena Frame
// ============================================================================

/// A scope frame in the binding arena.
///
/// Each frame records where its entries start in the flat `entries` Vec.
/// Popping a frame truncates `entries` back to this position, freeing
/// all bindings allocated within the scope.
#[derive(Debug, Clone, Copy)]
struct ArenaFrame {
    /// Index into `entries` where this frame's bindings begin.
    start: usize,
}

// ============================================================================
// Choice Point
// ============================================================================

/// A snapshot of arena state for backtracking.
///
/// WAM-inspired: when trying multiple rule alternatives, save the current
/// arena state. If a match fails, restore to this point in O(1).
#[derive(Debug, Clone, Copy)]
pub struct ChoicePoint {
    /// Number of entries at snapshot time.
    entries_len: usize,
    /// Number of frames at snapshot time.
    frames_len: usize,
}

// ============================================================================
// BindingArena
// ============================================================================

/// Per-thread binding arena with frame stack and shallow backtracking.
///
/// The arena provides O(1) rollback for failed pattern matches by truncating
/// the entries vector instead of deallocating individual bindings. This is
/// critical for the `try_match_all_rules` hot path where multiple
/// rules are tried against the same expression.
///
/// ## Capacity
///
/// Default capacity: 64 entries, 16 frames, 8 choice points.
/// Covers >99% of PLN evaluation without reallocation.
///
/// ## Thread Safety
///
/// The arena is designed for single-thread use within a trampoline invocation.
/// It is accessed via `thread_local!` storage, not shared across threads.
#[derive(Debug)]
pub struct BindingArena<V: MettaValueTrait> {
    /// Flat array of all binding entries across all frames.
    entries: Vec<BindingEntry<V>>,

    /// Stack of frame boundaries.
    frames: Vec<ArenaFrame>,

    /// Stack of choice points for backtracking.
    choice_points: Vec<ChoicePoint>,
}

impl<V: MettaValueTrait + Clone> BindingArena<V> {
    /// Create a new binding arena with default capacity.
    pub fn new() -> Self {
        Self::with_capacity(64, 16, 8)
    }

    /// Create a new binding arena with specified capacities.
    pub fn with_capacity(
        entry_capacity: usize,
        frame_capacity: usize,
        choice_point_capacity: usize,
    ) -> Self {
        Self {
            entries: Vec::with_capacity(entry_capacity),
            frames: Vec::with_capacity(frame_capacity),
            choice_points: Vec::with_capacity(choice_point_capacity),
        }
    }

    // ── Frame Operations ─────────────────────────────────────────────

    /// Push a new scope frame onto the arena.
    ///
    /// All subsequent `bind()` calls will add entries to this frame.
    /// Call `pop_frame()` to discard all entries in this scope.
    #[inline]
    pub fn push_frame(&mut self) {
        self.frames.push(ArenaFrame {
            start: self.entries.len(),
        });
    }

    /// Pop the topmost frame, discarding all its entries.
    ///
    /// This is O(1) — it truncates the entries vec back to the frame's
    /// start position. No individual deallocation occurs.
    ///
    /// Returns `true` if a frame was popped, `false` if no frames exist.
    #[inline]
    pub fn pop_frame(&mut self) -> bool {
        if let Some(frame) = self.frames.pop() {
            self.entries.truncate(frame.start);
            true
        } else {
            false
        }
    }

    /// Get the current frame depth (number of active frames).
    #[inline]
    pub fn frame_depth(&self) -> usize {
        self.frames.len()
    }

    // ── Binding Operations ───────────────────────────────────────────

    /// Bind a variable to a value in the current frame.
    ///
    /// Returns `true` on success. Returns `false` if the variable is already
    /// bound to a different value in any frame (conflict detection).
    #[inline]
    pub fn bind(&mut self, name: &'static str, value: V) -> bool {
        // Check for conflicts in existing entries (any frame)
        for entry in self.entries.iter().rev() {
            if entry.name == name {
                if entry.value == value {
                    return true; // Already bound to same value — idempotent
                } else {
                    return false; // Conflict — different value
                }
            }
        }
        self.entries.push(BindingEntry { name, value });
        true
    }

    /// Look up a variable binding, searching innermost frame first.
    ///
    /// Returns the most recently bound value for the given name, or None.
    #[inline]
    pub fn get(&self, name: &str) -> Option<&V> {
        // Search from the end (innermost scope first)
        for entry in self.entries.iter().rev() {
            if entry.name == name {
                return Some(&entry.value);
            }
        }
        None
    }

    /// Return the number of entries in the current frame.
    #[inline]
    pub fn current_frame_len(&self) -> usize {
        if let Some(frame) = self.frames.last() {
            self.entries.len() - frame.start
        } else {
            self.entries.len() // No frames — all entries are "global"
        }
    }

    /// Return the total number of entries across all frames.
    #[inline]
    pub fn total_entries(&self) -> usize {
        self.entries.len()
    }

    // ── Choice Point Operations (WAM-style backtracking) ─────────────

    /// Save the current arena state as a choice point.
    ///
    /// This is O(1) — only records the current lengths.
    /// Use `restore()` to roll back to this point when a match fails.
    #[inline]
    pub fn save_choice_point(&mut self) -> ChoicePoint {
        let cp = ChoicePoint {
            entries_len: self.entries.len(),
            frames_len: self.frames.len(),
        };
        self.choice_points.push(cp);
        cp
    }

    /// Restore the arena to the most recent choice point.
    ///
    /// Discards all entries and frames created since the choice point
    /// was saved. This is O(1) — just truncation, no deallocation.
    ///
    /// Returns `true` if a choice point was restored, `false` if none exist.
    #[inline]
    pub fn restore_choice_point(&mut self) -> bool {
        if let Some(cp) = self.choice_points.pop() {
            self.entries.truncate(cp.entries_len);
            self.frames.truncate(cp.frames_len);
            true
        } else {
            false
        }
    }

    /// Discard the most recent choice point without restoring.
    ///
    /// Called when a match succeeds and the choice point is no longer needed.
    /// Keeps all current entries and frames intact.
    #[inline]
    pub fn commit_choice_point(&mut self) -> bool {
        self.choice_points.pop().is_some()
    }

    /// Return the number of active choice points.
    #[inline]
    pub fn choice_point_depth(&self) -> usize {
        self.choice_points.len()
    }

    // ── Conversion ───────────────────────────────────────────────────

    /// Export the current frame's bindings as a `GenericBindings<V>`.
    ///
    /// This is the bridge between the arena (used during matching) and the
    /// continuation system (which stores `GenericBindings<V>`).
    ///
    /// Only exports bindings from the topmost frame.
    pub fn export_current_frame(&self) -> GenericBindings<V> {
        let start = self.frames.last().map_or(0, |f| f.start);
        let frame_entries = &self.entries[start..];

        match frame_entries.len() {
            0 => GenericBindings::Empty,
            1 => GenericBindings::Single((
                crate::backend::models::generic_bindings::ROOT_SCOPE,
                frame_entries[0].name,
                frame_entries[0].value.clone(),
            )),
            _ => {
                let mut bindings = GenericBindings::new();
                for entry in frame_entries {
                    bindings.insert(entry.name, entry.value.clone());
                }
                bindings
            }
        }
    }

    /// Export ALL bindings across all frames as a `GenericBindings<V>`.
    ///
    /// Inner (newer) bindings shadow outer (older) bindings for the same name.
    pub fn export_all(&self) -> GenericBindings<V> {
        if self.entries.is_empty() {
            return GenericBindings::Empty;
        }

        // Build from outermost to innermost so inner shadows outer
        let mut bindings = GenericBindings::new();
        for entry in &self.entries {
            bindings.insert(entry.name, entry.value.clone());
        }
        bindings
    }

    /// Import bindings from a `GenericBindings<V>` into the current frame.
    ///
    /// Returns `true` if all bindings were imported without conflict.
    /// Returns `false` if any binding conflicts with an existing entry.
    pub fn import(&mut self, bindings: &GenericBindings<V>) -> bool {
        for (name, value) in bindings.iter() {
            if !self.bind(name, value.clone()) {
                return false;
            }
        }
        true
    }

    // ── Reset ────────────────────────────────────────────────────────

    /// Clear the entire arena, retaining allocated capacity.
    #[inline]
    pub fn clear(&mut self) {
        self.entries.clear();
        self.frames.clear();
        self.choice_points.clear();
    }

    /// Check if the arena is empty (no entries, no frames).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.frames.is_empty()
    }

    // ── GC Root Collection ───────────────────────────────────────────

    /// Collect all values in the arena as GC roots.
    #[inline]
    pub fn collect_roots(&self, out: &mut Vec<V>) {
        for entry in &self.entries {
            out.push(entry.value.clone());
        }
    }
}

impl<V: MettaValueTrait + Clone> Default for BindingArena<V> {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Thread-Local Arena Access
// ============================================================================

use crate::backend::models::MettaValue;

thread_local! {
    /// Thread-local binding arena for the tree-walker evaluation.
    ///
    /// Initialized lazily on first access. Persists across trampoline invocations
    /// within the same thread (cleared between top-level evaluations).
    static THREAD_ARENA: RefCell<BindingArena<MettaValue>> = RefCell::new(BindingArena::new());
}

/// Access the thread-local binding arena.
///
/// The closure receives a mutable reference to the arena. The arena persists
/// across calls within the same thread (not cleared automatically).
///
/// # Panics
///
/// Panics if the arena is already borrowed (re-entrant access). This should
/// not happen in normal evaluation flow since the arena is only accessed
/// from the trampoline loop.
#[inline]
pub fn with_thread_arena<R>(f: impl FnOnce(&mut BindingArena<MettaValue>) -> R) -> R {
    THREAD_ARENA.with(|cell| {
        let mut arena = cell.borrow_mut();
        f(&mut arena)
    })
}

/// Clear the thread-local binding arena.
///
/// Called between top-level evaluations to reset arena state.
#[inline]
pub fn clear_thread_arena() {
    THREAD_ARENA.with(|cell| {
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

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    fn make_atom(s: &str) -> MettaValue {
        factory().atom(s)
    }

    fn make_long(n: i64) -> MettaValue {
        factory().long(n)
    }

    #[test]
    fn test_empty_arena() {
        let arena = BindingArena::<MettaValue>::new();
        assert!(arena.is_empty());
        assert_eq!(arena.total_entries(), 0);
        assert_eq!(arena.frame_depth(), 0);
    }

    #[test]
    fn test_bind_and_get() {
        let mut arena = BindingArena::new();
        arena.push_frame();

        assert!(arena.bind("$x", make_long(42)));
        assert_eq!(arena.get("$x").expect("bound").as_long(), Some(42));
        assert!(arena.get("$y").is_none());
    }

    #[test]
    fn test_bind_conflict() {
        let mut arena = BindingArena::new();
        arena.push_frame();

        assert!(arena.bind("$x", make_long(1)));
        assert!(!arena.bind("$x", make_long(2))); // Conflict
        assert!(arena.bind("$x", make_long(1))); // Same value — OK
    }

    #[test]
    fn test_frame_scoping() {
        let mut arena = BindingArena::new();

        // Outer frame
        arena.push_frame();
        arena.bind("$x", make_long(1));

        // Inner frame
        arena.push_frame();
        arena.bind("$y", make_long(2));

        assert_eq!(arena.total_entries(), 2);
        assert_eq!(arena.get("$x").expect("outer").as_long(), Some(1));
        assert_eq!(arena.get("$y").expect("inner").as_long(), Some(2));

        // Pop inner frame — $y is gone
        arena.pop_frame();
        assert_eq!(arena.total_entries(), 1);
        assert!(arena.get("$y").is_none());
        assert_eq!(arena.get("$x").expect("still there").as_long(), Some(1));

        // Pop outer frame — $x is gone
        arena.pop_frame();
        assert_eq!(arena.total_entries(), 0);
        assert!(arena.get("$x").is_none());
    }

    #[test]
    fn test_choice_point_save_restore() {
        let mut arena = BindingArena::new();

        arena.push_frame();
        arena.bind("$x", make_long(1));

        // Save choice point
        arena.save_choice_point();

        // Try a match — add more bindings
        arena.push_frame();
        arena.bind("$y", make_long(2));
        arena.bind("$z", make_long(3));

        assert_eq!(arena.total_entries(), 3);

        // Match failed — restore
        arena.restore_choice_point();
        assert_eq!(arena.total_entries(), 1);
        assert_eq!(arena.frame_depth(), 1);
        assert!(arena.get("$y").is_none());
        assert_eq!(arena.get("$x").expect("survived").as_long(), Some(1));
    }

    #[test]
    fn test_choice_point_commit() {
        let mut arena = BindingArena::new();

        arena.push_frame();
        arena.bind("$x", make_long(1));
        arena.save_choice_point();

        arena.bind("$y", make_long(2));

        // Match succeeded — commit (keep bindings)
        assert!(arena.commit_choice_point());
        assert_eq!(arena.total_entries(), 2);
        assert_eq!(arena.choice_point_depth(), 0);
    }

    #[test]
    fn test_nested_choice_points() {
        let mut arena = BindingArena::new();

        arena.push_frame();
        arena.bind("$a", make_long(1));
        arena.save_choice_point(); // CP1

        arena.bind("$b", make_long(2));
        arena.save_choice_point(); // CP2

        arena.bind("$c", make_long(3));
        assert_eq!(arena.total_entries(), 3);

        // Restore CP2 — $c gone
        arena.restore_choice_point();
        assert_eq!(arena.total_entries(), 2);
        assert!(arena.get("$c").is_none());

        // Restore CP1 — $b gone
        arena.restore_choice_point();
        assert_eq!(arena.total_entries(), 1);
        assert!(arena.get("$b").is_none());
        assert_eq!(arena.get("$a").expect("survived").as_long(), Some(1));
    }

    #[test]
    fn test_export_current_frame() {
        let mut arena = BindingArena::new();

        arena.push_frame();
        arena.bind("$x", make_long(1));
        arena.bind("$y", make_long(2));

        let bindings = arena.export_current_frame();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$x").expect("x").as_long(), Some(1));
        assert_eq!(bindings.get("$y").expect("y").as_long(), Some(2));
    }

    #[test]
    fn test_export_empty_frame() {
        let mut arena = BindingArena::<MettaValue>::new();
        arena.push_frame();

        let bindings = arena.export_current_frame();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_export_all_with_shadowing() {
        let mut arena = BindingArena::new();

        arena.push_frame();
        arena.bind("$x", make_long(1));

        // Note: can't bind $x to different value due to conflict detection.
        // But we can have different vars in different frames.
        arena.push_frame();
        arena.bind("$y", make_long(2));

        let bindings = arena.export_all();
        assert_eq!(bindings.len(), 2);
    }

    #[test]
    fn test_import_bindings() {
        let mut arena = BindingArena::new();
        arena.push_frame();

        let mut bindings = GenericBindings::new();
        bindings.insert("$x", make_long(1));
        bindings.insert("$y", make_long(2));

        assert!(arena.import(&bindings));
        assert_eq!(arena.total_entries(), 2);
        assert_eq!(arena.get("$x").expect("x").as_long(), Some(1));
    }

    #[test]
    fn test_import_with_conflict() {
        let mut arena = BindingArena::new();
        arena.push_frame();
        arena.bind("$x", make_long(1));

        let mut bindings = GenericBindings::new();
        bindings.insert("$x", make_long(99)); // Conflict!

        assert!(!arena.import(&bindings));
    }

    #[test]
    fn test_clear() {
        let mut arena = BindingArena::new();
        arena.push_frame();
        arena.bind("$x", make_long(1));
        arena.save_choice_point();

        arena.clear();
        assert!(arena.is_empty());
        assert_eq!(arena.choice_point_depth(), 0);
    }

    #[test]
    fn test_collect_roots() {
        let mut arena = BindingArena::new();
        arena.push_frame();
        arena.bind("$x", make_long(1));
        arena.bind("$y", make_long(2));

        let mut roots = Vec::new();
        arena.collect_roots(&mut roots);
        assert_eq!(roots.len(), 2);
    }

    #[test]
    fn test_thread_arena() {
        with_thread_arena(|arena| {
            arena.clear();
            arena.push_frame();
            arena.bind("$test", make_long(42));
            assert_eq!(arena.get("$test").expect("bound").as_long(), Some(42));
        });

        // Access again — arena persists
        with_thread_arena(|arena| {
            assert_eq!(arena.get("$test").expect("persisted").as_long(), Some(42));
            arena.clear();
        });
    }

    #[test]
    fn test_shallow_backtracking_pattern() {
        // Simulate trying 3 rules against an expression:
        // Rule 1: pattern fails at structural check
        // Rule 2: pattern matches with bindings
        // Rule 3: skipped (first match wins in deterministic case)
        let mut arena = BindingArena::new();
        arena.push_frame(); // Match scope

        // Try rule 1
        arena.save_choice_point();
        arena.push_frame();
        arena.bind("$x", make_long(1));
        // Structural check fails — restore
        arena.restore_choice_point();
        assert_eq!(arena.total_entries(), 0); // All rule-1 bindings gone

        // Try rule 2
        arena.save_choice_point();
        arena.push_frame();
        arena.bind("$y", make_long(2));
        arena.bind("$z", make_long(3));
        // Match succeeds — commit and export
        arena.commit_choice_point();
        let bindings = arena.export_current_frame();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$y").expect("y").as_long(), Some(2));
        assert_eq!(bindings.get("$z").expect("z").as_long(), Some(3));
    }

    #[test]
    fn test_arena_size() {
        // BindingArena is 3 Vecs = 3 * 24 = 72 bytes
        assert_eq!(std::mem::size_of::<BindingArena<MettaValue>>(), 72);
    }
}
