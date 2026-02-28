//! Thread-local trace collector for bytecode VM and JIT tiers.
//!
//! The tree-walker accesses the trace collector through `EvalContext::trace_collector()`.
//! The bytecode VM and JIT execution paths don't have access to an `EvalContext` — they
//! use their own execution contexts (`GenericBytecodeVM`, `JitContext`). Rather than
//! threading a trace collector through all of their generic type parameters and
//! constructors, we use a thread-local "sink" that is:
//!
//! 1. Set before calling into bytecode/JIT (`set_thread_trace_collector`)
//! 2. Read by instrumentation points inside the VM/JIT (`with_thread_trace_collector`)
//! 3. Cleared after the bytecode/JIT call returns (`clear_thread_trace_collector`)
//!
//! This is safe because:
//! - Each evaluation thread has its own thread-local
//! - The `Arc<TraceCollector>` is cloned into the thread-local, keeping it alive
//! - The collector is set/cleared in a scoped fashion by the `eval_inner_with_trace` caller

use std::cell::{Cell, RefCell};
use std::sync::Arc;

use super::collector::TraceCollector;

thread_local! {
    /// Thread-local trace collector for bytecode VM and JIT.
    /// Set before calling into non-tree-walker tiers; cleared after return.
    static THREAD_TRACE_COLLECTOR: RefCell<Option<Arc<TraceCollector>>> = const { RefCell::new(None) };

    /// Thread-local trace collector pointer for type inference and rule management.
    /// Set by the trampoline (which owns the reference via EvalContext) at entry
    /// and cleared at exit. Uses a raw pointer because the trampoline's lifetime
    /// outlives all type inference calls within its loop.
    static TRACE_COLLECTOR_REF: Cell<Option<*const TraceCollector>> = const { Cell::new(None) };
}

/// Set the thread-local trace collector for bytecode/JIT instrumentation.
///
/// Call this before dispatching to the bytecode VM or JIT tier.
/// The collector is cloned (Arc bump) and stored in thread-local storage.
#[inline]
pub fn set_thread_trace_collector(collector: &Arc<TraceCollector>) {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        *cell.borrow_mut() = Some(Arc::clone(collector));
    });
}

/// Clear the thread-local trace collector.
///
/// Call this after bytecode VM or JIT execution returns, or when the
/// trampoline exits. Clears both the Arc-based and ref-based thread-locals.
#[inline]
pub fn clear_thread_trace_collector() {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        *cell.borrow_mut() = None;
    });
    TRACE_COLLECTOR_REF.with(|c| c.set(None));
}

/// Execute a closure with the thread-local trace collector (if present).
///
/// Use this from inside bytecode VM opcode handlers and JIT runtime helpers
/// to emit trace events without requiring direct access to the collector.
///
/// If no collector is set (tracing not active), the closure is not called.
#[inline]
pub fn with_thread_trace_collector<R>(f: impl FnOnce(&TraceCollector) -> R) -> Option<R> {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        let borrow = cell.borrow();
        borrow.as_ref().map(|tc| f(tc))
    })
}

/// Set the thread-local trace collector reference for type inference instrumentation.
///
/// Called by the trampoline at entry. The raw pointer is valid for the duration
/// of the trampoline's execution — the trampoline holds the reference via
/// `EvalContext::trace_collector()` which returns `&TraceCollector` backed by
/// an `Arc<TraceCollector>` in `SessionContext`.
///
/// # Safety Contract
///
/// The caller must ensure the `TraceCollector` outlives all code that calls
/// `with_trace_collector_ref`. The trampoline guarantees this by calling
/// `clear_thread_trace_collector` before returning.
#[inline]
pub fn set_thread_trace_collector_ref(collector: &TraceCollector) {
    TRACE_COLLECTOR_REF.with(|c| c.set(Some(collector as *const _)));
}

/// Execute a closure with the ref-based thread-local trace collector (if present).
///
/// Used by type inference (`infer_types_generic`, `types_match_generic`) and
/// rule management (`add_rule`) to emit trace events without requiring an
/// `EvalContext` parameter.
///
/// Checks both the ref-based collector (set by the trampoline) and the Arc-based
/// collector (set by eval_inner_with_trace for bytecode/JIT).
///
/// If no collector is set (tracing not active), the closure is not called.
#[inline]
pub fn with_trace_collector_ref<R>(f: impl FnOnce(&TraceCollector) -> R) -> Option<R> {
    // First check the ref-based collector (trampoline path)
    TRACE_COLLECTOR_REF.with(|c| {
        if let Some(ptr) = c.get() {
            // SAFETY: pointer set by trampoline which outlives all type inference
            // calls within its loop. Trampoline clears it before returning.
            return Some(f(unsafe { &*ptr }));
        }
        // Fall back to Arc-based collector (bytecode/JIT path)
        THREAD_TRACE_COLLECTOR.with(|cell| {
            let borrow = cell.borrow();
            borrow.as_ref().map(|tc| f(tc))
        })
    })
}
