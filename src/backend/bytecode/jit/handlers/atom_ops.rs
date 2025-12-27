//! Atom operation handlers for JIT compilation
//!
//! Handles DeconAtom and Repr opcodes.

use cranelift::prelude::*;
use cranelift_module::{FuncId, Module};
use cranelift_jit::JITModule;

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;

/// Context for atom operations
pub struct AtomOpsHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub decon_atom_func_id: FuncId,
    pub repr_func_id: FuncId,
}

/// Compile DeconAtom opcode
/// Stack: [expr] -> [(head, tail)] - deconstruct S-expression
pub fn compile_decon_atom(
    ctx: &mut AtomOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.decon_atom_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let value = codegen.pop()?;
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile Repr opcode
/// Stack: [value] -> [string] - get string representation
pub fn compile_repr(
    ctx: &mut AtomOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.repr_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let value = codegen.pop()?;
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
    let result = codegen.builder.inst_results(inst)[0];
    codegen.push(result)?;

    Ok(())
}
