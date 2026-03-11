//! S-expression operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable S-expression operations:
//! - push_empty - Create an empty S-expression
//! - get_head - Get the first element of an S-expression
//! - get_tail - Get all elements except the first
//! - get_arity - Get the number of elements
//! - get_element - Get element at a specific index

use super::helpers::{metta_to_jit, value_to_jit_generic};
use crate::backend::bytecode::jit::types::{JitContext, JitValue, TAG_UNIT};
use crate::backend::models::{MettaValue, ValueView};

// =============================================================================
// S-Expression Operations (Stage 14: Head/Tail/Arity/Element)
// =============================================================================

/// Runtime function for PushEmpty opcode
///
/// Creates and returns an empty S-expression ().
///
/// # Returns
/// NaN-boxed inner pointer to empty SExpr
///
/// # Safety
/// No special safety requirements.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_empty() -> u64 {
    let empty = MettaValue::SExpr(Vec::new());
    value_to_jit_generic(&empty).to_bits()
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
/// NaN-boxed head element, or TAG_UNIT if empty/not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_head(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return TAG_UNIT;
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return TAG_UNIT;
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => {
            if items.is_empty() {
                TAG_UNIT
            } else {
                // Return the head element
                let head = &items[0];
                value_to_jit_generic(head).to_bits()
            }
        }
        // Quoted is transparent to car-atom: (car-atom (quote X)) → quote
        ValueView::Quoted(_) => {
            let quote_atom = MettaValue::Atom("quote".to_string());
            metta_to_jit(&quote_atom).to_bits()
        }
        ValueView::Float(_) | ValueView::Bool(_) | ValueView::Long(_) | ValueView::Unit
        | ValueView::Empty | ValueView::Atom(_) | ValueView::String(_) | ValueView::Error(_, _)
        | ValueView::Type(_) | ValueView::Conjunction(_) | ValueView::Space(_)
        | ValueView::State(_) | ValueView::Memo(_) => TAG_UNIT,
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
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_tail(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        // Return unit for non-heap values (SExpr(vec![]) → unit via factory)
        return JitValue::unit().to_bits();
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return JitValue::unit().to_bits();
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => {
            // Return tail (skip first element)
            let tail: Vec<MettaValue> = if items.len() > 1 {
                items[1..].to_vec()
            } else {
                Vec::new()
            };
            let expr = MettaValue::SExpr(tail);
            value_to_jit_generic(&expr).to_bits()
        }
        // Quoted is transparent to cdr-atom: (cdr-atom (quote X)) → (X)
        ValueView::Quoted(inner) => {
            let tail = MettaValue::SExpr(vec![inner]);
            value_to_jit_generic(&tail).to_bits()
        }
        _ => {
            // Return unit for non-SExpr values (SExpr(vec![]) → unit via factory)
            JitValue::unit().to_bits()
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
/// The inner pointer must be valid if val is TAG_PTR.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_arity(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Check if it's a heap pointer
    if !jit_val.is_heap() {
        return JitValue::from_long(0).to_bits();
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return JitValue::from_long(0).to_bits();
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    match metta_val.view() {
        ValueView::SExpr(items) => JitValue::from_long(items.len() as i64).to_bits(),
        ValueView::Float(_) | ValueView::Bool(_) | ValueView::Long(_) | ValueView::Unit
        | ValueView::Empty | ValueView::Atom(_) | ValueView::String(_) | ValueView::Error(_, _)
        | ValueView::Type(_) | ValueView::Conjunction(_) | ValueView::Space(_)
        | ValueView::State(_) | ValueView::Memo(_) | ValueView::Quoted(_) => {
            JitValue::from_long(0).to_bits()
        }
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
/// NaN-boxed element at index, or TAG_UNIT if out of bounds/not an SExpr
///
/// # Safety
/// The inner pointer must be valid if val is TAG_PTR.
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
        return TAG_UNIT;
    }

    let inner_ptr = jit_val.as_inner_ptr();
    if inner_ptr.is_null() {
        return TAG_UNIT;
    }

    let metta_val = MettaValue::from_inner(&*inner_ptr);
    let idx = index as usize;

    match metta_val.view() {
        ValueView::SExpr(items) => {
            if idx >= items.len() {
                TAG_UNIT
            } else {
                value_to_jit_generic(&items[idx]).to_bits()
            }
        }
        ValueView::Float(_) | ValueView::Bool(_) | ValueView::Long(_) | ValueView::Unit
        | ValueView::Empty | ValueView::Atom(_) | ValueView::String(_) | ValueView::Error(_, _)
        | ValueView::Type(_) | ValueView::Conjunction(_) | ValueView::Space(_)
        | ValueView::State(_) | ValueView::Memo(_) | ValueView::Quoted(_) => TAG_UNIT,
    }
}
