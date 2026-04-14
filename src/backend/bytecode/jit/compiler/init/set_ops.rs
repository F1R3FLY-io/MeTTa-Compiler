//! Set operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for set operations
//! and alpha-equivalence runtime functions:
//! - eval_if_equal, unique_atom, union_atom, intersection_atom, subtraction_atom

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::{JitError, JitResult};

/// Function IDs for set operations
pub struct SetOpsFuncIds {
    /// if-equal: alpha-equivalence conditional
    pub eval_if_equal_func_id: FuncId,
    /// unique-atom: deduplicate list by alpha-equivalence (matches MeTTa HE)
    pub unique_atom_func_id: FuncId,
    /// alpha-unique-atom: explicit alias of unique-atom (alpha-equivalence)
    pub alpha_unique_atom_func_id: FuncId,
    /// struct-unique-atom: deduplicate list by structural equality (PeTTa)
    pub struct_unique_atom_func_id: FuncId,
    /// union-atom: concatenate lists
    pub union_atom_func_id: FuncId,
    /// intersection-atom: multiset intersection
    pub intersection_atom_func_id: FuncId,
    /// subtraction-atom: multiset subtraction
    pub subtraction_atom_func_id: FuncId,
    /// msort: numeric ascending sort
    pub msort_func_id: FuncId,
}

/// Trait for set operations initialization - zero-cost static dispatch
pub trait SetOpsInit {
    /// Register set operations runtime symbols with JIT builder
    fn register_set_ops_symbols(builder: &mut JITBuilder);

    /// Declare set operations functions and return their FuncIds
    fn declare_set_ops_funcs<M: Module>(module: &mut M) -> JitResult<SetOpsFuncIds>;
}

impl<T> SetOpsInit for T {
    fn register_set_ops_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_eval_if_equal",
            runtime::set_ops::jit_runtime_eval_if_equal as *const u8,
        );
        builder.symbol(
            "jit_runtime_unique_atom",
            runtime::set_ops::jit_runtime_unique_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_alpha_unique_atom",
            runtime::set_ops::jit_runtime_alpha_unique_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_struct_unique_atom",
            runtime::set_ops::jit_runtime_struct_unique_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_msort",
            runtime::set_ops::jit_runtime_msort as *const u8,
        );
        builder.symbol(
            "jit_runtime_union_atom",
            runtime::set_ops::jit_runtime_union_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_intersection_atom",
            runtime::set_ops::jit_runtime_intersection_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_subtraction_atom",
            runtime::set_ops::jit_runtime_subtraction_atom as *const u8,
        );
    }

    fn declare_set_ops_funcs<M: Module>(module: &mut M) -> JitResult<SetOpsFuncIds> {


        // eval_if_equal: fn(ctx, pred1, pred2, then, else, ip) -> result
        let mut if_equal_sig = module.make_signature();
        if_equal_sig.params.push(AbiParam::new(types::I64)); // ctx
        if_equal_sig.params.push(AbiParam::new(types::I64)); // pred1
        if_equal_sig.params.push(AbiParam::new(types::I64)); // pred2
        if_equal_sig.params.push(AbiParam::new(types::I64)); // then
        if_equal_sig.params.push(AbiParam::new(types::I64)); // else
        if_equal_sig.params.push(AbiParam::new(types::I64)); // ip
        if_equal_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_if_equal_func_id = module
            .declare_function(
                "jit_runtime_eval_if_equal",
                Linkage::Import,
                &if_equal_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_eval_if_equal: {}",
                    e
                ))
            })?;

        // unique_atom: fn(ctx, list, ip) -> result
        let mut unary_sig = module.make_signature();
        unary_sig.params.push(AbiParam::new(types::I64)); // ctx
        unary_sig.params.push(AbiParam::new(types::I64)); // list
        unary_sig.params.push(AbiParam::new(types::I64)); // ip
        unary_sig.returns.push(AbiParam::new(types::I64)); // result

        let unique_atom_func_id = module
            .declare_function("jit_runtime_unique_atom", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_unique_atom: {}",
                    e
                ))
            })?;

        let alpha_unique_atom_func_id = module
            .declare_function(
                "jit_runtime_alpha_unique_atom",
                Linkage::Import,
                &unary_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_alpha_unique_atom: {}",
                    e
                ))
            })?;

        let struct_unique_atom_func_id = module
            .declare_function(
                "jit_runtime_struct_unique_atom",
                Linkage::Import,
                &unary_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_struct_unique_atom: {}",
                    e
                ))
            })?;

        let msort_func_id = module
            .declare_function("jit_runtime_msort", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_msort: {}",
                    e
                ))
            })?;

        // binary ops: fn(ctx, left, right, ip) -> result
        let mut binary_sig = module.make_signature();
        binary_sig.params.push(AbiParam::new(types::I64)); // ctx
        binary_sig.params.push(AbiParam::new(types::I64)); // left
        binary_sig.params.push(AbiParam::new(types::I64)); // right
        binary_sig.params.push(AbiParam::new(types::I64)); // ip
        binary_sig.returns.push(AbiParam::new(types::I64)); // result

        let union_atom_func_id = module
            .declare_function("jit_runtime_union_atom", Linkage::Import, &binary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_union_atom: {}",
                    e
                ))
            })?;

        let intersection_atom_func_id = module
            .declare_function(
                "jit_runtime_intersection_atom",
                Linkage::Import,
                &binary_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_intersection_atom: {}",
                    e
                ))
            })?;

        let subtraction_atom_func_id = module
            .declare_function(
                "jit_runtime_subtraction_atom",
                Linkage::Import,
                &binary_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_subtraction_atom: {}",
                    e
                ))
            })?;

        Ok(SetOpsFuncIds {
            eval_if_equal_func_id,
            unique_atom_func_id,
            alpha_unique_atom_func_id,
            struct_unique_atom_func_id,
            union_atom_func_id,
            intersection_atom_func_id,
            subtraction_atom_func_id,
            msort_func_id,
        })
    }
}
