//! Arithmetic function initialization for JIT compiler
//!
//! Handles symbol registration and function declaration for arithmetic
//! runtime functions: pow, sqrt, log, trig, rounding, etc.

use cranelift::prelude::*;
use cranelift_jit::JITBuilder;
use cranelift_module::{FuncId, Linkage, Module};

use crate::backend::bytecode::jit::runtime;
use crate::backend::bytecode::jit::types::JitResult;

/// Function IDs for arithmetic operations
pub struct ArithmeticFuncIds {
    /// Power function: base^exp
    pub pow_func_id: FuncId,
    /// Square root
    pub sqrt_func_id: FuncId,
    /// Natural logarithm
    pub log_func_id: FuncId,
    /// Truncate towards zero
    pub trunc_func_id: FuncId,
    /// Ceiling function
    pub ceil_func_id: FuncId,
    /// Floor function (math version)
    pub floor_math_func_id: FuncId,
    /// Round to nearest
    pub round_func_id: FuncId,
    /// Sine
    pub sin_func_id: FuncId,
    /// Cosine
    pub cos_func_id: FuncId,
    /// Tangent
    pub tan_func_id: FuncId,
    /// Arc sine
    pub asin_func_id: FuncId,
    /// Arc cosine
    pub acos_func_id: FuncId,
    /// Arc tangent
    pub atan_func_id: FuncId,
    /// Check if NaN
    pub isnan_func_id: FuncId,
    /// Check if infinite
    pub isinf_func_id: FuncId,
}

/// Trait for arithmetic initialization - zero-cost static dispatch
pub trait ArithmeticInit {
    /// Register arithmetic runtime symbols with JIT builder
    fn register_arithmetic_symbols(builder: &mut JITBuilder);

    /// Declare arithmetic functions and return their FuncIds
    fn declare_arithmetic_funcs<M: Module>(module: &mut M) -> JitResult<ArithmeticFuncIds>;
}

/// Implementation for any type (will be used by JitCompiler)
impl<T> ArithmeticInit for T {
    fn register_arithmetic_symbols(builder: &mut JITBuilder) {
        // Power function
        builder.symbol("jit_runtime_pow", runtime::jit_runtime_pow as *const u8);

        // Extended math operations
        builder.symbol("jit_runtime_sqrt", runtime::jit_runtime_sqrt as *const u8);
        builder.symbol("jit_runtime_log", runtime::jit_runtime_log as *const u8);
        builder.symbol("jit_runtime_trunc", runtime::jit_runtime_trunc as *const u8);
        builder.symbol("jit_runtime_ceil", runtime::jit_runtime_ceil as *const u8);
        builder.symbol(
            "jit_runtime_floor_math",
            runtime::jit_runtime_floor_math as *const u8,
        );
        builder.symbol("jit_runtime_round", runtime::jit_runtime_round as *const u8);

        // Trigonometric functions
        builder.symbol("jit_runtime_sin", runtime::jit_runtime_sin as *const u8);
        builder.symbol("jit_runtime_cos", runtime::jit_runtime_cos as *const u8);
        builder.symbol("jit_runtime_tan", runtime::jit_runtime_tan as *const u8);
        builder.symbol("jit_runtime_asin", runtime::jit_runtime_asin as *const u8);
        builder.symbol("jit_runtime_acos", runtime::jit_runtime_acos as *const u8);
        builder.symbol("jit_runtime_atan", runtime::jit_runtime_atan as *const u8);

        // Float predicates
        builder.symbol("jit_runtime_isnan", runtime::jit_runtime_isnan as *const u8);
        builder.symbol("jit_runtime_isinf", runtime::jit_runtime_isinf as *const u8);
    }

    fn declare_arithmetic_funcs<M: Module>(module: &mut M) -> JitResult<ArithmeticFuncIds> {
        use crate::backend::bytecode::jit::types::JitError;

        // Binary arithmetic signature: fn(a: u64, b: u64) -> u64
        let mut binary_sig = module.make_signature();
        binary_sig.params.push(AbiParam::new(types::I64));
        binary_sig.params.push(AbiParam::new(types::I64));
        binary_sig.returns.push(AbiParam::new(types::I64));

        // Unary arithmetic signature: fn(a: u64) -> u64
        let mut unary_sig = module.make_signature();
        unary_sig.params.push(AbiParam::new(types::I64));
        unary_sig.returns.push(AbiParam::new(types::I64));

        // Declare pow (binary)
        let pow_func_id = module
            .declare_function("jit_runtime_pow", Linkage::Import, &binary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_pow: {}", e))
            })?;

        // Declare unary math functions
        let sqrt_func_id = module
            .declare_function("jit_runtime_sqrt", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_sqrt: {}", e))
            })?;

        let log_func_id = module
            .declare_function("jit_runtime_log", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_log: {}", e))
            })?;

        let trunc_func_id = module
            .declare_function("jit_runtime_trunc", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_trunc: {}", e))
            })?;

        let ceil_func_id = module
            .declare_function("jit_runtime_ceil", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_ceil: {}", e))
            })?;

        let floor_math_func_id = module
            .declare_function("jit_runtime_floor_math", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_floor_math: {}",
                    e
                ))
            })?;

        let round_func_id = module
            .declare_function("jit_runtime_round", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_round: {}", e))
            })?;

        // Declare trigonometric functions
        let sin_func_id = module
            .declare_function("jit_runtime_sin", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_sin: {}", e))
            })?;

        let cos_func_id = module
            .declare_function("jit_runtime_cos", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_cos: {}", e))
            })?;

        let tan_func_id = module
            .declare_function("jit_runtime_tan", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_tan: {}", e))
            })?;

        let asin_func_id = module
            .declare_function("jit_runtime_asin", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_asin: {}", e))
            })?;

        let acos_func_id = module
            .declare_function("jit_runtime_acos", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_acos: {}", e))
            })?;

        let atan_func_id = module
            .declare_function("jit_runtime_atan", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_atan: {}", e))
            })?;

        // Declare float predicate functions
        let isnan_func_id = module
            .declare_function("jit_runtime_isnan", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_isnan: {}", e))
            })?;

        let isinf_func_id = module
            .declare_function("jit_runtime_isinf", Linkage::Import, &unary_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_isinf: {}", e))
            })?;

        Ok(ArithmeticFuncIds {
            pow_func_id,
            sqrt_func_id,
            log_func_id,
            trunc_func_id,
            ceil_func_id,
            floor_math_func_id,
            round_func_id,
            sin_func_id,
            cos_func_id,
            tan_func_id,
            asin_func_id,
            acos_func_id,
            atan_func_id,
            isnan_func_id,
            isinf_func_id,
        })
    }
}
