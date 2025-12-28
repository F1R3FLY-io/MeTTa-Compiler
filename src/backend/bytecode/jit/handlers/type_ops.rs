//! Type operation handlers for JIT compilation
//!
//! Handles GetType, CheckType, IsType, and AssertType opcodes.

use cranelift::prelude::*;
use cranelift_jit::JITModule;
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;

/// Context for type operations
pub struct TypeOpsHandlerContext<'a> {
    pub module: &'a mut JITModule,
    pub get_type_func_id: FuncId,
    pub check_type_func_id: FuncId,
    pub assert_type_func_id: FuncId,
}

/// Compile GetType opcode
/// Stack: [value] -> [type_atom]
pub fn compile_get_type(
    ctx: &mut TypeOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let val = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.get_type_func_id, codegen.builder.func);

    // Call jit_runtime_get_type(ctx, val, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile CheckType/IsType opcode
/// Stack: [value, type_atom] -> [bool]
pub fn compile_check_type(
    ctx: &mut TypeOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let type_atom = codegen.pop()?;
    let val = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.check_type_func_id, codegen.builder.func);

    // Call jit_runtime_check_type(ctx, val, type_atom, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, val, type_atom, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile AssertType opcode
/// Stack: [value, type_atom] -> [value] (value stays, type_atom consumed)
/// On type mismatch, runtime signals bailout error
pub fn compile_assert_type(
    ctx: &mut TypeOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'_, '_>,
    offset: usize,
) -> JitResult<()> {
    let type_atom = codegen.pop()?;
    let val = codegen.peek()?; // Peek - value stays on stack

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.assert_type_func_id, codegen.builder.func);

    // Call jit_runtime_assert_type(ctx, val, type_atom, ip)
    // Returns the value unchanged (bailout signaled on mismatch)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, val, type_atom, ip_val]);
    // Note: We don't use the return value since peek already left value on stack

    Ok(())
}
