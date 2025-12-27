//! Core helper functions for JIT runtime operations
//!
//! This module contains low-level helper functions used throughout the JIT runtime:
//! - NaN-boxing helpers (extract_long_signed, box_long)
//! - MettaValue <-> JitValue conversion (metta_to_jit, metta_to_jit_tracked)
//! - Error creation helpers (make_jit_error, make_jit_error_with_details)

use crate::backend::bytecode::jit::types::{
    JitContext, JitValue, PAYLOAD_MASK, TAG_HEAP, TAG_LONG,
};
use crate::backend::models::MettaValue;
use std::sync::Arc;

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
    match val {
        MettaValue::Long(n) => JitValue::from_long(*n),
        MettaValue::Bool(b) => JitValue::from_bool(*b),
        MettaValue::Nil => JitValue::nil(),
        MettaValue::Unit => JitValue::unit(),
        // For complex types, box and return heap pointer
        other => {
            let boxed = Box::new(other.clone());
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
    match val {
        MettaValue::Long(n) => JitValue::from_long(*n),
        MettaValue::Bool(b) => JitValue::from_bool(*b),
        MettaValue::Nil => JitValue::nil(),
        MettaValue::Unit => JitValue::unit(),
        // For complex types, box, track, and return heap pointer
        other => {
            let boxed = Box::new(other.clone());
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
    let error_val = MettaValue::Error(msg.to_string(), Arc::new(MettaValue::Nil));
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    (TAG_HEAP << 48) | (ptr as u64 & PAYLOAD_MASK)
}

/// Helper to create an error JitValue with message and details
///
/// Creates a heap-allocated Error value with additional detail information
/// and returns it as a NaN-boxed pointer.
pub fn make_jit_error_with_details(msg: &str, details: &str) -> u64 {
    let error_val = MettaValue::Error(
        msg.to_string(),
        Arc::new(MettaValue::Atom(details.to_string())),
    );
    let boxed = Box::new(error_val);
    let ptr = Box::into_raw(boxed);
    (TAG_HEAP << 48) | (ptr as u64 & PAYLOAD_MASK)
}
