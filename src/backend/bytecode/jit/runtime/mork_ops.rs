//! MORK Bridge runtime functions for JIT compilation
//!
//! This module provides FFI-callable MORK operations:
//! - mork_lookup - Look up a value in MORK PathMap
//! - mork_match - Match a pattern against MORK space
//! - mork_insert - Insert a value into MORK PathMap/space
//! - mork_delete - Delete a value from MORK PathMap/space

use crate::backend::bytecode::jit::types::{JitContext, JitValue};
use crate::backend::models::MettaValue;
use crate::backend::bytecode::mork_bridge::MorkBridge;
use super::helpers::metta_to_jit;
use super::space_ops::jit_runtime_space_match;

// =============================================================================
// Phase H: MORK Bridge
// =============================================================================

/// Look up a value in MORK PathMap
///
/// # Arguments
/// * `_ctx` - JIT context
/// * `path` - NaN-boxed path expression
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed value from MORK, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_lookup(
    ctx: *mut JitContext,
    path: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return JitValue::nil().to_bits(),
    };

    // Convert path to MettaValue
    let path_jit = JitValue::from_raw(path);
    let path_metta = path_jit.to_metta();

    // Try to look up through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.read();
        if let Ok(env_read) = env_guard {
            // Check if this is an atom and it exists in the space
            if let MettaValue::Atom(name) = &path_metta {
                if env_read.has_fact(name) {
                    return metta_to_jit(&path_metta).to_bits();
                }
            }

            // Check if this is an S-expression that exists in space
            if env_read.has_sexpr_fact(&path_metta) {
                return metta_to_jit(&path_metta).to_bits();
            }
        }
    }

    // Not found
    JitValue::nil().to_bits()
}

/// Match a pattern against MORK space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `pattern` - NaN-boxed pattern to match
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed result (may create choice points for multiple matches)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_match(
    ctx: *mut JitContext,
    pattern: u64,
    _ip: u64,
) -> u64 {
    // Delegate to space match (MORK uses similar semantics)
    jit_runtime_space_match(ctx, 0, pattern, 0, _ip)
}

/// Insert a value into MORK PathMap / space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `_path` - NaN-boxed path expression (unused for simple add)
/// * `value` - NaN-boxed value to insert
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 0 on success, -1 on error
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_insert(
    ctx: *mut JitContext,
    _path: u64,
    value: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return -1,
    };

    // Convert value to MettaValue
    let value_jit = JitValue::from_raw(value);
    let value_metta = value_jit.to_metta();

    // Try to insert through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.write();
        if let Ok(mut env_write) = env_guard {
            env_write.add_to_space(&value_metta);
            return 0; // Success
        }
    }

    -1 // Error (no bridge available)
}

/// Delete a value from MORK PathMap / space
///
/// # Arguments
/// * `ctx` - JIT context
/// * `path` - NaN-boxed path/value expression to delete
/// * `_ip` - Instruction pointer
///
/// # Returns
/// 1 if deleted, 0 if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_mork_delete(
    ctx: *mut JitContext,
    path: u64,
    _ip: u64,
) -> i64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return 0,
    };

    // Convert path to MettaValue
    let path_jit = JitValue::from_raw(path);
    let path_metta = path_jit.to_metta();

    // Try to delete through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.write();
        if let Ok(mut env_write) = env_guard {
            env_write.remove_from_space(&path_metta);
            return 1; // Deleted (or at least attempted)
        }
    }

    0 // Not found (no bridge available)
}
