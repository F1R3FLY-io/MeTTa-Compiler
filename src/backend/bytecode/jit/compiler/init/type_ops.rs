//! Type operations function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for type operations
//! runtime functions: get_type, check_type, assert_type.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for type operations
pub struct TypeOpsFuncIds {
    /// Get type of value
    pub get_type_func_id: FuncId,
    /// Check if value matches type
    pub check_type_func_id: FuncId,
    /// Assert value has type (error if not)
    pub assert_type_func_id: FuncId,
}

/// Trait for type operations initialization - zero-cost static dispatch
pub trait TypeOpsInit {
    /// Register type operations runtime symbols with JIT builder
    fn register_type_ops_symbols(builder: &mut JITBuilder);

    /// Declare type operations functions and return their FuncIds
    fn declare_type_ops_funcs<M: Module>(module: &mut M) -> JitResult<TypeOpsFuncIds>;
}

impl<T> TypeOpsInit for T {
    fn register_type_ops_symbols(builder: &mut JITBuilder) {
        builder.symbol("jit_runtime_get_type", runtime::jit_runtime_get_type as *const u8);
        builder.symbol("jit_runtime_check_type", runtime::jit_runtime_check_type as *const u8);
        builder.symbol("jit_runtime_assert_type", runtime::jit_runtime_assert_type as *const u8);
    }

    fn declare_type_ops_funcs<M: Module>(module: &mut M) -> JitResult<TypeOpsFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // get_type: fn(ctx, value, ip) -> type
        let mut get_type_sig = module.make_signature();
        get_type_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_type_sig.params.push(AbiParam::new(types::I64)); // value
        get_type_sig.params.push(AbiParam::new(types::I64)); // ip
        get_type_sig.returns.push(AbiParam::new(types::I64)); // type

        let get_type_func_id = module
            .declare_function("jit_runtime_get_type", Linkage::Import, &get_type_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_get_type: {}", e)))?;

        // check_type: fn(ctx, value, expected_type, ip) -> bool
        let mut check_type_sig = module.make_signature();
        check_type_sig.params.push(AbiParam::new(types::I64)); // ctx
        check_type_sig.params.push(AbiParam::new(types::I64)); // value
        check_type_sig.params.push(AbiParam::new(types::I64)); // expected_type
        check_type_sig.params.push(AbiParam::new(types::I64)); // ip
        check_type_sig.returns.push(AbiParam::new(types::I64)); // bool

        let check_type_func_id = module
            .declare_function("jit_runtime_check_type", Linkage::Import, &check_type_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_check_type: {}", e)))?;

        // assert_type: fn(ctx, value, expected_type, ip) -> value or error
        let mut assert_type_sig = module.make_signature();
        assert_type_sig.params.push(AbiParam::new(types::I64)); // ctx
        assert_type_sig.params.push(AbiParam::new(types::I64)); // value
        assert_type_sig.params.push(AbiParam::new(types::I64)); // expected_type
        assert_type_sig.params.push(AbiParam::new(types::I64)); // ip
        assert_type_sig.returns.push(AbiParam::new(types::I64)); // result

        let assert_type_func_id = module
            .declare_function("jit_runtime_assert_type", Linkage::Import, &assert_type_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_assert_type: {}", e)))?;

        Ok(TypeOpsFuncIds {
            get_type_func_id,
            check_type_func_id,
            assert_type_func_id,
        })
    }
}
