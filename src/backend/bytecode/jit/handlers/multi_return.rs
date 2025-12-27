//! Multi-value return handlers for JIT compilation
//!
//! Handles ReturnMulti and CollectN opcodes.

use cranelift::prelude::*;
use cranelift_module::{FuncId, Module};
use cranelift_jit::JITModule;

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for multi-return operations
pub struct MultiReturnHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub return_multi_func_id: FuncId,
    pub collect_n_func_id: FuncId,
}

/// Compile ReturnMulti opcode
/// Stack: [values...] -> signal - return all values on stack
pub fn compile_return_multi(
    ctx: &mut MultiReturnHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.return_multi_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let count = codegen.builder.ins().iconst(types::I64, 0); // 0 = return all
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, count, ip_val]);
    let signal = codegen.builder.inst_results(inst)[0];
    codegen.builder.ins().return_(&[signal]);

    Ok(())
}

/// Compile CollectN opcode
/// Stack: [] -> [sexpr] - collect up to N results from nondeterminism
pub fn compile_collect_n(
    ctx: &mut MultiReturnHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let max_count = chunk.code().get(offset + 1).copied().unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.collect_n_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let max_val = codegen.builder.ins().iconst(types::I64, max_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, max_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}
