//! Error handling runtime functions for JIT compilation
//!
//! This module provides FFI-callable error handling functions that are invoked
//! from JIT-compiled code when runtime errors occur. Each function sets the
//! bailout flag and records the error type and location.
//!
//! # FFI Functions
//! - `jit_runtime_type_error` - Called on type mismatch errors
//! - `jit_runtime_div_by_zero` - Called on division by zero
//! - `jit_runtime_stack_overflow` - Called when stack exceeds limit
//! - `jit_runtime_stack_underflow` - Called when popping from empty stack

use crate::backend::bytecode::jit::types::{JitBailoutReason, JitContext};

// =============================================================================
// Error Handling Runtime
// =============================================================================

/// Runtime function called on type error
///
/// Sets the bailout flag in the context and records the error location.
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_type_error(ctx: *mut JitContext, ip: u64, _expected: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::TypeError;
    }
}

/// Runtime function called on division by zero
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_div_by_zero(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::DivisionByZero;
    }
}

/// Runtime function called on stack overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_stack_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::StackOverflow;
    }
}

/// Runtime function called on stack underflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_stack_underflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::StackUnderflow;
    }
}
