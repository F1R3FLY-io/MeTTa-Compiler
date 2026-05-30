//! Thread-Local Frame Chain for GC Root Safety
//!
//! When nested trampoline executions occur (e.g., module import force-evals
//! `!(import! ...)` inside `eval_include_generic`), a GC safepoint in the inner
//! trampoline only collects roots from its own work_stack/continuations. The
//! caller's local `Vec<MettaValue>` (compiled expressions) is invisible to GC,
//! causing use-after-free when the caller continues iterating after the nested
//! trampoline returns.
//!
//! This module provides a **thread-local linked frame chain** where each frame
//! that holds live `MettaValue` references pushes an entry before entering a
//! nested trampoline. During safepoints, `collect_frame_chain_roots()` walks
//! the chain to collect ALL live roots from ALL caller frames.
//!
//! ## Design
//!
//! - **Zero heap allocation**: `EvalFrame` lives on the caller's stack frame
//! - **~2-4ns push/pop**: Two thread-local `Cell` operations each
//! - **Type-erased collectors**: `RootCollectorFn` is a function pointer, no
//!   dynamic dispatch or `dyn Trait`
//! - **Generic bridge**: `maybe_push_frame<C>()` pushes a frame protecting
//!   `Vec<MettaValue>` from GC during nested trampoline calls
//!
//! ## Stack Traces
//!
//! The frame chain also supports stack trace capture for error messages.
//! `capture_stack_trace()` walks the chain and collects `FrameLabel`s from
//! innermost (most recent) to outermost (root).

use std::cell::Cell;
use std::ptr;

use crate::backend::eval::trampoline::EvalContext;
use crate::backend::models::MettaValue;

// ============================================================================
// Core Data Structures
// ============================================================================

/// Type-erased root collector function.
///
/// Takes a raw pointer to caller data and appends live `MettaValue`s to `out`.
///
/// # Safety
///
/// `data` must point to a valid instance of the original type that was passed
/// to `EvalFrameGuard::push_vec()`. The caller must ensure the data outlives
/// the frame.
pub(crate) type RootCollectorFn = unsafe fn(data: *const (), out: &mut Vec<MettaValue>);

/// A single frame in the thread-local evaluation frame chain.
///
/// Each frame represents a caller that holds live `MettaValue` references
/// across a nested trampoline call. The frame chain is walked during GC
/// safepoints to collect roots from all active callers.
///
/// ## Layout (40 bytes)
///
/// - `parent`: 8 bytes (pointer to outer frame or null)
/// - `root_data`: 8 bytes (raw pointer to caller's root data)
/// - `root_collector`: 8 bytes (function pointer for type-erased collection)
/// - `label`: 16 bytes (enum with &'static str payload)
pub struct EvalFrame {
    /// Pointer to the next outer caller frame (null at root).
    parent: *const EvalFrame,
    /// Raw pointer to the caller's root data (e.g., `*const Vec<MettaValue>`).
    root_data: *const (),
    /// Type-erased collector function that extracts live values from `root_data`.
    root_collector: RootCollectorFn,
    /// Human-readable label for stack trace rendering.
    label: FrameLabel,
}

// SAFETY: EvalFrame is only accessed from the thread that created it via
// the thread-local FRAME_CHAIN_HEAD. The raw pointers point to data on that
// same thread's stack.
unsafe impl Send for EvalFrame {}

// `FrameLabel` relocated to [`super::frame_label`] (A5.0) so it survives A5.6's
// cfg-walling of this module to the slab build. Re-exported here so existing
// `frame_chain::FrameLabel` paths keep compiling in the slab build.
pub use crate::backend::eval::frame_label::FrameLabel;

// ============================================================================
// Thread-Local Chain Head
// ============================================================================

thread_local! {
    /// Head of the thread-local evaluation frame chain.
    ///
    /// Points to the most recently pushed `EvalFrame`, or null if no frames
    /// are active. Each frame's `parent` pointer links to the next outer frame.
    static FRAME_CHAIN_HEAD: Cell<*const EvalFrame> = const { Cell::new(ptr::null()) };
}

// ============================================================================
// RAII Guard
// ============================================================================

/// RAII guard that pushes/pops a frame on the thread-local chain.
///
/// When created, pushes an `EvalFrame` onto the chain head. When dropped,
/// restores the parent as the chain head. The frame is heap-allocated via
/// `Box` to ensure a stable address (stack-local frames would be invalidated
/// when the guard is returned from `push_vec`).
///
/// # Safety
///
/// The `root_data` pointer passed to `push_vec()` must outlive the guard.
/// This is enforced structurally: the guard is a local variable in the same
/// scope as the data it protects.
pub struct EvalFrameGuard {
    frame: Box<EvalFrame>,
}

impl EvalFrameGuard {
    /// Push a frame that protects a `Vec<MettaValue>` from GC.
    ///
    /// # Safety
    ///
    /// `data` must point to a valid `Vec<MettaValue>` that outlives the
    /// returned guard. Typically, both the Vec and the guard are locals in
    /// the same function scope, guaranteeing this.
    #[inline]
    pub unsafe fn push_vec(label: FrameLabel, data: *const Vec<MettaValue>) -> Self {
        unsafe { Self::push_custom(label, data as *const (), collect_vec_roots) }
    }

    /// Push a frame with a caller-supplied root collector.
    ///
    /// # Safety
    ///
    /// `data` must point to a value that outlives the returned guard, and
    /// `root_collector` must cast that pointer back to the same concrete type.
    #[inline]
    pub(crate) unsafe fn push_custom(
        label: FrameLabel,
        data: *const (),
        root_collector: RootCollectorFn,
    ) -> Self {
        let parent = FRAME_CHAIN_HEAD.with(|h| h.get());
        let frame = Box::new(EvalFrame {
            parent,
            root_data: data,
            root_collector,
            label,
        });
        // Push: set this frame as the new chain head.
        // The Box provides a stable heap address that won't be invalidated
        // when the guard is moved.
        FRAME_CHAIN_HEAD.with(|h| h.set(&*frame as *const EvalFrame));
        EvalFrameGuard { frame }
    }
}

impl Drop for EvalFrameGuard {
    #[inline]
    fn drop(&mut self) {
        // Pop: restore parent as chain head.
        FRAME_CHAIN_HEAD.with(|h| h.set(self.frame.parent));
    }
}

/// Root collector for `Vec<MettaValue>`.
///
/// # Safety
///
/// `data` must point to a valid `Vec<MettaValue>`.
unsafe fn collect_vec_roots(data: *const (), out: &mut Vec<MettaValue>) {
    let vec = unsafe { &*(data as *const Vec<MettaValue>) };
    out.extend_from_slice(vec);
}

// ============================================================================
// Generic Type Bridge
// ============================================================================

/// A4.2b — bundles the `frame_chain` guard with the typed K-spine
/// [`SuspendedActivationGuard`](crate::backend::eval::cesk::k_spine::SuspendedActivationGuard)
/// so every module/testing `maybe_push_frame` call site migrates to the typed
/// K-spine through this single chokepoint. The K-spine half is recorded only
/// under `gc_mode_is_index()` (so the slab build does ZERO extra work). Both
/// halves pop on drop; they touch different thread-locals, so drop order is
/// immaterial. (A5 collapses this back to a bare `SuspendedActivationGuard`
/// once `frame_chain` is deleted.)
pub struct FrameAndKSpineGuard {
    _frame: EvalFrameGuard,
    _kspine: Option<crate::backend::eval::cesk::k_spine::SuspendedActivationGuard>,
}

/// Push a frame that protects a `Vec<MettaValue>` from GC during nested
/// trampoline calls.
///
/// # Safety
///
/// `data` must point to a valid `Vec<MettaValue>` that outlives the returned
/// guard. Typically, both the Vec and the guard are locals in the same
/// function scope, guaranteeing this.
#[inline]
pub unsafe fn maybe_push_frame<C: EvalContext>(
    label: FrameLabel,
    data: *const Vec<MettaValue>,
) -> Option<FrameAndKSpineGuard> {
    // All contexts now use MettaValue — always push the frame_chain frame.
    let _frame = EvalFrameGuard::push_vec(label, data);
    // A4.2b: also record the typed K-spine `ExprVec` over the SAME `data`
    // pointer (read-only at safepoints). Index-gc only ⇒ byte-identical slab
    // path. SAFETY: `data` outlives the returned guard (caller contract above).
    let _kspine = if crate::backend::models::metta_value::gc_mode_is_index() {
        Some(
            crate::backend::eval::cesk::k_spine::SuspendedActivationGuard::push(
                crate::backend::eval::cesk::k_spine::SuspendedActivation::ExprVec { exprs: data },
            ),
        )
    } else {
        None
    };
    Some(FrameAndKSpineGuard { _frame, _kspine })
}

// ============================================================================
// Root Collection (GC Integration)
// ============================================================================

/// Walk the frame chain, invoking each frame's root collector to gather live
/// `MettaValue` references from all caller frames.
///
/// Called from the trampoline's safepoint code to ensure that values held by
/// callers of nested trampolines are visible to the GC mark phase.
///
/// # Thread Safety
///
/// This function accesses the thread-local `FRAME_CHAIN_HEAD` and follows
/// the chain of `EvalFrame` pointers. All frames are on the current thread's
/// stack, so no synchronization is needed.
pub fn collect_frame_chain_roots(out: &mut Vec<MettaValue>) {
    FRAME_CHAIN_HEAD.with(|h| {
        let mut ptr = h.get();
        while !ptr.is_null() {
            // SAFETY: All frames in the chain are live (on the stack of callers
            // that haven't returned yet). The root_data pointer was valid when
            // the frame was pushed, and the RAII guard ensures it's popped before
            // the data goes out of scope.
            let frame = unsafe { &*ptr };
            unsafe { (frame.root_collector)(frame.root_data, out) };
            ptr = frame.parent;
        }
    });
}

// ============================================================================
// Stack Trace Capture
// ============================================================================

/// Capture the current stack trace from the frame chain.
///
/// Returns frame labels from innermost (most recent) to outermost (root).
pub fn capture_stack_trace() -> Vec<FrameLabel> {
    let mut trace = Vec::new();
    FRAME_CHAIN_HEAD.with(|h| {
        let mut ptr = h.get();
        while !ptr.is_null() {
            let frame = unsafe { &*ptr };
            trace.push(frame.label);
            ptr = frame.parent;
        }
    });
    trace
}

/// Format a stack trace as a human-readable string.
///
/// Returns an empty string if no frames are active.
pub fn format_stack_trace() -> String {
    let trace = capture_stack_trace();
    if trace.is_empty() {
        return String::new();
    }

    let mut out = String::from("\n  in:");
    for (i, label) in trace.iter().enumerate() {
        out.push_str(&format!("\n    #{}: {}", i, label));
    }
    out
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_empty_chain_collects_nothing() {
        // Ensure chain head is null (should be default)
        let mut roots = Vec::new();
        collect_frame_chain_roots(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_single_frame_collects_roots() {
        let f = global_factory();
        let values = vec![f.atom("a"), f.long(42), f.bool(true)];

        let mut roots = Vec::new();
        {
            let _guard = unsafe { EvalFrameGuard::push_vec(FrameLabel::Include, &values) };
            collect_frame_chain_roots(&mut roots);
        }
        // Guard dropped — chain should be empty again
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].as_atom(), Some("a"));
        assert_eq!(roots[1].as_long(), Some(42));
        assert_eq!(roots[2].as_bool(), Some(true));

        // Verify chain is empty after guard drop
        let mut roots2 = Vec::new();
        collect_frame_chain_roots(&mut roots2);
        assert!(roots2.is_empty());
    }

    #[test]
    fn test_custom_frame_collects_roots() {
        struct CustomRoots {
            values: Vec<MettaValue>,
            extra: MettaValue,
        }

        unsafe fn collect_custom_roots(data: *const (), out: &mut Vec<MettaValue>) {
            let roots = unsafe { &*(data as *const CustomRoots) };
            out.extend_from_slice(&roots.values);
            out.push(roots.extra);
        }

        let f = global_factory();
        let roots_data = CustomRoots {
            values: vec![f.atom("queued"), f.long(7)],
            extra: f.bool(false),
        };

        let mut roots = Vec::new();
        {
            let _guard = unsafe {
                EvalFrameGuard::push_custom(
                    FrameLabel::Custom("custom-roots"),
                    &roots_data as *const CustomRoots as *const (),
                    collect_custom_roots,
                )
            };
            collect_frame_chain_roots(&mut roots);
        }

        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].as_atom(), Some("queued"));
        assert_eq!(roots[1].as_long(), Some(7));
        assert_eq!(roots[2].as_bool(), Some(false));
    }

    #[test]
    fn test_nested_frames_collect_all_roots() {
        let f = global_factory();
        let outer_values = vec![f.atom("outer1"), f.atom("outer2")];
        let inner_values = vec![f.atom("inner1")];

        let mut roots = Vec::new();
        {
            let _outer_guard =
                unsafe { EvalFrameGuard::push_vec(FrameLabel::Include, &outer_values) };
            {
                let _inner_guard =
                    unsafe { EvalFrameGuard::push_vec(FrameLabel::Import, &inner_values) };
                collect_frame_chain_roots(&mut roots);
            }
            // Inner guard dropped, outer still active
            let mut roots_after_inner = Vec::new();
            collect_frame_chain_roots(&mut roots_after_inner);
            assert_eq!(roots_after_inner.len(), 2, "only outer roots remain");
        }

        // Both guards dropped
        // roots should have inner + outer = 3 values
        assert_eq!(roots.len(), 3);
        // Inner frame is collected first (it's the chain head)
        assert_eq!(roots[0].as_atom(), Some("inner1"));
        assert_eq!(roots[1].as_atom(), Some("outer1"));
        assert_eq!(roots[2].as_atom(), Some("outer2"));
    }

    #[test]
    fn test_stack_trace_capture() {
        let f = global_factory();
        let v1 = vec![f.unit()];
        let v2 = vec![f.unit()];
        let v3 = vec![f.unit()];

        {
            let _g1 = unsafe { EvalFrameGuard::push_vec(FrameLabel::Eval, &v1) };
            {
                let _g2 = unsafe { EvalFrameGuard::push_vec(FrameLabel::Include, &v2) };
                {
                    let _g3 = unsafe { EvalFrameGuard::push_vec(FrameLabel::Import, &v3) };

                    let trace = capture_stack_trace();
                    assert_eq!(trace.len(), 3);
                    // Innermost first
                    assert!(matches!(trace[0], FrameLabel::Import));
                    assert!(matches!(trace[1], FrameLabel::Include));
                    assert!(matches!(trace[2], FrameLabel::Eval));
                }
            }
        }

        // All guards dropped — empty trace
        let trace = capture_stack_trace();
        assert!(trace.is_empty());
    }

    #[test]
    fn test_format_stack_trace_empty() {
        let trace = format_stack_trace();
        assert!(trace.is_empty());
    }

    #[test]
    fn test_format_stack_trace_nonempty() {
        let f = global_factory();
        let v1 = vec![f.unit()];
        let v2 = vec![f.unit()];

        let _g1 = unsafe { EvalFrameGuard::push_vec(FrameLabel::Include, &v1) };
        let _g2 = unsafe { EvalFrameGuard::push_vec(FrameLabel::AssertEqual, &v2) };

        let trace = format_stack_trace();
        assert!(trace.contains("#0: assertEqual"));
        assert!(trace.contains("#1: include"));
    }

    #[test]
    fn test_maybe_push_frame_metta_value() {
        use crate::backend::eval::trampoline::StaticEvalContext;

        let f = global_factory();
        let values: Vec<MettaValue> = vec![f.atom("test"), f.long(99)];

        let guard = unsafe { maybe_push_frame::<StaticEvalContext>(FrameLabel::Eval, &values) };
        assert!(guard.is_some(), "should push frame for MettaValue type");

        let mut roots = Vec::new();
        collect_frame_chain_roots(&mut roots);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].as_atom(), Some("test"));
        assert_eq!(roots[1].as_long(), Some(99));

        drop(guard);

        let mut roots2 = Vec::new();
        collect_frame_chain_roots(&mut roots2);
        assert!(roots2.is_empty());
    }

    #[test]
    fn test_frame_label_display() {
        assert_eq!(format!("{}", FrameLabel::Include), "include");
        assert_eq!(format!("{}", FrameLabel::Import), "import!");
        assert_eq!(format!("{}", FrameLabel::AssertEqual), "assertEqual");
        assert_eq!(format!("{}", FrameLabel::Eval), "eval");
        assert_eq!(format!("{}", FrameLabel::Custom("my-op")), "my-op");
    }
}
