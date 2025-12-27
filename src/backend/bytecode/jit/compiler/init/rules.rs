//! Rule dispatch function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for rule dispatch
//! runtime functions: dispatch_rules, try_rule, next_rule, commit_rule, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for rule dispatch operations
pub struct RulesFuncIds {
    /// Dispatch to matching rules
    pub dispatch_rules_func_id: FuncId,
    /// Try a specific rule
    pub try_rule_func_id: FuncId,
    /// Move to next rule
    pub next_rule_func_id: FuncId,
    /// Commit to current rule
    pub commit_rule_func_id: FuncId,
    /// Fail current rule
    pub fail_rule_func_id: FuncId,
    /// Lookup rules for a symbol
    pub lookup_rules_func_id: FuncId,
    /// Apply substitution
    pub apply_subst_func_id: FuncId,
    /// Define a new rule
    pub define_rule_func_id: FuncId,
}

/// Trait for rules initialization - zero-cost static dispatch
pub trait RulesInit {
    /// Register rules runtime symbols with JIT builder
    fn register_rules_symbols(builder: &mut JITBuilder);

    /// Declare rules functions and return their FuncIds
    fn declare_rules_funcs<M: Module>(module: &mut M) -> JitResult<RulesFuncIds>;
}

impl<T> RulesInit for T {
    fn register_rules_symbols(builder: &mut JITBuilder) {
        builder.symbol(
            "jit_runtime_dispatch_rules",
            runtime::jit_runtime_dispatch_rules as *const u8,
        );
        builder.symbol(
            "jit_runtime_try_rule",
            runtime::jit_runtime_try_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_next_rule",
            runtime::jit_runtime_next_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_commit_rule",
            runtime::jit_runtime_commit_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_fail_rule",
            runtime::jit_runtime_fail_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_lookup_rules",
            runtime::jit_runtime_lookup_rules as *const u8,
        );
        builder.symbol(
            "jit_runtime_apply_subst",
            runtime::jit_runtime_apply_subst as *const u8,
        );
        builder.symbol(
            "jit_runtime_define_rule",
            runtime::jit_runtime_define_rule as *const u8,
        );
    }

    fn declare_rules_funcs<M: Module>(module: &mut M) -> JitResult<RulesFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // dispatch_rules: fn(ctx, expr, ip) -> count
        let mut dispatch_rules_sig = module.make_signature();
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // ctx
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // expr
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // ip
        dispatch_rules_sig.returns.push(AbiParam::new(types::I64)); // count

        let dispatch_rules_func_id = module
            .declare_function(
                "jit_runtime_dispatch_rules",
                Linkage::Import,
                &dispatch_rules_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_dispatch_rules: {}",
                    e
                ))
            })?;

        // try_rule: fn(ctx, rule_idx, ip) -> result
        let mut try_rule_sig = module.make_signature();
        try_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        try_rule_sig.params.push(AbiParam::new(types::I64)); // rule_idx
        try_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        try_rule_sig.returns.push(AbiParam::new(types::I64)); // result

        let try_rule_func_id = module
            .declare_function("jit_runtime_try_rule", Linkage::Import, &try_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_try_rule: {}", e))
            })?;

        // next_rule: fn(ctx, ip) -> status
        let mut next_rule_sig = module.make_signature();
        next_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        next_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        next_rule_sig.returns.push(AbiParam::new(types::I64)); // status

        let next_rule_func_id = module
            .declare_function("jit_runtime_next_rule", Linkage::Import, &next_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_next_rule: {}",
                    e
                ))
            })?;

        // commit_rule: fn(ctx, ip) -> status
        let mut commit_rule_sig = module.make_signature();
        commit_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        commit_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        commit_rule_sig.returns.push(AbiParam::new(types::I64)); // status

        let commit_rule_func_id = module
            .declare_function("jit_runtime_commit_rule", Linkage::Import, &commit_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_commit_rule: {}",
                    e
                ))
            })?;

        // fail_rule: fn(ctx, ip) -> signal
        let mut fail_rule_sig = module.make_signature();
        fail_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        fail_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        fail_rule_sig.returns.push(AbiParam::new(types::I64)); // signal

        let fail_rule_func_id = module
            .declare_function("jit_runtime_fail_rule", Linkage::Import, &fail_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_fail_rule: {}",
                    e
                ))
            })?;

        // lookup_rules: fn(ctx, head_idx, ip) -> count
        let mut lookup_rules_sig = module.make_signature();
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // ctx
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // head_idx
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // ip
        lookup_rules_sig.returns.push(AbiParam::new(types::I64)); // count

        let lookup_rules_func_id = module
            .declare_function(
                "jit_runtime_lookup_rules",
                Linkage::Import,
                &lookup_rules_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_lookup_rules: {}",
                    e
                ))
            })?;

        // apply_subst: fn(ctx, expr, ip) -> result
        let mut apply_subst_sig = module.make_signature();
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // ctx
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // expr
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // ip
        apply_subst_sig.returns.push(AbiParam::new(types::I64)); // result

        let apply_subst_func_id = module
            .declare_function("jit_runtime_apply_subst", Linkage::Import, &apply_subst_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_apply_subst: {}",
                    e
                ))
            })?;

        // define_rule: fn(ctx, pattern_idx, ip) -> Unit
        let mut define_rule_sig = module.make_signature();
        define_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        define_rule_sig.params.push(AbiParam::new(types::I64)); // pattern_idx
        define_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        define_rule_sig.returns.push(AbiParam::new(types::I64)); // result

        let define_rule_func_id = module
            .declare_function("jit_runtime_define_rule", Linkage::Import, &define_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_define_rule: {}",
                    e
                ))
            })?;

        Ok(RulesFuncIds {
            dispatch_rules_func_id,
            try_rule_func_id,
            next_rule_func_id,
            commit_rule_func_id,
            fail_rule_func_id,
            lookup_rules_func_id,
            apply_subst_func_id,
            define_rule_func_id,
        })
    }
}
