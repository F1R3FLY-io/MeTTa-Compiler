//! Advanced nondeterminism runtime functions for JIT compilation
//!
//! This module provides FFI-callable advanced nondeterminism operations:
//! - cut - Prolog-style cut (prune search space)
//! - enter_cut_scope - Enter a cut scope (push marker)
//! - exit_cut_scope - Exit a cut scope (pop marker)
//! - guard - Guard condition for continuation
//! - amb - Inline nondeterministic choice
//! - commit - Soft cut (remove N choice points)
//! - backtrack - Force immediate backtracking
//! - begin_nondet - Begin nondeterministic section
//! - end_nondet - End nondeterministic section

use crate::backend::bytecode::jit::types::{JitContext, JitValue, JIT_SIGNAL_FAIL};
use tracing::warn;

// =============================================================================
// Phase G: Advanced Nondeterminism
// =============================================================================

/// Cut operation - prune search space by removing choice points back to the current cut scope
///
/// Prolog-style cut: removes choice points created since the current scope was entered,
/// but preserves choice points from outer scopes.
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_cut(ctx: *mut JitContext, _ip: u64) -> u64 {
    if ctx.is_null() {
        return JitValue::unit().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Check if we have cut markers available
    if ctx_ref.cut_marker_count > 0 && !ctx_ref.cut_markers.is_null() {
        // Get the most recent cut marker (choice point count at scope entry)
        let marker = *ctx_ref.cut_markers.add(ctx_ref.cut_marker_count - 1);

        // Remove choice points back to the marker, but not beyond it
        // This preserves choice points from outer scopes
        if ctx_ref.choice_point_count > marker {
            ctx_ref.choice_point_count = marker;
        }
    } else {
        // No cut markers - fall back to clearing all choice points
        // This happens when cut is called outside of a proper cut scope
        ctx_ref.choice_point_count = 0;
    }

    JitValue::unit().to_bits()
}

/// Enter a cut scope - push a cut marker
///
/// Call this when entering a scope that may use cut (e.g., rule body, once/1, etc.)
/// The marker records the current choice point count so cut knows where to stop.
///
/// # Arguments
/// * `ctx` - JIT context
///
/// # Returns
/// 1 on success, 0 on failure (no room for marker)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_enter_cut_scope(ctx: *mut JitContext) -> i64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx_ref = &mut *ctx;

    // Check if we have room for another marker
    if ctx_ref.cut_markers.is_null() || ctx_ref.cut_marker_count >= ctx_ref.cut_marker_cap {
        return 0; // No room
    }

    // Push the current choice point count as a marker
    *ctx_ref.cut_markers.add(ctx_ref.cut_marker_count) = ctx_ref.choice_point_count;
    ctx_ref.cut_marker_count += 1;

    1 // Success
}

/// Exit a cut scope - pop a cut marker
///
/// Call this when leaving a scope that may have used cut.
///
/// # Arguments
/// * `ctx` - JIT context
///
/// # Returns
/// 1 on success, 0 on failure (no markers to pop)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_exit_cut_scope(ctx: *mut JitContext) -> i64 {
    if ctx.is_null() {
        return 0;
    }
    let ctx_ref = &mut *ctx;

    // Check if we have markers to pop
    if ctx_ref.cut_marker_count == 0 {
        return 0; // No markers
    }

    ctx_ref.cut_marker_count -= 1;
    1 // Success
}

/// Guard condition - check if condition allows continuation
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `condition` - NaN-boxed boolean condition
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 1 if guard passes (proceed), 0 if guard fails (backtrack)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_guard(_ctx: *mut JitContext, condition: u64, _ip: u64) -> i64 {
    use crate::backend::bytecode::jit::types::TAG_BOOL;

    // True is TAG_BOOL | 1
    let tag_bool_true = TAG_BOOL | 1;

    if condition == tag_bool_true {
        1 // Guard passes, continue
    } else {
        0 // Guard fails, backtrack
    }
}

/// Amb operation - inline nondeterministic choice
///
/// Stack: [alt1, alt2, ..., altN] -> [selected]
/// Creates choice point for alternatives 2..N, returns first alternative
///
/// # Arguments
/// * `ctx` - JIT context
/// * `alt_count` - Number of alternatives on stack
/// * `ip` - Instruction pointer for backtracking
///
/// # Returns
/// NaN-boxed first alternative value
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_amb(ctx: *mut JitContext, alt_count: u64, ip: u64) -> u64 {
    if ctx.is_null() {
        return JitValue::nil().to_bits();
    }
    let ctx_ref = &mut *ctx;

    // Empty amb - fail immediately
    if alt_count == 0 {
        return JitValue::nil().to_bits();
    }

    // Pop all alternatives from stack
    let mut alts = Vec::with_capacity(alt_count as usize);
    for _ in 0..alt_count {
        if ctx_ref.sp < 1 {
            warn!(target: "mettatron::jit::runtime::nondet", ip, "Stack underflow in amb");
            return JitValue::nil().to_bits();
        }
        ctx_ref.sp -= 1;
        let val = *ctx_ref.value_stack.add(ctx_ref.sp);
        alts.push(val);
    }
    alts.reverse(); // Restore order: alt1, alt2, ..., altN

    // Single alternative - no choice point needed
    if alt_count == 1 {
        return alts[0].to_bits();
    }

    // Create choice point for remaining alternatives (2..N)
    if ctx_ref.choice_point_count < ctx_ref.choice_point_cap {
        let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
        cp.saved_sp = ctx_ref.sp as u64;
        cp.alt_count = (alt_count - 1) as u64;
        cp.current_index = 0;
        cp.saved_ip = ip;

        // Store remaining alternatives for backtracking
        // Note: In a full implementation, we'd store all alts[1..] in the choice point
        // For now, we just store the count

        ctx_ref.choice_point_count += 1;
    }

    // Return first alternative
    alts[0].to_bits()
}

/// Commit operation - remove N choice points (soft cut)
///
/// # Arguments
/// * `ctx` - JIT context
/// * `count` - Number of choice points to remove (0 = all)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed Unit
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_commit(ctx: *mut JitContext, count: u64, _ip: u64) -> u64 {
    if ctx.is_null() {
        return JitValue::unit().to_bits();
    }
    let ctx_ref = &mut *ctx;

    if count == 0 {
        // Remove all choice points (full cut)
        ctx_ref.choice_point_count = 0;
    } else {
        // Remove N most recent choice points
        let to_remove = (count as usize).min(ctx_ref.choice_point_count);
        ctx_ref.choice_point_count -= to_remove;
    }

    JitValue::unit().to_bits()
}

/// Backtrack operation - force immediate backtracking
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
///
/// # Returns
/// JIT signal for backtracking (-3 = FAIL signal)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_backtrack(_ctx: *mut JitContext, _ip: u64) -> i64 {
    // Return FAIL signal to trigger backtracking in the JIT dispatcher
    JIT_SIGNAL_FAIL
}

// =============================================================================
// Core Nondeterminism Markers
// =============================================================================

/// Begin nondeterministic section - increment fork depth counter
///
/// This marks the start of a code section that may contain nondeterministic
/// operations (Fork, Amb, etc.). The fork_depth counter helps track nesting
/// and can be used for optimization decisions.
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_begin_nondet(ctx: *mut JitContext, _ip: u64) {
    let ctx_ref = &mut *ctx;
    ctx_ref.fork_depth += 1;
}

/// End nondeterministic section - decrement fork depth counter
///
/// This marks the end of a code section that may contain nondeterministic
/// operations. Decrements the fork_depth counter (will not go below 0).
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_ip` - Instruction pointer
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_end_nondet(ctx: *mut JitContext, _ip: u64) {
    let ctx_ref = &mut *ctx;
    if ctx_ref.fork_depth > 0 {
        ctx_ref.fork_depth -= 1;
    }
}
