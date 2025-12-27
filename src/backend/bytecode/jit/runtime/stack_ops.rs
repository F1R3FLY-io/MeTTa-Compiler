//! Stack operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable stack manipulation functions:
//! - push, pop - Basic stack operations
//! - get_sp, set_sp - Stack pointer access
//! - load_constant - Constant pool access
//! - debug_print, debug_stack - Debugging utilities

use crate::backend::bytecode::jit::types::{JitContext, JitValue, PAYLOAD_MASK, TAG_HEAP, TAG_NIL};
use crate::backend::models::MettaValue;
use tracing::trace;

// =============================================================================
// Stack Operations Runtime
// =============================================================================

/// Push a value onto the JIT context's memory stack
///
/// Used when we need to materialize values to memory (e.g., for calls).
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push(ctx: *mut JitContext, val: u64) -> i32 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp >= ctx.stack_cap {
            return -1; // Stack overflow
        }
        *ctx.value_stack.add(ctx.sp) = JitValue::from_raw(val);
        ctx.sp += 1;
        0 // Success
    } else {
        -2 // Null context
    }
}

/// Pop a value from the JIT context's memory stack
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pop(ctx: *mut JitContext) -> u64 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp == 0 {
            // Stack underflow - return nil as sentinel
            return TAG_NIL;
        }
        ctx.sp -= 1;
        (*ctx.value_stack.add(ctx.sp)).to_bits()
    } else {
        TAG_NIL
    }
}

/// Get stack pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_sp(ctx: *const JitContext) -> u64 {
    if let Some(ctx) = ctx.as_ref() {
        ctx.sp as u64
    } else {
        0
    }
}

/// Set stack pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_set_sp(ctx: *mut JitContext, sp: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.sp = sp as usize;
    }
}

// =============================================================================
// Constant Pool Access
// =============================================================================

/// Load a constant from the constant pool
///
/// Returns the constant as a JitValue (boxing if necessary).
///
/// # Safety
/// The context pointer and constant index must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_constant(ctx: *const JitContext, index: u64) -> u64 {
    if let Some(ctx) = ctx.as_ref() {
        let idx = index as usize;
        if idx >= ctx.constants_len {
            // Invalid constant index - return nil
            return TAG_NIL;
        }

        let constant = &*ctx.constants.add(idx);

        // Try to NaN-box the constant
        match JitValue::try_from_metta(constant) {
            Some(jv) => jv.to_bits(),
            None => {
                // Can't NaN-box - return as heap pointer
                let ptr = constant as *const MettaValue;
                TAG_HEAP | ((ptr as u64) & PAYLOAD_MASK)
            }
        }
    } else {
        TAG_NIL
    }
}

// =============================================================================
// Debugging Runtime
// =============================================================================

/// Print a JitValue for debugging
#[no_mangle]
pub extern "C" fn jit_runtime_debug_print(val: u64) {
    let jv = JitValue::from_raw(val);
    trace!(target: "mettatron::jit::runtime::debug", ?jv, "Debug print");
}

/// Print the current stack for debugging
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_debug_stack(ctx: *const JitContext) {
    if let Some(ctx) = ctx.as_ref() {
        trace!(target: "mettatron::jit::runtime::debug", sp = ctx.sp, "Stack dump");
        for i in 0..ctx.sp {
            let val = *ctx.value_stack.add(i);
            trace!(target: "mettatron::jit::runtime::debug", index = i, ?val, "  Stack slot");
        }
    }
}
