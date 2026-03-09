//! State operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable state operations:
//! - new_state - Create a new mutable state cell
//! - get_state - Get the current value from a state cell
//! - change_state - Change the value of a state cell

use super::helpers::{make_jit_error, make_jit_error_with_details, metta_to_jit};
use crate::backend::bytecode::jit::types::{JitContext, JitValue};
use crate::backend::models::{MettaValue, ValueView};

// =============================================================================
// State Operations Runtime (Phase D.1)
// =============================================================================

/// Create a new mutable state cell with an initial value.
///
/// Used by `new-state` operation. Creates a state cell in the environment
/// and returns a State(id) value.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `initial_value`: NaN-boxed initial value for the state
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed State(id) value, or error if environment is not available
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_new_state(
    ctx: *mut JitContext,
    initial_value: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("new-state: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("new-state: environment not available");
    }

    // Cast env_ptr to Environment (MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>)
    use crate::backend::bytecode::MettaEnvironment;
    let env = &mut *(ctx_ref.env_ptr as *mut MettaEnvironment);

    // Convert JIT value to MettaValue
    let jit_val = JitValue::from_raw(initial_value);
    let metta_val = jit_val.to_metta();

    // Create state in environment
    let state_id = env.create_state(&metta_val);

    // Return State(id) as slab-allocated MettaValue
    let state_val = MettaValue::State(state_id);
    metta_to_jit(&state_val).to_bits()
}

/// Get the current value from a state cell.
///
/// Used by `get-state` operation. Retrieves the current value from a state cell.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `state_handle`: NaN-boxed State(id) value
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed current value, or error if state not found
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_get_state(
    ctx: *mut JitContext,
    state_handle: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("get-state: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("get-state: environment not available");
    }

    // Extract state ID from the handle
    let state_id = {
        let jit_val = JitValue::from_raw(state_handle);
        let metta_val = jit_val.to_metta();
        match metta_val.view() {
            ValueView::State(id) => id,
            _ => {
                return make_jit_error_with_details(
                    "get-state: expected State",
                    &format!("got {:?}", metta_val),
                );
            }
        }
    };

    // Optimization 5.1: Check state cache first
    if let Some(cached_value) = ctx_ref.state_cache_get(state_id) {
        return cached_value.to_bits();
    }

    // Cache miss: fetch from Environment (MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>)
    use crate::backend::bytecode::MettaEnvironment;
    let env = &*(ctx_ref.env_ptr as *const MettaEnvironment);

    // Get state value
    match env.get_state(state_id) {
        Some(value) => {
            // Convert to JIT value
            let jit_value = metta_to_jit(&value);

            // Update cache with the fetched value
            ctx_ref.state_cache_put(state_id, jit_value);

            jit_value.to_bits()
        }
        None => make_jit_error_with_details(
            "get-state: state not found",
            &format!("state_id={}", state_id),
        ),
    }
}

/// Change the value of a state cell.
///
/// Used by `change-state!` operation. Updates the value in a state cell
/// and returns the state handle.
///
/// # Arguments
/// - `ctx`: JIT context pointer (must have env_ptr set)
/// - `state_handle`: NaN-boxed State(id) value
/// - `new_value`: NaN-boxed new value for the state
/// - `_ip`: Instruction pointer (for bailout tracking)
///
/// # Returns
/// NaN-boxed State(id) value (same as input), or error if state not found
///
/// # Safety
/// - `ctx` must be a valid pointer to a JitContext with env_ptr set
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_change_state(
    ctx: *mut JitContext,
    state_handle: u64,
    new_value: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return make_jit_error("change-state!: null context"),
    };

    if ctx_ref.env_ptr.is_null() {
        return make_jit_error("change-state!: environment not available");
    }

    // Extract state ID from the handle
    let state_id = {
        let jit_val = JitValue::from_raw(state_handle);
        let metta_val = jit_val.to_metta();
        match metta_val.view() {
            ValueView::State(id) => id,
            _ => {
                return make_jit_error_with_details(
                    "change-state!: expected State",
                    &format!("got {:?}", metta_val),
                );
            }
        }
    };

    // Convert new value to MettaValue
    let jit_new_val = JitValue::from_raw(new_value);
    let metta_new_val = jit_new_val.to_metta();

    // Cast env_ptr to Environment (MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>)
    use crate::backend::bytecode::MettaEnvironment;
    let env = &mut *(ctx_ref.env_ptr as *mut MettaEnvironment);

    // Change state value
    if env.change_state(state_id, &metta_new_val) {
        // Optimization 5.1: Update cache with the new value
        // (more efficient than invalidating since next read will be a hit)
        ctx_ref.state_cache_put(state_id, jit_new_val);

        // Return the state handle unchanged
        state_handle
    } else {
        make_jit_error_with_details(
            "change-state!: state not found",
            &format!("state_id={}", state_id),
        )
    }
}
