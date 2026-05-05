//! Comparison and boolean operation handlers for JIT compilation
//!
//! Boolean ops: And, Or, Not, Xor
//! Comparison ops: Lt, Le, Gt, Ge, Eq, Ne, StructEq
//!
//! Ordered comparisons (Lt, Le, Gt, Ge) use integer fast-path + runtime fallback.
//! Equality (Eq) uses raw-bit identity check + runtime fallback for cross-type equality.

use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{JitResult, TAG_LONG, TAG_MASK};
use crate::backend::bytecode::Opcode;

/// Context for comparison handlers that need runtime function access
pub struct ComparisonHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub numeric_lt_func_id: FuncId,
    pub numeric_le_func_id: FuncId,
    pub numeric_gt_func_id: FuncId,
    pub numeric_ge_func_id: FuncId,
    pub numeric_eq_func_id: FuncId,
}

/// Compile boolean operation opcodes (no runtime calls needed)
pub fn compile_boolean_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::And => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_bool(a, offset)?;
            codegen.guard_bool(b, offset)?;

            let a_val = codegen.extract_bool(a);
            let b_val = codegen.extract_bool(b);
            let result = codegen.builder.ins().band(a_val, b_val);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Or => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_bool(a, offset)?;
            codegen.guard_bool(b, offset)?;

            let a_val = codegen.extract_bool(a);
            let b_val = codegen.extract_bool(b);
            let result = codegen.builder.ins().bor(a_val, b_val);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Not => {
            let a = codegen.pop()?;
            codegen.guard_bool(a, offset)?;

            let a_val = codegen.extract_bool(a);
            let one = codegen.builder.ins().iconst(types::I64, 1);
            let result = codegen.builder.ins().bxor(a_val, one);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Xor => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_bool(a, offset)?;
            codegen.guard_bool(b, offset)?;

            let a_val = codegen.extract_bool(a);
            let b_val = codegen.extract_bool(b);
            let result = codegen.builder.ins().bxor(a_val, b_val);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        _ => unreachable!("compile_boolean_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}

/// Emit an ordered comparison with integer fast-path and runtime float fallback.
fn emit_comparison_with_fallback<'a, 'b>(
    ctx: &mut ComparisonHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    runtime_func_id: FuncId,
    int_cc: IntCC,
) -> JitResult<()> {
    let b = codegen.pop()?;
    let a = codegen.pop()?;

    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_long = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);

    let a_tag = codegen.builder.ins().band(a, tag_mask);
    let b_tag = codegen.builder.ins().band(b, tag_mask);

    let a_is_long = codegen.builder.ins().icmp(IntCC::Equal, a_tag, tag_long);
    let b_is_long = codegen.builder.ins().icmp(IntCC::Equal, b_tag, tag_long);
    let both_long = codegen.builder.ins().band(a_is_long, b_is_long);

    let int_path = codegen.builder.create_block();
    let runtime_path = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(both_long, int_path, &[], runtime_path, &[]);

    // === Integer fast-path ===
    codegen.builder.switch_to_block(int_path);
    let a_val = codegen.extract_long(a);
    let b_val = codegen.extract_long(b);
    let cmp = codegen.builder.ins().icmp(int_cc, a_val, b_val);
    let result = codegen.builder.ins().uextend(types::I64, cmp);
    let boxed = codegen.box_bool(result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(boxed)]);

    // === Runtime float fallback ===
    codegen.builder.switch_to_block(runtime_path);
    let func_ref = ctx
        .module
        .declare_func_in_func(runtime_func_id, codegen.builder.func);
    let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
    let rt_result = codegen.builder.inst_results(call_inst)[0];
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(rt_result)]);

    // === Merge ===
    codegen.builder.switch_to_block(merge_block);
    codegen.builder.seal_block(int_path);
    codegen.builder.seal_block(runtime_path);
    codegen.builder.seal_block(merge_block);

    let result = codegen.builder.block_params(merge_block)[0];
    codegen.push(result)?;

    Ok(())
}

/// Compile comparison operation opcodes with integer fast-path and runtime float fallback
pub fn compile_comparison_op<'a, 'b>(
    ctx: &mut ComparisonHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
) -> JitResult<()> {
    match op {
        Opcode::Lt => {
            let func_id = ctx.numeric_lt_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedLessThan)
        }

        Opcode::Le => {
            let func_id = ctx.numeric_le_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedLessThanOrEqual)
        }

        Opcode::Gt => {
            let func_id = ctx.numeric_gt_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedGreaterThan)
        }

        Opcode::Ge => {
            let func_id = ctx.numeric_ge_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedGreaterThanOrEqual)
        }

        Opcode::Eq => {
            // H4 (2026-05-05) hard-cut: removed raw-bit-eq fast-path.
            // The fast-true on `a == b` (raw bit equality) leaks NaN-equals-itself:
            // identical NaN bit patterns would short-circuit to true even though
            // IEEE 754 says NaN != NaN. The runtime `numeric_eq` (now strict) does
            // the right thing including NaN handling. The only potential cost is
            // skipping the fast-path for atom-identity equality, but
            // `numeric_eq`'s structural match already short-circuits at the
            // `inner_ptr` level for identical heap pointers.
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.numeric_eq_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
            Ok(())
        }

        Opcode::Ne => {
            // H4 hard-cut: removed raw-bit-eq fast-false. Same NaN reasoning
            // as Eq above. Call runtime numeric_eq and negate.
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.numeric_eq_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let eq_result = codegen.builder.inst_results(call_inst)[0];
            let eq_bool = codegen.extract_bool(eq_result);
            let one = codegen.builder.ins().iconst(types::I64, 1);
            let neq_bool = codegen.builder.ins().bxor(eq_bool, one);
            let boxed_neq = codegen.box_bool(neq_bool);
            // Use the merge_block-style return for consistency with prior code.
            let merge_block = codegen.builder.create_block();
            codegen
                .builder
                .append_block_param(merge_block, types::I64);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(boxed_neq)]);

            // === Merge ===
            codegen.builder.switch_to_block(merge_block);
            codegen.builder.seal_block(merge_block);

            let result = codegen.builder.block_params(merge_block)[0];
            codegen.push(result)?;
            Ok(())
        }

        Opcode::StructEq => {
            // Structural equality: compare NaN-boxed values directly
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
            Ok(())
        }

        _ => unreachable!("compile_comparison_op called with wrong opcode: {:?}", op),
    }
}
