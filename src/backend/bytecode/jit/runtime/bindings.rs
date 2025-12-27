//! Binding/Environment runtime functions for JIT compilation
//!
//! This module provides FFI-callable binding operations:
//! - load_binding - Load a binding from the current environment
//! - store_binding - Store a binding in the current frame
//! - has_binding - Check if a binding exists
//! - clear_bindings - Clear bindings in current frame
//! - push/pop_binding_frame - Scope management
//! - fork/restore_bindings - Nondeterministic backtracking support

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, JitBindingEntry, JitBindingFrame,
    TAG_BOOL, TAG_NIL,
};

// =============================================================================
// Phase A: Binding/Environment Runtime Functions
// =============================================================================

/// Load a binding from the current binding environment.
///
/// Searches binding frames from innermost to outermost looking for a binding
/// with the given name index. The name is looked up in the constant pool.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// The bound value (NaN-boxed), or signals bailout if binding not found.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_binding(
    ctx: *mut JitContext,
    name_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return TAG_NIL,
    };

    let name_idx_u32 = name_idx as u32;

    // Search binding frames from innermost to outermost
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        // Start from the innermost frame (highest index)
        for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
            let frame = &*ctx_ref.binding_frames.add(frame_idx);

            // Search entries in this frame
            if frame.entries_count > 0 && !frame.entries.is_null() {
                for entry_idx in 0..frame.entries_count {
                    let entry = &*frame.entries.add(entry_idx);
                    if entry.name_idx == name_idx_u32 {
                        return entry.value.to_bits();
                    }
                }
            }
        }
    }

    // Binding not found - signal bailout
    ctx_ref.bailout = true;
    ctx_ref.bailout_ip = ip as usize;
    ctx_ref.bailout_reason = JitBailoutReason::InvalidBinding;

    TAG_NIL
}

/// Store a binding in the current (innermost) binding frame.
///
/// Creates or updates a binding in the current scope.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
/// * `value` - The value to bind (NaN-boxed)
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// 0 on success, -1 on error (no binding frames).
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_store_binding(
    ctx: *mut JitContext,
    name_idx: u64,
    value: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames_count == 0 || ctx_ref.binding_frames.is_null() {
        return -1; // No binding frames
    }

    let name_idx_u32 = name_idx as u32;
    let jit_value = JitValue::from_raw(value);

    // Get the current (innermost) frame
    let frame = &mut *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count - 1);

    // Check if binding already exists in this frame
    if frame.entries_count > 0 && !frame.entries.is_null() {
        for entry_idx in 0..frame.entries_count {
            let entry = &mut *frame.entries.add(entry_idx);
            if entry.name_idx == name_idx_u32 {
                // Update existing binding
                entry.value = jit_value;
                return 0;
            }
        }
    }

    // Add new binding to this frame
    // Ensure capacity
    if frame.entries.is_null() || frame.entries_count >= frame.entries_cap {
        // Need to grow or allocate
        let new_cap = if frame.entries_cap == 0 { 8 } else { frame.entries_cap * 2 };
        let layout = std::alloc::Layout::array::<JitBindingEntry>(new_cap)
            .expect("Layout calculation failed");
        let new_entries = std::alloc::alloc(layout) as *mut JitBindingEntry;

        if new_entries.is_null() {
            return -3; // Allocation failed
        }

        // Copy existing entries if any
        if !frame.entries.is_null() && frame.entries_count > 0 {
            std::ptr::copy_nonoverlapping(frame.entries, new_entries, frame.entries_count);
            // Free old allocation
            let old_layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                .expect("Layout calculation failed");
            std::alloc::dealloc(frame.entries as *mut u8, old_layout);
        }

        frame.entries = new_entries;
        frame.entries_cap = new_cap;
    }

    // Add the new entry
    let entry = &mut *frame.entries.add(frame.entries_count);
    entry.name_idx = name_idx_u32;
    entry.value = jit_value;
    frame.entries_count += 1;

    0 // Success
}

/// Check if a binding exists in any binding frame.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `name_idx` - Index of the variable name in the constant pool
///
/// # Returns
/// NaN-boxed Bool: true if binding exists, false otherwise.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_has_binding(
    ctx: *const JitContext,
    name_idx: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return TAG_BOOL, // false
    };

    let name_idx_u32 = name_idx as u32;

    // Search binding frames from innermost to outermost
    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        for frame_idx in (0..ctx_ref.binding_frames_count).rev() {
            let frame = &*ctx_ref.binding_frames.add(frame_idx);

            if frame.entries_count > 0 && !frame.entries.is_null() {
                for entry_idx in 0..frame.entries_count {
                    let entry = &*frame.entries.add(entry_idx);
                    if entry.name_idx == name_idx_u32 {
                        return TAG_BOOL | 1; // true
                    }
                }
            }
        }
    }

    TAG_BOOL // false
}

/// Clear all bindings in the current (innermost) binding frame.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_clear_bindings(ctx: *mut JitContext) {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return,
    };

    if ctx_ref.binding_frames_count > 0 && !ctx_ref.binding_frames.is_null() {
        let frame = &mut *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count - 1);
        frame.entries_count = 0;
        // Note: we don't deallocate entries array, just clear count for reuse
    }
}

/// Push a new binding frame onto the binding frame stack.
///
/// Creates a new scope level for bindings.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// 0 on success, -1 on stack overflow.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_binding_frame(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count >= ctx_ref.binding_frames_cap {
        return -1; // No capacity or overflow
    }

    // Create new frame with incremented scope depth
    let scope_depth = ctx_ref.binding_frames_count as u32;
    let new_frame = JitBindingFrame::new(scope_depth);
    *ctx_ref.binding_frames.add(ctx_ref.binding_frames_count) = new_frame;
    ctx_ref.binding_frames_count += 1;

    0 // Success
}

/// Pop the current binding frame from the binding frame stack.
///
/// Returns to the previous scope level.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// 0 on success, -1 if trying to pop the root frame.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pop_binding_frame(ctx: *mut JitContext) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -2,
    };

    if ctx_ref.binding_frames_count <= 1 {
        return -1; // Cannot pop root frame
    }

    // Get the frame being popped
    let frame_idx = ctx_ref.binding_frames_count - 1;
    let frame = &mut *ctx_ref.binding_frames.add(frame_idx);

    // Free entries array if allocated
    if !frame.entries.is_null() && frame.entries_cap > 0 {
        let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
            .expect("Layout calculation failed");
        std::alloc::dealloc(frame.entries as *mut u8, layout);
        frame.entries = std::ptr::null_mut();
        frame.entries_count = 0;
        frame.entries_cap = 0;
    }

    ctx_ref.binding_frames_count -= 1;
    0 // Success
}

// =============================================================================
// Space Ops Phase 4: Binding Frame Forking for Nondeterminism
// =============================================================================

/// Saved binding frames snapshot for nondeterministic restoration.
///
/// This structure holds a deep copy of binding frames that can be restored
/// during backtracking. It is allocated on the heap and the pointer is stored
/// in JitAlternative::payload3 for SpaceMatch alternatives.
#[repr(C)]
pub struct JitSavedBindings {
    /// Array of saved binding frames
    pub frames: *mut JitBindingFrame,
    /// Number of frames in the array
    pub frames_count: usize,
    /// Total allocated capacity
    pub frames_cap: usize,
}

impl JitSavedBindings {
    /// Create a new empty saved bindings structure
    pub fn new() -> Self {
        Self {
            frames: std::ptr::null_mut(),
            frames_count: 0,
            frames_cap: 0,
        }
    }

    /// Check if this snapshot contains any frames
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.frames_count == 0 || self.frames.is_null()
    }
}

impl Default for JitSavedBindings {
    fn default() -> Self {
        Self::new()
    }
}

/// Fork (deep copy) all current binding frames for branch isolation.
///
/// Creates a complete snapshot of the current binding state that can be
/// restored during backtracking. This is essential for nondeterministic
/// operations like space matching where each alternative needs its own
/// isolated binding environment.
///
/// # Arguments
/// * `ctx` - JIT context pointer
///
/// # Returns
/// Pointer to heap-allocated JitSavedBindings on success, null on failure.
/// The caller is responsible for freeing this via `jit_runtime_free_saved_bindings`.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_fork_bindings(ctx: *const JitContext) -> *mut JitSavedBindings {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };

    // If no binding frames, return empty snapshot
    if ctx_ref.binding_frames.is_null() || ctx_ref.binding_frames_count == 0 {
        let saved = Box::new(JitSavedBindings::new());
        return Box::into_raw(saved);
    }

    let frame_count = ctx_ref.binding_frames_count;

    // Allocate frame array
    let frames_layout = std::alloc::Layout::array::<JitBindingFrame>(frame_count)
        .expect("Layout calculation failed");
    let frames = std::alloc::alloc(frames_layout) as *mut JitBindingFrame;
    if frames.is_null() {
        return std::ptr::null_mut();
    }

    // Deep copy each frame
    for i in 0..frame_count {
        let src_frame = &*ctx_ref.binding_frames.add(i);
        let dst_frame = &mut *frames.add(i);

        dst_frame.scope_depth = src_frame.scope_depth;

        if src_frame.entries.is_null() || src_frame.entries_count == 0 {
            // Empty frame
            dst_frame.entries = std::ptr::null_mut();
            dst_frame.entries_count = 0;
            dst_frame.entries_cap = 0;
        } else {
            // Allocate and copy entries
            let entries_layout = std::alloc::Layout::array::<JitBindingEntry>(src_frame.entries_count)
                .expect("Layout calculation failed");
            let entries = std::alloc::alloc(entries_layout) as *mut JitBindingEntry;
            if entries.is_null() {
                // Cleanup already allocated frames on failure
                for j in 0..i {
                    let cleanup_frame = &*frames.add(j);
                    if !cleanup_frame.entries.is_null() && cleanup_frame.entries_cap > 0 {
                        let cleanup_layout = std::alloc::Layout::array::<JitBindingEntry>(cleanup_frame.entries_cap)
                            .expect("Layout calculation failed");
                        std::alloc::dealloc(cleanup_frame.entries as *mut u8, cleanup_layout);
                    }
                }
                std::alloc::dealloc(frames as *mut u8, frames_layout);
                return std::ptr::null_mut();
            }

            // Copy entry data
            std::ptr::copy_nonoverlapping(src_frame.entries, entries, src_frame.entries_count);
            dst_frame.entries = entries;
            dst_frame.entries_count = src_frame.entries_count;
            dst_frame.entries_cap = src_frame.entries_count; // Exact fit for snapshot
        }
    }

    // Create and return the saved bindings structure
    let saved = Box::new(JitSavedBindings {
        frames,
        frames_count: frame_count,
        frames_cap: frame_count,
    });
    Box::into_raw(saved)
}

/// Restore binding frames from a saved snapshot.
///
/// Replaces the current binding state with the saved snapshot. This is used
/// during backtracking to restore the binding environment to a previous state.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable)
/// * `saved` - Pointer to saved bindings from `jit_runtime_fork_bindings`
/// * `consume` - If true, frees the saved bindings after restoration
///
/// # Returns
/// 0 on success, -1 on error.
///
/// # Safety
/// Both pointers must be valid. The saved pointer must come from
/// `jit_runtime_fork_bindings`.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_restore_bindings(
    ctx: *mut JitContext,
    saved: *mut JitSavedBindings,
    consume: bool,
) -> i64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return -1,
    };

    let saved_ref = match saved.as_ref() {
        Some(s) => s,
        None => return -1,
    };

    // First, clear current binding frames (free entries)
    if !ctx_ref.binding_frames.is_null() {
        for i in 0..ctx_ref.binding_frames_count {
            let frame = &mut *ctx_ref.binding_frames.add(i);
            if !frame.entries.is_null() && frame.entries_cap > 0 {
                let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, layout);
                frame.entries = std::ptr::null_mut();
                frame.entries_count = 0;
                frame.entries_cap = 0;
            }
        }
    }

    // If saved is empty, just clear the count
    if saved_ref.is_empty() {
        ctx_ref.binding_frames_count = 0;
        if consume {
            let _ = Box::from_raw(saved);
        }
        return 0;
    }

    // Check capacity
    if saved_ref.frames_count > ctx_ref.binding_frames_cap {
        // Not enough capacity - this shouldn't happen in normal use
        // but we handle it gracefully
        if consume {
            jit_runtime_free_saved_bindings(saved);
        }
        return -2;
    }

    // Copy frames from saved to context
    for i in 0..saved_ref.frames_count {
        let src_frame = &*saved_ref.frames.add(i);
        let dst_frame = &mut *ctx_ref.binding_frames.add(i);

        dst_frame.scope_depth = src_frame.scope_depth;

        if src_frame.entries.is_null() || src_frame.entries_count == 0 {
            dst_frame.entries = std::ptr::null_mut();
            dst_frame.entries_count = 0;
            dst_frame.entries_cap = 0;
        } else {
            // Allocate and copy entries
            let entries_layout = std::alloc::Layout::array::<JitBindingEntry>(src_frame.entries_count)
                .expect("Layout calculation failed");
            let entries = std::alloc::alloc(entries_layout) as *mut JitBindingEntry;
            if entries.is_null() {
                ctx_ref.binding_frames_count = i;
                if consume {
                    jit_runtime_free_saved_bindings(saved);
                }
                return -3;
            }
            std::ptr::copy_nonoverlapping(src_frame.entries, entries, src_frame.entries_count);
            dst_frame.entries = entries;
            dst_frame.entries_count = src_frame.entries_count;
            dst_frame.entries_cap = src_frame.entries_count;
        }
    }

    ctx_ref.binding_frames_count = saved_ref.frames_count;

    // Consume the saved bindings if requested
    if consume {
        jit_runtime_free_saved_bindings(saved);
    }

    0 // Success
}

/// Free saved bindings without restoring them.
///
/// Used when discarding a choice point or when the alternative was never taken.
///
/// # Arguments
/// * `saved` - Pointer to saved bindings from `jit_runtime_fork_bindings`
///
/// # Safety
/// The pointer must come from `jit_runtime_fork_bindings`.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_free_saved_bindings(saved: *mut JitSavedBindings) {
    if saved.is_null() {
        return;
    }

    let saved_box = Box::from_raw(saved);

    // Free all entry arrays in frames
    if !saved_box.frames.is_null() {
        for i in 0..saved_box.frames_count {
            let frame = &*saved_box.frames.add(i);
            if !frame.entries.is_null() && frame.entries_cap > 0 {
                let layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, layout);
            }
        }

        // Free frames array
        if saved_box.frames_cap > 0 {
            let frames_layout = std::alloc::Layout::array::<JitBindingFrame>(saved_box.frames_cap)
                .expect("Layout calculation failed");
            std::alloc::dealloc(saved_box.frames as *mut u8, frames_layout);
        }
    }

    // Box is dropped here, freeing the JitSavedBindings struct
}

/// Get the size of saved bindings (for debugging/metrics).
///
/// # Returns
/// Total number of binding entries across all frames, or 0 if invalid.
///
/// # Safety
/// The pointer must come from `jit_runtime_fork_bindings` or be null.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_saved_bindings_size(saved: *const JitSavedBindings) -> usize {
    let saved_ref = match saved.as_ref() {
        Some(s) => s,
        None => return 0,
    };

    if saved_ref.is_empty() {
        return 0;
    }

    let mut total = 0;
    for i in 0..saved_ref.frames_count {
        let frame = &*saved_ref.frames.add(i);
        total += frame.entries_count;
    }
    total
}
