//! Value creation runtime functions for JIT compilation
//!
//! This module provides FFI-callable value creation operations:
//! - make_sexpr - Create an S-expression from array of values
//! - cons_atom - Prepend a value to an S-expression
//! - push_uri - Load a URI from the constant pool
//! - make_list - Create a proper Cons-based list
//! - make_quote - Wrap a value in a quote expression

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, PAYLOAD_MASK,
    TAG_HEAP, TAG_MASK, TAG_NIL,
};
use crate::backend::models::MettaValue;
use super::stack_ops::jit_runtime_load_constant;

// =============================================================================
// Phase 2a: Value Creation Runtime (MakeSExpr, ConsAtom)
// =============================================================================

/// Create an S-expression from an array of NaN-boxed values.
///
/// This function takes a pointer to an array of NaN-boxed values (u64) and
/// creates a MettaValue::SExpr from them.
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the new S-expression
///
/// # Safety
/// * The context pointer must be valid
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_sexpr(
    _ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let count = count as usize;

    // Handle empty S-expression
    if count == 0 {
        let sexpr = Box::new(MettaValue::SExpr(Vec::new()));
        let ptr = Box::into_raw(sexpr);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Convert each value to MettaValue
    let mut elements = Vec::with_capacity(count);
    for i in 0..count {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);
        elements.push(jit_val.to_metta());
    }

    // Create the S-expression and return as heap pointer
    let sexpr = Box::new(MettaValue::SExpr(elements));
    let ptr = Box::into_raw(sexpr);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Prepend a value to an S-expression (cons operation).
///
/// This function implements the cons-atom operation:
/// - If tail is an S-expression, prepend head to it
/// - If tail is Nil, create a single-element S-expression
/// - Otherwise, signal a type error
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `head` - NaN-boxed value to prepend
/// * `tail` - NaN-boxed S-expression or Nil
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the new S-expression
///
/// # Safety
/// * The context pointer must be valid
/// * head and tail must be valid NaN-boxed values
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cons_atom(
    ctx: *mut JitContext,
    head: u64,
    tail: u64,
    ip: u64,
) -> u64 {
    let head_val = JitValue::from_raw(head);
    let head_metta = head_val.to_metta();

    let tail_tag = tail & TAG_MASK;

    // Handle Nil tail
    if tail_tag == TAG_NIL {
        let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
        let ptr = Box::into_raw(sexpr);
        return TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK);
    }

    // Must be a heap pointer (S-expression)
    if tail_tag != TAG_HEAP {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return TAG_NIL;
    }

    // Get the tail as MettaValue
    let tail_ptr = (tail & PAYLOAD_MASK) as *const MettaValue;
    if tail_ptr.is_null() {
        if let Some(ctx) = ctx.as_mut() {
            ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
        }
        return TAG_NIL;
    }

    // Check if tail is an S-expression
    match &*tail_ptr {
        MettaValue::SExpr(elements) => {
            // Prepend head to the elements
            let mut new_elements = Vec::with_capacity(elements.len() + 1);
            new_elements.push(head_metta);
            new_elements.extend(elements.iter().cloned());

            let sexpr = Box::new(MettaValue::SExpr(new_elements));
            let ptr = Box::into_raw(sexpr);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        MettaValue::Nil => {
            // Treat Nil as empty S-expression
            let sexpr = Box::new(MettaValue::SExpr(vec![head_metta]));
            let ptr = Box::into_raw(sexpr);
            TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
        }
        _ => {
            // Type error: tail is not an S-expression or Nil
            if let Some(ctx) = ctx.as_mut() {
                ctx.signal_error(ip as usize, JitBailoutReason::TypeError);
            }
            TAG_NIL
        }
    }
}

// =============================================================================
// Phase 2b: Value Creation Runtime (PushUri, MakeList, MakeQuote)
// =============================================================================

/// Load a URI from the constant pool (same as PushConstant).
///
/// PushUri uses the same mechanism as PushConstant - it loads a value from
/// the constant pool by index. The value at that index should be a MettaValue
/// representing the URI.
///
/// This function is an alias for jit_runtime_load_constant for clarity.
///
/// # Safety
/// The context pointer and constant index must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push_uri(ctx: *const JitContext, index: u64) -> u64 {
    jit_runtime_load_constant(ctx, index)
}

/// Create a proper MeTTa list from an array of NaN-boxed values.
///
/// Builds a linked list using the (Cons elem rest) structure:
/// - Elements are popped in order and reversed to build (Cons elem (Cons ... Nil))
/// - Empty list is just Nil
///
/// For example, with values [1, 2, 3], creates:
/// (Cons 1 (Cons 2 (Cons 3 Nil)))
///
/// # Arguments
/// * `ctx` - JIT context (for error handling)
/// * `values_ptr` - Pointer to array of NaN-boxed u64 values
/// * `count` - Number of elements in the array
/// * `ip` - Instruction pointer for error reporting
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the list (or TAG_NIL for empty list)
///
/// # Safety
/// * The context pointer must be valid
/// * values_ptr must point to a valid array of count u64 values
/// * Each value must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_list(
    _ctx: *mut JitContext,
    values_ptr: *const u64,
    count: u64,
    _ip: u64,
) -> u64 {
    let count = count as usize;

    // Empty list is Nil
    if count == 0 {
        return TAG_NIL;
    }

    // Build the list from the end (reverse order to get proper Cons structure)
    // Start with Nil, then Cons each element from the end
    let mut list = MettaValue::Nil;

    for i in (0..count).rev() {
        let raw_val = *values_ptr.add(i);
        let jit_val = JitValue::from_raw(raw_val);
        let elem = jit_val.to_metta();

        // Build (Cons elem list)
        list = MettaValue::SExpr(vec![
            MettaValue::Atom("Cons".to_string()),
            elem,
            list,
        ]);
    }

    // Return as heap pointer
    let boxed = Box::new(list);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}

/// Wrap a value in a quote expression.
///
/// Creates (quote value) S-expression to prevent evaluation.
///
/// # Arguments
/// * `ctx` - JIT context (unused, for consistency)
/// * `val` - NaN-boxed value to quote
/// * `ip` - Instruction pointer (unused, for consistency)
///
/// # Returns
/// NaN-boxed TAG_HEAP pointer to the (quote value) S-expression
///
/// # Safety
/// * val must be a valid NaN-boxed value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_make_quote(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let inner = jit_val.to_metta();

    // Create (quote value)
    let quoted = MettaValue::SExpr(vec![
        MettaValue::Atom("quote".to_string()),
        inner,
    ]);

    let boxed = Box::new(quoted);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
}
