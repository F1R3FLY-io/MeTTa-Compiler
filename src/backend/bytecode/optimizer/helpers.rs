//! Helper functions for bytecode optimization.

use crate::backend::bytecode::opcodes::Opcode;

/// Get the size of an instruction at the given offset
///
/// Most instructions have a fixed size (opcode + immediate), but some like Fork
/// have variable-length encoding that depends on the immediate value.
pub fn instruction_size(code: &[u8], offset: usize) -> usize {
    if offset >= code.len() {
        return 1;
    }

    match Opcode::from_byte(code[offset]) {
        Some(Opcode::Fork) => {
            // Fork format: opcode:1 + count:2 + (count * const_index:2)
            // We need to read the count to determine total size
            if offset + 2 < code.len() {
                let count = u16::from_be_bytes([code[offset + 1], code[offset + 2]]) as usize;
                1 + 2 + (count * 2)
            } else {
                // Malformed bytecode, return minimal size
                3
            }
        }
        Some(opcode) => 1 + opcode.immediate_size(),
        None => 1, // Unknown opcode, assume 1 byte
    }
}
