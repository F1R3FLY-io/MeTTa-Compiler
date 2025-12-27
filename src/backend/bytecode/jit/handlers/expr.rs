//! Expression manipulation handlers for JIT compilation
//!
//! Handles: IndexAtom, MinAtom, MaxAtom


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::Opcode;

/// Context for expression manipulation handlers

pub struct ExprHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub index_atom_func_id: FuncId,
    pub min_atom_func_id: FuncId,
    pub max_atom_func_id: FuncId,
}

/// Compile expression manipulation opcodes via runtime calls

pub fn compile_expr_op<'a, 'b>(
    ctx: &mut ExprHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::IndexAtom => {
            // index-atom: [expr, index] -> [element]
            let index = codegen.pop()?;
            let expr = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.index_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, index, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::MinAtom => {
            // min-atom: [expr] -> [min value]
            let expr = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.min_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::MaxAtom => {
            // max-atom: [expr] -> [max value]
            let expr = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.max_atom_func_id, codegen.builder.func);

            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_expr_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
