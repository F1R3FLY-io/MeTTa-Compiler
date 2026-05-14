//! Core helper functions for JIT runtime operations
//!
//! This module contains low-level helper functions used throughout the JIT runtime:
//! - NaN-boxing helpers (extract_long_signed, box_long)
//! - MettaValue <-> JitValue conversion (metta_to_jit)
//! - Generic value conversion (value_to_jit_generic, jit_to_value_generic)
//! - Error creation helpers (make_jit_error, make_jit_error_with_details)
//!
//! ## Pointer Semantics
//!
//! TAG_PTR payloads store `*const MettaValueInner` — pointers to slab-allocated
//! inner data managed by the GC. No Box allocations are needed since the inner
//! data has 'static lifetime.

use crate::backend::bytecode::jit::types::{JitValue, PAYLOAD_MASK, TAG_LONG, TAG_PTR};
use crate::backend::models::{
    MettaValue, MettaValueFactory, MettaValueInner, MettaValueTrait, ValueView,
};

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

/// Convert a MettaValue to a JitValue.
///
/// For simple types (Long, Bool, Unit), creates a NaN-boxed value directly.
/// For complex types, stores the inner pointer (slab-allocated, 'static).
pub fn metta_to_jit(val: &MettaValue) -> JitValue {
    match val.view() {
        ValueView::Long(n) => JitValue::from_long(n),
        ValueView::Bool(b) => JitValue::from_bool(b),
        ValueView::Unit => JitValue::unit(),
        // Store pointer to slab-allocated inner data (no Box needed)
        _ => JitValue::from_inner_ptr(val.inner_ptr()),
    }
}

// =============================================================================
// Error Creation Helpers
// =============================================================================

/// Helper to create an error JitValue with a message.
///
/// Creates a slab-allocated Error value and returns it as a NaN-boxed TAG_PTR.
pub fn make_jit_error(msg: &str) -> u64 {
    let error_val = MettaValue::Error(MettaValue::Unit(), MettaValue::String(msg));
    TAG_PTR | (error_val.inner_ptr() as u64 & PAYLOAD_MASK)
}

/// Helper to create an error JitValue with message and details.
///
/// Creates a slab-allocated Error value and returns it as a NaN-boxed TAG_PTR.
pub fn make_jit_error_with_details(msg: &str, details: &str) -> u64 {
    let error_val = MettaValue::Error(MettaValue::Atom(details), MettaValue::String(msg));
    TAG_PTR | (error_val.inner_ptr() as u64 & PAYLOAD_MASK)
}

// =============================================================================
// Generic Value Conversion
// =============================================================================

/// Convert a generic value to a NaN-boxed JitValue.
///
/// For primitive types (Long, Bool, Unit), creates a NaN-boxed value directly.
/// For complex types, stores a pointer to the slab-allocated MettaValueInner.
///
/// # Type Parameters
/// - `V`: The value type implementing `MettaValueTrait`
///
/// # Arguments
/// - `val`: Reference to the value to convert
///
/// # Returns
/// A NaN-boxed `JitValue`
#[inline]
pub fn value_to_jit_generic<V>(val: &V) -> JitValue
where
    V: MettaValueTrait + Clone,
{
    if let Some(n) = val.as_long() {
        return JitValue::from_long(n);
    }
    if let Some(b) = val.as_bool() {
        return JitValue::from_bool(b);
    }
    if val.is_unit() {
        return JitValue::unit();
    }
    if val.is_empty() {
        return JitValue::empty();
    }

    // Complex types: store a pointer to the slab-allocated MettaValueInner.
    // MettaValue.inner_ref() is &'static MettaValueInner, so inner_ptr() gives
    // a persistent pointer that survives across function returns.
    // The GC handles memory management — no cleanup needed here.
    let ptr = val.inner_ptr();
    JitValue::from_inner_ptr(ptr)
}

/// Convert a JitValue back to a generic value using a factory.
///
/// # Safety
/// - TAG_PTR payloads must point to valid slab-allocated MettaValueInner data
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
    use crate::backend::bytecode::jit::types::{TAG_BOOL, TAG_EMPTY, TAG_UNIT};

    match jit_val.tag() {
        TAG_LONG => factory.long(jit_val.as_long()),
        TAG_BOOL => factory.bool(jit_val.as_bool()),
        TAG_UNIT => factory.unit(),
        TAG_EMPTY => factory.empty(),
        TAG_PTR => {
            let ptr = (jit_val.to_bits() & PAYLOAD_MASK) as *const MettaValueInner;
            debug_assert!(!ptr.is_null(), "jit_to_value_generic: Null inner pointer");
            // Reconstruct value from slab-allocated inner pointer
            V::from_inner_ptr(ptr)
        }
        _ => {
            // Unknown tag - return unit as fallback
            debug_assert!(
                false,
                "jit_to_value_generic: Unknown tag {:#x}",
                jit_val.tag()
            );
            factory.unit()
        }
    }
}
