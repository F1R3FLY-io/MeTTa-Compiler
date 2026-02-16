//! Stack operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable stack manipulation functions:
//! - push, pop - Basic stack operations with proper bailout signaling
//! - get_sp, set_sp - Stack pointer access
//! - load_constant - Constant pool access with bounds checking
//! - debug_print, debug_stack - Debugging utilities

use crate::backend::bytecode::jit::types::{
    JitBailoutReason, JitContext, JitValue, PAYLOAD_MASK, TAG_PTR, TAG_UNIT,
};
use tracing::trace;

// =============================================================================
// Stack Operations Runtime
// =============================================================================

/// Push a value onto the JIT context's memory stack
///
/// Used when we need to materialize values to memory (e.g., for calls).
/// On stack overflow, sets the bailout flag and reason for graceful fallback.
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_push(ctx: *mut JitContext, val: u64) -> i32 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp >= ctx.stack_cap {
            // Stack overflow - signal bailout for graceful fallback
            ctx.bailout = true;
            ctx.bailout_reason = JitBailoutReason::StackOverflow;
            return -1;
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
/// On stack underflow, sets the bailout flag and reason for graceful fallback.
///
/// # Safety
/// The context pointer and stack must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_pop(ctx: *mut JitContext) -> u64 {
    if let Some(ctx) = ctx.as_mut() {
        if ctx.sp == 0 {
            // Stack underflow - signal bailout for graceful fallback
            ctx.bailout = true;
            ctx.bailout_reason = JitBailoutReason::StackUnderflow;
            return TAG_UNIT;
        }
        ctx.sp -= 1;
        (*ctx.value_stack.add(ctx.sp)).to_bits()
    } else {
        TAG_UNIT
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
/// On out-of-bounds access, returns nil (no bailout since this is typically
/// a compilation bug rather than a runtime error).
///
/// # Safety
/// The context pointer and constant index must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_constant(ctx: *const JitContext, index: u64) -> u64 {
    if let Some(ctx) = ctx.as_ref() {
        let idx = index as usize;
        if idx >= ctx.constants_len {
            // Invalid constant index - this is likely a JIT compilation bug
            // but we handle gracefully by returning nil
            #[cfg(debug_assertions)]
            eprintln!(
                "JIT runtime: constant pool out of bounds (index={}, len={})",
                idx, ctx.constants_len
            );
            return TAG_UNIT;
        }

        let constant = &*ctx.constants.add(idx);

        // Try to NaN-box the constant
        match JitValue::try_from_metta(constant) {
            Some(jv) => jv.to_bits(),
            None => {
                // Can't NaN-box - return pointer to slab-allocated inner data
                let ptr = constant.inner_ptr();
                TAG_PTR | ((ptr as u64) & PAYLOAD_MASK)
            }
        }
    } else {
        TAG_UNIT
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
