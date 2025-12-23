//! Special forms function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for special forms
//! runtime functions: eval_if, eval_let, eval_match, eval_quote, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for special form operations
pub struct SpecialFormsFuncIds {
    /// Conditional evaluation
    pub eval_if_func_id: FuncId,
    /// Let binding
    pub eval_let_func_id: FuncId,
    /// Sequential let binding
    pub eval_let_star_func_id: FuncId,
    /// Pattern match evaluation
    pub eval_match_func_id: FuncId,
    /// Case dispatch
    pub eval_case_func_id: FuncId,
    /// Chain expressions
    pub eval_chain_func_id: FuncId,
    /// Quote expression
    pub eval_quote_func_id: FuncId,
    /// Unquote expression
    pub eval_unquote_func_id: FuncId,
    /// Force evaluation
    pub eval_eval_func_id: FuncId,
    /// Bind name to value
    pub eval_bind_func_id: FuncId,
    /// Create new space
    pub eval_new_func_id: FuncId,
    /// Collapse nondeterminism
    pub eval_collapse_func_id: FuncId,
    /// Superpose alternatives
    pub eval_superpose_func_id: FuncId,
    /// Memoize evaluation
    pub eval_memo_func_id: FuncId,
    /// Memoize first result
    pub eval_memo_first_func_id: FuncId,
    /// Pragma directive
    pub eval_pragma_func_id: FuncId,
    /// Define function
    pub eval_function_func_id: FuncId,
    /// Create lambda
    pub eval_lambda_func_id: FuncId,
    /// Apply function
    pub eval_apply_func_id: FuncId,
}

/// Trait for special forms initialization - zero-cost static dispatch
pub trait SpecialFormsInit {
    /// Register special forms runtime symbols with JIT builder
    fn register_special_forms_symbols(builder: &mut JITBuilder);

    /// Declare special forms functions and return their FuncIds
    fn declare_special_forms_funcs<M: Module>(module: &mut M) -> JitResult<SpecialFormsFuncIds>;
}

impl<T> SpecialFormsInit for T {
    fn register_special_forms_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_eval_if", runtime::jit_runtime_eval_if as *const u8);
        builder.symbol("jit_runtime_eval_let", runtime::jit_runtime_eval_let as *const u8);
        builder.symbol("jit_runtime_eval_let_star", runtime::jit_runtime_eval_let_star as *const u8);
        builder.symbol("jit_runtime_eval_match", runtime::jit_runtime_eval_match as *const u8);
        builder.symbol("jit_runtime_eval_case", runtime::jit_runtime_eval_case as *const u8);
        builder.symbol("jit_runtime_eval_chain", runtime::jit_runtime_eval_chain as *const u8);
        builder.symbol("jit_runtime_eval_quote", runtime::jit_runtime_eval_quote as *const u8);
        builder.symbol("jit_runtime_eval_unquote", runtime::jit_runtime_eval_unquote as *const u8);
        builder.symbol("jit_runtime_eval_eval", runtime::jit_runtime_eval_eval as *const u8);
        builder.symbol("jit_runtime_eval_bind", runtime::jit_runtime_eval_bind as *const u8);
        builder.symbol("jit_runtime_eval_new", runtime::jit_runtime_eval_new as *const u8);
        builder.symbol("jit_runtime_eval_collapse", runtime::jit_runtime_eval_collapse as *const u8);
        builder.symbol("jit_runtime_eval_superpose", runtime::jit_runtime_eval_superpose as *const u8);
        builder.symbol("jit_runtime_eval_memo", runtime::jit_runtime_eval_memo as *const u8);
        builder.symbol("jit_runtime_eval_memo_first", runtime::jit_runtime_eval_memo_first as *const u8);
        builder.symbol("jit_runtime_eval_pragma", runtime::jit_runtime_eval_pragma as *const u8);
        builder.symbol("jit_runtime_eval_function", runtime::jit_runtime_eval_function as *const u8);
        builder.symbol("jit_runtime_eval_lambda", runtime::jit_runtime_eval_lambda as *const u8);
        builder.symbol("jit_runtime_eval_apply", runtime::jit_runtime_eval_apply as *const u8);
    }

    fn declare_special_forms_funcs<M: Module>(module: &mut M) -> JitResult<SpecialFormsFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // eval_if: fn(ctx, condition, then_val, else_val, ip) -> result
        let mut eval_if_sig = module.make_signature();
        eval_if_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_if_sig.params.push(AbiParam::new(types::I64)); // condition
        eval_if_sig.params.push(AbiParam::new(types::I64)); // then_val
        eval_if_sig.params.push(AbiParam::new(types::I64)); // else_val
        eval_if_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_if_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_if_func_id = module
            .declare_function("jit_runtime_eval_if", Linkage::Import, &eval_if_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_if: {}", e)))?;

        // eval_let: fn(ctx, name_idx, value, ip) -> Unit
        let mut eval_let_sig = module.make_signature();
        eval_let_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_let_sig.params.push(AbiParam::new(types::I64)); // name_idx
        eval_let_sig.params.push(AbiParam::new(types::I64)); // value
        eval_let_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_let_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_let_func_id = module
            .declare_function("jit_runtime_eval_let", Linkage::Import, &eval_let_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_let: {}", e)))?;

        // eval_let_star: fn(ctx, ip) -> Unit
        let mut eval_let_star_sig = module.make_signature();
        eval_let_star_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_let_star_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_let_star_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_let_star_func_id = module
            .declare_function("jit_runtime_eval_let_star", Linkage::Import, &eval_let_star_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_let_star: {}", e)))?;

        // eval_match: fn(ctx, value, pattern, ip) -> bool
        let mut eval_match_sig = module.make_signature();
        eval_match_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_match_sig.params.push(AbiParam::new(types::I64)); // value
        eval_match_sig.params.push(AbiParam::new(types::I64)); // pattern
        eval_match_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_match_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_match_func_id = module
            .declare_function("jit_runtime_eval_match", Linkage::Import, &eval_match_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_match: {}", e)))?;

        // eval_case: fn(ctx, value, case_count, ip) -> case_index
        let mut eval_case_sig = module.make_signature();
        eval_case_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_case_sig.params.push(AbiParam::new(types::I64)); // value
        eval_case_sig.params.push(AbiParam::new(types::I64)); // case_count
        eval_case_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_case_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_case_func_id = module
            .declare_function("jit_runtime_eval_case", Linkage::Import, &eval_case_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_case: {}", e)))?;

        // eval_chain: fn(ctx, first, second, ip) -> second
        let mut eval_chain_sig = module.make_signature();
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // first
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // second
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_chain_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_chain_func_id = module
            .declare_function("jit_runtime_eval_chain", Linkage::Import, &eval_chain_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_chain: {}", e)))?;

        // Common signature for expr, ip -> result
        let mut expr_ip_sig = module.make_signature();
        expr_ip_sig.params.push(AbiParam::new(types::I64)); // ctx
        expr_ip_sig.params.push(AbiParam::new(types::I64)); // expr
        expr_ip_sig.params.push(AbiParam::new(types::I64)); // ip
        expr_ip_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_quote_func_id = module
            .declare_function("jit_runtime_eval_quote", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_quote: {}", e)))?;

        let eval_unquote_func_id = module
            .declare_function("jit_runtime_eval_unquote", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_unquote: {}", e)))?;

        let eval_eval_func_id = module
            .declare_function("jit_runtime_eval_eval", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_eval: {}", e)))?;

        // eval_bind: fn(ctx, name_idx, value, ip) -> Unit
        let eval_bind_func_id = module
            .declare_function("jit_runtime_eval_bind", Linkage::Import, &eval_let_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_bind: {}", e)))?;

        // eval_new: fn(ctx, ip) -> space
        let mut eval_new_sig = module.make_signature();
        eval_new_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_new_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_new_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_new_func_id = module
            .declare_function("jit_runtime_eval_new", Linkage::Import, &eval_new_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_new: {}", e)))?;

        let eval_collapse_func_id = module
            .declare_function("jit_runtime_eval_collapse", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_collapse: {}", e)))?;

        let eval_superpose_func_id = module
            .declare_function("jit_runtime_eval_superpose", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_superpose: {}", e)))?;

        let eval_memo_func_id = module
            .declare_function("jit_runtime_eval_memo", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_memo: {}", e)))?;

        let eval_memo_first_func_id = module
            .declare_function("jit_runtime_eval_memo_first", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_memo_first: {}", e)))?;

        let eval_pragma_func_id = module
            .declare_function("jit_runtime_eval_pragma", Linkage::Import, &expr_ip_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_pragma: {}", e)))?;

        // eval_function: fn(ctx, name_idx, param_count, ip) -> Unit
        let mut eval_function_sig = module.make_signature();
        eval_function_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_function_sig.params.push(AbiParam::new(types::I64)); // name_idx
        eval_function_sig.params.push(AbiParam::new(types::I64)); // param_count
        eval_function_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_function_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_function_func_id = module
            .declare_function("jit_runtime_eval_function", Linkage::Import, &eval_function_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_function: {}", e)))?;

        // eval_lambda: fn(ctx, param_count, ip) -> closure
        let mut eval_lambda_sig = module.make_signature();
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // param_count
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_lambda_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_lambda_func_id = module
            .declare_function("jit_runtime_eval_lambda", Linkage::Import, &eval_lambda_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_lambda: {}", e)))?;

        // eval_apply: fn(ctx, closure, arg_count, ip) -> result
        let mut eval_apply_sig = module.make_signature();
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // closure
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // arg_count
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_apply_sig.returns.push(AbiParam::new(types::I64)); // result

        let eval_apply_func_id = module
            .declare_function("jit_runtime_eval_apply", Linkage::Import, &eval_apply_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_eval_apply: {}", e)))?;

        Ok(SpecialFormsFuncIds {
            eval_if_func_id,
            eval_let_func_id,
            eval_let_star_func_id,
            eval_match_func_id,
            eval_case_func_id,
            eval_chain_func_id,
            eval_quote_func_id,
            eval_unquote_func_id,
            eval_eval_func_id,
            eval_bind_func_id,
            eval_new_func_id,
            eval_collapse_func_id,
            eval_superpose_func_id,
            eval_memo_func_id,
            eval_memo_first_func_id,
            eval_pragma_func_id,
            eval_function_func_id,
            eval_lambda_func_id,
            eval_apply_func_id,
        })
    }
}
