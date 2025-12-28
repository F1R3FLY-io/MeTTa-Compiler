//! S-expression operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for S-expression
//! runtime functions: push_empty, get_head, get_tail, make_sexpr, cons_atom, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for S-expression operations
pub struct SExprFuncIds {
    /// Push empty expression
    pub push_empty_func_id: FuncId,
    /// Get head of expression
    pub get_head_func_id: FuncId,
    /// Get tail of expression
    pub get_tail_func_id: FuncId,
    /// Get arity of expression
    pub get_arity_func_id: FuncId,
    /// Get element at index
    pub get_element_func_id: FuncId,
    /// Make new S-expression
    pub make_sexpr_func_id: FuncId,
    /// Cons atom to expression
    pub cons_atom_func_id: FuncId,
    /// Make list from stack values
    pub make_list_func_id: FuncId,
    /// Make quoted expression
    pub make_quote_func_id: FuncId,
}

/// Trait for S-expression initialization - zero-cost static dispatch
pub trait SExprInit {
    /// Register S-expression runtime symbols with JIT builder
    fn register_sexpr_symbols(builder: &mut JITBuilder);

    /// Declare S-expression functions and return their FuncIds
    fn declare_sexpr_funcs<M: Module>(module: &mut M) -> JitResult<SExprFuncIds>;
}

impl<T> SExprInit for T {
    fn register_sexpr_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_push_empty",
            runtime::jit_runtime_push_empty as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_head",
            runtime::jit_runtime_get_head as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_tail",
            runtime::jit_runtime_get_tail as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_arity",
            runtime::jit_runtime_get_arity as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_element",
            runtime::jit_runtime_get_element as *const u8,
        );
        builder.symbol(
            "jit_runtime_make_sexpr",
            runtime::jit_runtime_make_sexpr as *const u8,
        );
        builder.symbol(
            "jit_runtime_cons_atom",
            runtime::jit_runtime_cons_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_make_list",
            runtime::jit_runtime_make_list as *const u8,
        );
        builder.symbol(
            "jit_runtime_make_quote",
            runtime::jit_runtime_make_quote as *const u8,
        );
    }

    fn declare_sexpr_funcs<M: Module>(module: &mut M) -> JitResult<SExprFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // push_empty: fn() -> empty_sexpr (no parameters)
        let mut push_empty_sig = module.make_signature();
        push_empty_sig.returns.push(AbiParam::new(types::I64)); // empty_sexpr

        let push_empty_func_id = module
            .declare_function("jit_runtime_push_empty", Linkage::Import, &push_empty_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_push_empty: {}",
                    e
                ))
            })?;

        // get_head: fn(ctx, sexpr, ip) -> head
        let mut get_head_sig = module.make_signature();
        get_head_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_head_sig.params.push(AbiParam::new(types::I64)); // sexpr
        get_head_sig.params.push(AbiParam::new(types::I64)); // ip
        get_head_sig.returns.push(AbiParam::new(types::I64)); // head

        let get_head_func_id = module
            .declare_function("jit_runtime_get_head", Linkage::Import, &get_head_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_get_head: {}", e))
            })?;

        // get_tail: fn(ctx, sexpr, ip) -> tail
        let mut get_tail_sig = module.make_signature();
        get_tail_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_tail_sig.params.push(AbiParam::new(types::I64)); // sexpr
        get_tail_sig.params.push(AbiParam::new(types::I64)); // ip
        get_tail_sig.returns.push(AbiParam::new(types::I64)); // tail

        let get_tail_func_id = module
            .declare_function("jit_runtime_get_tail", Linkage::Import, &get_tail_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_get_tail: {}", e))
            })?;

        // get_arity: fn(ctx, sexpr, ip) -> arity
        let mut get_arity_sig = module.make_signature();
        get_arity_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_arity_sig.params.push(AbiParam::new(types::I64)); // sexpr
        get_arity_sig.params.push(AbiParam::new(types::I64)); // ip
        get_arity_sig.returns.push(AbiParam::new(types::I64)); // arity

        let get_arity_func_id = module
            .declare_function("jit_runtime_get_arity", Linkage::Import, &get_arity_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_arity: {}",
                    e
                ))
            })?;

        // get_element: fn(ctx, sexpr, index, ip) -> element
        let mut get_element_sig = module.make_signature();
        get_element_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_element_sig.params.push(AbiParam::new(types::I64)); // sexpr
        get_element_sig.params.push(AbiParam::new(types::I64)); // index
        get_element_sig.params.push(AbiParam::new(types::I64)); // ip
        get_element_sig.returns.push(AbiParam::new(types::I64)); // element

        let get_element_func_id = module
            .declare_function("jit_runtime_get_element", Linkage::Import, &get_element_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_element: {}",
                    e
                ))
            })?;

        // make_sexpr: fn(ctx, values_ptr, count, ip) -> sexpr
        let mut make_sexpr_sig = module.make_signature();
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // ctx
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // values_ptr
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // count
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // ip
        make_sexpr_sig.returns.push(AbiParam::new(types::I64)); // sexpr

        let make_sexpr_func_id = module
            .declare_function("jit_runtime_make_sexpr", Linkage::Import, &make_sexpr_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_sexpr: {}",
                    e
                ))
            })?;

        // cons_atom: fn(ctx, head, tail, ip) -> sexpr
        let mut cons_atom_sig = module.make_signature();
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // head
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // tail
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        cons_atom_sig.returns.push(AbiParam::new(types::I64)); // sexpr

        let cons_atom_func_id = module
            .declare_function("jit_runtime_cons_atom", Linkage::Import, &cons_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_cons_atom: {}",
                    e
                ))
            })?;

        // make_list: fn(ctx, values_ptr, count, ip) -> list
        let mut make_list_sig = module.make_signature();
        make_list_sig.params.push(AbiParam::new(types::I64)); // ctx
        make_list_sig.params.push(AbiParam::new(types::I64)); // values_ptr
        make_list_sig.params.push(AbiParam::new(types::I64)); // count
        make_list_sig.params.push(AbiParam::new(types::I64)); // ip
        make_list_sig.returns.push(AbiParam::new(types::I64)); // list

        let make_list_func_id = module
            .declare_function("jit_runtime_make_list", Linkage::Import, &make_list_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_list: {}",
                    e
                ))
            })?;

        // make_quote: fn(ctx, expr, ip) -> quoted
        let mut make_quote_sig = module.make_signature();
        make_quote_sig.params.push(AbiParam::new(types::I64)); // ctx
        make_quote_sig.params.push(AbiParam::new(types::I64)); // expr
        make_quote_sig.params.push(AbiParam::new(types::I64)); // ip
        make_quote_sig.returns.push(AbiParam::new(types::I64)); // quoted

        let make_quote_func_id = module
            .declare_function("jit_runtime_make_quote", Linkage::Import, &make_quote_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_quote: {}",
                    e
                ))
            })?;

        Ok(SExprFuncIds {
            push_empty_func_id,
            get_head_func_id,
            get_tail_func_id,
            get_arity_func_id,
            get_element_func_id,
            make_sexpr_func_id,
            cons_atom_func_id,
            make_list_func_id,
            make_quote_func_id,
        })
    }
}
