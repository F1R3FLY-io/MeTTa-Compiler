//! Arithmetic operation handlers for JIT compilation
//!
//! Handles: Add, Sub, Mul, Div, Mod, Neg, Abs, FloorDiv, Pow


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::Opcode;

/// Context for arithmetic handlers that need runtime function access

pub struct ArithmeticHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub pow_func_id: FuncId,
}

/// Compile simple arithmetic opcodes (no runtime calls needed)

pub fn compile_simple_arithmetic_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::Add => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            // Type guards
            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            // Extract payloads (lower 48 bits)
            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);

            // Perform addition
            let result = codegen.builder.ins().iadd(a_val, b_val);

            // Box result as Long
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Sub => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let result = codegen.builder.ins().isub(a_val, b_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Mul => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let result = codegen.builder.ins().imul(a_val, b_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Div => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);

            // Guard against division by zero
            codegen.guard_nonzero(b_val, offset)?;

            let result = codegen.builder.ins().sdiv(a_val, b_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Mod => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);

            codegen.guard_nonzero(b_val, offset)?;

            let result = codegen.builder.ins().srem(a_val, b_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Neg => {
            let a = codegen.pop()?;
            codegen.guard_long(a, offset)?;

            let a_val = codegen.extract_long(a);
            let result = codegen.builder.ins().ineg(a_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::Abs => {
            let a = codegen.pop()?;
            codegen.guard_long(a, offset)?;

            let a_val = codegen.extract_long(a);

            // abs(x) = x < 0 ? -x : x
            let zero = codegen.builder.ins().iconst(types::I64, 0);
            let is_neg = codegen.builder.ins().icmp(IntCC::SignedLessThan, a_val, zero);
            let negated = codegen.builder.ins().ineg(a_val);
            let result = codegen.builder.ins().select(is_neg, negated, a_val);

            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        Opcode::FloorDiv => {
            // For integers, floor division is the same as truncated division
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);

            codegen.guard_nonzero(b_val, offset)?;

            let result = codegen.builder.ins().sdiv(a_val, b_val);
            let boxed = codegen.box_long(result);
            codegen.push(boxed)?;
        }

        _ => unreachable!("compile_simple_arithmetic_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}

/// Compile Pow opcode via runtime call

pub fn compile_pow<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let exp = codegen.pop()?;
    let base = codegen.pop()?;

    // Import the pow function into this function's context
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pow_func_id, codegen.builder.func);

    // Call jit_runtime_pow(base, exp) - both are NaN-boxed
    let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;

    Ok(())
}
