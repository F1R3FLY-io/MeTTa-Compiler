//! Special forms operation handlers for JIT compilation
//!
//! Handles: EvalIf, EvalLet, EvalLetStar, EvalMatch, EvalCase, EvalChain,
//!          EvalQuote, EvalUnquote, EvalEval, EvalBind, EvalNew, EvalCollapse,
//!          EvalSuperpose, EvalMemo, EvalMemoFirst, EvalPragma, EvalFunction,
//!          EvalLambda, EvalApply
//!
//! EvalIf uses branchless select. EvalMatch uses inline ground-value fast-paths
//! (same as Match opcode). Other forms use FFI fallback.

use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{
    JitResult, TAG_BOOL, TAG_LONG, TAG_MASK, TAG_UNIT, TAG_VAR,
};
use crate::backend::bytecode::BytecodeChunk;

/// Context for special forms handlers that need runtime function access

pub struct SpecialFormsHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub store_binding_func_id: FuncId,
    pub pattern_match_func_id: FuncId,
    pub eval_case_func_id: FuncId,
    pub eval_quote_func_id: FuncId,
    pub eval_unquote_func_id: FuncId,
    pub eval_eval_func_id: FuncId,
    pub eval_new_func_id: FuncId,
    pub eval_collapse_func_id: FuncId,
    pub eval_superpose_func_id: FuncId,
    pub eval_memo_func_id: FuncId,
    pub eval_memo_first_func_id: FuncId,
    pub eval_pragma_func_id: FuncId,
    pub eval_function_func_id: FuncId,
    pub eval_lambda_func_id: FuncId,
    pub eval_apply_func_id: FuncId,
}

/// Compile EvalIf opcode
///
/// Native implementation using Cranelift select instruction.
/// Semantics: Only TAG_BOOL_FALSE and TAG_UNIT are falsy.
/// Everything else (including TAG_BOOL_TRUE, integers, heap values) is truthy.
/// Stack: [condition, then_val, else_val] -> [result]

pub fn compile_eval_if<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    let else_val = codegen.pop()?;
    let then_val = codegen.pop()?;
    let condition = codegen.pop()?;

    // Check for falsy values: TAG_BOOL_FALSE (TAG_BOOL | 0) or TAG_UNIT
    let tag_bool_false = codegen.const_bool(false);
    let tag_unit = codegen.const_unit();

    // is_false = (condition == TAG_BOOL_FALSE)
    let is_false = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, condition, tag_bool_false);

    // is_unit = (condition == TAG_UNIT)
    let is_unit = codegen.builder.ins().icmp(IntCC::Equal, condition, tag_unit);

    // is_falsy = is_false || is_unit
    let is_falsy = codegen.builder.ins().bor(is_false, is_unit);

    // result = is_falsy ? else_val : then_val
    let result = codegen.builder.ins().select(is_falsy, else_val, then_val);
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalLet opcode
///
/// Native implementation: call store_binding directly and return Unit inline.
/// Stack: [value] -> [Unit], name_idx from operand

pub fn compile_eval_let<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.store_binding_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

    // Store binding returns status (ignored), we always push Unit
    codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);

    // Push Unit result (inline, no function call needed)
    let unit_val = codegen.const_unit();
    codegen.push(unit_val)?;
    Ok(())
}

/// Compile EvalLetStar opcode
///
/// Let* bindings are handled sequentially by the bytecode compiler.
/// This opcode is a marker/placeholder that just returns Unit.
/// Stack: [] -> [Unit]

pub fn compile_eval_let_star<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    let unit_val = codegen.const_unit();
    codegen.push(unit_val)?;
    Ok(())
}

/// Compile EvalMatch opcode with inline fast-paths for ground values.
///
/// Same inline fast-paths as Match opcode: variable → true, raw equality → true,
/// both ground → false, otherwise FFI fallback.
/// Stack: [value, pattern] -> [bool]

pub fn compile_eval_match<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let pattern = codegen.pop()?;
    let value = codegen.pop()?;

    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let qnan_base = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);

    // Check if pattern is tagged (not a float)
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

    // === Tagged path ===
    codegen.builder.switch_to_block(tagged_path);
    let pattern_tag = codegen.builder.ins().band(pattern, tag_mask);

    // Variable pattern → always matches
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

    // Raw bit equality → match
    codegen.builder.switch_to_block(check_exact);
    let raw_eq = codegen.builder.ins().icmp(IntCC::Equal, pattern, value);

    let check_ground = codegen.builder.create_block();
    codegen
        .builder
        .ins()
        .brif(raw_eq, fast_true, &[], check_ground, &[]);

    // Both ground types → no match (raw eq already failed)
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

/// Compile EvalCase opcode
///
/// Stack: [value] -> [case_index], case_count from operand
/// Case dispatch is complex (loops over patterns, installs bindings),
/// so we keep it as a runtime call.

pub fn compile_eval_case<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let case_count = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_case_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let case_count_val = codegen.builder.ins().iconst(types::I64, case_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, value, case_count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalChain opcode
///
/// Native implementation: Just discard first, keep second.
/// Chain (;) evaluates both but only returns the second result.
/// Stack: [first, second] -> [second]

pub fn compile_eval_chain<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    let second = codegen.pop()?;
    let _first = codegen.pop()?; // Discard first value
    codegen.push(second)?;
    Ok(())
}

/// Compile EvalQuote opcode
///
/// Stack: [expr] -> [quoted]

pub fn compile_eval_quote<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_quote_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalUnquote opcode
///
/// Stack: [quoted] -> [result]

pub fn compile_eval_unquote<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_unquote_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalEval opcode
///
/// Stack: [expr] -> [result]

pub fn compile_eval_eval<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_eval_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalBind opcode
///
/// Native implementation: call store_binding directly and return Unit inline.
/// Same optimization as EvalLet - avoid the wrapper function.
/// Stack: [value] -> [Unit], name_idx from operand

pub fn compile_eval_bind<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let value = codegen.pop()?;
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.store_binding_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

    // Store binding returns status (ignored), we always push Unit
    codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);

    // Push Unit result (inline, no function call needed)
    let unit_val = codegen.const_unit();
    codegen.push(unit_val)?;
    Ok(())
}

/// Compile EvalNew opcode
///
/// Stack: [] -> [space]

pub fn compile_eval_new<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_new_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalCollapse opcode
///
/// Stack: [expr] -> [list]

pub fn compile_eval_collapse<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_collapse_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalSuperpose opcode
///
/// Stack: [list] -> [choice]

pub fn compile_eval_superpose<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let list = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_superpose_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, list, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalMemo opcode
///
/// Stack: [expr] -> [result]

pub fn compile_eval_memo<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_memo_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalMemoFirst opcode
///
/// Stack: [expr] -> [result]

pub fn compile_eval_memo_first<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_memo_first_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, expr, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalPragma opcode
///
/// Stack: [directive] -> [Unit]

pub fn compile_eval_pragma<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let directive = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_pragma_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, directive, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalFunction opcode
///
/// Stack: [] -> [Unit], name_idx and param_count from operands

pub fn compile_eval_function<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
    let param_count = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_function_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
    let param_count_val = codegen.builder.ins().iconst(types::I64, param_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, name_idx_val, param_count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalLambda opcode
///
/// Stack: [] -> [closure], param_count from operand

pub fn compile_eval_lambda<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let param_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_lambda_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let param_count_val = codegen.builder.ins().iconst(types::I64, param_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, param_count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile EvalApply opcode
///
/// Stack: [closure] -> [result], arg_count from operand

pub fn compile_eval_apply<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let closure = codegen.pop()?;
    let arg_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.eval_apply_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let arg_count_val = codegen.builder.ins().iconst(types::I64, arg_count);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, closure, arg_count_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
