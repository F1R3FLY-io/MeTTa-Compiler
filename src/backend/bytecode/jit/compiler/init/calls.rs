//! Call function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for call
//! runtime functions: call, tail_call, call_n, call_native, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for call operations
pub struct CallFuncIds {
    /// Call function with constant head symbol
    pub call_func_id: FuncId,
    /// Tail call with constant head symbol
    pub tail_call_func_id: FuncId,
    /// Call with stack-based head
    pub call_n_func_id: FuncId,
    /// Tail call with stack-based head
    pub tail_call_n_func_id: FuncId,
    /// Call native Rust function
    pub call_native_func_id: FuncId,
    /// Call external function
    pub call_external_func_id: FuncId,
    /// Call with caching
    pub call_cached_func_id: FuncId,
}

/// Trait for call initialization - zero-cost static dispatch
pub trait CallsInit {
    /// Register call runtime symbols with JIT builder
    fn register_calls_symbols(builder: &mut JITBuilder);

    /// Declare call functions and return their FuncIds
    fn declare_calls_funcs<M: Module>(module: &mut M) -> JitResult<CallFuncIds>;
}

impl<T> CallsInit for T {
    fn register_calls_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_call", runtime::jit_runtime_call as *const u8);
        builder.symbol(
            "jit_runtime_tail_call",
            runtime::jit_runtime_tail_call as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_n",
            runtime::jit_runtime_call_n as *const u8,
        );
        builder.symbol(
            "jit_runtime_tail_call_n",
            runtime::jit_runtime_tail_call_n as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_native",
            runtime::jit_runtime_call_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_external",
            runtime::jit_runtime_call_external as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_cached",
            runtime::jit_runtime_call_cached as *const u8,
        );
    }

    fn declare_calls_funcs<M: Module>(module: &mut M) -> JitResult<CallFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // call: fn(ctx, head_idx, args_ptr, arity, ip) -> result
        let mut call_sig = module.make_signature();
        call_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_sig.params.push(AbiParam::new(types::I64)); // head_idx
        call_sig.params.push(AbiParam::new(types::I64)); // args_ptr
        call_sig.params.push(AbiParam::new(types::I64)); // arity
        call_sig.params.push(AbiParam::new(types::I64)); // ip
        call_sig.returns.push(AbiParam::new(types::I64)); // result

        let call_func_id = module
            .declare_function("jit_runtime_call", Linkage::Import, &call_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_call: {}", e))
            })?;

        let tail_call_func_id = module
            .declare_function("jit_runtime_tail_call", Linkage::Import, &call_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_tail_call: {}",
                    e
                ))
            })?;

        // call_n: fn(ctx, head_val, args_ptr, arity, ip) -> result (head passed as value)
        let mut call_n_sig = module.make_signature();
        call_n_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_n_sig.params.push(AbiParam::new(types::I64)); // head_val
        call_n_sig.params.push(AbiParam::new(types::I64)); // args_ptr
        call_n_sig.params.push(AbiParam::new(types::I64)); // arity
        call_n_sig.params.push(AbiParam::new(types::I64)); // ip
        call_n_sig.returns.push(AbiParam::new(types::I64)); // result

        let call_n_func_id = module
            .declare_function("jit_runtime_call_n", Linkage::Import, &call_n_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_call_n: {}", e))
            })?;

        let tail_call_n_func_id = module
            .declare_function("jit_runtime_tail_call_n", Linkage::Import, &call_n_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_tail_call_n: {}",
                    e
                ))
            })?;

        // call_native: fn(ctx, func_ptr, arity, ip) -> result
        let mut call_native_sig = module.make_signature();
        call_native_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_native_sig.params.push(AbiParam::new(types::I64)); // func_ptr
        call_native_sig.params.push(AbiParam::new(types::I64)); // arity
        call_native_sig.params.push(AbiParam::new(types::I64)); // ip
        call_native_sig.returns.push(AbiParam::new(types::I64)); // result

        let call_native_func_id = module
            .declare_function("jit_runtime_call_native", Linkage::Import, &call_native_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_call_native: {}",
                    e
                ))
            })?;

        // call_external: fn(ctx, ext_idx, arity, ip) -> result
        let mut call_external_sig = module.make_signature();
        call_external_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_external_sig.params.push(AbiParam::new(types::I64)); // ext_idx
        call_external_sig.params.push(AbiParam::new(types::I64)); // arity
        call_external_sig.params.push(AbiParam::new(types::I64)); // ip
        call_external_sig.returns.push(AbiParam::new(types::I64)); // result

        let call_external_func_id = module
            .declare_function(
                "jit_runtime_call_external",
                Linkage::Import,
                &call_external_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_call_external: {}",
                    e
                ))
            })?;

        // call_cached: fn(ctx, head_idx, arg_count, ip) -> result
        let mut call_cached_sig = module.make_signature();
        call_cached_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_cached_sig.params.push(AbiParam::new(types::I64)); // head_idx
        call_cached_sig.params.push(AbiParam::new(types::I64)); // arg_count
        call_cached_sig.params.push(AbiParam::new(types::I64)); // ip
        call_cached_sig.returns.push(AbiParam::new(types::I64)); // result

        let call_cached_func_id = module
            .declare_function("jit_runtime_call_cached", Linkage::Import, &call_cached_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_call_cached: {}",
                    e
                ))
            })?;

        Ok(CallFuncIds {
            call_func_id,
            tail_call_func_id,
            call_n_func_id,
            tail_call_n_func_id,
            call_native_func_id,
            call_external_func_id,
            call_cached_func_id,
        })
    }
}
