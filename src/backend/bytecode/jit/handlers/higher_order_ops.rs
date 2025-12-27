//! Higher-order operation handlers for JIT compilation
//!
//! Handles MapAtom, FilterAtom, and FoldlAtom opcodes.
//! These operations require executing nested bytecode, so they bailout to VM.

use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for higher-order operations
pub struct HigherOrderOpsHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub map_atom_func_id: FuncId,
    pub filter_atom_func_id: FuncId,
    pub foldl_atom_func_id: FuncId,
}

/// Compile MapAtom opcode
/// Stack: [list] -> [result] - map function over list
pub fn compile_map_atom(
    ctx: &mut HigherOrderOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let chunk_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.map_atom_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let list = codegen.pop()?;
    let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, list, chunk_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile FilterAtom opcode
/// Stack: [list] -> [result] - filter list by predicate
pub fn compile_filter_atom(
    ctx: &mut HigherOrderOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let chunk_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.filter_atom_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let list = codegen.pop()?;
    let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, list, chunk_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile FoldlAtom opcode
/// Stack: [list, init] -> [result] - left fold over list
pub fn compile_foldl_atom(
    ctx: &mut HigherOrderOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let chunk_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.foldl_atom_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let init = codegen.pop()?;
    let list = codegen.pop()?;
    let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, list, init, chunk_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}
