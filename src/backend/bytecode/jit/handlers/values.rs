//! Value creation handlers for JIT compilation
//!
//! Handles: PushTrue, PushFalse, PushUnit, PushLongSmall, PushLong,
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
    /// Routes `Opcode::PushVariable` to a runtime fn that consults binding
    /// frames first, falling through to the constant-pool atom literal on
    /// miss — JIT analog of bytecode VM's `op_push_variable`. Bisimilarity
    /// with trampoline `apply_bindings_with_rename_scoped` is preserved
    /// (Plan agent's Phase 3 of cross-tier substitution fix).
    pub push_variable_with_fallback_func_id: FuncId,
}

/// Compile simple value creation opcodes (no runtime calls needed)
pub fn compile_simple_value_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::PushUnit => {
            let unit = codegen.const_unit();
            codegen.push(unit)?;
        }

        Opcode::PushTrue => {
            let t = codegen.const_bool(true);
            codegen.push(t)?;
        }

        Opcode::PushFalse => {
            let f = codegen.const_bool(false);
            codegen.push(f)?;
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

        Opcode::PushAtom | Opcode::PushString => {
            // Load atom/string from constant pool via runtime call.
            // (PushVariable was historically lumped here — incorrect; see
            // separate arm below for the bisimilarity-preserving routing.)
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

        Opcode::PushVariable => {
            // Phase 3 of cross-tier substitution fix: route PushVariable
            // through `jit_runtime_push_variable_with_fallback` so the
            // JIT consults the active binding frame first and falls
            // through to the atom literal on miss — matching bytecode
            // VM's `op_push_variable` (vm/mod.rs:1833-1855) and the
            // trampoline's `apply_bindings_with_rename_scoped`. Without
            // this, JIT-promoted rule chunks would push the atom literal
            // even when a binding for the variable existed.
            let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

            let func_ref = ctx.module.declare_func_in_func(
                ctx.push_variable_with_fallback_func_id,
                codegen.builder.func,
            );

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

        _ => unreachable!(
            "compile_runtime_value_op called with wrong opcode: {:?}",
            op
        ),
    }
    Ok(())
}
