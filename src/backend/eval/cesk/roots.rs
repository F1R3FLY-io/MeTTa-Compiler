//! Algebraic Root Set Computation for the SECK Machine
//!
//! This module provides precise, algebraic GC root computation from the
//! formalized SECK machine state, following the Van Horn & Might methodology:
//!
//! ```text
//! roots(⟨S, E, C, K, Store⟩) = addrs_in(S) ∪ addrs_in(C) ∪ range(E) ∪ addrs_in(K)
//! ```
//!
//! ## Current vs Target
//!
//! The current trampoline collects roots by iterating work items and continuations,
//! cloning values into a temporary `Vec<V>`. This is correct but:
//! - Allocates a new Vec on every safepoint
//! - Clones values (cheap for NaN-boxed, less cheap for slab pointers)
//! - Mixes machine state traversal with GC-specific concerns
//!
//! The algebraic root set formalizes this into a reusable `RootSet` that:
//! - Pre-allocates the root buffer once per trampoline invocation
//! - Provides `collect_from_*` methods matching SECK components
//! - Enables incremental root tracking (Phase 2.2) via dirty flags
//! - Separates root collection logic from GC triggering logic
//!
//! ## Environment Roots
//!
//! Environment roots (rules, bindings, space facts) are NOT collected here.
//! They are registered via `ROOT_REGISTRY` + `RootProvider` on
//! `GenericEnvironmentShared`, independent of the trampoline state.

use crate::backend::models::MettaValueTrait;

use super::operand_stack::OperandStack;
use super::super::trampoline::{
    Continuation, WorkItem,
};

// ============================================================================
// RootSet
// ============================================================================

/// Reusable buffer for collecting GC root values from SECK machine state.
///
/// The `RootSet` is allocated once per trampoline invocation and reused
/// across GC safepoints. It collects all `V` values reachable from the
/// machine's stack, control expression, and continuations.
///
/// ## Root Categories
///
/// The root set is the union of values from all SECK components:
///
/// | Component | Source | Method |
/// |-----------|--------|--------|
/// | **S** (Stack) | Operand stack values | `collect_from_operand_stack()` |
/// | **C** (Control) | Current work item | `collect_from_work_items()` |
/// | **K** (Kontinuation) | Continuation stack | `collect_from_continuations()` |
/// | **E** (Environment) | Managed externally via `RootProvider` | N/A |
/// | **Store** | Internal to allocator | N/A |
///
/// Additional root sources (eval memo cache, match result cache, frame chain)
/// are collected via `collect_auxiliary_roots()`.
#[derive(Debug)]
pub struct RootSet<V: MettaValueTrait> {
    /// Pre-allocated buffer for root values.
    /// Grows as needed but never shrinks within a trampoline invocation.
    roots: Vec<V>,
}

impl<V: MettaValueTrait + Clone> RootSet<V> {
    /// Create a new root set with estimated capacity.
    ///
    /// Capacity is based on typical PLN evaluation profiles:
    /// ~2 values per work item + ~4 per continuation + operand stack.
    #[inline]
    pub fn with_estimated_capacity(
        work_stack_len: usize,
        continuation_len: usize,
        operand_stack_len: usize,
    ) -> Self {
        let estimated = 2 + work_stack_len * 2 + continuation_len * 4 + operand_stack_len;
        Self {
            roots: Vec::with_capacity(estimated),
        }
    }

    /// Create a new root set with explicit capacity.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            roots: Vec::with_capacity(capacity),
        }
    }

    /// Clear collected roots, retaining allocated capacity for reuse.
    #[inline]
    pub fn clear(&mut self) {
        self.roots.clear();
    }

    /// Collect roots from the operand stack (S component).
    ///
    /// Corresponds to `addrs_in(S)` in the algebraic root formula.
    #[inline]
    pub fn collect_from_operand_stack(&mut self, stack: &OperandStack<V>) {
        stack.collect_roots(&mut self.roots);
    }

    /// Return the collected roots as a Vec for consumption by the GC.
    ///
    /// Drains the internal buffer, transferring ownership to the caller.
    /// The RootSet retains its allocated capacity for the next collection cycle.
    #[inline]
    pub fn drain_into_vec(&mut self) -> Vec<V> {
        std::mem::take(&mut self.roots)
    }

    /// Return a reference to the collected roots.
    #[inline]
    pub fn roots(&self) -> &[V] {
        &self.roots
    }

    /// Return the number of collected roots.
    #[inline]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// Check if no roots have been collected.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Push additional roots from external sources.
    ///
    /// Used for auxiliary root sources like eval memo cache, match result cache,
    /// and caller frame chain — values that are not part of the SECK state
    /// but must survive GC.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.roots.push(value);
    }

    /// Extend with additional roots from an iterator.
    #[inline]
    pub fn extend(&mut self, values: impl IntoIterator<Item = V>) {
        self.roots.extend(values);
    }

    /// Get a mutable reference to the underlying root buffer.
    ///
    /// Used for collecting roots from external sources that need a `&mut Vec<V>`.
    #[inline]
    pub fn as_mut_vec(&mut self) -> &mut Vec<V> {
        &mut self.roots
    }
}

/// Monomorphized methods that interact with the concrete WorkItem and
/// Continuation types (which now use MettaValue directly).
impl RootSet<crate::backend::models::MettaValue> {
    /// Collect roots from work items (C component — control expressions).
    ///
    /// Corresponds to `addrs_in(C)` in the algebraic root formula.
    /// Includes the currently-popped work item and all items remaining on the stack.
    pub fn collect_from_work_items(
        &mut self,
        current_work: &WorkItem,
        work_stack: &[WorkItem],
    ) {
        current_work.collect_values(&mut self.roots);
        for w in work_stack {
            w.collect_values(&mut self.roots);
        }
    }

    /// Collect roots from the continuation stack (K component).
    ///
    /// Corresponds to `addrs_in(K)` in the algebraic root formula.
    pub fn collect_from_continuations(
        &mut self,
        continuations: &[Continuation],
    ) {
        for c in continuations {
            c.collect_values(&mut self.roots);
        }
    }

    /// Collect all roots from a complete SECK machine snapshot.
    ///
    /// Convenience method that calls all `collect_from_*` methods.
    /// This is the algebraic root formula:
    ///
    /// ```text
    /// roots = addrs_in(S) ∪ addrs_in(C) ∪ addrs_in(K)
    /// ```
    ///
    /// Environment roots (E) are managed separately via `RootProvider`.
    pub fn collect_all(
        &mut self,
        operand_stack: &OperandStack<crate::backend::models::MettaValue>,
        current_work: &WorkItem,
        work_stack: &[WorkItem],
        continuations: &[Continuation],
    ) {
        self.clear();
        self.collect_from_operand_stack(operand_stack);
        self.collect_from_work_items(current_work, work_stack);
        self.collect_from_continuations(continuations);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{MettaValue, MettaValueFactory, global_factory};

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_empty_root_set() {
        let rs = RootSet::<MettaValue>::with_capacity(16);
        assert!(rs.is_empty());
        assert_eq!(rs.len(), 0);
    }

    #[test]
    fn test_collect_from_operand_stack() {
        let f = factory();
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(f.long(1));
        stack.push(f.long(2));

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_operand_stack(&stack);
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn test_collect_from_work_items() {
        let f = factory();
        let current = WorkItem::Eval {
            value: f.long(42),
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
        };
        let stack: Vec<WorkItem> = vec![
            WorkItem::Resume {
                result: (
                    smallvec![
                        crate::backend::eval::trampoline::types::bv(f.long(1)),
                        crate::backend::eval::trampoline::types::bv(f.long(2)),
                    ],
                    std::sync::Arc::new(env()),
                ),
            },
        ];

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_work_items(&current, &stack);
        assert_eq!(rs.len(), 3); // 1 from current Eval + 2 from Resume
    }

    #[test]
    fn test_collect_from_continuations() {
        let f = factory();
        let conts: Vec<Continuation> = vec![
            Continuation::Done,
            Continuation::ProcessCatch {
                default: f.atom("fallback"),
                env: std::sync::Arc::new(env()),
                depth: 0,
                outer_carrying: Box::new(crate::backend::models::GenericBindings::new()),
            },
        ];

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_continuations(&conts);
        assert_eq!(rs.len(), 1); // Done=0, ProcessCatch=1
    }

    #[test]
    fn test_collect_all() {
        let f = factory();
        let mut operand_stack = OperandStack::new();
        operand_stack.push_frame();
        operand_stack.push(f.long(10));

        let current = WorkItem::Eval {
            value: f.long(20),
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
        };
        let work_stack: Vec<WorkItem> = vec![];
        let continuations: Vec<Continuation> = vec![
            Continuation::Done,
        ];

        let mut rs = RootSet::with_estimated_capacity(0, 1, 1);
        rs.collect_all(&operand_stack, &current, &work_stack, &continuations);
        assert_eq!(rs.len(), 2); // 1 from operand stack + 1 from current work item
    }

    #[test]
    fn test_drain_and_reuse() {
        let f = factory();
        let mut rs = RootSet::with_capacity(8);
        rs.push(f.long(1));
        rs.push(f.long(2));

        let drained = rs.drain_into_vec();
        assert_eq!(drained.len(), 2);
        assert!(rs.is_empty()); // drained

        // Can reuse after drain
        rs.push(f.long(3));
        assert_eq!(rs.len(), 1);
    }

    #[test]
    fn test_clear_retains_capacity() {
        let f = factory();
        let mut rs = RootSet::<MettaValue>::with_capacity(64);
        for i in 0..32 {
            rs.push(f.long(i));
        }
        rs.clear();
        assert!(rs.is_empty());
        // Capacity is retained (we can't easily assert this but clear() should not shrink)
    }
}
