//! Binding types for JIT pattern variables.
//!
//! This module defines types for managing variable bindings in JIT-compiled code:
//! - [`JitBindingEntry`]: Individual variable binding
//! - [`JitBindingFrame`]: Stack frame of bindings for a scope

use super::value::JitValue;

// =============================================================================
// JitBindingEntry - Individual Binding Entry
// =============================================================================

/// A single binding entry for pattern variables.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitBindingEntry {
    /// Index into the constant pool for the variable name
    pub name_idx: u32,
    /// The bound value (NaN-boxed)
    pub value: JitValue,
}

impl JitBindingEntry {
    /// Create a new binding entry
    #[inline]
    pub fn new(name_idx: u32, value: JitValue) -> Self {
        Self { name_idx, value }
    }
}

// =============================================================================
// JitBindingFrame - Stack of Bindings for a Scope
// =============================================================================

/// A frame of bindings for pattern variables at a particular scope level.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Mirrors the VM's BindingFrame structure.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitBindingFrame {
    /// Pointer to array of binding entries
    pub entries: *mut JitBindingEntry,
    /// Number of entries in this frame
    pub entries_count: usize,
    /// Capacity of the entries array
    pub entries_cap: usize,
    /// Scope depth (0 = root frame)
    pub scope_depth: u32,
}

impl Default for JitBindingFrame {
    fn default() -> Self {
        Self {
            entries: std::ptr::null_mut(),
            entries_count: 0,
            entries_cap: 0,
            scope_depth: 0,
        }
    }
}

impl JitBindingFrame {
    /// Create a new empty binding frame
    pub fn new(scope_depth: u32) -> Self {
        Self {
            entries: std::ptr::null_mut(),
            entries_count: 0,
            entries_cap: 0,
            scope_depth,
        }
    }

    /// Create a binding frame with pre-allocated capacity
    ///
    /// # Safety
    /// The caller must ensure the allocated memory is valid for the frame's lifetime
    pub unsafe fn with_capacity(scope_depth: u32, capacity: usize) -> Self {
        if capacity == 0 {
            return Self::new(scope_depth);
        }
        let layout = std::alloc::Layout::array::<JitBindingEntry>(capacity)
            .expect("Layout calculation failed");
        let entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
        Self {
            entries,
            entries_count: 0,
            entries_cap: capacity,
            scope_depth,
        }
    }
}
