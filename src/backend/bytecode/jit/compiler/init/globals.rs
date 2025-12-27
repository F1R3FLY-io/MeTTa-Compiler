//! Global and closure operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for global/space access
//! runtime functions: load_global, store_global, load_space, load_upvalue.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for global and closure operations
pub struct GlobalsFuncIds {
    /// Load global variable
    pub load_global_func_id: FuncId,
    /// Store global variable
    pub store_global_func_id: FuncId,
    /// Load space by name
    pub load_space_func_id: FuncId,
    /// Load upvalue from closure
    pub load_upvalue_func_id: FuncId,
}

/// Trait for globals initialization - zero-cost static dispatch
pub trait GlobalsInit {
    /// Register globals runtime symbols with JIT builder
    fn register_globals_symbols(builder: &mut JITBuilder);

    /// Declare globals functions and return their FuncIds
    fn declare_globals_funcs<M: Module>(module: &mut M) -> JitResult<GlobalsFuncIds>;
}

impl<T> GlobalsInit for T {
    fn register_globals_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_load_global", runtime::jit_runtime_load_global as *const u8);
        builder.symbol("jit_runtime_store_global", runtime::jit_runtime_store_global as *const u8);
        builder.symbol("jit_runtime_load_space", runtime::jit_runtime_load_space as *const u8);
        builder.symbol("jit_runtime_load_upvalue", runtime::jit_runtime_load_upvalue as *const u8);
    }

    fn declare_globals_funcs<M: Module>(module: &mut M) -> JitResult<GlobalsFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // load_global: fn(ctx, symbol_idx, ip) -> value
        let mut load_global_sig = module.make_signature();
        load_global_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_global_sig.params.push(AbiParam::new(types::I64)); // symbol_idx
        load_global_sig.params.push(AbiParam::new(types::I64)); // ip
        load_global_sig.returns.push(AbiParam::new(types::I64)); // value

        let load_global_func_id = module
            .declare_function("jit_runtime_load_global", Linkage::Import, &load_global_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_load_global: {}", e)))?;

        // store_global: fn(ctx, symbol_idx, value, ip) -> unit
        let mut store_global_sig = module.make_signature();
        store_global_sig.params.push(AbiParam::new(types::I64)); // ctx
        store_global_sig.params.push(AbiParam::new(types::I64)); // symbol_idx
        store_global_sig.params.push(AbiParam::new(types::I64)); // value
        store_global_sig.params.push(AbiParam::new(types::I64)); // ip
        store_global_sig.returns.push(AbiParam::new(types::I64)); // unit

        let store_global_func_id = module
            .declare_function("jit_runtime_store_global", Linkage::Import, &store_global_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_store_global: {}", e)))?;

        // load_space: fn(ctx, name_idx, ip) -> space_handle
        let mut load_space_sig = module.make_signature();
        load_space_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_space_sig.params.push(AbiParam::new(types::I64)); // name_idx
        load_space_sig.params.push(AbiParam::new(types::I64)); // ip
        load_space_sig.returns.push(AbiParam::new(types::I64)); // space_handle

        let load_space_func_id = module
            .declare_function("jit_runtime_load_space", Linkage::Import, &load_space_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_load_space: {}", e)))?;

        // load_upvalue: fn(ctx, depth, index, ip) -> value
        let mut load_upvalue_sig = module.make_signature();
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // depth
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // index
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // ip
        load_upvalue_sig.returns.push(AbiParam::new(types::I64)); // value

        let load_upvalue_func_id = module
            .declare_function("jit_runtime_load_upvalue", Linkage::Import, &load_upvalue_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_load_upvalue: {}", e)))?;

        Ok(GlobalsFuncIds {
            load_global_func_id,
            store_global_func_id,
            load_space_func_id,
            load_upvalue_func_id,
        })
    }
}
