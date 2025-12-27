//! Type predicate handlers for JIT compilation
//!
//! Handles: IsVariable, IsSExpr, IsSymbol

use cranelift::prelude::*;

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{JitResult, TAG_ATOM, TAG_HEAP, TAG_VAR};
use crate::backend::bytecode::Opcode;

/// Compile type predicate opcodes

pub fn compile_type_predicate_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
) -> JitResult<()> {
    match op {
        Opcode::IsVariable => {
            // Check if value is a variable (TAG_VAR)
            let val = codegen.pop()?;
            let tag = codegen.extract_tag(val);
            let var_tag = codegen.builder.ins().iconst(types::I64, TAG_VAR as i64);
            let is_var = codegen.builder.ins().icmp(IntCC::Equal, tag, var_tag);
            // icmp returns i8, extend to i64 for boxing
            let is_var_i64 = codegen.builder.ins().uextend(types::I64, is_var);
            let result = codegen.box_bool(is_var_i64);
            codegen.push(result)?;
        }

        Opcode::IsSExpr => {
            // Check if value is an S-expression (TAG_HEAP)
            let val = codegen.pop()?;
            let tag = codegen.extract_tag(val);
            let heap_tag = codegen.builder.ins().iconst(types::I64, TAG_HEAP as i64);
            let is_sexpr = codegen.builder.ins().icmp(IntCC::Equal, tag, heap_tag);
            // icmp returns i8, extend to i64 for boxing
            let is_sexpr_i64 = codegen.builder.ins().uextend(types::I64, is_sexpr);
            let result = codegen.box_bool(is_sexpr_i64);
            codegen.push(result)?;
        }

        Opcode::IsSymbol => {
            // Check if value is a symbol/atom (TAG_ATOM)
            let val = codegen.pop()?;
            let tag = codegen.extract_tag(val);
            let atom_tag = codegen.builder.ins().iconst(types::I64, TAG_ATOM as i64);
            let is_sym = codegen.builder.ins().icmp(IntCC::Equal, tag, atom_tag);
            // icmp returns i8, extend to i64 for boxing
            let is_sym_i64 = codegen.builder.ins().uextend(types::I64, is_sym);
            let result = codegen.box_bool(is_sym_i64);
            codegen.push(result)?;
        }

        _ => unreachable!(
            "compile_type_predicate_op called with wrong opcode: {:?}",
            op
        ),
    }
    Ok(())
}
