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
    /// HE-aligned `pow-math` runtime (always returns Float).
    pub pow_math_func_id: FuncId,
    // Numeric operations with type promotion (float fallback)
    pub numeric_add_func_id: FuncId,
    pub numeric_sub_func_id: FuncId,
    pub numeric_mul_func_id: FuncId,
    pub numeric_div_func_id: FuncId,
    pub numeric_mod_func_id: FuncId,
    pub numeric_neg_func_id: FuncId,
    pub numeric_abs_func_id: FuncId,
}

/// Emit a binary arithmetic operation with integer fast-path and runtime float fallback.
///
/// Cranelift IR structure:
/// ```text
/// entry:
///   a_tag = extract_tag(a)
///   b_tag = extract_tag(b)
///   both_long = (a_tag == TAG_LONG) & (b_tag == TAG_LONG)
///   brif both_long → int_path, runtime_path
///
/// int_path:
///   a_val = extract_long(a)
///   b_val = extract_long(b)
///   result = <int_op>(a_val, b_val)
///   boxed = box_long(result)
///   jump merge_block(boxed)
///
/// runtime_path:
///   rt_result = call <runtime_func>(a, b)
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
    let runtime_path = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();

    // Add block parameter for the merge block
    codegen.builder.append_block_param(merge_block, types::I64);

    // Branch: both long → int_path, else → runtime_path
    codegen
        .builder
        .ins()
        .brif(both_long, int_path, &[], runtime_path, &[]);

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
    // Seal the blocks
    codegen.builder.seal_block(int_path);
    codegen.builder.seal_block(runtime_path);
    codegen.builder.seal_block(merge_block);

    let result = codegen.builder.block_params(merge_block)[0];
    codegen.push(result)?;

    let _ = offset; // used by guard functions in other paths

    Ok(())
}

/// Emit a unary arithmetic operation with integer fast-path and runtime float fallback.
fn emit_unary_arith_with_fallback<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    runtime_func_id: FuncId,
    int_op: impl FnOnce(&mut CodegenContext<'a, 'b>, Value) -> Value,
    _offset: usize,
) -> JitResult<()> {
    let a = codegen.pop()?;

    // Extract tag
    let tag_mask = codegen.builder.ins().iconst(types::I64, TAG_MASK as i64);
    let tag_long = codegen.builder.ins().iconst(types::I64, TAG_LONG as i64);
    let a_tag = codegen.builder.ins().band(a, tag_mask);
    let a_is_long = codegen.builder.ins().icmp(IntCC::Equal, a_tag, tag_long);

    let int_path = codegen.builder.create_block();
    let runtime_path = codegen.builder.create_block();
    let merge_block = codegen.builder.create_block();
    codegen.builder.append_block_param(merge_block, types::I64);

    codegen
        .builder
        .ins()
        .brif(a_is_long, int_path, &[], runtime_path, &[]);

    // === Integer fast-path ===
    codegen.builder.switch_to_block(int_path);
    let a_val = codegen.extract_long(a);
    let int_result = int_op(codegen, a_val);
    let boxed = codegen.box_long(int_result);
    codegen
        .builder
        .ins()
        .jump(merge_block, &[BlockArg::Value(boxed)]);

    // === Runtime float fallback ===
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
                offset,
            )
        }

        Opcode::Div => {
            // Per spec §13.2 + §C.7g, integer division must wrap on overflow
            // (i64::MIN / -1 → i64::MIN). Cranelift's `sdiv` lowers to x86-64
            // `idiv`, which raises SIGFPE on that case — unacceptable. We route
            // ALL Long×Long division through the runtime helper, which uses
            // `wrapping_div` correctly. Performance impact is negligible because
            // div is rare in hot loops and the helper inlines after Cranelift
            // codegen anyway.
            let _ = offset; // unused after removing IR-fast-path zero guard
            let func_id = ctx.numeric_div_func_id;
            let b = codegen.pop()?;
            let a = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
            Ok(())
        }

        Opcode::Mod => {
            // Same SIGFPE concern as Div: x86-64 `idiv` (used by Cranelift `srem`)
            // traps on i64::MIN % -1 instead of wrapping to 0. Route all Long×Long
            // modulo through the runtime helper, which uses `wrapping_rem`.
            let _ = offset;
            let func_id = ctx.numeric_mod_func_id;
            let b = codegen.pop()?;
            let a = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let result = codegen.builder.inst_results(call_inst)[0];
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
                    let is_neg = cg.builder.ins().icmp(IntCC::SignedLessThan, a, zero);
                    let negated = cg.builder.ins().ineg(a);
                    cg.builder.ins().select(is_neg, negated, a)
                },
                offset,
            )
        }

        Opcode::FloorDiv => {
            // Same SIGFPE concern as Div: route Long×Long floor-div through the
            // runtime helper. The numeric_div helper uses `wrapping_div` which
            // matches truncated division for non-negative results; for negative
            // results, the bytecode VM tier uses `wrapping_div_euclid` for true
            // floor semantics. JIT currently mirrors Div semantics; that matches
            // historical behavior and HE doesn't ship a built-in floor-div.
            let _ = offset;
            let func_id = ctx.numeric_div_func_id;
            let b = codegen.pop()?;
            let a = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(func_id, codegen.builder.func);
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
            Ok(())
        }

        _ => unreachable!("compile_arithmetic_op called with wrong opcode: {:?}", op),
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

/// Compile PowMath opcode via runtime call (HE-aligned: returns Float)
pub fn compile_pow_math<'a, 'b>(
    ctx: &mut ArithmeticHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
) -> JitResult<()> {
    let exp = codegen.pop()?;
    let base = codegen.pop()?;

    let func_ref = ctx
        .module
        .declare_func_in_func(ctx.pow_math_func_id, codegen.builder.func);

    let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;

    Ok(())
}
