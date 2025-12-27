//! Rule dispatch operation handlers for JIT compilation
//!
//! Handles: DispatchRules, TryRule, NextRule, CommitRule, FailRule, LookupRules, ApplySubst, DefineRule

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::BytecodeChunk;

/// Context for rule handlers that need runtime function access
pub struct RulesHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub dispatch_rules_func_id: FuncId,
    pub try_rule_func_id: FuncId,
    pub next_rule_func_id: FuncId,
    pub commit_rule_func_id: FuncId,
    pub fail_rule_func_id: FuncId,
    pub lookup_rules_func_id: FuncId,
    pub apply_subst_func_id: FuncId,
    pub define_rule_func_id: FuncId,
}

/// Compile DispatchRules opcode
///
/// Dispatch rules for an expression
/// Stack: [expr] -> [count]
pub fn compile_dispatch_rules<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.dispatch_rules_func_id, codegen.builder.func);

    // Call jit_runtime_dispatch_rules(ctx, expr, ip)
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

/// Compile TryRule opcode
///
/// Try a single rule
/// Stack: [] -> [result] (using rule_idx from operand)
pub fn compile_try_rule<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let rule_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.try_rule_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let rule_idx_val = codegen.builder.ins().iconst(types::I64, rule_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, rule_idx_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile NextRule opcode
///
/// Advance to next matching rule
/// Stack: [] -> [] (returns status)
pub fn compile_next_rule<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.next_rule_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    // Result is status, not pushed to stack
    Ok(())
}

/// Compile CommitRule opcode
///
/// Commit to current rule (cut)
/// Stack: [] -> []
pub fn compile_commit_rule<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.commit_rule_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    Ok(())
}

/// Compile FailRule opcode
///
/// Signal explicit rule failure
/// Stack: [] -> [] (signals backtracking)
pub fn compile_fail_rule<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.fail_rule_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
    // Result is signal, caller handles backtracking
    Ok(())
}

/// Compile LookupRules opcode
///
/// Look up rules by head symbol
/// Stack: [] -> [count]
pub fn compile_lookup_rules<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let head_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.lookup_rules_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let head_idx_val = codegen.builder.ins().iconst(types::I64, head_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, head_idx_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}

/// Compile ApplySubst opcode
///
/// Apply substitution to an expression
/// Stack: [expr] -> [result]
pub fn compile_apply_subst<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    offset: usize,
) -> JitResult<()> {
    let expr = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.apply_subst_func_id, codegen.builder.func);

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

/// Compile DefineRule opcode
///
/// Define a new rule
/// Stack: [pattern, body] -> [Unit]
pub fn compile_define_rule<'a, 'b>(
    ctx: &mut RulesHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
) -> JitResult<()> {
    let _body = codegen.pop()?;
    let _pattern = codegen.pop()?;
    let pattern_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.define_rule_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let pattern_idx_val = codegen.builder.ins().iconst(types::I64, pattern_idx);
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
    let call_inst = codegen
        .builder
        .ins()
        .call(func_ref, &[ctx_ptr, pattern_idx_val, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
