//! Expression manipulation runtime functions for JIT compilation
//!
//! This module provides FFI-callable expression operations:
//! - index_atom - Get element at index
//! - min_atom - Get minimum numeric value
//! - max_atom - Get maximum numeric value

use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{JitBailoutReason, JitContext, JitValue};
use crate::backend::models::MettaValue;

// =============================================================================
// Expression Manipulation Operations
// =============================================================================

/// Get element at index: index-atom(expr, index) -> element
///
/// # Safety
/// The context and inputs must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_index_atom(
    ctx: *mut JitContext,
    expr: u64,
    index: u64,
    ip: u64,
) -> u64 {
    let expr_jv = JitValue::from_raw(expr);
    let index_jv = JitValue::from_raw(index);
    let expr_mv = expr_jv.to_metta();
    let index_mv = index_jv.to_metta();

    let idx = match index_mv {
        MettaValue::Long(i) => i,
        _ => {
            // Type error
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.bailout = true;
                ctx_ref.bailout_ip = ip as usize;
                ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            }
            return JitValue::nil().to_bits();
        }
    };

    let result = match expr_mv {
        MettaValue::SExpr(items) => {
            if idx < 0 || idx as usize >= items.len() {
                // Index out of bounds - return nil
                MettaValue::Nil
            } else {
                items[idx as usize].clone()
            }
        }
        _ => {
            // Type error
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.bailout = true;
                ctx_ref.bailout_ip = ip as usize;
                ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            }
            return JitValue::nil().to_bits();
        }
    };

    metta_to_jit(&result).to_bits()
}

/// Get minimum value: min-atom(expr) -> min
///
/// # Safety
/// The context and inputs must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_min_atom(ctx: *mut JitContext, expr: u64, ip: u64) -> u64 {
    let expr_jv = JitValue::from_raw(expr);
    let expr_mv = expr_jv.to_metta();

    match expr_mv {
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                return JitValue::nil().to_bits();
            }

            let mut min_val: Option<f64> = None;
            let mut min_item: Option<&MettaValue> = None;

            for item in &items {
                let val = match item {
                    MettaValue::Long(x) => Some(*x as f64),
                    MettaValue::Float(x) => Some(*x),
                    _ => None,
                };

                if let Some(v) = val {
                    match min_val {
                        None => {
                            min_val = Some(v);
                            min_item = Some(item);
                        }
                        Some(current_min) if v < current_min => {
                            min_val = Some(v);
                            min_item = Some(item);
                        }
                        _ => {}
                    }
                }
            }

            match min_item {
                Some(item) => metta_to_jit(item).to_bits(),
                None => JitValue::nil().to_bits(),
            }
        }
        _ => {
            // Type error
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.bailout = true;
                ctx_ref.bailout_ip = ip as usize;
                ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            }
            JitValue::nil().to_bits()
        }
    }
}

/// Get maximum value: max-atom(expr) -> max
///
/// # Safety
/// The context and inputs must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_max_atom(ctx: *mut JitContext, expr: u64, ip: u64) -> u64 {
    let expr_jv = JitValue::from_raw(expr);
    let expr_mv = expr_jv.to_metta();

    match expr_mv {
        MettaValue::SExpr(items) => {
            if items.is_empty() {
                return JitValue::nil().to_bits();
            }

            let mut max_val: Option<f64> = None;
            let mut max_item: Option<&MettaValue> = None;

            for item in &items {
                let val = match item {
                    MettaValue::Long(x) => Some(*x as f64),
                    MettaValue::Float(x) => Some(*x),
                    _ => None,
                };

                if let Some(v) = val {
                    match max_val {
                        None => {
                            max_val = Some(v);
                            max_item = Some(item);
                        }
                        Some(current_max) if v > current_max => {
                            max_val = Some(v);
                            max_item = Some(item);
                        }
                        _ => {}
                    }
                }
            }

            match max_item {
                Some(item) => metta_to_jit(item).to_bits(),
                None => JitValue::nil().to_bits(),
            }
        }
        _ => {
            // Type error
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.bailout = true;
                ctx_ref.bailout_ip = ip as usize;
                ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            }
            JitValue::nil().to_bits()
        }
    }
}
