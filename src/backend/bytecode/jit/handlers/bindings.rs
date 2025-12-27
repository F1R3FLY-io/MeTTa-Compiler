//! Binding operation handlers for JIT compilation
//!
//! Handles: LoadBinding, StoreBinding, HasBinding, ClearBindings, PushBindingFrame, PopBindingFrame

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for binding handlers that need runtime function access

pub struct BindingHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub load_binding_func_id: FuncId,
    pub store_binding_func_id: FuncId,
    pub has_binding_func_id: FuncId,
    pub clear_bindings_func_id: FuncId,
    pub push_binding_frame_func_id: FuncId,
    pub pop_binding_frame_func_id: FuncId,
}

/// Compile LoadBinding opcode
///
/// Load binding by name index via runtime call
/// Stack: [] -> [value]

pub fn compile_load_binding<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.load_binding_func_id, codegen.builder.func);

    // Call jit_runtime_load_binding(ctx, name_idx, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile StoreBinding opcode
///
/// Store binding by name index via runtime call
/// Stack: [value] -> []

pub fn compile_store_binding<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

    let value = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.store_binding_func_id, codegen.builder.func);

    // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);
    // Result is status code, ignored for now
    Ok(())
}

/// Compile HasBinding opcode
///
/// Check if binding exists by name index via runtime call
/// Stack: [] -> [bool]

pub fn compile_has_binding<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.has_binding_func_id, codegen.builder.func);

    // Call jit_runtime_has_binding(ctx, name_idx)
    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile ClearBindings opcode
///
/// Clear all bindings via runtime call
/// Stack: [] -> []

pub fn compile_clear_bindings<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.clear_bindings_func_id, codegen.builder.func);

    // Call jit_runtime_clear_bindings(ctx)
    let ctx_ptr = codegen.ctx_ptr();
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
    Ok(())
}

/// Compile PushBindingFrame opcode
///
/// Push new binding frame via runtime call
/// Stack: [] -> []

pub fn compile_push_binding_frame<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.push_binding_frame_func_id, codegen.builder.func);

    // Call jit_runtime_push_binding_frame(ctx)
    let ctx_ptr = codegen.ctx_ptr();
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
    // Result is status code, ignored for now
    Ok(())
}

/// Compile PopBindingFrame opcode
///
/// Pop binding frame via runtime call
/// Stack: [] -> []

pub fn compile_pop_binding_frame<'a, 'b>(
    ctx: &mut BindingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pop_binding_frame_func_id, codegen.builder.func);

    // Call jit_runtime_pop_binding_frame(ctx)
    let ctx_ptr = codegen.ctx_ptr();
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
    // Result is status code, ignored for now
    Ok(())
}
