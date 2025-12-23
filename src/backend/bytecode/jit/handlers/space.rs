//! Space and state operation handlers for JIT compilation
//!
//! Handles: SpaceAdd, SpaceRemove, SpaceGetAtoms, SpaceMatch, NewState, GetState, ChangeState

#[cfg(feature = "jit")]
use cranelift::prelude::*;
#[cfg(feature = "jit")]
use cranelift_jit::JITModule;
#[cfg(feature = "jit")]
use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;

/// Context for space handlers that need runtime function access
#[cfg(feature = "jit")]
pub struct SpaceHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub space_add_func_id: FuncId,
    pub space_remove_func_id: FuncId,
    pub space_get_atoms_func_id: FuncId,
    pub space_match_func_id: FuncId,
    pub new_state_func_id: FuncId,
    pub get_state_func_id: FuncId,
    pub change_state_func_id: FuncId,
}

/// Compile SpaceAdd opcode
///
/// Add atom to space
/// Stack: [space, atom] -> [bool]
#[cfg(feature = "jit")]
pub fn compile_space_add<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let atom = codegen.pop()?;
    let space = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.space_add_func_id, codegen.builder.func);

    // Call jit_runtime_space_add(ctx, space, atom, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, space, atom, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile SpaceRemove opcode
///
/// Remove atom from space
/// Stack: [space, atom] -> [bool]
#[cfg(feature = "jit")]
pub fn compile_space_remove<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let atom = codegen.pop()?;
    let space = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.space_remove_func_id, codegen.builder.func);

    // Call jit_runtime_space_remove(ctx, space, atom, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, space, atom, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile SpaceGetAtoms opcode
///
/// Get all atoms from space
/// Stack: [space] -> [list]
#[cfg(feature = "jit")]
pub fn compile_space_get_atoms<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let space = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.space_get_atoms_func_id, codegen.builder.func);

    // Call jit_runtime_space_get_atoms(ctx, space, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, space, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile SpaceMatch opcode
///
/// Match pattern in space
/// Stack: [space, pattern, template] -> [results]
#[cfg(feature = "jit")]
pub fn compile_space_match<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let template = codegen.pop()?;
    let pattern = codegen.pop()?;
    let space = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.space_match_func_id, codegen.builder.func);

    // Call jit_runtime_space_match_nondet(ctx, space, pattern, template, ip)
    // Uses nondeterministic semantics with choice points for multiple matches
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, space, pattern, template, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile NewState opcode
///
/// Create a new mutable state cell
/// Stack: [initial_value] -> [state_handle]
#[cfg(feature = "jit")]
pub fn compile_new_state<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let initial_value = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.new_state_func_id, codegen.builder.func);

    // Call jit_runtime_new_state(ctx, initial_value, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, initial_value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile GetState opcode
///
/// Get current value from a state cell
/// Stack: [state_handle] -> [value]
#[cfg(feature = "jit")]
pub fn compile_get_state<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let state_handle = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.get_state_func_id, codegen.builder.func);

    // Call jit_runtime_get_state(ctx, state_handle, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, state_handle, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile ChangeState opcode
///
/// Change value of a state cell
/// Stack: [state_handle, new_value] -> [state_handle]
#[cfg(feature = "jit")]
pub fn compile_change_state<'a, 'b>(
    ctx: &mut SpaceHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let new_value = codegen.pop()?;
    let state_handle = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.change_state_func_id, codegen.builder.func);

    // Call jit_runtime_change_state(ctx, state_handle, new_value, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, state_handle, new_value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
