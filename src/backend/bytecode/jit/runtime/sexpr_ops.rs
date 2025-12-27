//! S-expression operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable S-expression operations:
//! - push_empty - Create an empty S-expression
//! - get_head - Get the first element of an S-expression
//! - get_tail - Get all elements except the first
//! - get_arity - Get the number of elements
//! - get_element - Get element at a specific index

use crate::backend::bytecode::jit::types::{
    JitContext, JitValue, PAYLOAD_MASK, TAG_HEAP, TAG_NIL,
};
use crate::backend::models::MettaValue;

// =============================================================================
// S-Expression Operations (Stage 14: Head/Tail/Arity/Element)
// =============================================================================

/// Runtime function for PushEmpty opcode
///
/// Creates and returns an empty S-expression ().
///
/// # Returns
/// NaN-boxed heap pointer to empty SExpr
///
/// # Safety
/// No special safety requirements.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_empty() -> u64 {
    let empty = MettaValue::SExpr(Vec::new());
    let boxed = Box::new(empty);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Runtime function for GetHead opcode
///
/// Gets the head (first element) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed head element, or TAG_NIL if empty/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_head(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return TAG_NIL;
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return TAG_NIL;
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                TAG_NIL
            } else {
                // Return the head element
                let head = &items[0];
                match JitValue::try_from_metta(head) {
                    Some(jv) => jv.to_bits(),
                    None => {
                        // Need to heap-allocate for non-primitive types
                        let boxed = Box::new(head.clone());
                        let ptr = Box::into_raw(boxed);
                        TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
                    }
                }
            }
        }
        _ => TAG_NIL,
    }
}

/// Runtime function for GetTail opcode
///
/// Gets the tail (all elements except first) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed heap pointer to tail SExpr, or empty SExpr if empty/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_tail(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        // Return empty SExpr for non-heap values
        let empty = MettaValue::SExpr(Vec::new());
        let boxed = Box::new(empty);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        let empty = MettaValue::SExpr(Vec::new());
        let boxed = Box::new(empty);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            // Return tail (skip first element)
            let tail: Vec<MettaValue> = if items.len() > 1 {
                items[1..].to_vec()
            } else {
                Vec::new()
            };
            let expr = MettaValue::SExpr(tail);
            let boxed = Box::new(expr);
            let ptr = Box::into_raw(boxed);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        _ => {
            // Return empty SExpr for non-SExpr values
            let empty = MettaValue::SExpr(Vec::new());
            let boxed = Box::new(empty);
            let ptr = Box::into_raw(boxed);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
    }
}

/// Runtime function for GetArity opcode
///
/// Gets the arity (number of elements) of an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed Long containing the arity, or 0 if not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_arity(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_val = &*metta_ptr;
    match metta_val {
        MettaValue::SExpr(items) => {
            JitValue::from_long(items.len() as i64).to_bits()
        }
        _ => JitValue::from_long(0).to_bits(),
    }
}

/// Runtime function for GetElement opcode
///
/// Gets an element at a specific index from an S-expression.
///
/// # Arguments
/// * `ctx` - JIT context pointer (for error handling)
/// * `val` - NaN-boxed value (expected to be heap pointer to SExpr)
/// * `index` - Element index (0-based)
/// * `ip` - Instruction pointer (for error reporting)
///
/// # Returns
/// NaN-boxed element at index, or TAG_NIL if out of bounds/not an SExpr
///
/// # Safety
/// The heap pointer must be valid if val is TAG_HEAP.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_element(
    _ctx: *mut JitContext,
    val: u64,
    index: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return TAG_NIL;
    }

    let metta_ptr = jit_val.as_heap_ptr();
    if metta_ptr.is_null() {
        return TAG_NIL;
    }

    let metta_val = &*metta_ptr;
    let idx = index as usize;

    match metta_val {
        MettaValue::SExpr(items) => {
            if idx >= items.len() {
                TAG_NIL
            } else {
                let elem = &items[idx];
                match JitValue::try_from_metta(elem) {
                    Some(jv) => jv.to_bits(),
                    None => {
                        // Need to heap-allocate for non-primitive types
                        let boxed = Box::new(elem.clone());
                        let ptr = Box::into_raw(boxed);
                        TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
                    }
                }
            }
        }
        _ => TAG_NIL,
    }
}
