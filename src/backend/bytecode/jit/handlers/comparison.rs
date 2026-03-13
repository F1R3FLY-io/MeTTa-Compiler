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

/// Emit an ordered comparison with integer + float fast-paths and runtime fallback.
///
/// Three-way branching: Long×Long uses icmp, Float×Float uses fcmp,
/// mixed types fall back to runtime FFI.
fn emit_comparison_with_fallback<'a, 'b>(
    ctx: &mut ComparisonHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    runtime_func_id: FuncId,
    int_cc: IntCC,
    float_cc: FloatCC,
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
    let check_float = codegen.builder.create_block();
    let float_path = codegen.builder.create_block();
    let runtime_path = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(both_long, int_path, &[], check_float, &[]);

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

    // === Check both float ===
    codegen.builder.switch_to_block(check_float);
    let qnan_base = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);
    let a_qnan = codegen.builder.ins().band(a, qnan_base);
    let b_qnan = codegen.builder.ins().band(b, qnan_base);
    let a_is_float = codegen.builder.ins().icmp(IntCC::NotEqual, a_qnan, qnan_base);
    let b_is_float = codegen.builder.ins().icmp(IntCC::NotEqual, b_qnan, qnan_base);
    let both_float = codegen.builder.ins().band(a_is_float, b_is_float);
    codegen
        .builder
        .ins()
        .brif(both_float, float_path, &[], runtime_path, &[]);

    // === Float fast-path ===
    codegen.builder.switch_to_block(float_path);
    let a_f64 = codegen.bitcast_to_f64(a);
    let b_f64 = codegen.bitcast_to_f64(b);
    let fcmp = codegen.builder.ins().fcmp(float_cc, a_f64, b_f64);
    let fresult = codegen.builder.ins().uextend(types::I64, fcmp);
    let fboxed = codegen.box_bool(fresult);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(fboxed)]);

    // === Runtime fallback (mixed types) ===
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
    codegen.builder.seal_block(check_float);
    codegen.builder.seal_block(float_path);
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
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedLessThan, FloatCC::LessThan)
        }

        Opcode::Le => {
            let func_id = ctx.numeric_le_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedLessThanOrEqual, FloatCC::LessThanOrEqual)
        }

        Opcode::Gt => {
            let func_id = ctx.numeric_gt_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedGreaterThan, FloatCC::GreaterThan)
        }

        Opcode::Ge => {
            let func_id = ctx.numeric_ge_func_id;
            emit_comparison_with_fallback(ctx, codegen, func_id, IntCC::SignedGreaterThanOrEqual, FloatCC::GreaterThanOrEqual)
        }

        Opcode::Eq => {
            // Equality uses bit-level identity check as fast-path, then
            // runtime numeric_eq for cross-type comparison (e.g., Long(2) == Float(2.0))
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            // Fast path: identical NaN-boxed bits → definitely equal
            let raw_eq = codegen.builder.ins().icmp(IntCC::Equal, a, b);

            let fast_true = codegen.builder.create_block();
            let slow_check = codegen.builder.create_block();
            let merge_block = codegen.builder.create_block();
            codegen
                .builder
                .append_block_param(merge_block, types::I64);

            codegen
                .builder
                .ins()
                .brif(raw_eq, fast_true, &[], slow_check, &[]);

            // === Fast true ===
            codegen.builder.switch_to_block(fast_true);
            let true_val = codegen.builder.ins().iconst(types::I64, 1);
            let boxed_true = codegen.box_bool(true_val);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(boxed_true)]);

            // === Slow check (runtime numeric_eq) ===
            codegen.builder.switch_to_block(slow_check);
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.numeric_eq_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let rt_result = codegen.builder.inst_results(call_inst)[0];
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(rt_result)]);

            // === Merge ===
            codegen.builder.switch_to_block(merge_block);
            codegen.builder.seal_block(fast_true);
            codegen.builder.seal_block(slow_check);
            codegen.builder.seal_block(merge_block);

            let result = codegen.builder.block_params(merge_block)[0];
            codegen.push(result)?;
            Ok(())
        }

        Opcode::Ne => {
            // Ne: bit-level identity check → definitely not-equal is false.
            // Otherwise call runtime numeric_eq and negate.
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let raw_eq = codegen.builder.ins().icmp(IntCC::Equal, a, b);

            let fast_false = codegen.builder.create_block();
            let slow_check = codegen.builder.create_block();
            let merge_block = codegen.builder.create_block();
            codegen
                .builder
                .append_block_param(merge_block, types::I64);

            codegen
                .builder
                .ins()
                .brif(raw_eq, fast_false, &[], slow_check, &[]);

            // === Fast false (same bits → equal → != is false) ===
            codegen.builder.switch_to_block(fast_false);
            let false_val = codegen.builder.ins().iconst(types::I64, 0);
            let boxed_false = codegen.box_bool(false_val);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(boxed_false)]);

            // === Slow check ===
            codegen.builder.switch_to_block(slow_check);
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.numeric_eq_func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let eq_result = codegen.builder.inst_results(call_inst)[0];
            // Negate: extract bool, xor with 1, rebox
            let eq_bool = codegen.extract_bool(eq_result);
            let one = codegen.builder.ins().iconst(types::I64, 1);
            let neq_bool = codegen.builder.ins().bxor(eq_bool, one);
            let boxed_neq = codegen.box_bool(neq_bool);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(boxed_neq)]);

            // === Merge ===
            codegen.builder.switch_to_block(merge_block);
            codegen.builder.seal_block(fast_false);
            codegen.builder.seal_block(slow_check);
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
