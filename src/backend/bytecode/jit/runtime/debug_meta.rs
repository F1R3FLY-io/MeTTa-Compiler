//! Debug and meta-level runtime functions for JIT compilation
//!
//! This module provides FFI-callable debug and meta operations:
//! - trace - Emit a trace event for debugging
//! - breakpoint - Breakpoint for debugging
//! - get_metatype - Get meta-level type of a value
//! - bloom_check - Fast bloom filter check before MORK lookup

use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{JitContext, JitValue};
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

// =============================================================================
// S1 TOPLEVEL (2026-05-13): HE runner-mode helpers
// =============================================================================

/// Enter HE INTERPRET runner mode.
///
/// Set `JitContext::interpret_mode = true` so downstream call-support
/// gates (`jit_runtime_dispatch_*`) emit observable results for bare
/// S-exprs instead of swallowing them under HE ADD-mode semantics.
///
/// Lowered from `Opcode::EnterInterpretMode` (0x2D) emitted by the
/// bytecode compiler at the start of a `(! expr)` directive body.
///
/// # Safety
/// `ctx` must point to a live `JitContext` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_enter_interpret_mode(ctx: *mut JitContext, _ip: u64) -> u64 {
    let ctx_ref = unsafe { &mut *ctx };
    ctx_ref.interpret_mode = true;
    // S2 BANG-WORD (2026-05-13): toggle bang_body in lockstep so JIT-side
    // decl-atom handlers see the strict signal (= the `!` body is active).
    ctx_ref.bang_body = true;
    0
}

/// Exit HE INTERPRET runner mode.
///
/// Clear `JitContext::interpret_mode` so the next directive starts in
/// HE ADD mode (the default). Lowered from `Opcode::ExitInterpretMode`
/// (0x2E) emitted at the end of a `(! expr)` directive body.
///
/// # Safety
/// `ctx` must point to a live `JitContext` for the duration of the call.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_exit_interpret_mode(ctx: *mut JitContext, _ip: u64) -> u64 {
    let ctx_ref = unsafe { &mut *ctx };
    ctx_ref.interpret_mode = false;
    // S2 BANG-WORD (2026-05-13): clear bang_body in lockstep.
    ctx_ref.bang_body = false;
    0
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

/// Phase 1.9: Get meta-level type of a value (HE 4-category vocabulary).
///
/// Plan S7 (RC-METATYPE-VOCAB, 2026-05-14): all four tiers (T0 trampoline,
/// T1 VM, T2/T3 JIT) delegate to `ValueView::metatype()` — the single source
/// of truth in `src/backend/models/metta_value.rs`. Returns one of:
/// - "Expression" — S-expressions and Quoted wrappers
/// - "Symbol"     — plain non-variable atoms
/// - "Variable"   — `$`-prefixed atoms
/// - "Grounded"   — primitives, errors, state, space, etc.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext (unused; reserved for future)
/// - val must be a valid JIT-encoded NaN-boxed value
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
    // Plan S7: delegate to ValueView::metatype() so the JIT, VM, and
    // trampoline tiers share one HE-aligned 4-category vocabulary.
    let metta = jit_val.to_metta();
    let metatype = metta.view().metatype();

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
