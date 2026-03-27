//! Pre-Allocated Operand Stack for the SECK Machine
//!
//! The operand stack (S component of the SECK machine) holds intermediate
//! evaluation results, replacing the per-`Resume` `SmallVec<[V; 2]>` allocation
//! pattern from the SECD machine tradition.
//!
//! ## Motivation
//!
//! In the current trampoline, each `WorkItem::Resume` carries a
//! `SmallVec<[V; 2]>` for results. While SmallVec inlines up to 2 elements,
//! the allocation and initialization overhead occurs on every Resume. The
//! operand stack pre-allocates a reusable buffer that persists across evaluation
//! steps.
//!
//! ## Design
//!
//! The operand stack provides:
//! - **Push/pop semantics** for intermediate results
//! - **Frame markers** to delimit result sets (e.g., for nondeterministic branches)
//! - **Bulk drain** to extract a complete result set into a SmallVec
//! - **Pre-allocated capacity** — grows once and reuses across trampoline iterations
//!
//! ## Integration with Resume
//!
//! The operand stack coexists with `WorkItem::Resume` during the transition
//! period. Phase 0.5 will gradually migrate Resume sites to use the operand stack
//! directly for hot paths, while cold paths continue using Resume unchanged.

use std::fmt::Debug;

use smallvec::SmallVec;

use crate::backend::models::MettaValueTrait;

// ============================================================================
// OperandStack
// ============================================================================

/// Pre-allocated operand stack for intermediate evaluation results.
///
/// The stack stores values and frame markers interleaved:
///
/// ```text
/// Bottom: [FRAME] [val1] [val2] [FRAME] [val3] ...  :Top
///          ^                      ^
///          frame 0                frame 1
/// ```
///
/// Frame markers enable efficient result set extraction: to collect all results
/// from the current frame, drain values from the top until hitting a frame marker.
///
/// ## Capacity
///
/// Initial capacity is 32 (covers >99% of PLN evaluations). The stack grows
/// as needed via `Vec::push`. Capacity is never shrunk — it persists across
/// trampoline iterations within a single `eval_trampoline` call.
///
/// ## Value Ownership
///
/// Values are moved into the stack (owned). This avoids clone overhead for
/// single-use results (the common case). Multi-use results should be cloned
/// before pushing.
#[derive(Debug)]
pub struct OperandStack<V: MettaValueTrait> {
    /// Interleaved values and frame markers.
    /// Frame markers are encoded as entries in `frame_positions`.
    values: Vec<V>,

    /// Stack of frame boundary positions in `values`.
    /// Each entry records the index in `values` where a frame begins.
    /// The topmost frame's results are `values[*frame_positions.last().. ]`.
    frame_positions: Vec<usize>,
}

impl<V: MettaValueTrait> OperandStack<V> {
    /// Create a new operand stack with default capacity.
    ///
    /// Pre-allocates space for 32 values and 8 frames, covering >99% of
    /// typical PLN evaluation depths.
    #[inline]
    pub fn new() -> Self {
        Self::with_capacity(32, 8)
    }

    /// Create a new operand stack with the given capacities.
    #[inline]
    pub fn with_capacity(value_capacity: usize, frame_capacity: usize) -> Self {
        Self {
            values: Vec::with_capacity(value_capacity),
            frame_positions: Vec::with_capacity(frame_capacity),
        }
    }

    /// Push a new frame marker onto the stack.
    ///
    /// All subsequent `push` calls will add values to this frame until
    /// `pop_frame` is called to extract them.
    #[inline]
    pub fn push_frame(&mut self) {
        self.frame_positions.push(self.values.len());
    }

    /// Push a value onto the current frame.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.values.push(value);
    }

    /// Push multiple values onto the current frame.
    #[inline]
    pub fn push_many(&mut self, values: impl IntoIterator<Item = V>) {
        self.values.extend(values);
    }

    /// Pop and return all values in the topmost frame as a SmallVec.
    ///
    /// Returns `None` if no frame is active. This is the primary interface
    /// for extracting evaluation results — it returns the exact same
    /// `SmallVec<[V; 2]>` type used by `EvalResult`.
    ///
    /// ## Performance
    ///
    /// For 0-2 results (93%+ of cases), the SmallVec is stack-allocated.
    /// For >2 results, heap allocation occurs (same as current Resume behavior).
    #[inline]
    pub fn pop_frame(&mut self) -> Option<SmallVec<[V; 2]>> {
        let frame_start = self.frame_positions.pop()?;
        let results: SmallVec<[V; 2]> = self.values.drain(frame_start..).collect();
        Some(results)
    }

    /// Pop and return all values in the topmost frame as a Vec.
    ///
    /// Similar to `pop_frame` but returns a Vec, which is useful when
    /// the caller needs to grow the result set.
    #[inline]
    pub fn pop_frame_vec(&mut self) -> Option<Vec<V>> {
        let frame_start = self.frame_positions.pop()?;
        let results: Vec<V> = self.values.drain(frame_start..).collect();
        Some(results)
    }

    /// Pop a single value from the top of the stack.
    ///
    /// Does NOT respect frame boundaries — caller must ensure this doesn't
    /// cross a frame marker. Used for single-result fast paths.
    #[inline]
    pub fn pop(&mut self) -> Option<V> {
        // Safety: only pop if we're above the current frame's start
        if let Some(&frame_start) = self.frame_positions.last() {
            if self.values.len() > frame_start {
                return self.values.pop();
            }
        }
        // No frame active or at frame boundary
        None
    }

    /// Peek at the top value without removing it.
    #[inline]
    pub fn peek(&self) -> Option<&V> {
        self.values.last()
    }

    /// Return the number of values in the current frame.
    #[inline]
    pub fn current_frame_len(&self) -> usize {
        if let Some(&frame_start) = self.frame_positions.last() {
            self.values.len() - frame_start
        } else {
            0
        }
    }

    /// Return the total number of values on the stack (across all frames).
    #[inline]
    pub fn total_len(&self) -> usize {
        self.values.len()
    }

    /// Return the number of active frames.
    #[inline]
    pub fn frame_depth(&self) -> usize {
        self.frame_positions.len()
    }

    /// Check if the stack is completely empty (no frames, no values).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty() && self.frame_positions.is_empty()
    }

    /// Clear the entire stack, removing all values and frames.
    ///
    /// Retains allocated capacity for reuse.
    #[inline]
    pub fn clear(&mut self) {
        self.values.clear();
        self.frame_positions.clear();
    }

    /// Collect all values on the stack as GC roots.
    ///
    /// Used during GC safepoints to register operand stack values as roots.
    /// This is the algebraic root contribution: `addrs_in(S)`.
    #[inline]
    pub fn collect_roots(&self, out: &mut Vec<V>) {
        out.extend(self.values.iter().cloned());
    }
}

impl<V: MettaValueTrait> Default for OperandStack<V> {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValue, global_factory, MettaValueFactory};

    fn make_atom(s: &str) -> MettaValue {
        global_factory().atom(s)
    }

    fn make_long(n: i64) -> MettaValue {
        global_factory().long(n)
    }

    #[test]
    fn test_empty_stack() {
        let stack = OperandStack::<MettaValue>::new();
        assert!(stack.is_empty());
        assert_eq!(stack.total_len(), 0);
        assert_eq!(stack.frame_depth(), 0);
    }

    #[test]
    fn test_single_frame_single_value() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(42));

        assert_eq!(stack.current_frame_len(), 1);
        assert_eq!(stack.total_len(), 1);
        assert_eq!(stack.frame_depth(), 1);

        let results = stack.pop_frame().expect("frame should exist");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(42));
        assert!(stack.is_empty());
    }

    #[test]
    fn test_single_frame_multiple_values() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(1));
        stack.push(make_long(2));
        stack.push(make_long(3));

        assert_eq!(stack.current_frame_len(), 3);

        let results = stack.pop_frame().expect("frame should exist");
        assert_eq!(results.len(), 3);
        assert_eq!(results[0].as_long(), Some(1));
        assert_eq!(results[1].as_long(), Some(2));
        assert_eq!(results[2].as_long(), Some(3));
    }

    #[test]
    fn test_nested_frames() {
        let mut stack = OperandStack::new();

        // Outer frame
        stack.push_frame();
        stack.push(make_atom("outer1"));
        stack.push(make_atom("outer2"));

        // Inner frame
        stack.push_frame();
        stack.push(make_atom("inner1"));

        assert_eq!(stack.frame_depth(), 2);
        assert_eq!(stack.current_frame_len(), 1);

        // Pop inner frame
        let inner = stack.pop_frame().expect("inner frame");
        assert_eq!(inner.len(), 1);
        assert_eq!(inner[0].as_atom(), Some("inner1"));

        // Pop outer frame
        assert_eq!(stack.current_frame_len(), 2);
        let outer = stack.pop_frame().expect("outer frame");
        assert_eq!(outer.len(), 2);
        assert_eq!(outer[0].as_atom(), Some("outer1"));
        assert_eq!(outer[1].as_atom(), Some("outer2"));

        assert!(stack.is_empty());
    }

    #[test]
    fn test_pop_no_frame() {
        let mut stack: OperandStack<MettaValue> = OperandStack::new();
        assert!(stack.pop_frame().is_none());
        assert!(stack.pop().is_none());
    }

    #[test]
    fn test_pop_single_value() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(10));
        stack.push(make_long(20));

        let v = stack.pop().expect("should pop value");
        assert_eq!(v.as_long(), Some(20));
        assert_eq!(stack.current_frame_len(), 1);
    }

    #[test]
    fn test_pop_respects_frame_boundary() {
        let mut stack = OperandStack::<MettaValue>::new();
        stack.push_frame();
        // Empty frame — pop should return None
        assert!(stack.pop().is_none());
    }

    #[test]
    fn test_push_many() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push_many(vec![make_long(1), make_long(2), make_long(3)]);

        assert_eq!(stack.current_frame_len(), 3);
        let results = stack.pop_frame().expect("frame");
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_collect_roots() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(1));
        stack.push_frame();
        stack.push(make_long(2));
        stack.push(make_long(3));

        let mut roots = Vec::new();
        stack.collect_roots(&mut roots);
        assert_eq!(roots.len(), 3); // All values across all frames
    }

    #[test]
    fn test_clear() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(1));
        stack.push_frame();
        stack.push(make_long(2));

        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.total_len(), 0);
        assert_eq!(stack.frame_depth(), 0);
    }

    #[test]
    fn test_pop_frame_vec() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(1));
        stack.push(make_long(2));

        let results = stack.pop_frame_vec().expect("frame");
        assert_eq!(results.len(), 2);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_peek() {
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(make_long(42));

        assert_eq!(stack.peek().expect("peek").as_long(), Some(42));
        assert_eq!(stack.current_frame_len(), 1); // peek doesn't remove
    }

    #[test]
    fn test_stack_size() {
        // OperandStack should be 3 Vecs = 3 * 24 = 72 bytes (or similar)
        let size = std::mem::size_of::<OperandStack<MettaValue>>();
        // Two Vecs: values (24 bytes) + frame_positions (24 bytes) = 48 bytes
        assert_eq!(size, 48);
    }
}
