//! Arithmetic operation handlers for JIT compilation
//!
//! Handles: Add, Sub, Mul, Div, Mod, Neg, Abs, FloorDiv, Pow
//!
//! Binary arithmetic ops (Add, Sub, Mul, Div, Mod) use an integer fast-path:
//! if both operands are TAG_LONG, perform the operation inline with Cranelift IR.
//! Otherwise, call a runtime FFI function that handles all type combinations
//! (Long×Long, Float×Float, Long×Float, Float×Long).

use cranelift::codegen::ir::BlockArg;
use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{JitResult, TAG_LONG, TAG_MASK};
use crate::backend::bytecode::Opcode;

/// Context for arithmetic handlers that need runtime function access
pub struct ArithmeticHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub pow_func_id: FuncId,
    // Numeric operations with type promotion (float fallback)
    pub numeric_add_func_id: FuncId,
    pub numeric_sub_func_id: FuncId,
    pub numeric_mul_func_id: FuncId,
    pub numeric_div_func_id: FuncId,
    pub numeric_mod_func_id: FuncId,
    pub numeric_neg_func_id: FuncId,
    pub numeric_abs_func_id: FuncId,
}

/// Emit a binary arithmetic operation with integer + float fast-paths and runtime fallback.
///
/// Three-way branching eliminates FFI calls for both Long×Long AND Float×Float:
///
/// ```text
/// entry:
///   both_long = (a_tag == TAG_LONG) & (b_tag == TAG_LONG)
///   brif both_long → int_path, check_float
///
/// check_float:
///   both_float = neither value is NaN-boxed (raw IEEE 754 doubles)
///   brif both_float → float_path, runtime_path
///
/// int_path:
///   result = <int_op>(extract_long(a), extract_long(b))
///   jump merge_block(box_long(result))
///
/// float_path:
///   result = <float_op>(bitcast_f64(a), bitcast_f64(b))
///   jump merge_block(bitcast_i64(result))
///
/// runtime_path:
///   rt_result = call <runtime_func>(a, b)  // mixed types
///   jump merge_block(rt_result)
///
/// merge_block(result):
///   push(result)
/// ```
fn emit_binary_arith_with_fallback<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    runtime_func_id: FuncId,
    int_op: impl FnOnce(&mut CodegenContext<'a, 'b>, Value, Value) -> Value,
    float_op: impl FnOnce(&mut CodegenContext<'a, 'b>, Value, Value) -> Value,
    offset: usize,
) -> JitResult<()> {
    let b = codegen.pop()?;
    let a = codegen.pop()?;

    // Extract tags
    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_long = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);

    let a_tag = codegen.builder.ins().band(a, tag_mask);
    let b_tag = codegen.builder.ins().band(b, tag_mask);

    let a_is_long = codegen.builder.ins().icmp(IntCC::Equal, a_tag, tag_long);
    let b_is_long = codegen.builder.ins().icmp(IntCC::Equal, b_tag, tag_long);
    let both_long = codegen.builder.ins().band(a_is_long, b_is_long);

    // Create blocks
    let int_path = codegen.builder.create_block();
    let check_float = codegen.builder.create_block();
    let float_path = codegen.builder.create_block();
    let runtime_path = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();

    // Add block parameter for the merge block
    codegen
        .builder
        .append_block_param(merge_block, types::I64);

    // Branch: both long → int_path, else → check_float
    codegen
        .builder
        .ins()
        .brif(both_long, int_path, &[], check_float, &[]);

    // === Integer fast-path ===
    codegen.builder.switch_to_block(int_path);
    let a_val = codegen.extract_long(a);
    let b_val = codegen.extract_long(b);
    let int_result = int_op(codegen, a_val, b_val);
    let boxed = codegen.box_long(int_result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(boxed)]);

    // === Check both float ===
    // A value is a float if it's NOT a NaN-boxed tagged value.
    // NaN-boxed values have (val & QNAN_BASE) == QNAN_BASE where QNAN_BASE = TAG_LONG.
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
    let float_result = float_op(codegen, a_f64, b_f64);
    let float_as_i64 = codegen.bitcast_from_f64(float_result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(float_as_i64)]);

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

    let _ = offset; // used by guard functions in other paths

    Ok(())
}

/// Emit a unary arithmetic operation with integer + float fast-paths and runtime fallback.
fn emit_unary_arith_with_fallback<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    runtime_func_id: FuncId,
    int_op: impl FnOnce(&mut CodegenContext<'a, 'b>, Value) -> Value,
    float_op: impl FnOnce(&mut CodegenContext<'a, 'b>, Value) -> Value,
    _offset: usize,
) -> JitResult<()> {
    let a = codegen.pop()?;

    // Extract tag
    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_long = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);
    let a_tag = codegen.builder.ins().band(a, tag_mask);
    let a_is_long = codegen.builder.ins().icmp(IntCC::Equal, a_tag, tag_long);

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
        .brif(a_is_long, int_path, &[], check_float, &[]);

    // === Integer fast-path ===
    codegen.builder.switch_to_block(int_path);
    let a_val = codegen.extract_long(a);
    let int_result = int_op(codegen, a_val);
    let boxed = codegen.box_long(int_result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(boxed)]);

    // === Check float ===
    codegen.builder.switch_to_block(check_float);
    let qnan_base = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);
    let a_qnan = codegen.builder.ins().band(a, qnan_base);
    let a_is_float = codegen.builder.ins().icmp(IntCC::NotEqual, a_qnan, qnan_base);
    codegen
        .builder
        .ins()
        .brif(a_is_float, float_path, &[], runtime_path, &[]);

    // === Float fast-path ===
    codegen.builder.switch_to_block(float_path);
    let a_f64 = codegen.bitcast_to_f64(a);
    let float_result = float_op(codegen, a_f64);
    let float_as_i64 = codegen.bitcast_from_f64(float_result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(float_as_i64)]);

    // === Runtime fallback ===
    codegen.builder.switch_to_block(runtime_path);
    let func_ref = ctx
        .module
        .declare_func_in_func(runtime_func_id, codegen.builder.func);
    let call_inst = codegen.builder.ins().call(func_ref, &[a]);
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

/// Compile arithmetic opcodes with integer fast-path and runtime float fallback
pub fn compile_arithmetic_op<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::Add => {
            let func_id = ctx.numeric_add_func_id;
            emit_binary_arith_with_fallback(
                ctx,
                codegen,
                func_id,
                |cg, a, b| cg.builder.ins().iadd(a, b),
                |cg, a, b| cg.builder.ins().fadd(a, b),
                offset,
            )
        }

        Opcode::Sub => {
            let func_id = ctx.numeric_sub_func_id;
            emit_binary_arith_with_fallback(
                ctx,
                codegen,
                func_id,
                |cg, a, b| cg.builder.ins().isub(a, b),
                |cg, a, b| cg.builder.ins().fsub(a, b),
                offset,
            )
        }

        Opcode::Mul => {
            let func_id = ctx.numeric_mul_func_id;
            emit_binary_arith_with_fallback(
                ctx,
                codegen,
                func_id,
                |cg, a, b| cg.builder.ins().imul(a, b),
                |cg, a, b| cg.builder.ins().fmul(a, b),
                offset,
            )
        }

        Opcode::Div => {
            // Division needs zero-check in int path; float div handles 0.0 natively (→ ±Inf)
            let func_id = ctx.numeric_div_func_id;
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

            // === Integer fast-path with zero-check ===
            codegen.builder.switch_to_block(int_path);
            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            codegen.guard_nonzero(b_val, offset)?;
            let int_result = codegen.builder.ins().sdiv(a_val, b_val);
            let boxed = codegen.box_long(int_result);
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

            // === Float fast-path (IEEE 754 div handles 0.0 → ±Inf natively) ===
            codegen.builder.switch_to_block(float_path);
            let a_f64 = codegen.bitcast_to_f64(a);
            let b_f64 = codegen.bitcast_to_f64(b);
            let float_result = codegen.builder.ins().fdiv(a_f64, b_f64);
            let float_as_i64 = codegen.bitcast_from_f64(float_result);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(float_as_i64)]);

            // === Runtime fallback (mixed types) ===
            codegen.builder.switch_to_block(runtime_path);
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
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

        Opcode::Mod => {
            // Modulo needs zero-check in int path; float uses Cranelift frem (fmod)
            let func_id = ctx.numeric_mod_func_id;
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

            // === Integer fast-path with zero-check ===
            codegen.builder.switch_to_block(int_path);
            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            codegen.guard_nonzero(b_val, offset)?;
            let int_result = codegen.builder.ins().srem(a_val, b_val);
            let boxed = codegen.box_long(int_result);
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
            // Note: Cranelift doesn't have a native `frem` instruction on all targets.
            // Use the runtime for float mod to ensure correct IEEE 754 semantics.
            // Fall through to runtime for now — revisit if profiling shows this matters.
            codegen.builder.ins().jump(runtime_path, &[]);

            // === Runtime fallback ===
            codegen.builder.switch_to_block(runtime_path);
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
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

        Opcode::Neg => {
            let func_id = ctx.numeric_neg_func_id;
            emit_unary_arith_with_fallback(
                ctx,
                codegen,
                func_id,
                |cg, a| cg.builder.ins().ineg(a),
                |cg, a| cg.builder.ins().fneg(a),
                offset,
            )
        }

        Opcode::Abs => {
            let func_id = ctx.numeric_abs_func_id;
            emit_unary_arith_with_fallback(
                ctx,
                codegen,
                func_id,
                |cg, a| {
                    // abs(x) = x < 0 ? -x : x
                    let zero = cg.builder.ins().iconst(types::I64, 0);
                    let is_neg = cg
                        .builder
                        .ins()
                        .icmp(IntCC::SignedLessThan, a, zero);
                    let negated = cg.builder.ins().ineg(a);
                    cg.builder.ins().select(is_neg, negated, a)
                },
                |cg, a| cg.builder.ins().fabs(a),
                offset,
            )
        }

        Opcode::FloorDiv => {
            // FloorDiv: for integers, same as truncated division
            // For floats, use fdiv then floor
            let func_id = ctx.numeric_div_func_id;
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

            codegen.builder.switch_to_block(int_path);
            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            codegen.guard_nonzero(b_val, offset)?;
            let int_result = codegen.builder.ins().sdiv(a_val, b_val);
            let boxed = codegen.box_long(int_result);
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

            // === Float fast-path: fdiv then floor ===
            codegen.builder.switch_to_block(float_path);
            let a_f64 = codegen.bitcast_to_f64(a);
            let b_f64 = codegen.bitcast_to_f64(b);
            let div_result = codegen.builder.ins().fdiv(a_f64, b_f64);
            let floored = codegen.builder.ins().floor(div_result);
            let float_as_i64 = codegen.bitcast_from_f64(floored);
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(float_as_i64)]);

            codegen.builder.switch_to_block(runtime_path);
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let rt_result = codegen.builder.inst_results(call_inst)[0];
            codegen
                .builder
                .ins()
                .jump(merge_block, &[BlockArg::Value(rt_result)]);

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

        _ => unreachable!(
            "compile_arithmetic_op called with wrong opcode: {:?}",
            op
        ),
    }
}

/// Compile Pow opcode via runtime call
pub fn compile_pow<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let exp = codegen.pop()?;
    let base = codegen.pop()?;

    // Import the pow function into this function's context
    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pow_func_id, codegen.builder.func);

    // Call jit_runtime_pow(base, exp) - both are NaN-boxed
    let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;

    Ok(())
}
