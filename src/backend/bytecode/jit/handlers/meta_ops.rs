//! Meta operation handlers for JIT compilation
//!
//! Handles GetMetaType and BloomCheck opcodes.

use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;

/// Context for meta operations
pub struct MetaOpsHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub get_metatype_func_id: FuncId,
    pub bloom_check_func_id: FuncId,
}

/// Compile GetMetaType opcode
/// Stack: [value] -> [metatype_atom] - get meta-level type
pub fn compile_get_metatype(
    ctx: &mut MetaOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.get_metatype_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let value = codegen.pop()?;
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, value, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile BloomCheck opcode
/// Stack: [key] -> [bool] - bloom filter pre-check
pub fn compile_bloom_check(
    ctx: &mut MetaOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.bloom_check_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let key = codegen.pop()?;
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, key, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}
