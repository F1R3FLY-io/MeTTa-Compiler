//! Nondeterminism operation handlers for JIT compilation
//!
//! Handles: Fork, Yield, Collect, Cut, Guard, Amb, Commit, Backtrack, Fail, BeginNondet, EndNondet

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for nondeterminism handlers that need runtime function access

pub struct NondetHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub fork_native_func_id: FuncId,
    pub yield_native_func_id: FuncId,
    pub collect_native_func_id: FuncId,
    pub cut_func_id: FuncId,
    pub guard_func_id: FuncId,
    pub amb_func_id: FuncId,
    pub commit_func_id: FuncId,
    pub backtrack_func_id: FuncId,
    pub begin_nondet_func_id: FuncId,
    pub end_nondet_func_id: FuncId,
}

/// Compile Fork opcode
///
/// Fork: count:u16 (followed by count u16 indices in bytecode)
/// Stack: [] -> [first_alternative]
/// Stage 2 JIT: Use native fork which creates choice points without bailout

pub fn compile_fork<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let count = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

    // Use fork_native instead of fork to avoid bailing to VM
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.fork_native_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let count_val = codegen.builder.ins().iconst(types::I64, count as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

    if count > 0 {
        // Allocate stack slot for indices array
        let indices_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (count * 8) as u32, // 8 bytes per u64
            8,
        ));

        // Read indices from bytecode and store in slot
        for i in 0..count {
            // Each index is at offset + 3 + (i * 2)
            let idx = chunk.read_u16(offset + 3 + (i * 2)).unwrap_or(0);
            let idx_val = codegen.builder.ins().iconst(types::I64, idx as i64);
            let slot_offset = (i * 8) as i32;
            codegen
                .builder
                .ins()
                .stack_store(idx_val, indices_slot, slot_offset);
        }

        // Get pointer to indices array
        let indices_ptr = codegen
            .builder
            .ins()
            .stack_addr(types::I64, indices_slot, 0);

        // Call jit_runtime_fork_native(ctx, count, indices_ptr, ip)
        let call_inst = codegen
            .builder
            .ins()
            .call(func_ref, &[ctx_ptr, count_val, indices_ptr, ip_val]);
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    } else {
        // No alternatives - pass null pointer
        let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
        let call_inst = codegen
            .builder
            .ins()
            .call(func_ref, &[ctx_ptr, count_val, null_ptr, ip_val]);
        let result = codegen.builder.inst_results(call_inst)[0];
        codegen.push(result)?;
    }
    Ok(())
}

/// Compile Yield opcode
///
/// Stage 2 JIT: Yield stores result and returns signal to dispatcher
/// Stack: [value] -> []
/// Returns: JIT_SIGNAL_YIELD to signal dispatcher

pub fn compile_yield<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.yield_native_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

    // Call jit_runtime_yield_native(ctx, value, ip) -> signal
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, value, ip_val]);
    let signal = codegen.builder.inst_results(call_inst)[0];

    // Return the signal to dispatcher (JIT_SIGNAL_YIELD = 2)
    // Dispatcher will handle backtracking and re-entry
    codegen.builder.ins().return_(&[signal]);
    Ok(())
}

/// Compile Collect opcode
///
/// Stage 2 JIT: Collect gathers all yielded results into SExpr
/// Stack: [] -> [SExpr of results]
/// Note: chunk_index is ignored in native version - results stored in ctx.results

pub fn compile_collect<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let _chunk_index = chunk.read_u16(offset + 1).unwrap_or(0);

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.collect_native_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();

    // Call jit_runtime_collect_native(ctx) -> NaN-boxed SExpr
    let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile Cut opcode
///
/// Stack: [] -> [Unit] - prune all choice points

pub fn compile_cut<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.cut_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile Guard opcode
///
/// Stack: [bool] -> [] - backtrack if false

pub fn compile_guard<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let condition = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.guard_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, condition, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];

    // If result is 0, return FAIL signal
    let zero = codegen.builder.ins().iconst(types::I64, 0);
    let is_fail = codegen.builder.ins().icmp(IntCC::Equal, result, zero);

    let fail_block = codegen.builder.create_block();
    let cont_block = codegen.builder.create_block();

    codegen
        .builder
        .ins()
        .brif(is_fail, fail_block, &[], cont_block, &[]);

    // Fail block - return FAIL signal
    codegen.builder.switch_to_block(fail_block);
    codegen.builder.seal_block(fail_block);
    let fail_signal = codegen
        .builder
        .ins()
        .iconst(types::I64, crate::backend::bytecode::jit::JIT_SIGNAL_FAIL);
    codegen.builder.ins().return_(&[fail_signal]);

    // Continue block
    codegen.builder.switch_to_block(cont_block);
    codegen.builder.seal_block(cont_block);
    Ok(())
}

/// Compile Amb opcode
///
/// Stack: [alt1, alt2, ..., altN] -> [selected]
/// alt_count from operand

pub fn compile_amb<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let alt_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.amb_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let alt_count_val = codegen.builder.ins().iconst(types::I64, alt_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, alt_count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile Commit opcode
///
/// Stack: [] -> [Unit] - remove N choice points

pub fn compile_commit<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.commit_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let count_val = codegen.builder.ins().iconst(types::I64, count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile Backtrack opcode
///
/// Stack: [] -> [] - force immediate backtracking

pub fn compile_backtrack<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.backtrack_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    let signal = codegen.builder.inst_results(call_inst)[0];
    // Return the FAIL signal
    codegen.builder.ins().return_(&[signal]);
    Ok(())
}

/// Compile Fail opcode
///
/// Stack: [] -> [] - trigger immediate backtracking
/// Simply return the FAIL signal - semantically identical to Backtrack

pub fn compile_fail<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    let signal = codegen
        .builder
        .ins()
        .iconst(types::I64, crate::backend::bytecode::jit::JIT_SIGNAL_FAIL);
    codegen.builder.ins().return_(&[signal]);
    Ok(())
}

/// Compile BeginNondet opcode
///
/// Stack: [] -> [] - mark start of nondeterministic section
/// Increment fork_depth in JitContext

pub fn compile_begin_nondet<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.begin_nondet_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    // No return value, just a marker
    Ok(())
}

/// Compile EndNondet opcode
///
/// Stack: [] -> [] - mark end of nondeterministic section
/// Decrement fork_depth in JitContext

pub fn compile_end_nondet<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.end_nondet_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    // No return value, just a marker
    Ok(())
}
