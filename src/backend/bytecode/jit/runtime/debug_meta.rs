//! Debug and meta-level runtime functions for JIT compilation
//!
//! This module provides FFI-callable debug and meta operations:
//! - trace - Emit a trace event for debugging
//! - breakpoint - Breakpoint for debugging
//! - get_metatype - Get meta-level type of a value
//! - bloom_check - Fast bloom filter check before MORK lookup

use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{
    JitContext, JitValue, TAG_ATOM, TAG_BOOL, TAG_HEAP, TAG_LONG, TAG_NIL, TAG_UNIT, TAG_VAR,
};
use crate::backend::models::MettaValue;
use tracing::{debug, trace};

// =============================================================================
// Phase I: Debug/Meta
// =============================================================================

/// Emit a trace event for debugging
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `msg_idx` - Index of message in constant pool
/// * `value` - NaN-boxed value to trace
/// * `ip` - Instruction pointer
///
/// # Safety
/// The caller must ensure `_ctx` points to a valid `JitContext` if not null.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_trace(
    _ctx: *mut JitContext,
    msg_idx: u64,
    value: u64,
    ip: u64,
) {
    // Convert value to string for tracing
    let jit_val = JitValue::from_raw(value);
    let metta_val = jit_val.to_metta();

    // Trace output
    trace!(target: "mettatron::jit::runtime::trace", ip, msg_idx, ?metta_val, "Trace");
}

/// Breakpoint for debugging
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `bp_id` - Breakpoint identifier
/// * `ip` - Instruction pointer
///
/// # Returns
/// -1 to pause execution, 0 to continue
///
/// # Safety
/// The caller must ensure `_ctx` points to a valid `JitContext` if not null.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_breakpoint(_ctx: *mut JitContext, bp_id: u64, ip: u64) -> i64 {
    // Log breakpoint hit
    debug!(target: "mettatron::jit::runtime::breakpoint", bp_id, ip, "Breakpoint hit");

    // In a full implementation, this would check a debugger flag
    // and potentially pause execution. For now, always continue.
    0 // Continue
}

// =============================================================================
// Phase 1.9: Type Operations - GetMetaType
// =============================================================================

/// Phase 1.9: Get meta-level type of a value
///
/// Returns the meta-type of a value, which is one of:
/// - "Expression" for S-expressions
/// - "Symbol" for atoms/symbols
/// - "Variable" for variables
/// - "Grounded" for ground types (numbers, bools, strings)
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed Atom string representing the meta-type
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_metatype(
    _ctx: *mut JitContext,
    val: u64,
    _ip: u64,
) -> u64 {
    let jit_val = JitValue::from_raw(val);

    // Determine meta-type from tag
    let tag = jit_val.tag();
    let metatype = match tag {
        t if t == TAG_HEAP => {
            // Could be SExpr or other heap type
            let metta = jit_val.to_metta();
            match metta {
                MettaValue::SExpr(_) => "Expression",
                MettaValue::String(_) => "Grounded",
                _ => "Expression",
            }
        }
        t if t == TAG_ATOM => "Symbol",
        t if t == TAG_VAR => "Variable",
        t if t == TAG_LONG => "Grounded",
        t if t == TAG_BOOL => "Grounded",
        t if t == TAG_NIL => "Grounded",
        t if t == TAG_UNIT => "Grounded",
        _ => "Unknown",
    };

    let result = MettaValue::Atom(metatype.to_string());
    metta_to_jit(&result).to_bits()
}

// =============================================================================
// Phase 1.10: MORK/Debug Operations - BloomCheck
// =============================================================================

/// Phase 1.10: Fast bloom filter check before MORK lookup
///
/// Performs a probabilistic check to determine if a key might exist
/// in the MORK trie. Returns:
/// - false: Key definitely does not exist (skip lookup)
/// - true: Key possibly exists (proceed with full lookup)
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed bool (always true as conservative fallback)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_bloom_check(
    _ctx: *const JitContext,
    _key: u64,
    _ip: u64,
) -> u64 {
    // Conservative implementation: always say "maybe present"
    // This means we never skip lookups, but we're always correct.
    // A full implementation would check the actual bloom filter.
    JitValue::from_bool(true).to_bits()
}

// Note: Halt (0xFF) is handled directly in JIT codegen by returning
// JIT_SIGNAL_HALT signal, so no runtime function is needed.
