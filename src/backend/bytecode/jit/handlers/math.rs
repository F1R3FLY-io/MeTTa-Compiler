//! Extended math operation handlers for JIT compilation
//!
//! Handles: Sqrt, Log, Trunc, Ceil, FloorMath, Round, Sin, Cos, Tan, Asin, Acos, Atan, IsNan, IsInf

#[cfg(feature = "jit")]
use cranelift::prelude::*;
#[cfg(feature = "jit")]
use cranelift_jit::JITModule;
#[cfg(feature = "jit")]
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::Opcode;

/// Context for extended math handlers that need runtime function access
#[cfg(feature = "jit")]
pub struct MathHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub sqrt_func_id: FuncId,
    pub log_func_id: FuncId,
    pub trunc_func_id: FuncId,
    pub ceil_func_id: FuncId,
    pub floor_math_func_id: FuncId,
    pub round_func_id: FuncId,
    pub sin_func_id: FuncId,
    pub cos_func_id: FuncId,
    pub tan_func_id: FuncId,
    pub asin_func_id: FuncId,
    pub acos_func_id: FuncId,
    pub atan_func_id: FuncId,
    pub isnan_func_id: FuncId,
    pub isinf_func_id: FuncId,
}

/// Compile extended math opcodes via runtime calls
#[cfg(feature = "jit")]
pub fn compile_extended_math_op<'a, 'b>(
    ctx: &mut MathHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
) -> JitResult<()> {
    match op {
        Opcode::Sqrt => {
            // sqrt-math: [value] -> [sqrt(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.sqrt_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Log => {
            // log-math: [base, value] -> [log_base(value)]
            let value = codegen.pop()?;
            let base = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.log_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[base, value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Trunc => {
            // trunc-math: [value] -> [trunc(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.trunc_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Ceil => {
            // ceil-math: [value] -> [ceil(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.ceil_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::FloorMath => {
            // floor-math: [value] -> [floor(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.floor_math_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Round => {
            // round-math: [value] -> [round(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.round_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Sin => {
            // sin-math: [value] -> [sin(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.sin_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Cos => {
            // cos-math: [value] -> [cos(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.cos_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Tan => {
            // tan-math: [value] -> [tan(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.tan_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Asin => {
            // asin-math: [value] -> [asin(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.asin_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Acos => {
            // acos-math: [value] -> [acos(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.acos_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::Atan => {
            // atan-math: [value] -> [atan(value)]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.atan_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::IsNan => {
            // isnan-math: [value] -> [bool]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.isnan_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::IsInf => {
            // isinf-math: [value] -> [bool]
            let value = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.isinf_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[value]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_extended_math_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
