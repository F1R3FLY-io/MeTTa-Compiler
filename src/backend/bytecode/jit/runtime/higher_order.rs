//! Higher-order runtime functions for JIT compilation
//!
//! This module provides FFI-callable higher-order operations:
//! - map_atom - Map a function over S-expression elements
//! - filter_atom - Filter S-expression elements by predicate
//! - foldl_atom - Left fold over S-expression elements
//! - decon_atom - Deconstruct an S-expression into (head, tail) pair
//! - repr - Convert value to string representation

use super::helpers::metta_to_jit;
use crate::backend::bytecode::chunk::BytecodeChunk;
use crate::backend::bytecode::jit::types::{JitBailoutReason, JitContext, JitValue};
use crate::backend::bytecode::vm::BytecodeVM;
use crate::backend::models::MettaValue;
use std::sync::Arc;

// =============================================================================
// Phase 1.7: S-Expression Operations - DeconAtom, Repr
// =============================================================================

/// Phase 1.7: Deconstruct an S-expression into (head, tail) pair
///
/// Given an S-expression, returns a new S-expression containing
/// the head (first element) and tail (remaining elements).
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed S-expression `(head tail)`, or Nil for non-S-expressions
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_decon_atom(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let metta_val = jit_val.to_metta();

    match metta_val {
        MettaValue::SExpr(elems) if !elems.is_empty() => {
            let head = elems[0].clone();
            let tail = MettaValue::SExpr(elems[1..].to_vec());
            let result = MettaValue::SExpr(vec![head, tail]);
            metta_to_jit(&result).to_bits()
        }
        MettaValue::SExpr(_) => {
            // Empty S-expression - return (Nil, ())
            let result = MettaValue::SExpr(vec![MettaValue::Nil, MettaValue::SExpr(vec![])]);
            metta_to_jit(&result).to_bits()
        }
        _ => {
            // Non-S-expression - return Nil
            JitValue::nil().to_bits()
        }
    }
}

/// Phase 1.7: Convert value to string representation
///
/// Creates a string representation of any MeTTa value.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed String containing the representation
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_repr(_ctx: *mut JitContext, val: u64, _ip: u64) -> u64 {
    let jit_val = JitValue::from_raw(val);
    let metta_val = jit_val.to_metta();

    // Format the value as a string (using Debug since Display not impl'd)
    let repr = format!("{:?}", metta_val);
    let result = MettaValue::String(repr);
    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Phase 1.8: Higher-Order Operations - MapAtom, FilterAtom, FoldlAtom
// =============================================================================

/// Helper: Execute a template chunk with a single bound value (for map/filter)
///
/// Creates a mini-VM, pushes the binding value, and executes the template.
fn execute_template_single(chunk: &Arc<BytecodeChunk>, binding: MettaValue) -> MettaValue {
    let mut vm = BytecodeVM::new(Arc::clone(chunk));
    // Push binding as local slot 0
    vm.push_initial_value(binding);
    // Execute and return first result
    match vm.run() {
        Ok(results) => results.into_iter().next().unwrap_or(MettaValue::Unit),
        Err(_) => MettaValue::Unit,
    }
}

/// Helper: Execute a foldl template chunk with accumulator and item
///
/// Creates a mini-VM, pushes (acc, item) as local slots, and executes.
fn execute_foldl_template(
    chunk: &Arc<BytecodeChunk>,
    acc: MettaValue,
    item: MettaValue,
) -> MettaValue {
    let mut vm = BytecodeVM::new(Arc::clone(chunk));
    // Push acc as local slot 0, item as local slot 1
    vm.push_initial_value(acc);
    vm.push_initial_value(item);
    // Execute and return first result (the new accumulator)
    match vm.run() {
        Ok(results) => results.into_iter().next().unwrap_or(MettaValue::Unit),
        Err(_) => MettaValue::Unit,
    }
}

/// Phase 1.8: Map a function over S-expression elements
///
/// Applies a bytecode chunk to each element of an S-expression,
/// collecting results. Now executes natively using mini-VM for templates.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// NaN-boxed S-expression with mapped results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_map_atom(
    ctx: *mut JitContext,
    list: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return list, // Null context, return original
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        // No chunk available, bail out
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return list;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => {
            // Not a list, return original
            return list;
        }
    };

    // Get the template sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let template_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            // Invalid chunk index, bail out
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return list;
        }
    };

    // Map over each element
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let result = execute_template_single(&template_chunk, item);
        results.push(result);
    }

    // Return mapped results as S-expression
    let result = MettaValue::SExpr(results);
    metta_to_jit(&result).to_bits()
}

/// Phase 1.8: Filter S-expression elements by predicate
///
/// Applies a predicate bytecode chunk to each element, keeping only
/// elements where the predicate returns true. Now executes natively.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// NaN-boxed S-expression with filtered results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_filter_atom(
    ctx: *mut JitContext,
    list: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return list,
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return list;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => return list,
    };

    // Get the predicate sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let predicate_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return list;
        }
    };

    // Filter elements where predicate returns true
    let mut results = Vec::new();
    for item in items {
        let result = execute_template_single(&predicate_chunk, item.clone());
        // Check if predicate returned true
        if matches!(result, MettaValue::Bool(true)) {
            results.push(item);
        }
    }

    // Return filtered results as S-expression
    let result = MettaValue::SExpr(results);
    metta_to_jit(&result).to_bits()
}

/// Phase 1.8: Left fold over S-expression elements
///
/// Applies a binary function to accumulator and each element,
/// threading the result through. Now executes natively.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - ctx.current_chunk must point to a valid BytecodeChunk
///
/// # Returns
/// Accumulated result
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_foldl_atom(
    ctx: *mut JitContext,
    list: u64,
    init: u64,
    chunk_idx: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return init,
    };

    // Get the current chunk to access sub-chunks
    if ctx_ref.current_chunk.is_null() {
        ctx_ref.bailout = true;
        ctx_ref.bailout_ip = ip as usize;
        ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
        return init;
    }

    // Get the list items
    let jit_list = JitValue::from_raw(list);
    let metta_list = jit_list.to_metta();
    let items = match metta_list {
        MettaValue::SExpr(items) => items,
        _ => return init, // Not a list, return init
    };

    // Get the operation sub-chunk from the parent chunk
    let chunk = &*(ctx_ref.current_chunk as *const BytecodeChunk);
    let op_chunk = match chunk.get_chunk_constant(chunk_idx as u16) {
        Some(c) => c,
        None => {
            ctx_ref.bailout = true;
            ctx_ref.bailout_ip = ip as usize;
            ctx_ref.bailout_reason = JitBailoutReason::HigherOrderOp;
            return init;
        }
    };

    // Get initial accumulator value
    let jit_init = JitValue::from_raw(init);
    let mut acc = jit_init.to_metta();

    // Fold over elements
    for item in items {
        acc = execute_foldl_template(&op_chunk, acc, item);
    }

    // Return accumulated result
    metta_to_jit(&acc).to_bits()
}
