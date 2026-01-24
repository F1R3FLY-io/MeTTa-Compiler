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

/// Runtime function called on integer overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_integer_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::IntegerOverflow;
    }
}

/// Runtime function called on binding frame overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_binding_frame_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::BindingFrameOverflow;
    }
}

/// Runtime function called on invalid binding lookup
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_invalid_binding(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        ctx.bailout_reason = JitBailoutReason::InvalidBinding;
    }
}

/// Runtime function called on choice point overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_choice_point_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        // Reuse StackOverflow for choice point overflow since we don't have a dedicated variant
        // The VM can distinguish by checking the choice_point_count vs cap
        ctx.bailout_reason = JitBailoutReason::StackOverflow;
        #[cfg(debug_assertions)]
        eprintln!(
            "JIT runtime: choice point overflow at ip={} (count={}, cap={})",
            ip, ctx.choice_point_count, ctx.choice_point_cap
        );
    }
}

/// Runtime function called on results buffer overflow
///
/// # Safety
/// The context pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_results_overflow(ctx: *mut JitContext, ip: u64) {
    if let Some(ctx) = ctx.as_mut() {
        ctx.bailout = true;
        ctx.bailout_ip = ip as usize;
        // Reuse StackOverflow for results overflow since we don't have a dedicated variant
        ctx.bailout_reason = JitBailoutReason::StackOverflow;
        #[cfg(debug_assertions)]
        eprintln!(
            "JIT runtime: results buffer overflow at ip={} (count={}, cap={})",
            ip, ctx.results_count, ctx.results_cap
        );
    }
}
