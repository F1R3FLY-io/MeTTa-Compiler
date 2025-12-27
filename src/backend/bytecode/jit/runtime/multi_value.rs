//! Multi-value return runtime functions for JIT compilation
//!
//! This module provides FFI-callable multi-value return operations:
//! - return_multi - Return multiple values for nondeterminism
//! - collect_n - Collect up to N nondeterministic results

use crate::backend::bytecode::jit::types::{
    JitContext, JIT_SIGNAL_FAIL, JIT_SIGNAL_YIELD, PAYLOAD_MASK, TAG_HEAP,
};
use crate::backend::models::MettaValue;

// =============================================================================
// Phase 1.3: Multi-Value Return - ReturnMulti, CollectN
// =============================================================================

/// Phase 1.3: Return multiple values for nondeterminism
///
/// Signals that the current computation has multiple results available.
/// The count parameter indicates how many values are on the stack to return.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - Stack must contain at least `count` values
///
/// # Returns
/// JIT_SIGNAL_YIELD if results were stored, JIT_SIGNAL_FAIL if no results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_return_multi(
    ctx: *mut JitContext,
    count: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_mut().expect("return_multi: null context");

    let count = count as usize;
    if count == 0 {
        return JIT_SIGNAL_FAIL as u64;
    }

    // Store results from stack
    for i in 0..count {
        if ctx.results_count >= ctx.results_cap {
            break;
        }
        let stack_idx = ctx.sp.saturating_sub(count - i);
        let val = *ctx.value_stack.add(stack_idx);
        *ctx.results.add(ctx.results_count) = val;
        ctx.results_count += 1;
    }

    // Pop values from stack
    ctx.sp = ctx.sp.saturating_sub(count);

    JIT_SIGNAL_YIELD as u64
}

/// Phase 1.3: Collect up to N nondeterministic results
///
/// Collects at most `max_count` results from the results buffer into
/// an S-expression. If fewer results are available, returns those.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed S-expression containing collected results
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_collect_n(
    ctx: *mut JitContext,
    max_count: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_mut().expect("collect_n: null context");

    let count = (max_count as usize).min(ctx.results_count);

    if count == 0 {
        // Return empty S-expression
        let empty_sexpr = MettaValue::SExpr(vec![]);
        let boxed = Box::new(empty_sexpr);
        let ptr = Box::into_raw(boxed);
        return TAG_HEAP | (ptr as u64 & PAYLOAD_MASK);
    }

    // Collect results into MettaValue vec
    let mut results = Vec::with_capacity(count);
    for i in 0..count {
        let jit_val = *ctx.results.add(i);
        let metta_val = jit_val.to_metta();
        results.push(metta_val);
    }

    // Clear collected results
    ctx.results_count = ctx.results_count.saturating_sub(count);

    // Return as S-expression
    let sexpr = MettaValue::SExpr(results);
    let boxed = Box::new(sexpr);
    let ptr = Box::into_raw(boxed);
    TAG_HEAP | (ptr as u64 & PAYLOAD_MASK)
}
