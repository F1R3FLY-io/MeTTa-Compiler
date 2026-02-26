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

use std::cell::RefCell;
use std::sync::Arc;

use super::collector::TraceCollector;

thread_local! {
    /// Thread-local trace collector for bytecode VM and JIT.
    /// Set before calling into non-tree-walker tiers; cleared after return.
    static THREAD_TRACE_COLLECTOR: RefCell<Option<Arc<TraceCollector>>> = const { RefCell::new(None) };
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
/// Call this after bytecode VM or JIT execution returns.
#[inline]
pub fn clear_thread_trace_collector() {
    THREAD_TRACE_COLLECTOR.with(|cell| {
        *cell.borrow_mut() = None;
    });
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
