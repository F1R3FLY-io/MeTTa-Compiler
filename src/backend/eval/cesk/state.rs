//! SECK Machine State
//!
//! The `SeckState` type formalizes the implicit machine state scattered across
//! the trampoline loop's local variables into a single, coherent structure.
//!
//! ## Machine Components
//!
//! ```text
//! ⟨S, E, C, K, Store⟩
//!
//! S:     OperandStack<MettaValue> — pre-allocated operand stack for intermediate results
//! E:     MettaEnvironment         — environment (variable bindings, rules)
//! C:     Work stack               — Vec<WorkItem> (control expressions + Resume)
//! K:     Continuation             — Vec<Continuation> (saved machine states)
//! Store: Global slab allocator    — parameterized allocation subsystem
//! ```
//!
//! ## Design Philosophy
//!
//! This is a **formalization layer** — the trampoline loop continues to operate
//! as before, but the state is now organized into a single struct that can be:
//!
//! - **Inspected**: all machine state is in one place for debugging
//! - **Serialized**: the dump (SECD-style) enables checkpointing
//! - **Analyzed**: AAM analysis (Phase 4) operates on abstract SECK states
//! - **Root-collected**: algebraic root set computation traverses one struct
//!
//! ## Transition to Usage
//!
//! Phase 0.5 will modify `eval_trampoline` to construct a `SeckState`
//! at entry and destructure it at exit. The transition is gradual — the trampoline
//! loop reads fields from the state instead of separate local variables.

use std::fmt::Debug;

use crate::backend::environment::MettaEnvironment;
use crate::backend::models::MettaValue;

use super::operand_stack::OperandStack;
use super::roots::RootSet;
use super::super::trampoline::{
    Continuation, EvalResult, WorkItem,
};

// ============================================================================
// SeckState — The Complete Machine State
// ============================================================================

/// The complete state of the SECK abstract machine.
///
/// Wraps all five components of the machine into a single struct:
///
/// | Field | Component | Description |
/// |-------|-----------|-------------|
/// | `operand_stack` | **S** (Stack) | Intermediate results |
/// | `work_stack` | **C** (Control) | Pending evaluation work |
/// | `continuations` | **K** (Kontinuation) | Saved machine states |
/// | `root_set` | (GC support) | Reusable root buffer |
/// | `gc_counter` | (GC support) | Safepoint iteration counter |
/// | `eval_count` | (Debug) | Total eval steps for diagnostics |
///
/// The **E** (Environment) component is carried within each work item and
/// continuation, following the existing CoW (clone-on-write via Arc) design.
///
/// The **Store** component is external — it's provided by the `EvalContext`
/// and shared across all machine states (it's the global slab allocator).
pub struct SeckState {
    // ── S: Operand Stack ─────────────────────────────────────────────
    /// Pre-allocated operand stack for intermediate evaluation results.
    /// Replaces per-Resume SmallVec allocation for hot paths.
    pub operand_stack: OperandStack<MettaValue>,

    // ── C: Control (Work Stack) ──────────────────────────────────────
    /// Pending evaluation work items. The trampoline pops from this stack
    /// on each iteration. Contains Eval, EvalWithBindings, and Resume items.
    pub work_stack: Vec<WorkItem>,

    // ── K: Kontinuation Stack ────────────────────────────────────────
    /// Saved machine states (continuations). When an Eval produces a result,
    /// the Resume item at the top of the work stack triggers the topmost
    /// continuation to process the result.
    pub continuations: Vec<Continuation>,

    // ── GC Support ───────────────────────────────────────────────────
    /// Reusable root set buffer for GC safepoint root collection.
    /// Allocated once, cleared and reused across safepoints.
    pub root_set: RootSet<MettaValue>,

    /// Safepoint iteration counter. Wrapping u16 — safepoint triggers
    /// when `gc_counter & 0xFFF == 0` (every 4096 iterations).
    pub gc_counter: u16,

    // ── Diagnostics ──────────────────────────────────────────────────
    /// Total evaluation steps performed (for debug tracing).
    pub eval_count: u64,

    // ── Result ───────────────────────────────────────────────────────
    /// Final evaluation result. Set when the Done continuation is reached.
    pub final_result: Option<EvalResult>,
}

impl SeckState {
    /// Create a new SECK machine state initialized for evaluating `value` in `env`.
    ///
    /// Sets up:
    /// - Empty operand stack with default capacity
    /// - Work stack with initial `Eval` work item
    /// - Continuation stack with `Done` sentinel
    /// - Empty root set with estimated capacity
    pub fn new(value: MettaValue, env: MettaEnvironment) -> Self {
        let mut work_stack = Vec::with_capacity(32);
        work_stack.push(WorkItem::Eval {
            value,
            env: std::sync::Arc::new(env),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
        });

        let mut continuations = Vec::with_capacity(64);
        continuations.push(Continuation::Done);

        Self {
            operand_stack: OperandStack::new(),
            work_stack,
            continuations,
            root_set: RootSet::with_capacity(128),
            gc_counter: 0,
            eval_count: 0,
            final_result: None,
        }
    }

    /// Check if the machine has terminated (no more work to do).
    #[inline]
    pub fn is_halted(&self) -> bool {
        self.work_stack.is_empty()
    }

    /// Pop the next work item from the work stack.
    #[inline]
    pub fn pop_work(&mut self) -> Option<WorkItem> {
        self.work_stack.pop()
    }

    /// Push a work item onto the work stack.
    #[inline]
    pub fn push_work(&mut self, work: WorkItem) {
        self.work_stack.push(work);
    }

    /// Push a continuation onto the continuation stack.
    #[inline]
    pub fn push_continuation(&mut self, cont: Continuation) {
        self.continuations.push(cont);
    }

    /// Pop the topmost continuation.
    #[inline]
    pub fn pop_continuation(&mut self) -> Option<Continuation> {
        self.continuations.pop()
    }

    /// Increment the GC counter and check if a safepoint should be considered.
    ///
    /// Returns `true` every 4096 iterations (when `gc_counter & 0xFFF == 0`).
    #[inline]
    pub fn tick_gc(&mut self) -> bool {
        self.gc_counter = self.gc_counter.wrapping_add(1);
        self.gc_counter & 0xFFF == 0
    }

    /// Collect all GC roots from the current machine state.
    ///
    /// Implements the algebraic root formula:
    /// ```text
    /// roots = addrs_in(S) ∪ addrs_in(C) ∪ addrs_in(K)
    /// ```
    ///
    /// The `current_work` parameter is the work item that was just popped
    /// from the work stack (it's not on the stack but still holds live values).
    pub fn collect_gc_roots(&mut self, current_work: &WorkItem) {
        self.root_set.clear();
        self.root_set.collect_from_operand_stack(&self.operand_stack);
        self.root_set.collect_from_work_items(current_work, &self.work_stack);
        self.root_set.collect_from_continuations(&self.continuations);
    }

    /// Take the collected roots as a Vec for the GC subsystem.
    ///
    /// Drains the root set buffer, transferring ownership. The root set
    /// retains its allocated capacity for the next safepoint.
    #[inline]
    pub fn take_roots(&mut self) -> Vec<MettaValue> {
        self.root_set.drain_into_vec()
    }

    /// Get the depth of the work stack (pending work items).
    #[inline]
    pub fn work_depth(&self) -> usize {
        self.work_stack.len()
    }

    /// Get the depth of the continuation stack.
    #[inline]
    pub fn continuation_depth(&self) -> usize {
        self.continuations.len()
    }
}

impl Debug for SeckState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeckState")
            .field("operand_stack_len", &self.operand_stack.total_len())
            .field("work_stack_len", &self.work_stack.len())
            .field("continuations_len", &self.continuations.len())
            .field("gc_counter", &self.gc_counter)
            .field("eval_count", &self.eval_count)
            .field("has_result", &self.final_result.is_some())
            .finish()
    }
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

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_new_state() {
        let f = factory();
        let state = SeckState::new(f.long(42), env());

        assert!(!state.is_halted());
        assert_eq!(state.work_depth(), 1);
        assert_eq!(state.continuation_depth(), 1); // Done sentinel
        assert_eq!(state.eval_count, 0);
        assert!(state.final_result.is_none());
    }

    #[test]
    fn test_pop_work() {
        let f = factory();
        let mut state = SeckState::new(f.long(42), env());

        let work = state.pop_work();
        assert!(work.is_some());
        assert!(state.is_halted()); // work stack is now empty
    }

    #[test]
    fn test_push_pop_continuation() {
        let f = factory();
        let mut state = SeckState::new(f.long(42), env());

        state.push_continuation(Continuation::ProcessIsError {
            env: std::sync::Arc::new(env()),
            depth: 0,
            outer_carrying: Box::new(crate::backend::models::GenericBindings::new()),
        });
        assert_eq!(state.continuation_depth(), 2); // Done + ProcessIsError

        let cont = state.pop_continuation();
        assert!(matches!(cont, Some(Continuation::ProcessIsError { .. })));
    }

    #[test]
    fn test_tick_gc() {
        let f = factory();
        let mut state = SeckState::new(f.long(42), env());

        // Tick 4095 times without triggering
        for _ in 0..4095 {
            assert!(!state.tick_gc());
        }
        // 4096th tick should trigger
        assert!(state.tick_gc());

        // Next 4095 ticks should not trigger
        for _ in 0..4095 {
            assert!(!state.tick_gc());
        }
        // 8192nd tick should trigger again
        assert!(state.tick_gc());
    }

    #[test]
    fn test_collect_gc_roots() {
        let f = factory();
        let mut state = SeckState::new(f.long(42), env());

        let current_work = state.pop_work().expect("has work");
        state.collect_gc_roots(&current_work);

        assert_eq!(state.root_set.len(), 1); // The Eval work item's value
    }

    #[test]
    fn test_debug_format() {
        let f = factory();
        let state = SeckState::new(f.long(42), env());

        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("SeckState"));
        assert!(debug_str.contains("work_stack_len: 1"));
    }
}
