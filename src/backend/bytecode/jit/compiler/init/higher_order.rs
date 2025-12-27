//! Higher-order operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for higher-order
//! runtime functions: map_atom, filter_atom, foldl_atom, decon_atom, repr.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for higher-order operations
pub struct HigherOrderFuncIds {
    /// Map function over list
    pub map_atom_func_id: FuncId,
    /// Filter list by predicate
    pub filter_atom_func_id: FuncId,
    /// Left fold over list
    pub foldl_atom_func_id: FuncId,
    /// Deconstruct atom to (head, tail)
    pub decon_atom_func_id: FuncId,
    /// Get string representation
    pub repr_func_id: FuncId,
}

/// Trait for higher-order initialization - zero-cost static dispatch
pub trait HigherOrderInit {
    /// Register higher-order runtime symbols with JIT builder
    fn register_higher_order_symbols(builder: &mut JITBuilder);

    /// Declare higher-order functions and return their FuncIds
    fn declare_higher_order_funcs<M: Module>(module: &mut M) -> JitResult<HigherOrderFuncIds>;
}

impl<T> HigherOrderInit for T {
    fn register_higher_order_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_map_atom",
            runtime::jit_runtime_map_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_filter_atom",
            runtime::jit_runtime_filter_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_foldl_atom",
            runtime::jit_runtime_foldl_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_decon_atom",
            runtime::jit_runtime_decon_atom as *const u8,
        );
        builder.symbol("jit_runtime_repr", runtime::jit_runtime_repr as *const u8);
    }

    fn declare_higher_order_funcs<M: Module>(module: &mut M) -> JitResult<HigherOrderFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // map_atom: fn(ctx, list, func_chunk, ip) -> result
        let mut map_atom_sig = module.make_signature();
        map_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        map_atom_sig.params.push(AbiParam::new(types::I64)); // list
        map_atom_sig.params.push(AbiParam::new(types::I64)); // func_chunk
        map_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        map_atom_sig.returns.push(AbiParam::new(types::I64)); // result

        let map_atom_func_id = module
            .declare_function("jit_runtime_map_atom", Linkage::Import, &map_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_map_atom: {}", e))
            })?;

        // filter_atom: fn(ctx, list, predicate_chunk, ip) -> result
        let mut filter_atom_sig = module.make_signature();
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // list
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // predicate_chunk
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        filter_atom_sig.returns.push(AbiParam::new(types::I64)); // result

        let filter_atom_func_id = module
            .declare_function("jit_runtime_filter_atom", Linkage::Import, &filter_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_filter_atom: {}",
                    e
                ))
            })?;

        // foldl_atom: fn(ctx, list, init, func_chunk, ip) -> result
        let mut foldl_atom_sig = module.make_signature();
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // list
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // init
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // func_chunk
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        foldl_atom_sig.returns.push(AbiParam::new(types::I64)); // result

        let foldl_atom_func_id = module
            .declare_function("jit_runtime_foldl_atom", Linkage::Import, &foldl_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_foldl_atom: {}",
                    e
                ))
            })?;

        // decon_atom: fn(ctx, value, ip) -> (head, tail) pair
        let mut decon_atom_sig = module.make_signature();
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // value
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        decon_atom_sig.returns.push(AbiParam::new(types::I64)); // result

        let decon_atom_func_id = module
            .declare_function("jit_runtime_decon_atom", Linkage::Import, &decon_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_decon_atom: {}",
                    e
                ))
            })?;

        // repr: fn(ctx, value, ip) -> string
        let mut repr_sig = module.make_signature();
        repr_sig.params.push(AbiParam::new(types::I64)); // ctx
        repr_sig.params.push(AbiParam::new(types::I64)); // value
        repr_sig.params.push(AbiParam::new(types::I64)); // ip
        repr_sig.returns.push(AbiParam::new(types::I64)); // result

        let repr_func_id = module
            .declare_function("jit_runtime_repr", Linkage::Import, &repr_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_repr: {}", e))
            })?;

        Ok(HigherOrderFuncIds {
            map_atom_func_id,
            filter_atom_func_id,
            foldl_atom_func_id,
            decon_atom_func_id,
            repr_func_id,
        })
    }
}
