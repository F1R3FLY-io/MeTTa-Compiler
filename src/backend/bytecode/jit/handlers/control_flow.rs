//! Control flow handlers for JIT compilation
//!
//! Handles: Return, Jump, JumpIfFalse, JumpIfTrue, JumpShort, JumpIfFalseShort,
//! JumpIfTrueShort, JumpIfNil, JumpIfError, JumpTable, Halt

use cranelift::prelude::*;

use cranelift::codegen::ir::BlockArg;

use cranelift_frontend::Switch;

use std::collections::HashMap;

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::{JitError, JitResult};
use crate::backend::bytecode::{BytecodeChunk, Opcode};

use crate::backend::bytecode::jit::types::TAG_NIL;

use crate::backend::bytecode::jit::types::TAG_ERROR;

/// Compile Return opcode

pub fn compile_return<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    // Return top of stack or 0
    let result = codegen
        .pop()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));
    codegen.builder.ins().return_(&[result]);
    codegen.mark_terminated();
    Ok(())
}

/// Compile Halt opcode

pub fn compile_halt<'a, 'b>(codegen: &mut CodegenContext<'a, 'b>) -> JitResult<()> {
    // Stack: [] -> signal - halt execution
    let signal = codegen
        .builder
        .ins()
        .iconst(types::I64, super::super::JIT_SIGNAL_HALT);
    codegen.builder.ins().return_(&[signal]);
    codegen.mark_terminated();
    Ok(())
}

/// Compile Jump opcode (unconditional)

pub fn compile_jump<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Unconditional jump with 2-byte signed offset
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
    let target = (next_ip as isize + rel_offset as isize) as usize;

    if let Some(&target_block) = offset_to_block.get(&target) {
        // For merge blocks, pass the stack top as argument
        if merge_blocks.contains_key(&target) {
            let stack_top = codegen
                .peek()
                .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));
            codegen
                .builder
                .ins()
                .jump(target_block, &[BlockArg::Value(stack_top)]);
        } else {
            codegen.builder.ins().jump(target_block, &[]);
        }
        codegen.mark_terminated();
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "Jump target {} not found in block map (offset={}, next_ip={}, rel={})",
            target, offset, next_ip, rel_offset
        )))
    }
}

/// Compile JumpShort opcode (unconditional, 1-byte offset)

pub fn compile_jump_short<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Unconditional jump with 1-byte signed offset
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
    let target = (next_ip as isize + rel_offset as isize) as usize;

    if let Some(&target_block) = offset_to_block.get(&target) {
        // For merge blocks, pass the stack top as argument
        if merge_blocks.contains_key(&target) {
            let stack_top = codegen
                .peek()
                .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));
            codegen
                .builder
                .ins()
                .jump(target_block, &[BlockArg::Value(stack_top)]);
        } else {
            codegen.builder.ins().jump(target_block, &[]);
        }
        codegen.mark_terminated();
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpShort target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfFalse opcode

pub fn compile_jump_if_false<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if top of stack is false
    let cond = codegen.pop()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
    let target = (next_ip as isize + rel_offset as isize) as usize;

    // Extract bool value (assumes already bool type)
    let cond_val = codegen.extract_bool(cond);
    let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

    // Get stack value for merge blocks
    let stack_top = codegen
        .peek()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        // brif branches to first block if cond is true, second if false
        // We want to jump to target if false, so: true -> fallthrough, false -> target
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            fallthrough_block,
            fallthrough_args,
            target_block,
            target_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        // Fallthrough is just the next instruction, no block needed
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, cont_block, &[], target_block, target_args);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfFalse target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfFalseShort opcode

pub fn compile_jump_if_false_short<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if false with 1-byte offset
    let cond = codegen.pop()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
    let target = (next_ip as isize + rel_offset as isize) as usize;

    let cond_val = codegen.extract_bool(cond);
    let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

    // Get stack value for merge blocks
    let stack_top = codegen
        .peek()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            fallthrough_block,
            fallthrough_args,
            target_block,
            target_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, cont_block, &[], target_block, target_args);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfFalseShort target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfTrue opcode

pub fn compile_jump_if_true<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if top of stack is true
    let cond = codegen.pop()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
    let target = (next_ip as isize + rel_offset as isize) as usize;

    let cond_val = codegen.extract_bool(cond);
    let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

    // Get stack value for merge blocks
    let stack_top = codegen
        .peek()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        // brif branches to first block if cond is true
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            target_block,
            target_args,
            fallthrough_block,
            fallthrough_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, target_block, target_args, cont_block, &[]);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfTrue target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfTrueShort opcode

pub fn compile_jump_if_true_short<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if true with 1-byte offset
    let cond = codegen.pop()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
    let target = (next_ip as isize + rel_offset as isize) as usize;

    let cond_val = codegen.extract_bool(cond);
    let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

    // Get stack value for merge blocks
    let stack_top = codegen
        .peek()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            target_block,
            target_args,
            fallthrough_block,
            fallthrough_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, target_block, target_args, cont_block, &[]);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfTrueShort target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfNil opcode

pub fn compile_jump_if_nil<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if top of stack is nil (pops the value)
    let val = codegen.pop()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
    let target = (next_ip as isize + rel_offset as isize) as usize;

    // Check if value is nil: tag == TAG_NIL
    // icmp returns i8 (0 or 1), suitable for brif
    let tag = codegen.extract_tag(val);
    let nil_tag = codegen.builder.ins().iconst(types::I64, TAG_NIL as i64);
    let cond_i8 = codegen.builder.ins().icmp(IntCC::Equal, tag, nil_tag);

    // Get stack value for merge blocks (use previous stack top since we popped)
    let stack_top = codegen
        .peek()
        .unwrap_or_else(|_| codegen.builder.ins().iconst(types::I64, 0));

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        // brif branches to first block if cond is true (is_nil)
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            target_block,
            target_args,
            fallthrough_block,
            fallthrough_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, target_block, target_args, cont_block, &[]);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfNil target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpIfError opcode

pub fn compile_jump_if_error<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
    merge_blocks: &HashMap<usize, bool>,
) -> JitResult<()> {
    // Conditional jump if top of stack is error (peeks - does NOT pop)
    let val = codegen.peek()?;
    let instr_size = 1 + op.immediate_size();
    let next_ip = offset + instr_size;
    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
    let target = (next_ip as isize + rel_offset as isize) as usize;

    // Check if value is error: tag == TAG_ERROR
    // icmp returns i8 (0 or 1), suitable for brif
    let tag = codegen.extract_tag(val);
    let error_tag = codegen.builder.ins().iconst(types::I64, TAG_ERROR as i64);
    let cond_i8 = codegen.builder.ins().icmp(IntCC::Equal, tag, error_tag);

    // Get stack value for merge blocks (use val since we didn't pop)
    let stack_top = val;

    // Prepare arguments for each branch based on whether target is a merge block
    let target_is_merge = merge_blocks.contains_key(&target);
    let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

    if let (Some(&target_block), Some(&fallthrough_block)) =
        (offset_to_block.get(&target), offset_to_block.get(&next_ip))
    {
        // brif branches to first block if cond is true (is_error)
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen.builder.ins().brif(
            cond_i8,
            target_block,
            target_args,
            fallthrough_block,
            fallthrough_args,
        );
        codegen.mark_terminated();
        Ok(())
    } else if let Some(&target_block) = offset_to_block.get(&target) {
        let cont_block = codegen.builder.create_block();
        let target_args: &[BlockArg] = if target_is_merge {
            &[BlockArg::Value(stack_top)]
        } else {
            &[]
        };
        codegen
            .builder
            .ins()
            .brif(cond_i8, target_block, target_args, cont_block, &[]);
        codegen.builder.switch_to_block(cont_block);
        codegen.builder.seal_block(cont_block);
        Ok(())
    } else {
        Err(JitError::CompilationError(format!(
            "JumpIfError target {} not found in block map",
            target
        )))
    }
}

/// Compile JumpTable opcode (multi-way branch / switch)

pub fn compile_jump_table<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    offset: usize,
    offset_to_block: &HashMap<usize, Block>,
) -> JitResult<()> {
    // Stack: [selector_hash] -> [] - jump to offset based on hash match
    // JumpTable uses 2 bytes for table index
    let table_index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

    // Get the jump table from chunk
    let jump_table = match chunk.get_jump_table(table_index) {
        Some(jt) => jt.clone(),
        None => {
            // No table found - bail to VM
            let _selector = codegen.pop()?;
            let signal = codegen
                .builder
                .ins()
                .iconst(types::I64, super::super::JIT_SIGNAL_BAILOUT);
            codegen.builder.ins().return_(&[signal]);
            codegen.mark_terminated();
            return Ok(());
        }
    };

    // Pop the selector value (hash to match against entries)
    let selector = codegen.pop()?;

    // Get default block
    let default_block = match offset_to_block.get(&jump_table.default_offset) {
        Some(&block) => block,
        None => {
            // Default not found - bail
            let signal = codegen
                .builder
                .ins()
                .iconst(types::I64, super::super::JIT_SIGNAL_BAILOUT);
            codegen.builder.ins().return_(&[signal]);
            codegen.mark_terminated();
            return Ok(());
        }
    };

    if jump_table.entries.is_empty() {
        // No entries - just jump to default
        codegen.builder.ins().jump(default_block, &[]);
        codegen.mark_terminated();
    } else {
        // Use Cranelift's Switch which handles both dense and sparse cases efficiently
        // It automatically chooses between br_table, binary search, or linear scan
        let mut switch = Switch::new();

        for (hash, target_offset) in &jump_table.entries {
            let target_block = match offset_to_block.get(target_offset) {
                Some(&block) => block,
                None => default_block,
            };
            // Switch uses u128 keys, convert hash
            switch.set_entry(*hash as u128, target_block);
        }

        // Emit the switch - this generates optimal code (br_table for dense,
        // binary search for sparse, etc.)
        switch.emit(codegen.builder, selector, default_block);
        codegen.mark_terminated();
    }

    Ok(())
}
