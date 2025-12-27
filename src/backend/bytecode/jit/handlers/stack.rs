//! Stack operation handlers for JIT compilation
//!
//! Handles: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN

use crate::backend::bytecode::jit::codegen::CodegenContext;
use crate::backend::bytecode::jit::types::JitResult;
use crate::backend::bytecode::{BytecodeChunk, Opcode};

/// Compile a stack operation opcode to Cranelift IR
pub fn compile_stack_op<'a, 'b>(
    codegen: &mut CodegenContext<'a, 'b>,
    chunk: &BytecodeChunk,
    op: Opcode,
    offset: usize,
) -> JitResult<()> {
    match op {
        Opcode::Nop => {
            // No operation
        }

        Opcode::Pop => {
            // Handle scope cleanup: if stack is empty, the value being popped
            // was a local stored via StoreLocal (in JIT's separate locals storage)
            if codegen.stack_depth() > 0 {
                codegen.pop()?;
            }
            // If stack is empty, this is a no-op (the "local" isn't on our stack)
        }

        Opcode::Dup => {
            let val = codegen.peek()?;
            codegen.push(val)?;
        }

        Opcode::Swap => {
            // Handle scope cleanup pattern: when StoreLocal stores values
            // to separate local slots, subsequent Swap has nothing to swap with.
            if codegen.stack_depth() >= 2 {
                let a = codegen.pop()?;
                let b = codegen.pop()?;
                codegen.push(a)?;
                codegen.push(b)?;
            }
            // If stack_depth < 2, this is a no-op (scope cleanup for JIT-stored locals)
        }

        Opcode::Rot3 => {
            // [a, b, c] -> [c, a, b] (VM semantics)
            let c = codegen.pop()?;
            let b = codegen.pop()?;
            let a = codegen.pop()?;
            codegen.push(c)?;
            codegen.push(a)?;
            codegen.push(b)?;
        }

        Opcode::Over => {
            // (a b -- a b a)
            let b = codegen.pop()?;
            let a = codegen.peek()?;
            codegen.push(b)?;
            codegen.push(a)?;
        }

        Opcode::DupN => {
            // Read operand from bytecode
            let n = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
            let mut vals = Vec::with_capacity(n);
            for _ in 0..n {
                vals.push(codegen.pop()?);
            }
            vals.reverse();
            // Push original values
            for &v in &vals {
                codegen.push(v)?;
            }
            // Push duplicates
            for &v in &vals {
                codegen.push(v)?;
            }
        }

        Opcode::PopN => {
            let n = chunk.read_byte(offset + 1).unwrap_or(0);
            for _ in 0..n {
                codegen.pop()?;
            }
        }

        _ => unreachable!("compile_stack_op called with non-stack opcode: {:?}", op),
    }
    Ok(())
}
