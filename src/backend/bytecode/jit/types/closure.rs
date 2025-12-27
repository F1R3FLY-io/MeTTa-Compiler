//! Lambda closure representation for JIT execution.
//!
//! This module defines the [`JitClosure`] type for lambda expressions
//! that capture their environment.

use super::binding::JitBindingFrame;

// =============================================================================
// JitClosure - Lambda Closure Representation
// =============================================================================

/// A closure for lambda expressions in JIT-compiled code.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Closures capture their environment at creation time.
#[repr(C)]
#[derive(Debug)]
pub struct JitClosure {
    /// Number of parameters expected
    pub param_count: u32,
    /// Pointer to parameter name indices (into constant pool)
    pub param_names: *const u32,
    /// Pointer to the bytecode body chunk
    pub body_chunk: *const (),
    /// Captured binding frames - copy of bindings at closure creation
    pub captured_frames: *mut JitBindingFrame,
    /// Number of captured frames
    pub captured_frame_count: usize,
}

impl Default for JitClosure {
    fn default() -> Self {
        Self {
            param_count: 0,
            param_names: std::ptr::null(),
            body_chunk: std::ptr::null(),
            captured_frames: std::ptr::null_mut(),
            captured_frame_count: 0,
        }
    }
}

impl JitClosure {
    /// Create a new closure with no captured environment
    #[inline]
    pub fn new(param_count: u32, body_chunk: *const ()) -> Self {
        Self {
            param_count,
            param_names: std::ptr::null(),
            body_chunk,
            captured_frames: std::ptr::null_mut(),
            captured_frame_count: 0,
        }
    }

    /// Create a closure with captured binding frames
    ///
    /// # Safety
    /// The caller must ensure `captured_frames` points to valid JitBindingFrame data
    /// that will outlive this closure.
    #[inline]
    pub unsafe fn with_captured_env(
        param_count: u32,
        body_chunk: *const (),
        captured_frames: *mut JitBindingFrame,
        captured_frame_count: usize,
    ) -> Self {
        Self {
            param_count,
            param_names: std::ptr::null(),
            body_chunk,
            captured_frames,
            captured_frame_count,
        }
    }
}
