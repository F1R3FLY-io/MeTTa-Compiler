//! Pattern matching operation handlers for JIT compilation
//!
//! Handles: Match, MatchBind, MatchHead, MatchArity, MatchGuard, Unify, UnifyBind
//!
//! Match uses inline fast-paths for ground values (Long/Bool/Unit) and variables,
//! falling back to runtime FFI for complex patterns (S-expressions, atoms, strings).
//! MatchArity and MatchHead use quick-reject for non-pointer values.

use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{
    JitResult, TAG_BOOL, TAG_LONG, TAG_MASK, TAG_PTR, TAG_UNIT, TAG_VAR,
};
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

/// Compile Match opcode with inline fast-paths for ground values.
///
/// Pattern match without binding.
/// Stack: [pattern, value] -> [bool]
///
/// Fast-paths (inline Cranelift IR, no FFI call):
/// 1. Pattern is a variable (TAG_VAR) → always matches
/// 2. Pattern == value (raw bit equality) → matches (for tagged values only)
/// 3. Both pattern and value are ground types (Long/Bool/Unit) → no match
///    (since raw equality already failed, same-tag or cross-tag → mismatch)
///
/// Fallback: FFI call to jit_runtime_pattern_match for complex patterns
/// (S-expressions, atoms, strings, floats).

pub fn compile_match<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let pattern = codegen.pop()?;

    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let qnan_base = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);

    // Check if pattern is a tagged value (not a float).
    // Floats have (val & QNAN_BASE) != QNAN_BASE.
    let p_qnan = codegen.builder.ins().band(pattern, qnan_base);
    let p_is_tagged = codegen.builder.ins().icmp(IntCC::Equal, p_qnan, qnan_base);

    let tagged_path = codegen.builder.create_block();
    let runtime_path = codegen.builder.create_block();
    let fast_true = codegen.builder.create_block();
    let fast_false = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(p_is_tagged, tagged_path, &[], runtime_path, &[]);

    // === Tagged path: check variable, raw equality, ground type ===
    codegen.builder.switch_to_block(tagged_path);
    let pattern_tag = codegen.builder.ins().band(pattern, tag_mask);

    // Check if pattern is a variable → always matches
    let tag_var_const = codegen.builder.ins().iconst(types::I64, TAG_VAR as i64);
    let is_var = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, pattern_tag, tag_var_const);

    let check_exact = codegen.builder.create_block();
    codegen
        .builder
        .ins()
        .brif(is_var, fast_true, &[], check_exact, &[]);

    // Check raw bit equality → match (correct for all tagged types)
    codegen.builder.switch_to_block(check_exact);
    let raw_eq = codegen.builder.ins().icmp(IntCC::Equal, pattern, value);

    let check_ground = codegen.builder.create_block();
    codegen
        .builder
        .ins()
        .brif(raw_eq, fast_true, &[], check_ground, &[]);

    // Check if both are ground types → definitely no match (since raw eq failed)
    codegen.builder.switch_to_block(check_ground);
    let value_tag = codegen.builder.ins().band(value, tag_mask);

    let tag_long = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);
    let tag_bool = codegen.builder.ins().iconst(types::I64, TAG_BOOL as i64);
    let tag_unit = codegen.builder.ins().iconst(types::I64, TAG_UNIT as i64);

    let p_is_long = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, pattern_tag, tag_long);
    let p_is_bool = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, pattern_tag, tag_bool);
    let p_is_unit = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, pattern_tag, tag_unit);
    let p_ground_1 = codegen.builder.ins().bor(p_is_long, p_is_bool);
    let p_is_ground = codegen.builder.ins().bor(p_ground_1, p_is_unit);

    let v_is_long = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, value_tag, tag_long);
    let v_is_bool = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, value_tag, tag_bool);
    let v_is_unit = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, value_tag, tag_unit);
    let v_ground_1 = codegen.builder.ins().bor(v_is_long, v_is_bool);
    let v_is_ground = codegen.builder.ins().bor(v_ground_1, v_is_unit);

    let both_ground = codegen.builder.ins().band(p_is_ground, v_is_ground);
    codegen
        .builder
        .ins()
        .brif(both_ground, fast_false, &[], runtime_path, &[]);

    // === Fast true ===
    codegen.builder.switch_to_block(fast_true);
    let true_val = codegen.const_bool(true);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(true_val)]);

    // === Fast false ===
    codegen.builder.switch_to_block(fast_false);
    let false_val = codegen.const_bool(false);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(false_val)]);

    // === Runtime fallback ===
    codegen.builder.switch_to_block(runtime_path);
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pattern_match_func_id, codegen.builder.func);
    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
    let rt_result = codegen.builder.inst_results(call_inst)[0];
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(rt_result)]);

    // === Merge ===
    codegen.builder.switch_to_block(merge_block);
    codegen.builder.seal_block(tagged_path);
    codegen.builder.seal_block(check_exact);
    codegen.builder.seal_block(check_ground);
    codegen.builder.seal_block(fast_true);
    codegen.builder.seal_block(fast_false);
    codegen.builder.seal_block(runtime_path);
    codegen.builder.seal_block(merge_block);

    let result = codegen.builder.block_params(merge_block)[0];
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

/// Compile MatchHead opcode with non-S-expr quick-reject.
///
/// Match head symbol of S-expression.
/// Stack: [expr] -> [bool]
/// Operand: 1-byte index into constant pool for expected head symbol
///
/// Fast-path: if expr is not TAG_PTR, it cannot be an S-expression → false.
/// Otherwise: FFI call to jit_runtime_match_head.

pub fn compile_match_head<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let expected_head_idx = chunk.read_byte(offset + 1).unwrap_or(0);
    let expr = codegen.pop()?;

    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_ptr = codegen.builder.ins().iconst(types::I64, TAG_PTR as i64);
    let expr_tag = codegen.builder.ins().band(expr, tag_mask);
    let is_ptr = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, expr_tag, tag_ptr);

    let runtime_path = codegen.builder.create_block();
    let fast_false = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(is_ptr, runtime_path, &[], fast_false, &[]);

    // === Fast false (not a pointer → not an S-expression) ===
    codegen.builder.switch_to_block(fast_false);
    let false_val = codegen.const_bool(false);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(false_val)]);

    // === Runtime path (TAG_PTR → might be S-expression, check head) ===
    codegen.builder.switch_to_block(runtime_path);
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.match_head_func_id, codegen.builder.func);
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
    let rt_result = codegen.builder.inst_results(call_inst)[0];
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(rt_result)]);

    // === Merge ===
    codegen.builder.switch_to_block(merge_block);
    codegen.builder.seal_block(fast_false);
    codegen.builder.seal_block(runtime_path);
    codegen.builder.seal_block(merge_block);

    let result = codegen.builder.block_params(merge_block)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile MatchArity opcode with non-S-expr quick-reject.
///
/// Check if S-expression has expected arity.
/// Stack: [expr] -> [bool]
/// Operand: 1-byte expected arity
///
/// Fast-path: if expr is not TAG_PTR, it cannot be an S-expression → false.
/// Otherwise: FFI call to jit_runtime_match_arity.

pub fn compile_match_arity<'a, 'b>(
    ctx: &mut PatternMatchingHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let expected_arity = chunk.read_byte(offset + 1).unwrap_or(0);
    let expr = codegen.pop()?;

    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_ptr = codegen.builder.ins().iconst(types::I64, TAG_PTR as i64);
    let expr_tag = codegen.builder.ins().band(expr, tag_mask);
    let is_ptr = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, expr_tag, tag_ptr);

    let runtime_path = codegen.builder.create_block();
    let fast_false = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(is_ptr, runtime_path, &[], fast_false, &[]);

    // === Fast false (not a pointer → not an S-expression) ===
    codegen.builder.switch_to_block(fast_false);
    let false_val = codegen.const_bool(false);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(false_val)]);

    // === Runtime path (TAG_PTR → might be S-expression) ===
    codegen.builder.switch_to_block(runtime_path);
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.match_arity_func_id, codegen.builder.func);
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
    let rt_result = codegen.builder.inst_results(call_inst)[0];
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(rt_result)]);

    // === Merge ===
    codegen.builder.switch_to_block(merge_block);
    codegen.builder.seal_block(fast_false);
    codegen.builder.seal_block(runtime_path);
    codegen.builder.seal_block(merge_block);

    let result = codegen.builder.block_params(merge_block)[0];
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
