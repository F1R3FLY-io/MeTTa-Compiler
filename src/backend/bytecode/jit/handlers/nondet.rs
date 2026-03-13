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

/// Compile Fork opcode — with inline single-alternative fast path (Phase 10)
///
/// Fork: count:u16 (followed by count u16 indices in bytecode)
/// Stack: [] -> [first_alternative]
///
/// When count == 1, the fork is trivial: load the single constant and push it.
/// No choice point is needed since there's only one alternative.
/// This eliminates the FFI call overhead for the common single-alternative case.
///
/// When count > 1, falls back to FFI `jit_runtime_fork_native` which creates
/// choice points for backtracking across multiple alternatives.
///
/// When count == 0, pushes Unit (no alternatives).

pub fn compile_fork<'a, 'b>(
    ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    use crate::backend::bytecode::jit::types::JitContext;

    let count = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

    if count == 0 {
        // No alternatives — push Unit
        let unit_val = codegen
            .builder
            .ins()
            .iconst(types::I64, crate::backend::bytecode::jit::types::JitValue::unit().to_bits() as i64);
        codegen.push(unit_val)?;
        return Ok(());
    }

    if count == 1 {
        // Single-alternative fast path: inline constant load, no choice point.
        // The first alternative index is at offset + 3.
        let alt_idx = chunk.read_u16(offset + 3).unwrap_or(0) as usize;

        // Load the constant from JitContext.constants[alt_idx]
        let ctx_ptr = codegen.ctx_ptr();

        // Load constants pointer
        let constants_ptr = codegen.builder.ins().load(
            types::I64,
            MemFlags::trusted(),
            ctx_ptr,
            JitContext::OFFSET_CONSTANTS,
        );

        // Calculate address: constants_ptr + alt_idx * 8 (u64 = 8 bytes)
        let byte_offset = codegen.builder.ins().iconst(types::I64, (alt_idx * 8) as i64);
        let addr = codegen.builder.ins().iadd(constants_ptr, byte_offset);

        // Load the constant value
        let value = codegen
            .builder
            .ins()
            .load(types::I64, MemFlags::trusted(), addr, 0);

        codegen.push(value)?;
        return Ok(());
    }

    // Multi-alternative path: use FFI for choice point management
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.fork_native_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let count_val = codegen.builder.ins().iconst(types::I64, count as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

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
    Ok(())
}

/// Compile Yield opcode — INLINE version (Phase 10.2)
///
/// Stage 2 JIT: Yield stores result and returns signal to dispatcher.
/// Directly stores the result to results[results_count] and increments the
/// count via memory operations, avoiding the FFI call overhead.
///
/// Stack: [value] -> []
/// Returns: JIT_SIGNAL_YIELD to signal dispatcher
///
/// Fast path (inline):
///   1. Load results_count and results_cap
///   2. If results_count < results_cap: store value at results[results_count], increment
///   3. Set resume_ip
///   4. Return JIT_SIGNAL_YIELD
///
/// The overflow case (results_count >= results_cap) still yields but loses the result,
/// matching the behavior of jit_runtime_yield_native.

pub fn compile_yield<'a, 'b>(
    _ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    use crate::backend::bytecode::jit::types::JitContext;

    let value = codegen.pop()?;
    let ctx_ptr = codegen.ctx_ptr();

    // Load results_count and results_cap
    let results_count = codegen.builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        ctx_ptr,
        JitContext::OFFSET_RESULTS_COUNT,
    );
    let results_cap = codegen.builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        ctx_ptr,
        JitContext::OFFSET_RESULTS_CAP,
    );

    // Check: results_count < results_cap
    let has_space = codegen
        .builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, results_count, results_cap);

    let store_block = codegen.builder.create_block();
    let skip_block = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();

    codegen
        .builder
        .ins()
        .brif(has_space, store_block, &[], skip_block, &[]);

    // Store block: results[results_count] = value; results_count++
    codegen.builder.switch_to_block(store_block);
    codegen.builder.seal_block(store_block);
    {
        // Load results pointer
        let results_ptr = codegen.builder.ins().load(
            types::I64,
            MemFlags::trusted(),
            ctx_ptr,
            JitContext::OFFSET_RESULTS,
        );

        // Calculate address: results_ptr + results_count * 8 (JitValue is u64 = 8 bytes)
        let eight = codegen.builder.ins().iconst(types::I64, 8);
        let byte_offset = codegen.builder.ins().imul(results_count, eight);
        let addr = codegen.builder.ins().iadd(results_ptr, byte_offset);

        // Store value
        codegen
            .builder
            .ins()
            .store(MemFlags::trusted(), value, addr, 0);

        // Increment results_count
        let one = codegen.builder.ins().iconst(types::I64, 1);
        let new_count = codegen.builder.ins().iadd(results_count, one);
        codegen.builder.ins().store(
            MemFlags::trusted(),
            new_count,
            ctx_ptr,
            JitContext::OFFSET_RESULTS_COUNT,
        );
    }
    codegen.builder.ins().jump(merge_block, &[]);

    // Skip block: overflow — result lost but execution continues
    codegen.builder.switch_to_block(skip_block);
    codegen.builder.seal_block(skip_block);
    codegen.builder.ins().jump(merge_block, &[]);

    // Merge block: set resume_ip and return JIT_SIGNAL_YIELD
    codegen.builder.switch_to_block(merge_block);
    codegen.builder.seal_block(merge_block);

    // Set resume_ip = offset
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    codegen.builder.ins().store(
        MemFlags::trusted(),
        ip_val,
        ctx_ptr,
        JitContext::OFFSET_RESUME_IP,
    );

    // Return JIT_SIGNAL_YIELD
    let yield_signal = codegen
        .builder
        .ins()
        .iconst(types::I64, crate::backend::bytecode::jit::JIT_SIGNAL_YIELD);
    codegen.builder.ins().return_(&[yield_signal]);
    codegen.mark_terminated();
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
    codegen.mark_terminated();

    // Continue block
    codegen.builder.switch_to_block(cont_block);
    codegen.builder.seal_block(cont_block);
    codegen.clear_terminated(); // Reset flag for new unterminated block
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
    codegen.mark_terminated();
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
    codegen.mark_terminated();
    Ok(())
}

/// Compile BeginNondet opcode — INLINE version (Phase 10.2)
///
/// Stack: [] -> [] - mark start of nondeterministic section
/// Directly increments fork_depth and sets in_nondet_mode via memory stores,
/// eliminating the FFI call overhead (~5-15 cycles saved per invocation).

pub fn compile_begin_nondet<'a, 'b>(
    _ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    _offset: usize,
) -> JitResult<()> {
    use crate::backend::bytecode::jit::types::JitContext;

    let ctx_ptr = codegen.ctx_ptr();

    // Store true (1) to in_nondet_mode
    let one_i8 = codegen.builder.ins().iconst(types::I8, 1);
    codegen.builder.ins().store(
        MemFlags::trusted(),
        one_i8,
        ctx_ptr,
        JitContext::OFFSET_IN_NONDET_MODE,
    );

    // Load fork_depth, add 1, store back
    let fork_depth = codegen.builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        ctx_ptr,
        JitContext::OFFSET_FORK_DEPTH,
    );
    let one = codegen.builder.ins().iconst(types::I64, 1);
    let new_depth = codegen.builder.ins().iadd(fork_depth, one);
    codegen.builder.ins().store(
        MemFlags::trusted(),
        new_depth,
        ctx_ptr,
        JitContext::OFFSET_FORK_DEPTH,
    );

    Ok(())
}

/// Compile EndNondet opcode — INLINE version (Phase 10.2)
///
/// Stack: [] -> [] - mark end of nondeterministic section
/// Directly decrements fork_depth and conditionally clears in_nondet_mode.

pub fn compile_end_nondet<'a, 'b>(
    _ctx: &mut NondetHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    _offset: usize,
) -> JitResult<()> {
    use crate::backend::bytecode::jit::types::JitContext;

    let ctx_ptr = codegen.ctx_ptr();

    // Load fork_depth
    let fork_depth = codegen.builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        ctx_ptr,
        JitContext::OFFSET_FORK_DEPTH,
    );

    // Clamp: new_depth = max(fork_depth - 1, 0)
    let one = codegen.builder.ins().iconst(types::I64, 1);
    let decremented = codegen.builder.ins().isub(fork_depth, one);
    let zero = codegen.builder.ins().iconst(types::I64, 0);
    let new_depth = codegen.builder.ins().smax(decremented, zero);

    // Store new fork_depth
    codegen.builder.ins().store(
        MemFlags::trusted(),
        new_depth,
        ctx_ptr,
        JitContext::OFFSET_FORK_DEPTH,
    );

    // If new_depth == 0, set in_nondet_mode = false; otherwise keep true.
    // icmp returns I8 (0 or 1). We want in_nondet_mode = (new_depth != 0).
    // is_zero: 1 if depth==0, 0 otherwise
    let is_zero = codegen
        .builder
        .ins()
        .icmp_imm(IntCC::Equal, new_depth, 0);
    // nondet_flag = 1 - is_zero: 0 if depth==0, 1 if depth!=0
    let one_i8 = codegen.builder.ins().iconst(types::I8, 1);
    let nondet_flag = codegen.builder.ins().isub(one_i8, is_zero);
    codegen.builder.ins().store(
        MemFlags::trusted(),
        nondet_flag,
        ctx_ptr,
        JitContext::OFFSET_IN_NONDET_MODE,
    );

    Ok(())
}
