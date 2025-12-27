//! Nondeterminism function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for nondeterminism
//! runtime functions: fork, yield, collect, cut, guard, amb, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for nondeterminism operations
pub struct NondetFuncIds {
    /// Fork execution with native choice points
    pub fork_native_func_id: FuncId,
    /// Yield a result (native)
    pub yield_native_func_id: FuncId,
    /// Collect results (native)
    pub collect_native_func_id: FuncId,
    /// Cut (prune alternatives)
    pub cut_func_id: FuncId,
    /// Guard condition
    pub guard_func_id: FuncId,
    /// Amb (nondeterministic choice)
    pub amb_func_id: FuncId,
    /// Commit to current choice
    pub commit_func_id: FuncId,
    /// Backtrack to previous choice
    pub backtrack_func_id: FuncId,
    /// Begin nondeterministic block
    pub begin_nondet_func_id: FuncId,
    /// End nondeterministic block
    pub end_nondet_func_id: FuncId,
}

/// Trait for nondeterminism initialization - zero-cost static dispatch
pub trait NondetInit {
    /// Register nondeterminism runtime symbols with JIT builder
    fn register_nondet_symbols(builder: &mut JITBuilder);

    /// Declare nondeterminism functions and return their FuncIds
    fn declare_nondet_funcs<M: Module>(module: &mut M) -> JitResult<NondetFuncIds>;
}

impl<T> NondetInit for T {
    fn register_nondet_symbols(builder: &mut JITBuilder) {
        // Native nondeterminism functions
        builder.symbol("jit_runtime_fork_native", runtime::jit_runtime_fork_native as *const u8);
        builder.symbol("jit_runtime_yield_native", runtime::jit_runtime_yield_native as *const u8);
        builder.symbol("jit_runtime_collect_native", runtime::jit_runtime_collect_native as *const u8);

        // Advanced nondeterminism
        builder.symbol("jit_runtime_cut", runtime::jit_runtime_cut as *const u8);
        builder.symbol("jit_runtime_guard", runtime::jit_runtime_guard as *const u8);
        builder.symbol("jit_runtime_amb", runtime::jit_runtime_amb as *const u8);
        builder.symbol("jit_runtime_commit", runtime::jit_runtime_commit as *const u8);
        builder.symbol("jit_runtime_backtrack", runtime::jit_runtime_backtrack as *const u8);

        // Nondet block markers
        builder.symbol("jit_runtime_begin_nondet", runtime::jit_runtime_begin_nondet as *const u8);
        builder.symbol("jit_runtime_end_nondet", runtime::jit_runtime_end_nondet as *const u8);
    }

    fn declare_nondet_funcs<M: Module>(module: &mut M) -> JitResult<NondetFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // fork_native: fn(ctx, n_alternatives, resume_ip, ip) -> result
        let mut fork_sig = module.make_signature();
        fork_sig.params.push(AbiParam::new(types::I64)); // ctx
        fork_sig.params.push(AbiParam::new(types::I64)); // n_alternatives
        fork_sig.params.push(AbiParam::new(types::I64)); // resume_ip
        fork_sig.params.push(AbiParam::new(types::I64)); // ip
        fork_sig.returns.push(AbiParam::new(types::I64)); // result

        let fork_native_func_id = module
            .declare_function("jit_runtime_fork_native", Linkage::Import, &fork_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_fork_native: {}", e)))?;

        // yield_native: fn(ctx, value, ip) -> signal
        let mut yield_sig = module.make_signature();
        yield_sig.params.push(AbiParam::new(types::I64)); // ctx
        yield_sig.params.push(AbiParam::new(types::I64)); // value
        yield_sig.params.push(AbiParam::new(types::I64)); // ip
        yield_sig.returns.push(AbiParam::new(types::I64)); // signal

        let yield_native_func_id = module
            .declare_function("jit_runtime_yield_native", Linkage::Import, &yield_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_yield_native: {}", e)))?;

        // collect_native: fn(ctx, ip) -> results
        let mut collect_sig = module.make_signature();
        collect_sig.params.push(AbiParam::new(types::I64)); // ctx
        collect_sig.params.push(AbiParam::new(types::I64)); // ip
        collect_sig.returns.push(AbiParam::new(types::I64)); // results

        let collect_native_func_id = module
            .declare_function("jit_runtime_collect_native", Linkage::Import, &collect_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_collect_native: {}", e)))?;

        // cut: fn(ctx, ip) -> ()
        let mut cut_sig = module.make_signature();
        cut_sig.params.push(AbiParam::new(types::I64)); // ctx
        cut_sig.params.push(AbiParam::new(types::I64)); // ip

        let cut_func_id = module
            .declare_function("jit_runtime_cut", Linkage::Import, &cut_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_cut: {}", e)))?;

        // guard: fn(ctx, condition, ip) -> bool
        let mut guard_sig = module.make_signature();
        guard_sig.params.push(AbiParam::new(types::I64)); // ctx
        guard_sig.params.push(AbiParam::new(types::I64)); // condition
        guard_sig.params.push(AbiParam::new(types::I64)); // ip
        guard_sig.returns.push(AbiParam::new(types::I64)); // bool

        let guard_func_id = module
            .declare_function("jit_runtime_guard", Linkage::Import, &guard_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_guard: {}", e)))?;

        // amb: fn(ctx, choices, ip) -> result
        let mut amb_sig = module.make_signature();
        amb_sig.params.push(AbiParam::new(types::I64)); // ctx
        amb_sig.params.push(AbiParam::new(types::I64)); // choices
        amb_sig.params.push(AbiParam::new(types::I64)); // ip
        amb_sig.returns.push(AbiParam::new(types::I64)); // result

        let amb_func_id = module
            .declare_function("jit_runtime_amb", Linkage::Import, &amb_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_amb: {}", e)))?;

        // commit: fn(ctx, ip) -> ()
        let mut commit_sig = module.make_signature();
        commit_sig.params.push(AbiParam::new(types::I64)); // ctx
        commit_sig.params.push(AbiParam::new(types::I64)); // ip

        let commit_func_id = module
            .declare_function("jit_runtime_commit", Linkage::Import, &commit_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_commit: {}", e)))?;

        // backtrack: fn(ctx, ip) -> signal
        let mut backtrack_sig = module.make_signature();
        backtrack_sig.params.push(AbiParam::new(types::I64)); // ctx
        backtrack_sig.params.push(AbiParam::new(types::I64)); // ip
        backtrack_sig.returns.push(AbiParam::new(types::I64)); // signal

        let backtrack_func_id = module
            .declare_function("jit_runtime_backtrack", Linkage::Import, &backtrack_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_backtrack: {}", e)))?;

        // begin_nondet: fn(ctx, ip) -> ()
        let mut begin_sig = module.make_signature();
        begin_sig.params.push(AbiParam::new(types::I64)); // ctx
        begin_sig.params.push(AbiParam::new(types::I64)); // ip

        let begin_nondet_func_id = module
            .declare_function("jit_runtime_begin_nondet", Linkage::Import, &begin_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_begin_nondet: {}", e)))?;

        // end_nondet: fn(ctx, ip) -> ()
        let mut end_sig = module.make_signature();
        end_sig.params.push(AbiParam::new(types::I64)); // ctx
        end_sig.params.push(AbiParam::new(types::I64)); // ip

        let end_nondet_func_id = module
            .declare_function("jit_runtime_end_nondet", Linkage::Import, &end_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_end_nondet: {}", e)))?;

        Ok(NondetFuncIds {
            fork_native_func_id,
            yield_native_func_id,
            collect_native_func_id,
            cut_func_id,
            guard_func_id,
            amb_func_id,
            commit_func_id,
            backtrack_func_id,
            begin_nondet_func_id,
            end_nondet_func_id,
        })
    }
}
