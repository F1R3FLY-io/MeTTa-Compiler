//! Pattern matching operation handlers for JIT compilation
//!
//! Handles: Match, MatchBind, MatchHead, MatchArity, MatchGuard, Unify, UnifyBind

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for pattern matching handlers that need runtime function access
pub struct PatternMatchingHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub pattern_match_func_id: FuncId,
    pub pattern_match_bind_func_id: FuncId,
    pub match_head_func_id: FuncId,
    pub match_arity_func_id: FuncId,
    pub unify_func_id: FuncId,
    pub unify_bind_func_id: FuncId,
}

/// Compile Match opcode
///
/// Pattern match without binding
/// Stack: [pattern, value] -> [bool]
pub fn compile_match<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let pattern = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pattern_match_func_id, codegen.builder.func);

    // Call jit_runtime_pattern_match(ctx, pattern, value, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MatchBind opcode
///
/// Pattern match with variable binding
/// Stack: [pattern, value] -> [bool]
pub fn compile_match_bind<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let pattern = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pattern_match_bind_func_id, codegen.builder.func);

    // Call jit_runtime_pattern_match_bind(ctx, pattern, value, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MatchHead opcode
///
/// Match head symbol of S-expression
/// Stack: [expr] -> [bool]
/// Operand: 1-byte index into constant pool for expected head symbol
pub fn compile_match_head<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let expected_head_idx = chunk.read_byte(offset + 1).unwrap_or(0);
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.match_head_func_id, codegen.builder.func);

    // Call jit_runtime_match_head(ctx, expr, expected_head_idx, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let head_idx_val = codegen
        .builder
        .ins()
        .iconst(types::I64, expected_head_idx as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, head_idx_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MatchArity opcode
///
/// Check if S-expression has expected arity
/// Stack: [expr] -> [bool]
/// Operand: 1-byte expected arity
pub fn compile_match_arity<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let expected_arity = chunk.read_byte(offset + 1).unwrap_or(0);
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.match_arity_func_id, codegen.builder.func);

    // Call jit_runtime_match_arity(ctx, expr, expected_arity, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let arity_val = codegen
        .builder
        .ins()
        .iconst(types::I64, expected_arity as i64);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, arity_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MatchGuard opcode
///
/// Match with guard condition
/// Stack: [pattern, value, guard] -> [bool]
/// Operand: 2-byte guard chunk index (currently unused in this implementation)
pub fn compile_match_guard<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let _guard_idx = chunk.read_u16(offset + 1).unwrap_or(0);
    let guard = codegen.pop()?;
    let value = codegen.pop()?;
    let pattern = codegen.pop()?;

    // First do the match
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pattern_match_bind_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
    let match_result = codegen.builder.inst_results(call_inst)[0];

    // AND the match result with the guard value
    // Both are NaN-boxed bools, so we need to check if both are TAG_BOOL_TRUE
    let true_val = codegen.const_bool(true);
    let false_val = codegen.const_bool(false);
    let match_is_true = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, match_result, true_val);
    let guard_is_true = codegen.builder.ins().icmp(IntCC::Equal, guard, true_val);
    let both_true = codegen.builder.ins().band(match_is_true, guard_is_true);
    let result = codegen.builder.ins().select(both_true, true_val, false_val);
    codegen.push(result)?;
    Ok(())
}

/// Compile Unify opcode
///
/// Unify two values (bidirectional pattern matching)
/// Stack: [a, b] -> [bool]
pub fn compile_unify<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let b = codegen.pop()?;
    let a = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.unify_func_id, codegen.builder.func);

    // Call jit_runtime_unify(ctx, a, b, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, a, b, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile UnifyBind opcode
///
/// Unify two values with variable binding
/// Stack: [a, b] -> [bool]
pub fn compile_unify_bind<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let b = codegen.pop()?;
    let a = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.unify_bind_func_id, codegen.builder.func);

    // Call jit_runtime_unify_bind(ctx, a, b, ip)
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, a, b, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
