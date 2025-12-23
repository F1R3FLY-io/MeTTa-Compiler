//! Helper functions for bytecode optimization.

use crate::backend::bytecode::opcodes::Opcode;

/// Get the size of an instruction at the given offset
pub fn instruction_size(code: &[u8], offset: usize) -> usize {
    if offset >= code.len() {
        return 1;
    }

    match Opcode::from_byte(code[offset]) {
        Some(opcode) => 1 + opcode.immediate_size(),
        None => 1, // Unknown opcode, assume 1 byte
    }
}
