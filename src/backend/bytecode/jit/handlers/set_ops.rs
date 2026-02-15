//! Set operations and alpha-equivalence handlers for JIT compilation
//!
//! Handles: EvalIfEqual, UniqueAtom, UnionAtom, IntersectionAtom, SubtractionAtom
//! All operations use runtime bailout (call into Rust functions).

use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::Opcode;

/// Context for set operations handlers
pub struct SetOpsHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub eval_if_equal_func_id: FuncId,
    pub unique_atom_func_id: FuncId,
    pub union_atom_func_id: FuncId,
    pub intersection_atom_func_id: FuncId,
    pub subtraction_atom_func_id: FuncId,
}

/// Compile set operations and alpha-equivalence opcodes via runtime calls
pub fn compile_set_op<'a, 'b>(
    ctx: &mut SetOpsHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::EvalIfEqual => {
            // if-equal: [pred1, pred2, then, else] -> [result]
            let else_val = codegen.pop()?;
            let then_val = codegen.pop()?;
            let pred2 = codegen.pop()?;
            let pred1 = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.eval_if_equal_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen.builder.ins().call(
                func_ref,
                &[ctx_ptr, pred1, pred2, then_val, else_val, ip_val],
            );
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::UniqueAtom => {
            // unique-atom: [list] -> [deduped_list]
            let list = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.unique_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen
                .builder
                .ins()
                .call(func_ref, &[ctx_ptr, list, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::UnionAtom => {
            // union-atom: [left, right] -> [combined]
            let right = codegen.pop()?;
            let left = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.union_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen
                .builder
                .ins()
                .call(func_ref, &[ctx_ptr, left, right, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::IntersectionAtom => {
            // intersection-atom: [left, right] -> [intersection]
            let right = codegen.pop()?;
            let left = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.intersection_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen
                .builder
                .ins()
                .call(func_ref, &[ctx_ptr, left, right, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::SubtractionAtom => {
            // subtraction-atom: [left, right] -> [difference]
            let right = codegen.pop()?;
            let left = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.subtraction_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen
                .builder
                .ins()
                .call(func_ref, &[ctx_ptr, left, right, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_set_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
