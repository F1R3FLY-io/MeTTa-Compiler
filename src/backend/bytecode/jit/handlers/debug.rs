//! Debug and meta operation handlers for JIT compilation
//!
//! Handles: Trace, Breakpoint

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for debug handlers that need runtime function access

pub struct DebugHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub trace_func_id: FuncId,
    pub breakpoint_func_id: FuncId,
}

/// Compile Trace opcode
///
/// Emit trace event
/// Stack: [value] -> [], msg_idx from operand

pub fn compile_trace<'a, 'b>(
    ctx: &mut DebugHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let msg_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.trace_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let msg_idx_val = codegen.builder.ins().iconst(types::I64, msg_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    // Trace has no return value, just call it
    codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, msg_idx_val, value, ip_val]);
    Ok(())
}

/// Compile Breakpoint opcode
///
/// Debugger breakpoint
/// Stack: [] -> [], bp_id from operand

pub fn compile_breakpoint<'a, 'b>(
    ctx: &mut DebugHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let bp_id = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.breakpoint_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let bp_id_val = codegen.builder.ins().iconst(types::I64, bp_id);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, bp_id_val, ip_val]);
    // Result is signal, caller handles it
    Ok(())
}
