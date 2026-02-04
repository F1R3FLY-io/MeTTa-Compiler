//! Core helper functions for JIT runtime operations
//!
//! This module contains low-level helper functions used throughout the JIT runtime:
//! - NaN-boxing helpers (extract_long_signed, box_long)
//! - MettaValue <-> JitValue conversion (metta_to_jit, metta_to_jit_tracked)
//! - Generic value conversion (value_to_jit_generic, jit_to_value_generic)
//! - Error creation helpers (make_jit_error, make_jit_error_with_details)
//!
//! ## Zero-Conversion Support
//!
//! The generic conversion functions support both heap (`MettaValue`) and arena
//! (`ArenaValue<'static>`) modes without type conversion overhead. The key insight
//! is that JitValue's NaN-boxing stores 48-bit pointers, and both MettaValue and
//! ArenaValueInner pointers fit in 48 bits.

use crate::backend::bytecode::jit::types::{
    JitContext, JitValue, JitValueMode, PAYLOAD_MASK, TAG_HEAP, TAG_LONG,
};
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait};

// =============================================================================
// NaN-Boxing Helpers
// =============================================================================

/// Extract signed 64-bit integer from NaN-boxed Long
///
/// The payload is in the lower 48 bits. We need to sign-extend from 48 bits
/// to recover negative values correctly.
#[inline]
pub fn extract_long_signed(val: u64) -> i64 {
    let payload = val & PAYLOAD_MASK;
    // Sign extend from 48 bits
    const SIGN_BIT: u64 = 0x0000_8000_0000_0000;
    if payload & SIGN_BIT != 0 {
        (payload | 0xFFFF_0000_0000_0000) as i64
    } else {
        payload as i64
    }
}

/// Box a signed 64-bit integer as NaN-boxed Long
///
/// Creates a NaN-boxed value with the Long tag and the integer payload.
#[inline]
pub fn box_long(n: i64) -> u64 {
    TAG_LONG | ((n as u64) & PAYLOAD_MASK)
}

// =============================================================================
// MettaValue <-> JitValue Conversion
// =============================================================================

/// Convert a MettaValue to a JitValue
///
/// For simple types (Long, Bool, Nil, Unit), creates a NaN-boxed value directly.
/// For complex types (SExpr, Atom, String, etc.), boxes the value and returns a heap pointer.
pub fn metta_to_jit(val: &MettaValue) -> JitValue {
    match val.inner() {
        MettaValueInner::Long(n) => JitValue::from_long(*n),
        MettaValueInner::Bool(b) => JitValue::from_bool(*b),
        MettaValueInner::Nil => JitValue::nil(),
        MettaValueInner::Unit => JitValue::unit(),
        // For complex types, box and return heap pointer
        _ => {
            let boxed = Box::new(val.clone());
            JitValue::from_heap_ptr(Box::into_raw(boxed))
        }
    }
}

/// Convert a MettaValue to a JitValue with heap tracking.
///
/// For simple types (Long, Bool, Nil, Unit), creates a NaN-boxed value directly.
/// For complex types (SExpr, Atom, String, etc.), boxes the value, tracks the
/// allocation in the context, and returns a heap pointer.
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext (or null to disable tracking)
pub unsafe fn metta_to_jit_tracked(val: &MettaValue, ctx: *mut JitContext) -> JitValue {
    match val.inner() {
        MettaValueInner::Long(n) => JitValue::from_long(*n),
        MettaValueInner::Bool(b) => JitValue::from_bool(*b),
        MettaValueInner::Nil => JitValue::nil(),
        MettaValueInner::Unit => JitValue::unit(),
        // For complex types, box, track, and return heap pointer
        _ => {
            let boxed = Box::new(val.clone());
            let ptr = Box::into_raw(boxed);
            // Track the allocation if context has heap tracking enabled
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.track_heap_allocation(ptr);
            }
            JitValue::from_heap_ptr(ptr)
        }
    }
}

// =============================================================================
// Error Creation Helpers
// =============================================================================

/// Helper to create an error JitValue with a message
///
/// Creates a heap-allocated Error value and returns it as a NaN-boxed pointer.
pub fn make_jit_error(msg: &str) -> u64 {
    let error_val = MettaValue::Error(msg.to_string(), MettaValue::Nil());
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    ((TAG_HEAP as u64) << 48) | (ptr as u64 & PAYLOAD_MASK)
}

/// Helper to create an error JitValue with message and details
///
/// Creates a heap-allocated Error value with additional detail information
/// and returns it as a NaN-boxed pointer.
pub fn make_jit_error_with_details(msg: &str, details: &str) -> u64 {
    let error_val = MettaValue::Error(msg.to_string(), MettaValue::Atom(details.to_string()));
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    ((TAG_HEAP as u64) << 48) | (ptr as u64 & PAYLOAD_MASK)
}

// =============================================================================
// Generic Value Conversion (Zero-Conversion Support)
// =============================================================================

/// Convert a generic value to JitValue based on mode.
///
/// For primitive types (Long, Bool, Nil, Unit), creates a NaN-boxed value directly.
/// For complex types, stores the pointer in the payload based on mode:
/// - Heap mode: Creates a Box and stores the raw pointer
/// - Arena mode: Stores the value pointer directly (arena values are Copy)
///
/// # Type Parameters
/// - `V`: The value type implementing `MettaValueTrait`
///
/// # Arguments
/// - `val`: Reference to the value to convert
/// - `mode`: The JIT value mode (Heap or Arena)
///
/// # Returns
/// A NaN-boxed `JitValue`
pub fn value_to_jit_generic<V>(val: &V, mode: JitValueMode) -> JitValue
where
    V: MettaValueTrait + Clone,
{
    // Handle primitive types (same for both modes)
    if let Some(n) = val.as_long() {
        return JitValue::from_long(n);
    }
    if let Some(b) = val.as_bool() {
        return JitValue::from_bool(b);
    }
    if val.is_nil() {
        return JitValue::nil();
    }
    if val.is_unit() {
        return JitValue::unit();
    }

    // Complex types - store pointer based on mode
    match mode {
        JitValueMode::Heap => {
            // Heap mode: Box the value and store raw pointer
            // The Box will be reconstructed and dropped during cleanup
            let boxed = Box::new(val.clone());
            let ptr = Box::into_raw(boxed) as *const ();
            JitValue::from_raw(TAG_HEAP | (ptr as u64 & PAYLOAD_MASK))
        }
        JitValueMode::Arena => {
            // Arena mode: Store the value pointer directly
            // ArenaValue is Copy, so the pointer is just a reference
            // The arena handles memory management, no cleanup needed
            let ptr = val as *const V as *const ();
            JitValue::from_raw(TAG_HEAP | (ptr as u64 & PAYLOAD_MASK))
        }
    }
}

/// Convert a JitValue back to a generic value using a factory.
///
/// # Safety
/// - For heap mode, the pointer must point to a valid `Box<V>` allocation
/// - For arena mode, the pointer must point to a valid arena-allocated value
///
/// # Type Parameters
/// - `V`: The value type implementing `MettaValueTrait`
/// - `F`: The factory type for creating values
///
/// # Arguments
/// - `jit_val`: The NaN-boxed JitValue to convert
/// - `factory`: Factory for creating values (used for primitives)
///
/// # Returns
/// The reconstructed value
pub unsafe fn jit_to_value_generic<V, F>(jit_val: JitValue, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    use crate::backend::bytecode::jit::types::{TAG_BOOL, TAG_NIL, TAG_UNIT};

    match jit_val.tag() {
        TAG_LONG => factory.long(jit_val.as_long()),
        TAG_BOOL => factory.bool(jit_val.as_bool()),
        TAG_NIL => factory.nil(),
        TAG_UNIT => factory.unit(),
        TAG_HEAP => {
            let ptr = (jit_val.to_bits() & PAYLOAD_MASK) as *const V;
            debug_assert!(
                !ptr.is_null(),
                "jit_to_value_generic: Null heap pointer"
            );
            // Clone the value - works for both heap (Arc clone) and arena (Copy)
            (*ptr).clone()
        }
        _ => {
            // Unknown tag - return nil as fallback
            debug_assert!(
                false,
                "jit_to_value_generic: Unknown tag {:#x}",
                jit_val.tag()
            );
            factory.nil()
        }
    }
}

/// Convert a generic value to JitValue with context-based heap tracking.
///
/// For heap mode, this tracks the allocation in the context's heap tracker
/// so it can be cleaned up when JIT execution completes.
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext (or null to disable tracking)
pub unsafe fn value_to_jit_tracked_generic<V>(val: &V, ctx: *mut JitContext) -> JitValue
where
    V: MettaValueTrait + Clone,
{
    // Get mode from context (default to Heap if null)
    let mode = if ctx.is_null() {
        JitValueMode::Heap
    } else {
        (*ctx).value_mode()
    };

    // Handle primitive types (no tracking needed)
    if let Some(n) = val.as_long() {
        return JitValue::from_long(n);
    }
    if let Some(b) = val.as_bool() {
        return JitValue::from_bool(b);
    }
    if val.is_nil() {
        return JitValue::nil();
    }
    if val.is_unit() {
        return JitValue::unit();
    }

    // Complex types - store pointer based on mode
    match mode {
        JitValueMode::Heap => {
            // Heap mode: Box the value and track for cleanup
            let boxed = Box::new(val.clone());
            let ptr = Box::into_raw(boxed);

            // Track allocation if context has heap tracking enabled
            if let Some(ctx_ref) = ctx.as_mut() {
                // Note: We're storing a V* but tracking as MettaValue*
                // This works because we only care about the raw pointer for cleanup
                ctx_ref.track_heap_allocation(ptr as *mut MettaValue);
            }

            JitValue::from_raw(TAG_HEAP | (ptr as u64 & PAYLOAD_MASK))
        }
        JitValueMode::Arena => {
            // Arena mode: Store pointer directly, no tracking needed
            let ptr = val as *const V as *const ();
            JitValue::from_raw(TAG_HEAP | (ptr as u64 & PAYLOAD_MASK))
        }
    }
}

// =============================================================================
// Mode-Specific Helpers
// =============================================================================

/// Check if a JitContext is in arena mode.
///
/// Returns false if the context pointer is null.
#[inline]
pub fn is_arena_mode(ctx: *const JitContext) -> bool {
    if ctx.is_null() {
        false
    } else {
        unsafe { (*ctx).is_arena_mode() }
    }
}

/// Get the value mode from a JitContext.
///
/// Returns Heap if the context pointer is null.
#[inline]
pub fn get_value_mode(ctx: *const JitContext) -> JitValueMode {
    if ctx.is_null() {
        JitValueMode::Heap
    } else {
        unsafe { (*ctx).value_mode() }
    }
}
