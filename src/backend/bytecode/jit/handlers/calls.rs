//! Call operation handlers for JIT compilation
//!
//! Handles: Call, TailCall, CallN, TailCallN, CallNative, CallExternal, CallCached


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for call handlers that need runtime function access

pub struct CallHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub call_func_id: FuncId,
    pub tail_call_func_id: FuncId,
    pub call_n_func_id: FuncId,
    pub tail_call_n_func_id: FuncId,
    pub call_native_func_id: FuncId,
    pub call_external_func_id: FuncId,
    pub call_cached_func_id: FuncId,
}

/// Compile Call opcode
///
/// Call: head_index:u16 arity:u8
/// Stack: [arg1, arg2, ..., argN] -> [result]

pub fn compile_call<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let head_index = chunk.read_u16(offset + 1).unwrap_or(0);
    let arity = chunk.code()[offset + 3] as usize;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.call_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let head_index_val = codegen.builder.ins().iconst(types::I64, head_index as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

    if arity > 0 {
        // Allocate a stack slot for the arguments
        let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (arity * 8) as u32, // 8 bytes per JitValue
            8,
        ));

        // Pop arguments and store in stack slot (they're in stack order)
        for i in (0..arity).rev() {
            let arg = codegen.pop()?;
            let slot_offset = (i * 8) as i32;
            codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
        }

        // Get pointer to arguments array
        let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);

        // Call jit_runtime_call(ctx, head_index, args_ptr, arity, ip)
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_index_val, args_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    } else {
        // No args - pass null pointer
        let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_index_val, null_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    }

    // Note: Bailout is always set by jit_runtime_call, so the result
    // is the call expression for the VM to dispatch.
    // The caller should check ctx.bailout after JIT returns.
    Ok(())
}

/// Compile TailCall opcode
///
/// TailCall: head_index:u16 arity:u8
/// Same as Call but signals TCO to VM

pub fn compile_tail_call<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let head_index = chunk.read_u16(offset + 1).unwrap_or(0);
    let arity = chunk.code()[offset + 3] as usize;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.tail_call_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let head_index_val = codegen.builder.ins().iconst(types::I64, head_index as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

    if arity > 0 {
        let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (arity * 8) as u32,
            8,
        ));

        for i in (0..arity).rev() {
            let arg = codegen.pop()?;
            let slot_offset = (i * 8) as i32;
            codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
        }

        let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_index_val, args_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    } else {
        let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_index_val, null_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    }
    Ok(())
}

/// Compile CallN opcode
///
/// CallN: arity:u8
/// Stack: [head, arg1, arg2, ..., argN] -> [result]
/// Unlike Call, head is on stack not in constant pool

pub fn compile_call_n<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let arity = chunk.code()[offset + 1] as usize;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.call_n_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

    if arity > 0 {
        // Allocate a stack slot for the arguments
        let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (arity * 8) as u32, // 8 bytes per JitValue
            8,
        ));

        // Pop arguments in reverse order and store in stack slot
        for i in (0..arity).rev() {
            let arg = codegen.pop()?;
            let slot_offset = (i * 8) as i32;
            codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
        }

        // Pop head value (it's below the args on stack)
        let head_val = codegen.pop()?;

        // Get pointer to arguments array
        let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);

        // Call jit_runtime_call_n(ctx, head_val, args_ptr, arity, ip)
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_val, args_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    } else {
        // No args - just pop head
        let head_val = codegen.pop()?;
        let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_val, null_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    }
    Ok(())
}

/// Compile TailCallN opcode
///
/// TailCallN: arity:u8
/// Stack: [head, arg1, arg2, ..., argN] -> [result]
/// Same as CallN but signals TCO to VM

pub fn compile_tail_call_n<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let arity = chunk.code()[offset + 1] as usize;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.tail_call_n_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

    if arity > 0 {
        let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (arity * 8) as u32,
            8,
        ));

        for i in (0..arity).rev() {
            let arg = codegen.pop()?;
            let slot_offset = (i * 8) as i32;
            codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
        }

        // Pop head value
        let head_val = codegen.pop()?;

        let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_val, args_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    } else {
        let head_val = codegen.pop()?;
        let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
        let call_inst = codegen.builder.ins().call(
            func_ref,
            &[ctx_ptr, head_val, null_ptr, arity_val, ip_val],
        );
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    }
    Ok(())
}

/// Compile CallNative opcode
///
/// Stack: [args...] -> [result]
/// func_id: u16, arity: u8

pub fn compile_call_native<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let func_id = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
    let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.call_native_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let func_id_val = codegen.builder.ins().iconst(types::I64, func_id);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, func_id_val, arity_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile CallExternal opcode
///
/// Stack: [args...] -> [result]
/// name_idx: u16, arity: u8

pub fn compile_call_external<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
    let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.call_external_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, arity_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile CallCached opcode
///
/// Stack: [args...] -> [result]
/// head_idx: u16, arity: u8

pub fn compile_call_cached<'a, 'b>(
    ctx: &mut CallHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let head_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
    let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.call_cached_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let head_idx_val = codegen.builder.ins().iconst(types::I64, head_idx);
    let arity_val = codegen.builder.ins().iconst(types::I64, arity);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, head_idx_val, arity_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
