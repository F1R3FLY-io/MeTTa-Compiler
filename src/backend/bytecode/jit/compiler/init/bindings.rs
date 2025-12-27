//! Binding function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for variable
//! binding runtime functions: load_binding, store_binding, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for binding operations
pub struct BindingFuncIds {
    /// Load binding by variable name hash
    pub load_binding_func_id: FuncId,
    /// Store binding by variable name hash
    pub store_binding_func_id: FuncId,
    /// Check if binding exists
    pub has_binding_func_id: FuncId,
    /// Clear all bindings
    pub clear_bindings_func_id: FuncId,
    /// Push new binding frame
    pub push_binding_frame_func_id: FuncId,
    /// Pop binding frame
    pub pop_binding_frame_func_id: FuncId,
}

/// Trait for binding initialization - zero-cost static dispatch
pub trait BindingsInit {
    /// Register binding runtime symbols with JIT builder
    fn register_bindings_symbols(builder: &mut JITBuilder);

    /// Declare binding functions and return their FuncIds
    fn declare_bindings_funcs<M: Module>(module: &mut M) -> JitResult<BindingFuncIds>;
}

impl<T> BindingsInit for T {
    fn register_bindings_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_load_binding",
            runtime::jit_runtime_load_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_store_binding",
            runtime::jit_runtime_store_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_has_binding",
            runtime::jit_runtime_has_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_clear_bindings",
            runtime::jit_runtime_clear_bindings as *const u8,
        );
        builder.symbol(
            "jit_runtime_push_binding_frame",
            runtime::jit_runtime_push_binding_frame as *const u8,
        );
        builder.symbol(
            "jit_runtime_pop_binding_frame",
            runtime::jit_runtime_pop_binding_frame as *const u8,
        );
    }

    fn declare_bindings_funcs<M: Module>(module: &mut M) -> JitResult<BindingFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // load_binding: fn(ctx, var_hash, ip) -> value
        let mut load_sig = module.make_signature();
        load_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_sig.params.push(AbiParam::new(types::I64)); // var_hash
        load_sig.params.push(AbiParam::new(types::I64)); // ip
        load_sig.returns.push(AbiParam::new(types::I64)); // value

        let load_binding_func_id = module
            .declare_function("jit_runtime_load_binding", Linkage::Import, &load_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_load_binding: {}",
                    e
                ))
            })?;

        // store_binding: fn(ctx, var_hash, value, ip) -> ()
        let mut store_sig = module.make_signature();
        store_sig.params.push(AbiParam::new(types::I64)); // ctx
        store_sig.params.push(AbiParam::new(types::I64)); // var_hash
        store_sig.params.push(AbiParam::new(types::I64)); // value
        store_sig.params.push(AbiParam::new(types::I64)); // ip

        let store_binding_func_id = module
            .declare_function("jit_runtime_store_binding", Linkage::Import, &store_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_store_binding: {}",
                    e
                ))
            })?;

        // has_binding: fn(ctx, name_idx) -> bool
        let mut has_sig = module.make_signature();
        has_sig.params.push(AbiParam::new(types::I64)); // ctx
        has_sig.params.push(AbiParam::new(types::I64)); // name_idx
        has_sig.returns.push(AbiParam::new(types::I64)); // bool

        let has_binding_func_id = module
            .declare_function("jit_runtime_has_binding", Linkage::Import, &has_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_has_binding: {}",
                    e
                ))
            })?;

        // clear_bindings: fn(ctx) -> ()
        let mut clear_sig = module.make_signature();
        clear_sig.params.push(AbiParam::new(types::I64)); // ctx

        let clear_bindings_func_id = module
            .declare_function("jit_runtime_clear_bindings", Linkage::Import, &clear_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_clear_bindings: {}",
                    e
                ))
            })?;

        // push_binding_frame: fn(ctx) -> ()
        let mut push_sig = module.make_signature();
        push_sig.params.push(AbiParam::new(types::I64)); // ctx

        let push_binding_frame_func_id = module
            .declare_function("jit_runtime_push_binding_frame", Linkage::Import, &push_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_push_binding_frame: {}",
                    e
                ))
            })?;

        // pop_binding_frame: fn(ctx) -> ()
        let mut pop_sig = module.make_signature();
        pop_sig.params.push(AbiParam::new(types::I64)); // ctx

        let pop_binding_frame_func_id = module
            .declare_function("jit_runtime_pop_binding_frame", Linkage::Import, &pop_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_pop_binding_frame: {}",
                    e
                ))
            })?;

        Ok(BindingFuncIds {
            load_binding_func_id,
            store_binding_func_id,
            has_binding_func_id,
            clear_bindings_func_id,
            push_binding_frame_func_id,
            pop_binding_frame_func_id,
        })
    }
}
