//! Debug and meta operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for debug/meta
//! runtime functions: trace, breakpoint, get_metatype, bloom_check, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for debug and meta operations
pub struct DebugFuncIds {
    /// Trace execution
    pub trace_func_id: FuncId,
    /// Set breakpoint
    pub breakpoint_func_id: FuncId,
    /// Get metatype of value
    pub get_metatype_func_id: FuncId,
    /// Check bloom filter
    pub bloom_check_func_id: FuncId,
    /// Return multiple values
    pub return_multi_func_id: FuncId,
    /// Collect up to N results
    pub collect_n_func_id: FuncId,
}

/// Trait for debug initialization - zero-cost static dispatch
pub trait DebugInit {
    /// Register debug runtime symbols with JIT builder
    fn register_debug_symbols(builder: &mut JITBuilder);

    /// Declare debug functions and return their FuncIds
    fn declare_debug_funcs<M: Module>(module: &mut M) -> JitResult<DebugFuncIds>;
}

impl<T> DebugInit for T {
    fn register_debug_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_trace", runtime::jit_runtime_trace as *const u8);
        builder.symbol("jit_runtime_breakpoint", runtime::jit_runtime_breakpoint as *const u8);
        builder.symbol("jit_runtime_get_metatype", runtime::jit_runtime_get_metatype as *const u8);
        builder.symbol("jit_runtime_bloom_check", runtime::jit_runtime_bloom_check as *const u8);
        builder.symbol("jit_runtime_return_multi", runtime::jit_runtime_return_multi as *const u8);
        builder.symbol("jit_runtime_collect_n", runtime::jit_runtime_collect_n as *const u8);
    }

    fn declare_debug_funcs<M: Module>(module: &mut M) -> JitResult<DebugFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // trace: fn(ctx, msg_idx, value, ip) -> void
        let mut trace_sig = module.make_signature();
        trace_sig.params.push(AbiParam::new(types::I64)); // ctx
        trace_sig.params.push(AbiParam::new(types::I64)); // msg_idx
        trace_sig.params.push(AbiParam::new(types::I64)); // value
        trace_sig.params.push(AbiParam::new(types::I64)); // ip
        // No return value for trace

        let trace_func_id = module
            .declare_function("jit_runtime_trace", Linkage::Import, &trace_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_trace: {}", e)))?;

        // breakpoint: fn(ctx, bp_id, ip) -> signal
        let mut breakpoint_sig = module.make_signature();
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // ctx
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // bp_id
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // ip
        breakpoint_sig.returns.push(AbiParam::new(types::I64)); // -1=pause, 0=continue

        let breakpoint_func_id = module
            .declare_function("jit_runtime_breakpoint", Linkage::Import, &breakpoint_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_breakpoint: {}", e)))?;

        // get_metatype: fn(ctx, value, ip) -> metatype atom
        let mut get_metatype_sig = module.make_signature();
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // value
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // ip
        get_metatype_sig.returns.push(AbiParam::new(types::I64)); // metatype

        let get_metatype_func_id = module
            .declare_function("jit_runtime_get_metatype", Linkage::Import, &get_metatype_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_get_metatype: {}", e)))?;

        // bloom_check: fn(ctx, key, ip) -> bool
        let mut bloom_check_sig = module.make_signature();
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // ctx
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // key
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // ip
        bloom_check_sig.returns.push(AbiParam::new(types::I64)); // bool

        let bloom_check_func_id = module
            .declare_function("jit_runtime_bloom_check", Linkage::Import, &bloom_check_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_bloom_check: {}", e)))?;

        // return_multi: fn(ctx, count, ip) -> signal
        let mut return_multi_sig = module.make_signature();
        return_multi_sig.params.push(AbiParam::new(types::I64)); // ctx
        return_multi_sig.params.push(AbiParam::new(types::I64)); // count
        return_multi_sig.params.push(AbiParam::new(types::I64)); // ip
        return_multi_sig.returns.push(AbiParam::new(types::I64)); // signal

        let return_multi_func_id = module
            .declare_function("jit_runtime_return_multi", Linkage::Import, &return_multi_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_return_multi: {}", e)))?;

        // collect_n: fn(ctx, max_count, ip) -> SExpr
        let mut collect_n_sig = module.make_signature();
        collect_n_sig.params.push(AbiParam::new(types::I64)); // ctx
        collect_n_sig.params.push(AbiParam::new(types::I64)); // max_count
        collect_n_sig.params.push(AbiParam::new(types::I64)); // ip
        collect_n_sig.returns.push(AbiParam::new(types::I64)); // SExpr

        let collect_n_func_id = module
            .declare_function("jit_runtime_collect_n", Linkage::Import, &collect_n_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_collect_n: {}", e)))?;

        Ok(DebugFuncIds {
            trace_func_id,
            breakpoint_func_id,
            get_metatype_func_id,
            bloom_check_func_id,
            return_multi_func_id,
            collect_n_func_id,
        })
    }
}
