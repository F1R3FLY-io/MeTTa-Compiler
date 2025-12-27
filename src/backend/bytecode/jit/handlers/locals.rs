//! Local variable handlers for JIT compilation
//!
//! Handles: LoadLocal, StoreLocal, LoadLocalWide, StoreLocalWide

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::{BytecodeChunk, Opcode};

/// Compile local variable opcodes

pub fn compile_local_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::LoadLocal => {
            let index = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
            codegen.load_local(index)?;
        }

        Opcode::StoreLocal => {
            let index = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
            codegen.store_local(index)?;
        }

        Opcode::LoadLocalWide => {
            let index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
            codegen.load_local(index)?;
        }

        Opcode::StoreLocalWide => {
            let index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
            codegen.store_local(index)?;
        }

        _ => unreachable!("compile_local_op called with wrong opcode: {:?}", op),
    }
    Ok(())
}
