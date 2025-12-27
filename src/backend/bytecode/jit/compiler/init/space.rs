//! Space operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for space operations
//! runtime functions: space_add, space_remove, space_match, state ops, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for space operations
pub struct SpaceFuncIds {
    /// Add atom to space
    pub space_add_func_id: FuncId,
    /// Remove atom from space
    pub space_remove_func_id: FuncId,
    /// Get all atoms from space
    pub space_get_atoms_func_id: FuncId,
    /// Match pattern in space
    pub space_match_func_id: FuncId,
    /// Create new state
    pub new_state_func_id: FuncId,
    /// Get state value
    pub get_state_func_id: FuncId,
    /// Change state value
    pub change_state_func_id: FuncId,
}

/// Trait for space initialization - zero-cost static dispatch
pub trait SpaceInit {
    /// Register space runtime symbols with JIT builder
    fn register_space_symbols(builder: &mut JITBuilder);

    /// Declare space functions and return their FuncIds
    fn declare_space_funcs<M: Module>(module: &mut M) -> JitResult<SpaceFuncIds>;
}

impl<T> SpaceInit for T {
    fn register_space_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_space_add",
            runtime::jit_runtime_space_add as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_remove",
            runtime::jit_runtime_space_remove as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_get_atoms",
            runtime::jit_runtime_space_get_atoms as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_match_nondet",
            runtime::jit_runtime_space_match_nondet as *const u8,
        );
        builder.symbol(
            "jit_runtime_new_state",
            runtime::jit_runtime_new_state as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_state",
            runtime::jit_runtime_get_state as *const u8,
        );
        builder.symbol(
            "jit_runtime_change_state",
            runtime::jit_runtime_change_state as *const u8,
        );
    }

    fn declare_space_funcs<M: Module>(module: &mut M) -> JitResult<SpaceFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // space_add: fn(ctx, space, atom, ip) -> u64 (Bool)
        let mut space_add_sig = module.make_signature();
        space_add_sig.params.push(AbiParam::new(types::I64)); // ctx
        space_add_sig.params.push(AbiParam::new(types::I64)); // space
        space_add_sig.params.push(AbiParam::new(types::I64)); // atom
        space_add_sig.params.push(AbiParam::new(types::I64)); // ip
        space_add_sig.returns.push(AbiParam::new(types::I64)); // result

        let space_add_func_id = module
            .declare_function("jit_runtime_space_add", Linkage::Import, &space_add_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_space_add: {}",
                    e
                ))
            })?;

        // space_remove: fn(ctx, space, atom, ip) -> u64 (Bool)
        let mut space_remove_sig = module.make_signature();
        space_remove_sig.params.push(AbiParam::new(types::I64)); // ctx
        space_remove_sig.params.push(AbiParam::new(types::I64)); // space
        space_remove_sig.params.push(AbiParam::new(types::I64)); // atom
        space_remove_sig.params.push(AbiParam::new(types::I64)); // ip
        space_remove_sig.returns.push(AbiParam::new(types::I64)); // result

        let space_remove_func_id = module
            .declare_function(
                "jit_runtime_space_remove",
                Linkage::Import,
                &space_remove_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_space_remove: {}",
                    e
                ))
            })?;

        // space_get_atoms: fn(ctx, space, ip) -> u64 (SExpr)
        let mut space_get_atoms_sig = module.make_signature();
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // ctx
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // space
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // ip
        space_get_atoms_sig.returns.push(AbiParam::new(types::I64)); // result

        let space_get_atoms_func_id = module
            .declare_function(
                "jit_runtime_space_get_atoms",
                Linkage::Import,
                &space_get_atoms_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_space_get_atoms: {}",
                    e
                ))
            })?;

        // space_match_nondet: fn(ctx, space, pattern, template, ip) -> u64 (SExpr)
        let mut space_match_sig = module.make_signature();
        space_match_sig.params.push(AbiParam::new(types::I64)); // ctx
        space_match_sig.params.push(AbiParam::new(types::I64)); // space
        space_match_sig.params.push(AbiParam::new(types::I64)); // pattern
        space_match_sig.params.push(AbiParam::new(types::I64)); // template
        space_match_sig.params.push(AbiParam::new(types::I64)); // ip
        space_match_sig.returns.push(AbiParam::new(types::I64)); // result

        let space_match_func_id = module
            .declare_function(
                "jit_runtime_space_match_nondet",
                Linkage::Import,
                &space_match_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_space_match_nondet: {}",
                    e
                ))
            })?;

        // new_state: fn(ctx, initial_value, ip) -> state_handle
        let mut new_state_sig = module.make_signature();
        new_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        new_state_sig.params.push(AbiParam::new(types::I64)); // initial_value
        new_state_sig.params.push(AbiParam::new(types::I64)); // ip
        new_state_sig.returns.push(AbiParam::new(types::I64)); // state_handle

        let new_state_func_id = module
            .declare_function("jit_runtime_new_state", Linkage::Import, &new_state_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_new_state: {}",
                    e
                ))
            })?;

        // get_state: fn(ctx, state_handle, ip) -> value
        let mut get_state_sig = module.make_signature();
        get_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_state_sig.params.push(AbiParam::new(types::I64)); // state_handle
        get_state_sig.params.push(AbiParam::new(types::I64)); // ip
        get_state_sig.returns.push(AbiParam::new(types::I64)); // value

        let get_state_func_id = module
            .declare_function("jit_runtime_get_state", Linkage::Import, &get_state_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_state: {}",
                    e
                ))
            })?;

        // change_state: fn(ctx, state_handle, new_value, ip) -> state_handle
        let mut change_state_sig = module.make_signature();
        change_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        change_state_sig.params.push(AbiParam::new(types::I64)); // state_handle
        change_state_sig.params.push(AbiParam::new(types::I64)); // new_value
        change_state_sig.params.push(AbiParam::new(types::I64)); // ip
        change_state_sig.returns.push(AbiParam::new(types::I64)); // state_handle

        let change_state_func_id = module
            .declare_function(
                "jit_runtime_change_state",
                Linkage::Import,
                &change_state_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_change_state: {}",
                    e
                ))
            })?;

        Ok(SpaceFuncIds {
            space_add_func_id,
            space_remove_func_id,
            space_get_atoms_func_id,
            space_match_func_id,
            new_state_func_id,
            get_state_func_id,
            change_state_func_id,
        })
    }
}
