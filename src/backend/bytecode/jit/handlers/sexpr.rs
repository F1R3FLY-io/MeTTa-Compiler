//! S-expression operation handlers for JIT compilation
//!
//! Handles: GetHead, GetTail, GetArity, GetElement, MakeSExpr, MakeSExprLarge, ConsAtom, MakeList, MakeQuote


use cranelift::prelude::*;

use cranelift_jit::JITModule;

use cranelift_module::{FuncId, Module};

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::{BytecodeChunk, Opcode};

/// Context for S-expression handlers that need runtime function access

pub struct SExprHandlerContext<'m> {
    pub module: &'m mut JITModule,
    pub get_head_func_id: FuncId,
    pub get_tail_func_id: FuncId,
    pub get_arity_func_id: FuncId,
    pub get_element_func_id: FuncId,
    pub make_sexpr_func_id: FuncId,
    pub cons_atom_func_id: FuncId,
    pub make_list_func_id: FuncId,
    pub make_quote_func_id: FuncId,
}

/// Compile S-expression access opcodes (GetHead, GetTail, GetArity, GetElement)

pub fn compile_sexpr_access_op<'a, 'b>(
    ctx: &mut SExprHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::GetHead => {
            // Get first element of S-expression via runtime call
            let val = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.get_head_func_id, codegen.builder.func);

            // Call jit_runtime_get_head(ctx, val, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::GetTail => {
            // Get tail (all but first) of S-expression via runtime call
            let val = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.get_tail_func_id, codegen.builder.func);

            // Call jit_runtime_get_tail(ctx, val, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::GetArity => {
            // Get arity (element count) of S-expression via runtime call
            let val = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.get_arity_func_id, codegen.builder.func);

            // Call jit_runtime_get_arity(ctx, val, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::GetElement => {
            // Get element by index from S-expression via runtime call
            let index = chunk.read_byte(offset + 1).unwrap_or(0) as i64;
            let val = codegen.pop()?;
            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.get_element_func_id, codegen.builder.func);

            // Call jit_runtime_get_element(ctx, val, index, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let index_val = codegen.builder.ins().iconst(types::I64, index);
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, index_val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_sexpr_access_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}

/// Compile S-expression creation opcodes (MakeSExpr, MakeSExprLarge, ConsAtom, MakeList, MakeQuote)

pub fn compile_sexpr_create_op<'a, 'b>(
    ctx: &mut SExprHandlerContext<'_>,
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::MakeSExpr => {
            // Create S-expression from N stack values
            // Stack: [v1, v2, ..., vN] -> [sexpr]
            let arity = chunk.read_byte(offset + 1).unwrap_or(0) as usize;

            // Pop all values in reverse order (they'll be stored bottom-up)
            let mut values = Vec::with_capacity(arity);
            for _ in 0..arity {
                values.push(codegen.pop()?);
            }
            values.reverse(); // Restore original order

            // Create a stack slot to hold the array of values
            let slot_size = (arity * 8) as u32; // 8 bytes per u64
            let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size,
                0,
            ));

            // Store values to the stack slot
            for (i, val) in values.iter().enumerate() {
                let slot_offset = (i * 8) as i32;
                codegen.builder.ins().stack_store(*val, slot, slot_offset);
            }

            // Get pointer to the stack slot
            let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.make_sexpr_func_id, codegen.builder.func);

            // Call jit_runtime_make_sexpr(ctx, values_ptr, count, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::MakeSExprLarge => {
            // Same as MakeSExpr but with u16 arity
            // Stack: [v1, v2, ..., vN] -> [sexpr]
            let arity = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

            // Pop all values in reverse order
            let mut values = Vec::with_capacity(arity);
            for _ in 0..arity {
                values.push(codegen.pop()?);
            }
            values.reverse();

            // Create a stack slot to hold the array of values
            let slot_size = (arity * 8) as u32;
            let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size,
                0,
            ));

            // Store values to the stack slot
            for (i, val) in values.iter().enumerate() {
                let slot_offset = (i * 8) as i32;
                codegen.builder.ins().stack_store(*val, slot, slot_offset);
            }

            // Get pointer to the stack slot
            let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.make_sexpr_func_id, codegen.builder.func);

            // Call jit_runtime_make_sexpr(ctx, values_ptr, count, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::ConsAtom => {
            // Prepend head to tail S-expression
            // Stack: [head, tail] -> [sexpr]
            let tail = codegen.pop()?;
            let head = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.cons_atom_func_id, codegen.builder.func);

            // Call jit_runtime_cons_atom(ctx, head, tail, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, head, tail, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::MakeList => {
            // Create proper list from N stack values
            // Stack: [v1, v2, ..., vN] -> [(Cons v1 (Cons v2 ... Nil))]
            let arity = chunk.read_byte(offset + 1).unwrap_or(0) as usize;

            // Pop all values in reverse order
            let mut values = Vec::with_capacity(arity);
            for _ in 0..arity {
                values.push(codegen.pop()?);
            }
            values.reverse(); // Restore original order

            // Create a stack slot to hold the array of values
            let slot_size = (arity * 8).max(8) as u32; // At least 8 bytes
            let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                slot_size,
                0,
            ));

            // Store values to the stack slot
            for (i, val) in values.iter().enumerate() {
                let slot_offset = (i * 8) as i32;
                codegen.builder.ins().stack_store(*val, slot, slot_offset);
            }

            // Get pointer to the stack slot
            let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.make_list_func_id, codegen.builder.func);

            // Call jit_runtime_make_list(ctx, values_ptr, count, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        Opcode::MakeQuote => {
            // Wrap value in quote expression
            // Stack: [val] -> [(quote val)]
            let val = codegen.pop()?;

            let func_ref = ctx
                .module
                .declare_func_in_func(ctx.make_quote_func_id, codegen.builder.func);

            // Call jit_runtime_make_quote(ctx, val, ip)
            let ctx_ptr = codegen.ctx_ptr();
            let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
            let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
            let result = codegen.builder.inst_results(call_inst)[0];
            codegen.push(result)?;
        }

        _ => unreachable!("compile_sexpr_create_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
