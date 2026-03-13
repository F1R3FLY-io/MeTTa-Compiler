//! WAM Trail: Undo log for binding slot mutations.
//!
//! The trail records every binding slot mutation so that bindings can be undone
//! when backtracking through nondeterministic alternatives. This replaces the
//! current approach of cloning `GenericBindings<V>` for each branch.
//!
//! # Performance
//!
//! - **93% single-match case**: Trail is written but never unwound (zero undo cost).
//!   The trail entries are simply discarded when the single match succeeds.
//! - **Nondeterministic case**: Only the modified slots are restored (O(changed) vs
//!   O(all_slots) for full clone). Typical PLN rules modify 2-4 slots per match.
//! - **Memory**: Pre-allocated Vec<TrailEntry> with capacity 64. Each entry is 12 bytes
//!   (u16 slot_index + 2 bytes padding + 8 bytes MettaValue). Total: ~768 bytes initial.

use crate::backend::models::MettaValue;

use super::binding_frame::WamBindingFrame;

/// A single trail entry recording a binding slot mutation.
///
/// When a `BindSlot` instruction executes, the previous value at that slot
/// is recorded here. On backtrack, the slot is restored to this previous value.
#[derive(Clone, Copy, Debug)]
pub struct TrailEntry {
    /// Index into the binding frame's `slots` array.
    pub slot_index: u16,
    /// The value that was in the slot before the binding.
    /// For unbound slots, this is the UNBOUND sentinel value.
    pub previous: MettaValue,
}

/// WAM trail: a stack of binding events for undo on backtrack.
///
/// The trail is append-only during forward execution. On backtrack,
/// entries are popped in LIFO order and slots are restored.
pub struct Trail {
    /// Trail entries, most recent at the end.
    entries: Vec<TrailEntry>,
}

impl Trail {
    /// Create a new trail with pre-allocated capacity.
    ///
    /// Capacity 64 covers typical PLN evaluation depths (10-20 rules × 2-4 vars each).
    pub fn new() -> Self {
        Trail {
            entries: Vec::with_capacity(64),
        }
    }

    /// Create a new trail with specified capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Trail {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Snapshot the current trail position.
    ///
    /// Returns a mark that can be passed to `unwind_to()` to restore all
    /// bindings made after this point.
    #[inline]
    pub fn mark(&self) -> usize {
        self.entries.len()
    }

    /// Record a binding mutation for later undo.
    #[inline]
    pub fn push(&mut self, entry: TrailEntry) {
        self.entries.push(entry);
    }

    /// Undo all bindings made since `mark`, restoring slots to their previous values.
    ///
    /// Entries are processed in reverse order (LIFO) to correctly handle
    /// multiple mutations to the same slot within a single alternative.
    ///
    /// # Arguments
    /// * `mark` - Trail position returned by a previous `mark()` call.
    /// * `frame` - The binding frame whose slots are being restored.
    pub fn unwind_to(&mut self, mark: usize, frame: &mut WamBindingFrame) {
        while self.entries.len() > mark {
            let entry = self.entries.pop().expect("trail non-empty above mark");
            frame.set_slot_unchecked(entry.slot_index, entry.previous);
        }
    }

    /// Undo all bindings since `mark` across multiple frames.
    ///
    /// Each trail entry records which frame slot it belongs to. When frames are
    /// stacked (nested let/chain), this variant restores slots in the correct frame.
    ///
    /// For Phase 1, all bindings are in a single frame, so `unwind_to` suffices.
    /// This method is provided for future multi-frame support.
    pub fn unwind_to_multi(&mut self, mark: usize, frames: &mut [WamBindingFrame]) {
        while self.entries.len() > mark {
            let entry = self.entries.pop().expect("trail non-empty above mark");
            // For now, all entries target the last frame.
            // Future: add frame_index to TrailEntry for multi-frame support.
            if let Some(frame) = frames.last_mut() {
                frame.set_slot_unchecked(entry.slot_index, entry.previous);
            }
        }
    }

    /// Number of entries currently on the trail.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the trail is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Clear the trail, keeping allocated capacity.
    #[inline]
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Collect all MettaValues referenced by trail entries (for GC root reporting).
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        for entry in &self.entries {
            out.push(entry.previous);
        }
    }
}

impl Default for Trail {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_trail_mark_and_len() {
        let trail = Trail::new();
        assert_eq!(trail.mark(), 0);
        assert!(trail.is_empty());
        assert_eq!(trail.len(), 0);
    }

    #[test]
    fn test_trail_push_increments_len() {
        let mut trail = Trail::new();
        trail.push(TrailEntry {
            slot_index: 0,
            previous: MettaValue::inline_unit(),
        });
        assert_eq!(trail.len(), 1);
        assert!(!trail.is_empty());
    }

    #[test]
    fn test_trail_mark_snapshot() {
        let mut trail = Trail::new();
        trail.push(TrailEntry {
            slot_index: 0,
            previous: MettaValue::inline_unit(),
        });
        let mark = trail.mark();
        assert_eq!(mark, 1);

        trail.push(TrailEntry {
            slot_index: 1,
            previous: MettaValue::Long(42),
        });
        assert_eq!(trail.mark(), 2);
        assert_eq!(trail.len(), 2);

        // Mark is still 1 (snapshot, not live reference)
        assert_eq!(mark, 1);
    }

    #[test]
    fn test_trail_unwind_restores_slots() {
        let mut trail = Trail::new();
        let mut frame = WamBindingFrame::new(3);

        // Initial state: all UNBOUND
        let mark = trail.mark();

        // Bind slot 0 = 42
        let prev0 = frame.get_slot(0);
        trail.push(TrailEntry {
            slot_index: 0,
            previous: prev0,
        });
        frame.set_slot_unchecked(0, MettaValue::Long(42));

        // Bind slot 2 = 99
        let prev2 = frame.get_slot(2);
        trail.push(TrailEntry {
            slot_index: 2,
            previous: prev2,
        });
        frame.set_slot_unchecked(2, MettaValue::Long(99));

        // Verify bindings took effect
        assert_eq!(frame.get_slot(0), MettaValue::Long(42));
        assert_eq!(frame.get_slot(2), MettaValue::Long(99));

        // Unwind
        trail.unwind_to(mark, &mut frame);

        // Slots should be restored to UNBOUND
        assert_eq!(frame.get_slot(0), WamBindingFrame::unbound());
        assert_eq!(frame.get_slot(2), WamBindingFrame::unbound());
        assert_eq!(trail.len(), 0);
    }

    #[test]
    fn test_trail_unwind_lifo_order() {
        let mut trail = Trail::new();
        let mut frame = WamBindingFrame::new(2);

        let mark = trail.mark();

        // First bind slot 0 = 10
        trail.push(TrailEntry {
            slot_index: 0,
            previous: WamBindingFrame::unbound(),
        });
        frame.set_slot_unchecked(0, MettaValue::Long(10));

        // Then re-bind slot 0 = 20 (overwrite)
        trail.push(TrailEntry {
            slot_index: 0,
            previous: MettaValue::Long(10),
        });
        frame.set_slot_unchecked(0, MettaValue::Long(20));

        assert_eq!(frame.get_slot(0), MettaValue::Long(20));

        // Unwind should restore to UNBOUND (the original value before mark)
        trail.unwind_to(mark, &mut frame);
        assert_eq!(frame.get_slot(0), WamBindingFrame::unbound());
    }

    #[test]
    fn test_trail_clear() {
        let mut trail = Trail::new();
        trail.push(TrailEntry {
            slot_index: 0,
            previous: MettaValue::inline_unit(),
        });
        trail.push(TrailEntry {
            slot_index: 1,
            previous: MettaValue::Long(1),
        });
        assert_eq!(trail.len(), 2);

        trail.clear();
        assert_eq!(trail.len(), 0);
        assert!(trail.is_empty());
    }

    #[test]
    fn test_trail_gc_roots() {
        let mut trail = Trail::new();
        trail.push(TrailEntry {
            slot_index: 0,
            previous: MettaValue::Long(42),
        });
        trail.push(TrailEntry {
            slot_index: 1,
            previous: MettaValue::Long(99),
        });

        let mut roots = Vec::new();
        trail.collect_gc_roots(&mut roots);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0], MettaValue::Long(42));
        assert_eq!(roots[1], MettaValue::Long(99));
    }
}
