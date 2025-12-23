//! MORK Bridge operation handlers for JIT compilation
//!
//! Handles: MorkLookup, MorkMatch, MorkInsert, MorkDelete

#[cfg(feature = "jit")]
use cranelift::prelude::*;
#[cfg(feature = "jit")]
use cranelift_jit::JITModule;
#[cfg(feature = "jit")]
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;

/// Context for MORK handlers that need runtime function access
#[cfg(feature = "jit")]
pub struct MorkHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub mork_lookup_func_id: FuncId,
    pub mork_match_func_id: FuncId,
    pub mork_insert_func_id: FuncId,
    pub mork_delete_func_id: FuncId,
}

/// Compile MorkLookup opcode
///
/// Lookup value at MORK path
/// Stack: [path] -> [value]
#[cfg(feature = "jit")]
pub fn compile_mork_lookup<'a, 'b>(
    ctx: &mut MorkHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let path = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.mork_lookup_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, path, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MorkMatch opcode
///
/// Match pattern at MORK path
/// Stack: [path, pattern] -> [results]
#[cfg(feature = "jit")]
pub fn compile_mork_match<'a, 'b>(
    ctx: &mut MorkHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let pattern = codegen.pop()?;
    let path = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.mork_match_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, path, pattern, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MorkInsert opcode
///
/// Insert value at MORK path
/// Stack: [path, value] -> [bool]
#[cfg(feature = "jit")]
pub fn compile_mork_insert<'a, 'b>(
    ctx: &mut MorkHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let path = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.mork_insert_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, path, value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MorkDelete opcode
///
/// Delete value at MORK path
/// Stack: [path] -> [bool]
#[cfg(feature = "jit")]
pub fn compile_mork_delete<'a, 'b>(
    ctx: &mut MorkHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let path = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.mork_delete_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, path, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
