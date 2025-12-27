//! Special forms operation handlers for JIT compilation
//!
//! Handles: EvalIf, EvalLet, EvalLetStar, EvalMatch, EvalCase, EvalChain,
//!          EvalQuote, EvalUnquote, EvalEval, EvalBind, EvalNew, EvalCollapse,
//!          EvalSuperpose, EvalMemo, EvalMemoFirst, EvalPragma, EvalFunction,
//!          EvalLambda, EvalApply


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
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
/// Semantics: Only TAG_BOOL_FALSE and TAG_NIL are falsy.
/// Everything else (including TAG_BOOL_TRUE, integers, heap values) is truthy.
/// Stack: [condition, then_val, else_val] -> [result]

pub fn compile_eval_if<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let else_val = codegen.pop()?;
    let then_val = codegen.pop()?;
    let condition = codegen.pop()?;

    // Check for falsy values: TAG_BOOL_FALSE (TAG_BOOL | 0) or TAG_NIL
    let tag_bool_false = codegen.const_bool(false);
    let tag_nil = codegen.const_nil();

    // is_false = (condition == TAG_BOOL_FALSE)
    let is_false = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, condition, tag_bool_false);

    // is_nil = (condition == TAG_NIL)
    let is_nil = codegen
        .builder
        .ins()
        .icmp(IntCC::Equal, condition, tag_nil);

    // is_falsy = is_false || is_nil
    let is_falsy = codegen.builder.ins().bor(is_false, is_nil);

    // result = is_falsy ? else_val : then_val
    let result = codegen
        .builder
        .ins()
        .select(is_falsy, else_val, then_val);
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

pub fn compile_eval_let_star<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let unit_val = codegen.const_unit();
    codegen.push(unit_val)?;
    Ok(())
}

/// Compile EvalMatch opcode
///
/// Native implementation: call pattern_match directly instead of wrapper.
/// Stack: [value, pattern] -> [bool]

pub fn compile_eval_match<'a, 'b>(
    ctx: &mut SpecialFormsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let pattern = codegen.pop()?;
    let value = codegen.pop()?;

    // Call jit_runtime_pattern_match(ctx, pattern, value, ip)
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pattern_match_func_id, codegen.builder.func);

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

pub fn compile_eval_chain<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
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
    let call_inst = codegen.builder.ins().call(
        func_ref,
        &[ctx_ptr, name_idx_val, param_count_val, ip_val],
    );
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
