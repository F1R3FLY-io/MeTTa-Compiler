//! Global and closure runtime functions for JIT compilation
//!
//! This module provides FFI-callable global and closure operations:
//! - load_global - Load global variable by symbol index
//! - store_global - Store global variable by symbol index
//! - load_space - Load space handle by name index
//! - load_grounded_space - Load pre-resolved grounded space by index
//! - load_upvalue - Load variable from enclosing scope (closures)
//!
//! Also provides helper functions:
//! - grounded_space_index - Get grounded space index from name
//! - is_grounded_ref - Check if a name is a grounded reference

use super::helpers::metta_to_jit;
use crate::backend::bytecode::jit::types::{JitContext, JitValue};
use crate::backend::bytecode::mork_bridge::MorkBridge;
use crate::backend::models::MettaValue;

// =============================================================================
// Phase 1.5: Global/Space Access - LoadGlobal, StoreGlobal, LoadSpace
// =============================================================================

/// Phase 1.5: Load global variable by symbol index
///
/// Looks up a global variable by its symbol index. First checks type annotations,
/// then falls back to checking if the atom exists in the space.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed value of the global, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_global(
    ctx: *const JitContext,
    symbol_idx: u64,
    _ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_ref() {
        Some(c) => c,
        None => return JitValue::nil().to_bits(),
    };

    // Get symbol name from constants
    let symbol_name = if symbol_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(symbol_idx as usize);
        let constant = &*constant_ptr;
        match constant {
            MettaValue::Atom(name) => Some(name.clone()),
            _ => None,
        }
    } else {
        None
    };

    let name = match symbol_name {
        Some(n) => n,
        None => return JitValue::nil().to_bits(),
    };

    // Try to look up through the bridge if available
    if !ctx_ref.bridge_ptr.is_null() {
        let bridge = &*(ctx_ref.bridge_ptr as *const MorkBridge);
        let env_arc = bridge.environment();
        let env_guard = env_arc.read();

        // First check if there's a type annotation for this symbol
        if let Ok(env_read) = env_guard {
            if let Some(type_val) = env_read.get_type(&name) {
                return metta_to_jit(&type_val).to_bits();
            }

            // Check if the atom itself exists in the space
            if env_read.has_fact(&name) {
                // Return the atom itself as the value
                return metta_to_jit(&MettaValue::Atom(name)).to_bits();
            }
        }
    }

    // Not found - return Nil
    JitValue::nil().to_bits()
}

/// Phase 1.5: Store global variable by symbol index
///
/// Stores a value in the global variable registry by its symbol index.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// Unit value on success
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_store_global(
    _ctx: *mut JitContext,
    symbol_idx: u64,
    _value: u64,
    _ip: u64,
) -> u64 {
    // Placeholder implementation - full implementation needs
    // mutable access to global registry
    let _ = symbol_idx;
    JitValue::unit().to_bits()
}

/// Phase 1.5: Load space handle by name index
///
/// Retrieves a space handle by its name. Spaces are named containers for atoms.
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed space handle (currently returns Nil as placeholder)
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_space(
    ctx: *const JitContext,
    name_idx: u64,
    _ip: u64,
) -> u64 {
    use crate::backend::bytecode::space_registry::SpaceRegistry;
    use crate::backend::models::SpaceHandle;

    let ctx_ref = ctx.as_ref().expect("load_space: null context");

    // Get space name from constants
    let space_name = if name_idx < ctx_ref.constants_len as u64 {
        let constant_ptr = ctx_ref.constants.add(name_idx as usize);
        let constant = &*constant_ptr;
        match constant {
            MettaValue::Atom(name) => name.clone(),
            MettaValue::String(name) => name.clone(),
            _ => {
                // Not a valid space name
                return JitValue::nil().to_bits();
            }
        }
    } else {
        return JitValue::nil().to_bits();
    };

    // Space Ops Phase 2: Check for grounded references first
    // If this is a grounded reference and we have pre-resolved spaces, use them
    let grounded_idx = grounded_space_index(&space_name);
    if grounded_idx != u64::MAX && ctx_ref.has_grounded_spaces() {
        return jit_runtime_load_grounded_space(ctx, grounded_idx);
    }

    // If we have a space registry, use it to get or create the named space
    if !ctx_ref.space_registry.is_null() {
        let registry = &*(ctx_ref.space_registry as *const SpaceRegistry);
        let space = registry.get_or_create(&space_name);
        return metta_to_jit(&MettaValue::Space(space)).to_bits();
    }

    // Fallback: create a standalone space with the name (not shared)
    // This maintains backwards compatibility when no registry is available
    let space_id = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        space_name.hash(&mut hasher);
        hasher.finish()
    };
    let space = SpaceHandle::new(space_id, space_name);
    metta_to_jit(&MettaValue::Space(space)).to_bits()
}

/// Space Ops Phase 1: Load pre-resolved grounded space by index
///
/// Retrieves a grounded space (&self, &kb, &stack) by its index.
/// These spaces are pre-resolved at JIT entry for O(1) access.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `index` - Grounded space index: 0 = &self, 1 = &kb, 2 = &stack
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
/// - grounded_spaces must have been set via set_grounded_spaces()
///
/// # Returns
/// NaN-boxed space handle, or Nil if index out of bounds or not available
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_grounded_space(
    ctx: *const JitContext,
    index: u64,
) -> u64 {
    use crate::backend::models::SpaceHandle;

    let ctx = ctx.as_ref().expect("load_grounded_space: null context");

    // Check bounds
    let index = index as usize;
    if index >= ctx.grounded_spaces_count {
        return JitValue::nil().to_bits();
    }

    // Check if grounded spaces are set
    if ctx.grounded_spaces.is_null() {
        return JitValue::nil().to_bits();
    }

    // Load the pre-resolved space handle pointer
    let space_ptr = *ctx.grounded_spaces.add(index);
    if space_ptr.is_null() {
        return JitValue::nil().to_bits();
    }

    // The space_ptr points to a SpaceHandle
    let space_handle = &*(space_ptr as *const SpaceHandle);

    // Return as NaN-boxed heap pointer to a Space value
    // We need to create a MettaValue::Space and return its heap pointer
    metta_to_jit(&MettaValue::Space(space_handle.clone())).to_bits()
}

/// Space Ops Phase 1: Get grounded space index from name
///
/// Maps grounded reference names to indices:
/// - "self" or "&self" -> 0
/// - "kb" or "&kb" -> 1
/// - "stack" or "&stack" -> 2
///
/// # Returns
/// Index (0-2) or u64::MAX if not a grounded reference
#[inline]
pub fn grounded_space_index(name: &str) -> u64 {
    match name {
        "self" | "&self" => 0,
        "kb" | "&kb" => 1,
        "stack" | "&stack" => 2,
        _ => u64::MAX,
    }
}

/// Space Ops Phase 1: Check if a name is a grounded reference
#[inline]
pub fn is_grounded_ref(name: &str) -> bool {
    grounded_space_index(name) != u64::MAX
}

// =============================================================================
// Phase 1.6: Closure Support - LoadUpvalue
// =============================================================================

/// Phase 1.6: Load variable from enclosing scope (closures)
///
/// Traverses upvalue frames to load a captured variable.
/// - `depth`: How many frames up to traverse
/// - `index`: Index of variable within that frame
///
/// # Safety
/// - ctx must be a valid pointer to a JitContext
///
/// # Returns
/// NaN-boxed value from the closure environment, or Nil if not found
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_load_upvalue(
    ctx: *const JitContext,
    depth: u64,
    index: u64,
    _ip: u64,
) -> u64 {
    let ctx = ctx.as_ref().expect("load_upvalue: null context");

    // Traverse binding frames to find the value
    // Each frame represents a scope level
    let depth = depth as usize;
    let index = index as usize;

    // Calculate target frame (binding frames are in reverse order)
    if ctx.binding_frames_count <= depth {
        return JitValue::nil().to_bits();
    }

    let frame_idx = ctx.binding_frames_count - 1 - depth;
    let frame = ctx.binding_frames.add(frame_idx);
    let frame_ref = &*frame;

    // Get value from that frame at the given index
    if index < frame_ref.entries_count {
        let entry = frame_ref.entries.add(index);
        let entry_ref = &*entry;
        return entry_ref.value.to_bits();
    }

    JitValue::nil().to_bits()
}
