//! Pattern matching function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for pattern matching
//! runtime functions: pattern_match, pattern_match_bind, unify, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for pattern matching operations
pub struct PatternMatchingFuncIds {
    /// Pattern match without binding
    pub pattern_match_func_id: FuncId,
    /// Pattern match with variable binding
    pub pattern_match_bind_func_id: FuncId,
    /// Match head symbol
    pub match_head_func_id: FuncId,
    /// Match arity
    pub match_arity_func_id: FuncId,
    /// Unify two expressions
    pub unify_func_id: FuncId,
    /// Unify with binding
    pub unify_bind_func_id: FuncId,
}

/// Trait for pattern matching initialization - zero-cost static dispatch
pub trait PatternMatchingInit {
    /// Register pattern matching runtime symbols with JIT builder
    fn register_pattern_matching_symbols(builder: &mut JITBuilder);

    /// Declare pattern matching functions and return their FuncIds
    fn declare_pattern_matching_funcs<M: Module>(module: &mut M) -> JitResult<PatternMatchingFuncIds>;
}

impl<T> PatternMatchingInit for T {
    fn register_pattern_matching_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_pattern_match", runtime::jit_runtime_pattern_match as *const u8);
        builder.symbol("jit_runtime_pattern_match_bind", runtime::jit_runtime_pattern_match_bind as *const u8);
        builder.symbol("jit_runtime_match_head", runtime::jit_runtime_match_head as *const u8);
        builder.symbol("jit_runtime_match_arity", runtime::jit_runtime_match_arity as *const u8);
        builder.symbol("jit_runtime_unify", runtime::jit_runtime_unify as *const u8);
        builder.symbol("jit_runtime_unify_bind", runtime::jit_runtime_unify_bind as *const u8);
    }

    fn declare_pattern_matching_funcs<M: Module>(module: &mut M) -> JitResult<PatternMatchingFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // pattern_match: fn(ctx, value, pattern, ip) -> bool
        let mut pattern_match_sig = module.make_signature();
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // ctx
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // value
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // pattern
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // ip
        pattern_match_sig.returns.push(AbiParam::new(types::I64)); // bool

        let pattern_match_func_id = module
            .declare_function("jit_runtime_pattern_match", Linkage::Import, &pattern_match_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_pattern_match: {}", e)))?;

        // pattern_match_bind: fn(ctx, value, pattern, ip) -> bool
        let pattern_match_bind_func_id = module
            .declare_function("jit_runtime_pattern_match_bind", Linkage::Import, &pattern_match_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_pattern_match_bind: {}", e)))?;

        // match_head: fn(ctx, value, head_idx, ip) -> bool
        let mut match_head_sig = module.make_signature();
        match_head_sig.params.push(AbiParam::new(types::I64)); // ctx
        match_head_sig.params.push(AbiParam::new(types::I64)); // value
        match_head_sig.params.push(AbiParam::new(types::I64)); // head_idx
        match_head_sig.params.push(AbiParam::new(types::I64)); // ip
        match_head_sig.returns.push(AbiParam::new(types::I64)); // bool

        let match_head_func_id = module
            .declare_function("jit_runtime_match_head", Linkage::Import, &match_head_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_match_head: {}", e)))?;

        // match_arity: fn(ctx, value, arity, ip) -> bool
        let mut match_arity_sig = module.make_signature();
        match_arity_sig.params.push(AbiParam::new(types::I64)); // ctx
        match_arity_sig.params.push(AbiParam::new(types::I64)); // value
        match_arity_sig.params.push(AbiParam::new(types::I64)); // arity
        match_arity_sig.params.push(AbiParam::new(types::I64)); // ip
        match_arity_sig.returns.push(AbiParam::new(types::I64)); // bool

        let match_arity_func_id = module
            .declare_function("jit_runtime_match_arity", Linkage::Import, &match_arity_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_match_arity: {}", e)))?;

        // unify: fn(ctx, a, b, ip) -> bool
        let mut unify_sig = module.make_signature();
        unify_sig.params.push(AbiParam::new(types::I64)); // ctx
        unify_sig.params.push(AbiParam::new(types::I64)); // a
        unify_sig.params.push(AbiParam::new(types::I64)); // b
        unify_sig.params.push(AbiParam::new(types::I64)); // ip
        unify_sig.returns.push(AbiParam::new(types::I64)); // bool

        let unify_func_id = module
            .declare_function("jit_runtime_unify", Linkage::Import, &unify_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_unify: {}", e)))?;

        // unify_bind: fn(ctx, a, b, ip) -> bool
        let unify_bind_func_id = module
            .declare_function("jit_runtime_unify_bind", Linkage::Import, &unify_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_unify_bind: {}", e)))?;

        Ok(PatternMatchingFuncIds {
            pattern_match_func_id,
            pattern_match_bind_func_id,
            match_head_func_id,
            match_arity_func_id,
            unify_func_id,
            unify_bind_func_id,
        })
    }
}
