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
//! TAG_PTR/TAG_ERROR payloads are store-shaped. Legacy slab builds carry
//! `*const MettaValueInner`; index-gc builds carry arena `Addr` bits in the
//! pointer-width payload. The decode policy is pinned by
//! `formal/rocq/gc/JitPayloadConversionStorePolicy.v`.

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
/// For complex types, stores the mode-shaped inner payload produced by
/// `inner_ptr`.
pub fn metta_to_jit(val: &MettaValue) -> JitValue {
    match val.view() {
        ValueView::Long(n) => JitValue::from_long(n),
        ValueView::Bool(b) => JitValue::from_bool(b),
        ValueView::Unit => JitValue::unit(),
        // Store the mode-shaped heap payload (legacy pointer or index Addr).
        _ => JitValue::from_inner_ptr(val.inner_ptr()),
    }
}

// =============================================================================
// Error Creation Helpers
// =============================================================================

/// Helper to create an error JitValue with a message.
///
/// Creates an Error value in the active store and returns its mode-shaped
/// payload as a NaN-boxed TAG_PTR.
pub fn make_jit_error(msg: &str) -> u64 {
    let error_val = MettaValue::Error(MettaValue::Unit(), MettaValue::String(msg));
    TAG_PTR | (error_val.inner_ptr() as u64 & PAYLOAD_MASK)
}

/// Helper to create an error JitValue with message and details.
///
/// Creates an Error value in the active store and returns its mode-shaped
/// payload as a NaN-boxed TAG_PTR.
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
/// For complex types, stores the active-store payload for the value's inner
/// representation.
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

    // Complex types: store the active-store heap payload. In slab mode this is
    // a stable inner pointer; in index mode it is the arena Addr bits.
    let ptr = val.inner_ptr();
    JitValue::from_inner_ptr(ptr)
}

/// Convert a JitValue back to a generic value using a factory.
///
/// # Safety
/// - TAG_PTR payloads must have been produced for the active store
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
            // The null check is a SLAB invariant: under index-gc the payload is the
            // bare arena `Addr` bits and `Addr(0)` (payload 0) is a VALID address,
            // so the assert is meaningless there (it would false-fire). `from_inner_ptr`
            // is itself mode-aware (reconstructs the handle in index mode), so the
            // value is correct either way — only the assert needs the gate.
            debug_assert!(
                crate::backend::models::metta_value::gc_mode_is_index() || !ptr.is_null(),
                "jit_to_value_generic: Null inner pointer"
            );
            // Reconstruct value (slab: deref the inner ptr; index: from the Addr bits).
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

#[cfg(test)]
mod inc2b_index_tests {
    use super::*;
    use crate::backend::eval::cesk::index_heap::IndexFactory;

    /// Inc 2b: the JIT pack (`metta_to_jit` → `JitValue::from_inner_ptr`) and unpack
    /// (`JitValue::to_metta`) are mode-aware — in index mode a heap value's payload
    /// carries the bare arena `Addr` bits (the INDEX_KEY_TAG masked off at the 48-bit
    /// boundary) and unpack reconstructs the handle via `from_addr`, never a slab deref.
    #[test]
    fn jit_value_roundtrips_in_index_mode() {
        let f = IndexFactory;

        // Heap value (ground SExpr) → TAG_PTR → reconstructed handle, structurally equal.
        let v = f.sexpr(vec![f.atom("foo"), f.long(7)]);
        let jv = metta_to_jit(&v);
        assert_eq!(jv.tag(), TAG_PTR, "heap value packs to TAG_PTR");
        let back = unsafe { jv.to_metta() };
        assert_eq!(
            back, v,
            "JIT pack/unpack round-trips structurally in index mode"
        );

        // Inline scalars are tag-encoded directly (mode-independent).
        assert_eq!(unsafe { metta_to_jit(&f.long(42)).to_metta() }, f.long(42));
        assert_eq!(
            unsafe { metta_to_jit(&f.bool(true)).to_metta() },
            f.bool(true)
        );

        // Error value (TAG_PTR via the Error inner) round-trips and stays an error.
        let e = f.error(f.atom("BadType"), f.string("msg"));
        let eback = unsafe { metta_to_jit(&e).to_metta() };
        assert!(
            eback.is_error(),
            "error value survives the JIT round-trip in index mode"
        );

        // The error-builder helpers also pack to a valid TAG_PTR payload (no assert trip).
        let bits = make_jit_error("boom");
        assert_eq!(JitValue::from_raw(bits).tag(), TAG_PTR);
    }
}
