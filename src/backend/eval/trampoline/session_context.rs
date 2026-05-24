//! Session-based Evaluation Context
//!
//! This module provides `SessionContext`, the `EvalContext` implementation for
//! session-scoped slab-allocated evaluation. It bridges the `MettaState` (which
//! coordinates GC) to the generic trampoline engine.
//!
//! ## Single Factory Model
//!
//! With the slab allocator, the dual-arena model (eval + storage) collapses into
//! a single `GcFactory` backed by the global `SlabAllocator`. All allocations —
//! intermediates and persistent values alike — go through the same factory. The
//! slab's mark-sweep GC handles reclamation of unreachable values.
//!
//! ## Usage
//!
//! `SessionContext` is used internally by `eval_trampoline` and `eval`
//! when an `MettaState` is provided. It implements `EvalContext` to route all
//! allocations through the global slab factory.

use std::cell::Cell;
#[cfg(feature = "trace")]
use std::sync::Arc;

use crate::backend::models::{
    alloc_count_snapshot, global_factory, register_temporary_roots, request_gc, GcFactory,
    MettaState, MettaValue,
};

use super::context::{EvalContext, MettaEnvironment};

/// Safepoint allocation count threshold.
///
/// When the global alloc count grows by this amount since the last safepoint,
/// the trampoline pauses for GC. Uses alloc count (not committed bytes)
/// because free-list reuse doesn't grow committed bytes, but dead objects
/// still accumulate in the environment (PathMap, rules, etc.).
///
/// 500K allocations at ~48-80 bytes each ≈ 24-40 MB of new objects per
/// safepoint. At Robot's ~1 GB/5s allocation rate (~4M allocs/s), this
/// fires roughly every ~0.12 seconds.
const SAFEPOINT_ALLOC_THRESHOLD: u64 = 500_000;

// ============================================================================
// SessionContext
// ============================================================================

/// Session-based evaluation context using slab-allocated `GcFactory`.
///
/// Provides a single `GcFactory` for all allocations while implementing
/// `EvalContext` for use with the generic trampoline engine.
///
/// ## Single Factory Model
///
/// All allocations go through `GcFactory`, backed by the global `SlabAllocator`.
/// The previous dual-arena split (eval vs. storage) is no longer needed because
/// the slab's snapshot-based mark-sweep GC handles reclamation. Both
/// `eval_factory()` and `storage_factory()` return the same `GcFactory` for
/// backward compatibility.
pub struct SessionContext<'s> {
    /// Reference to the MettaState coordinating GC
    state: &'s MettaState,

    /// Factory backed by the global SlabAllocator
    factory: GcFactory,

    /// Alloc count at the last safepoint check.
    /// Cell for interior mutability (should_safepoint takes &self).
    last_safepoint_allocs: Cell<u64>,

    /// Optional trace collector for evaluation tracing.
    /// Present only when `--trace FILE` was specified and the `eval-trace` feature is enabled.
    #[cfg(feature = "trace")]
    trace_collector: Option<Arc<crate::backend::trace::TraceCollector>>,
}

// Manual Debug impl to skip last_safepoint_bytes (Cell is not Debug in all contexts)
impl<'s> std::fmt::Debug for SessionContext<'s> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionContext")
            .field("state", &self.state)
            .field("factory", &self.factory)
            .finish()
    }
}

impl<'s> SessionContext<'s> {
    /// Create a new session context from an MettaState reference.
    ///
    /// The factory is obtained from `global_factory()`, which returns a
    /// `GcFactory` backed by the process-wide `SlabAllocator`.
    #[inline]
    pub fn new(state: &'s MettaState) -> Self {
        Self {
            state,
            factory: global_factory(),
            last_safepoint_allocs: Cell::new(alloc_count_snapshot()),
            #[cfg(feature = "trace")]
            trace_collector: None,
        }
    }

    /// Attach a trace collector to this session context.
    ///
    /// When a trace collector is attached, evaluation events will be emitted
    /// to the collector's output file. This is called when `--trace FILE` is
    /// specified on the command line.
    #[cfg(feature = "trace")]
    #[inline]
    pub fn with_trace_collector(
        mut self,
        collector: Arc<crate::backend::trace::TraceCollector>,
    ) -> Self {
        self.trace_collector = Some(collector);
        self
    }

    /// Get the factory for intermediate (eval) allocations.
    ///
    /// Returns the same `GcFactory` used for all allocations. This method
    /// exists for backward compatibility with code that distinguished between
    /// eval and storage factories.
    #[inline]
    pub fn eval_factory(&self) -> GcFactory {
        self.factory
    }

    /// Get the factory for persistent (storage) allocations.
    ///
    /// Returns the same `GcFactory` used for all allocations. This method
    /// exists for backward compatibility with code that distinguished between
    /// eval and storage factories.
    #[inline]
    pub fn storage_factory(&self) -> GcFactory {
        self.factory
    }

    /// Get reference to the MettaState.
    #[inline]
    pub fn state(&self) -> &'s MettaState {
        self.state
    }
}

// EvalContext routes all allocations through the single GcFactory
impl<'s> EvalContext for SessionContext<'s> {
    #[inline]
    fn factory(&self) -> &GcFactory {
        &self.factory
    }

    /// No-op: session-based GC replaces polling-based GC.
    ///
    /// Previously called every 256 trampoline iterations to apply backpressure
    /// and spawn the GC cron manager. With session-based GC, reclamation
    /// happens asynchronously when `SessionGuard` drops (between top-level
    /// expressions), so no polling or backpressure is needed during eval.
    #[inline]
    fn maybe_gc(&self) {
        // Session-based GC: all reclamation is triggered by SessionGuard::drop()
        // which enqueues the session's context_id for async bulk release on a
        // dedicated background thread. No polling or backpressure needed here.
    }

    /// Check if a GC safepoint should be taken based on allocation count
    /// or pending GC request.
    ///
    /// Returns `true` when EITHER:
    /// 1. The global alloc count has grown by `SAFEPOINT_ALLOC_THRESHOLD`
    ///    since the last safepoint (alloc-pressure path). Uses alloc count
    ///    instead of committed bytes because free-list reuse doesn't grow
    ///    committed bytes — after initial page allocation, committed bytes
    ///    plateau even as new objects are allocated from the free list.
    /// 2. The GC has explicitly requested cooperation via `is_gc_requested()`
    ///    (pressure-driven path). This generalizes the safepoint mechanism
    ///    to hot paths that allocate small-but-often: such workloads can
    ///    inflate `committed_bytes` past `gc_threshold` while individual
    ///    workers stay below `SAFEPOINT_ALLOC_THRESHOLD` per-iteration,
    ///    starving GC indefinitely. Honoring `is_gc_requested()` ensures
    ///    cooperation regardless of per-thread allocation cadence.
    #[inline]
    fn should_safepoint(&self) -> bool {
        // Pressure-driven path: GC has explicitly asked for cooperation.
        // A single Acquire load; negligible cost when no GC is pending.
        if crate::backend::models::gc_allocator::is_gc_requested() {
            self.last_safepoint_allocs.set(alloc_count_snapshot());
            return true;
        }
        // Alloc-delta path.
        let current = alloc_count_snapshot();
        let last = self.last_safepoint_allocs.get();
        if current.wrapping_sub(last) >= SAFEPOINT_ALLOC_THRESHOLD {
            self.last_safepoint_allocs.set(current);
            true
        } else {
            false
        }
    }

    /// Phase 9 purely-async safepoint: register trampoline roots + signal
    /// GC. Does NOT wait on quiescence; the trampoline never blocks for
    /// GC progress. Mark-sweep runs concurrently via `maybe_async_gc()`
    /// triggered by the cron monitor or any subsequent allocation.
    ///
    /// # Root visibility invariant (Phase 9)
    ///
    /// The roots passed here are registered with the global
    /// `ROOT_REGISTRY` via `SafepointRootHandle`. They remain registered
    /// until `_root_handle` drops at the end of this function. That
    /// window is sufficient for any concurrent mark-sweep snapshot taken
    /// during this function call to observe them. Subsequent snapshots
    /// (after this function returns) rely on the per-thread current-iter
    /// root cell + Phase 6/8 dispatch RootProviders to keep the
    /// trampoline's reachable values visible.
    fn perform_safepoint(&self, roots: Vec<MettaValue>) {
        let _root_handle = register_temporary_roots(roots);
        request_gc();
        // _root_handle drops here → unregisters temporary roots.
    }

    #[cfg(feature = "trace")]
    #[inline]
    fn trace_collector(&self) -> Option<&crate::backend::trace::TraceCollector> {
        self.trace_collector.as_deref()
    }

    #[inline]
    fn try_compiled_dispatch(
        &self,
        value: &MettaValue,
        env: &MettaEnvironment,
        compilation_hash: u64,
    ) -> Option<(Vec<MettaValue>, MettaEnvironment)> {
        crate::backend::bytecode::tiered_cache::try_sub_expr_dispatch_with_hash(
            compilation_hash,
            value,
            env,
        )
    }

    #[inline]
    fn try_compiled_dispatch_with_bindings(
        &self,
        value: &MettaValue,
        env: &MettaEnvironment,
        compilation_hash: u64,
    ) -> Option<(
        Vec<(
            MettaValue,
            crate::backend::models::GenericBindings<MettaValue>,
        )>,
        MettaEnvironment,
    )> {
        crate::backend::bytecode::tiered_cache::try_sub_expr_dispatch_with_hash_bindings(
            compilation_hash,
            value,
            env,
        )
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_session_context_creation() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // Both factory accessors should work and produce valid values
        let eval_value = ctx.eval_factory().atom("eval");
        let storage_value = ctx.storage_factory().atom("storage");

        assert!(eval_value.is_atom());
        assert!(storage_value.is_atom());
    }

    #[test]
    fn test_session_context_factory_trait() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // EvalContext::factory() should return the GcFactory
        let value = ctx.factory().atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_single_factory_model() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);

        // eval_factory and storage_factory return the same GcFactory,
        // so allocations from either are interchangeable
        let eval_values: Vec<_> = (0..100).map(|i| ctx.eval_factory().long(i)).collect();
        let storage_values: Vec<_> = (0..100)
            .map(|i| ctx.storage_factory().long(i + 1000))
            .collect();

        // Both sets should be independently accessible
        for (i, v) in eval_values.iter().enumerate() {
            assert_eq!(v.as_long(), Some(i as i64));
        }
        for (i, v) in storage_values.iter().enumerate() {
            assert_eq!(v.as_long(), Some((i + 1000) as i64));
        }
    }

    #[test]
    fn test_session_context_debug() {
        let state = MettaState::new();
        let ctx = SessionContext::new(&state);
        let debug_str = format!("{:?}", ctx);
        assert!(debug_str.contains("SessionContext"));
    }
}
