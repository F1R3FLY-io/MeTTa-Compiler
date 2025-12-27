//! Global and closure operation handlers for JIT compilation
//!
//! Handles LoadGlobal, StoreGlobal, LoadSpace, and LoadUpvalue opcodes.

use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for global/closure operations
pub struct GlobalsHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub load_global_func_id: FuncId,
    pub store_global_func_id: FuncId,
    pub load_space_func_id: FuncId,
    pub load_upvalue_func_id: FuncId,
}

/// Compile LoadGlobal opcode
/// Stack: [] -> [value] - load global variable by symbol index
pub fn compile_load_global(
    ctx: &mut GlobalsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    // Read 2 bytes as u16 (little-endian)
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let symbol_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.load_global_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let idx_val = codegen.builder.ins().iconst(types::I64, symbol_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, idx_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile StoreGlobal opcode
/// Stack: [value] -> [] - store global variable by symbol index
pub fn compile_store_global(
    ctx: &mut GlobalsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let symbol_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.store_global_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let value = codegen.pop()?;
    let idx_val = codegen.builder.ins().iconst(types::I64, symbol_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, idx_val, value, ip_val]);
    // Result is Unit, but we don't push it (store is side-effect only)

    Ok(())
}

/// Compile LoadSpace opcode
/// Stack: [] -> [space_handle] - load space by name index
pub fn compile_load_space(
    ctx: &mut GlobalsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
    let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
    let name_idx = (b1 << 8 | b0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.load_space_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, idx_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile LoadUpvalue opcode
/// Stack: [] -> [value] - load from enclosing scope
/// Operand: 2 bytes (depth: u8, index: u8)
pub fn compile_load_upvalue(
    ctx: &mut GlobalsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let code = chunk.code();
    let depth = code.get(offset + 1).copied().unwrap_or(0) as i64;
    let index = code.get(offset + 2).copied().unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.load_upvalue_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let depth_val = codegen.builder.ins().iconst(types::I64, depth);
    let index_val = codegen.builder.ins().iconst(types::I64, index);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, depth_val, index_val, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}
