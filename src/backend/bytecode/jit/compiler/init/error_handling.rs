//! Error handling function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for error handling
//! runtime functions: type_error, div_by_zero, stack_overflow, integer_overflow,
//! binding errors, and overflow conditions.
//!
//! These functions are called from bailout blocks instead of using trap(),
//! which allows graceful fallback to the bytecode interpreter.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for error handling operations
///
/// These are called from JIT bailout blocks to signal errors back to the
/// interpreter without causing SIGILL from trap() instructions.
#[derive(Clone, Copy)]
pub struct ErrorFuncIds {
    /// Type error: expected type didn't match actual type
    pub type_error_func_id: FuncId,
    /// Division by zero error
    pub div_by_zero_func_id: FuncId,
    /// Stack/arithmetic overflow error
    pub overflow_func_id: FuncId,
    /// Integer overflow error (for iadd/isub/imul)
    pub integer_overflow_func_id: FuncId,
    /// Stack underflow error
    pub stack_underflow_func_id: FuncId,
    /// Binding frame overflow error
    pub binding_frame_overflow_func_id: FuncId,
    /// Invalid binding error
    pub invalid_binding_func_id: FuncId,
    /// Choice point overflow error
    pub choice_point_overflow_func_id: FuncId,
    /// Results buffer overflow error
    pub results_overflow_func_id: FuncId,
}

/// Trait for error handling initialization - zero-cost static dispatch
pub trait ErrorHandlingInit {
    /// Register error handling runtime symbols with JIT builder
    fn register_error_handling_symbols(builder: &mut JITBuilder);

    /// Declare error handling functions and return their FuncIds
    fn declare_error_handling_funcs<M: Module>(module: &mut M) -> JitResult<ErrorFuncIds>;
}

/// Implementation for any type (will be used by JitCompiler)
impl<T> ErrorHandlingInit for T {
    fn register_error_handling_symbols(builder: &mut JITBuilder) {
        // Type error: fn(ctx: *mut JitContext, ip: u64, expected: u64) -> ()
        builder.symbol(
            "jit_runtime_type_error",
            runtime::jit_runtime_type_error as *const u8,
        );

        // Division by zero: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_div_by_zero",
            runtime::jit_runtime_div_by_zero as *const u8,
        );

        // Stack overflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_stack_overflow",
            runtime::jit_runtime_stack_overflow as *const u8,
        );

        // Stack underflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_stack_underflow",
            runtime::jit_runtime_stack_underflow as *const u8,
        );

        // Integer overflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_integer_overflow",
            runtime::jit_runtime_integer_overflow as *const u8,
        );

        // Binding frame overflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_binding_frame_overflow",
            runtime::jit_runtime_binding_frame_overflow as *const u8,
        );

        // Invalid binding: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_invalid_binding",
            runtime::jit_runtime_invalid_binding as *const u8,
        );

        // Choice point overflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_choice_point_overflow",
            runtime::jit_runtime_choice_point_overflow as *const u8,
        );

        // Results buffer overflow: fn(ctx: *mut JitContext, ip: u64) -> ()
        builder.symbol(
            "jit_runtime_results_overflow",
            runtime::jit_runtime_results_overflow as *const u8,
        );
    }

    fn declare_error_handling_funcs<M: Module>(module: &mut M) -> JitResult<ErrorFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // Type error signature: fn(ctx: *mut, ip: u64, expected: u64) -> ()
        let mut type_error_sig = module.make_signature();
        type_error_sig.params.push(AbiParam::new(types::I64)); // ctx
        type_error_sig.params.push(AbiParam::new(types::I64)); // ip
        type_error_sig.params.push(AbiParam::new(types::I64)); // expected
        // No return value - function sets bailout flag and returns

        // Binary error signature: fn(ctx: *mut, ip: u64) -> ()
        let mut binary_error_sig = module.make_signature();
        binary_error_sig.params.push(AbiParam::new(types::I64)); // ctx
        binary_error_sig.params.push(AbiParam::new(types::I64)); // ip
        // No return value

        // Declare type error function
        let type_error_func_id = module
            .declare_function("jit_runtime_type_error", Linkage::Import, &type_error_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_type_error: {}",
                    e
                ))
            })?;

        // Declare division by zero function
        let div_by_zero_func_id = module
            .declare_function(
                "jit_runtime_div_by_zero",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_div_by_zero: {}",
                    e
                ))
            })?;

        // Declare overflow function
        let overflow_func_id = module
            .declare_function(
                "jit_runtime_stack_overflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_stack_overflow: {}",
                    e
                ))
            })?;

        // Declare stack underflow function
        let stack_underflow_func_id = module
            .declare_function(
                "jit_runtime_stack_underflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_stack_underflow: {}",
                    e
                ))
            })?;

        // Declare integer overflow function
        let integer_overflow_func_id = module
            .declare_function(
                "jit_runtime_integer_overflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_integer_overflow: {}",
                    e
                ))
            })?;

        // Declare binding frame overflow function
        let binding_frame_overflow_func_id = module
            .declare_function(
                "jit_runtime_binding_frame_overflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_binding_frame_overflow: {}",
                    e
                ))
            })?;

        // Declare invalid binding function
        let invalid_binding_func_id = module
            .declare_function(
                "jit_runtime_invalid_binding",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_invalid_binding: {}",
                    e
                ))
            })?;

        // Declare choice point overflow function
        let choice_point_overflow_func_id = module
            .declare_function(
                "jit_runtime_choice_point_overflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_choice_point_overflow: {}",
                    e
                ))
            })?;

        // Declare results buffer overflow function
        let results_overflow_func_id = module
            .declare_function(
                "jit_runtime_results_overflow",
                Linkage::Import,
                &binary_error_sig,
            )
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_results_overflow: {}",
                    e
                ))
            })?;

        Ok(ErrorFuncIds {
            type_error_func_id,
            div_by_zero_func_id,
            overflow_func_id,
            integer_overflow_func_id,
            stack_underflow_func_id,
            binding_frame_overflow_func_id,
            invalid_binding_func_id,
            choice_point_overflow_func_id,
            results_overflow_func_id,
        })
    }
}
