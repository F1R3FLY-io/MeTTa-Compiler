//! Value creation handlers for JIT compilation
//!
//! Handles: PushNil, PushTrue, PushFalse, PushUnit, PushLongSmall, PushLong,
//!          PushConstant, PushEmpty, PushAtom, PushString, PushVariable


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::{BytecodeChunk, Opcode};

/// Context for value creation handlers that need runtime function access

pub struct ValueHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub load_const_func_id: FuncId,
    pub push_empty_func_id: FuncId,
    pub push_uri_func_id: FuncId,
}

/// Compile simple value creation opcodes (no runtime calls needed)
pub fn compile_simple_value_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::PushNil => {
            let nil = codegen.const_nil();
            codegen.push(nil)?;
        }

        Opcode::PushTrue => {
            let t = codegen.const_bool(true);
            codegen.push(t)?;
        }

        Opcode::PushFalse => {
            let f = codegen.const_bool(false);
            codegen.push(f)?;
        }

        Opcode::PushUnit => {
            let unit = codegen.const_unit();
            codegen.push(unit)?;
        }

        Opcode::PushLongSmall => {
            let n = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
            let val = codegen.const_long(n as i64);
            codegen.push(val)?;
        }

        _ => unreachable!("compile_simple_value_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}

/// Compile value creation opcodes that require runtime calls

pub fn compile_runtime_value_op<'a, 'b>(
    ctx: &mut ValueHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::PushLong => {
            // Stage 2: Load large integer from constant pool via runtime call
            let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

            // Import the load_constant function into this function's context
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.load_const_func_id, codegen.builder.func);

            // Call jit_runtime_load_constant(ctx, index)
            let ctx_ptr = codegen.ctx_ptr();
            let idx_val = codegen.builder.ins().iconst(types::I64, idx);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::PushConstant => {
            // Stage 2: Load generic constant via runtime call
            let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.load_const_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let idx_val = codegen.builder.ins().iconst(types::I64, idx);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::PushEmpty => {
            // Create empty S-expression via runtime call
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.push_empty_func_id, codegen.builder.func);

            let call_inst = codegen.builder.ins().call(func_ref, &[]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::PushAtom | Opcode::PushString | Opcode::PushVariable => {
            // Load atom/string/variable from constant pool via runtime call
            let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.load_const_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let idx_val = codegen.builder.ins().iconst(types::I64, idx);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::PushUri => {
            // Load URI from constant pool via runtime call
            let index = chunk.read_u16(offset + 1).unwrap_or(0);

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.push_uri_func_id, codegen.builder.func);

            // Call jit_runtime_push_uri(ctx, index)
            let ctx_ptr = codegen.ctx_ptr();
            let index_val = codegen.builder.ins().iconst(types::I64, index as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, index_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_runtime_value_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
