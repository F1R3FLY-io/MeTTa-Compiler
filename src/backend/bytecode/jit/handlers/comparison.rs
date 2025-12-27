//! Comparison and boolean operation handlers for JIT compilation
//!
//! Boolean ops: And, Or, Not, Xor
//! Comparison ops: Lt, Le, Gt, Ge, Eq, Ne, StructEq

use cranelift::prelude::*;

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::Opcode;

/// Compile boolean operation opcodes
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

/// Compile comparison operation opcodes
pub fn compile_comparison_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::Lt => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let cmp = codegen
                .builder
                .ins()
                .icmp(IntCC::SignedLessThan, a_val, b_val);

            // Convert i8 comparison result to i64
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Le => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let cmp = codegen
                .builder
                .ins()
                .icmp(IntCC::SignedLessThanOrEqual, a_val, b_val);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Gt => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let cmp = codegen
                .builder
                .ins()
                .icmp(IntCC::SignedGreaterThan, a_val, b_val);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Ge => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            codegen.guard_long(a, offset)?;
            codegen.guard_long(b, offset)?;

            let a_val = codegen.extract_long(a);
            let b_val = codegen.extract_long(b);
            let cmp = codegen
                .builder
                .ins()
                .icmp(IntCC::SignedGreaterThanOrEqual, a_val, b_val);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Eq => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            // For equality, we can compare the raw bits
            // (same tag + same payload = equal)
            let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::Ne => {
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let cmp = codegen.builder.ins().icmp(IntCC::NotEqual, a, b);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        Opcode::StructEq => {
            // Structural equality: compare NaN-boxed values directly
            // For primitive types (Long, Bool, Nil, Unit), bit comparison is correct
            // For heap types, this compares references (deep comparison would need runtime)
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
            let result = codegen.builder.ins().uextend(types::I64, cmp);
            let boxed = codegen.box_bool(result);
            codegen.push(boxed)?;
        }

        _ => unreachable!("compile_comparison_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
