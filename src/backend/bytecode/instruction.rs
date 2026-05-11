//! Shared bytecode instruction decoding helpers.

use super::opcodes::Opcode;

/// Return the encoded size of the instruction at `offset`.
///
/// `Opcode::immediate_size()` is only valid for fixed-width operands. Variable
/// length opcodes such as `Fork` and `ForkInline` include a u16 count followed
/// by a u16 table.
pub(crate) fn instruction_size(code: &[u8], offset: usize) -> usize {
    if offset >= code.len() {
        return 1;
    }

    match Opcode::from_byte(code[offset]) {
        Some(Opcode::Fork | Opcode::ForkInline) => {
            if offset + 2 >= code.len() {
                return code.len() - offset;
            }
            let count = u16::from_be_bytes([code[offset + 1], code[offset + 2]]) as usize;
            let encoded = 3 + count * 2;
            encoded.min(code.len() - offset)
        }
        Some(opcode) => 1 + opcode.immediate_size(),
        None => 1,
    }
}

/// Read the absolute branch targets from a `ForkInline` instruction.
pub(crate) fn fork_inline_targets(code: &[u8], offset: usize) -> Vec<usize> {
    if offset + 2 >= code.len() || Opcode::from_byte(code[offset]) != Some(Opcode::ForkInline) {
        return Vec::new();
    }

    let count = u16::from_be_bytes([code[offset + 1], code[offset + 2]]) as usize;
    let mut targets = Vec::with_capacity(count);
    let mut pos = offset + 3;
    for _ in 0..count {
        if pos + 1 >= code.len() {
            break;
        }
        targets.push(u16::from_be_bytes([code[pos], code[pos + 1]]) as usize);
        pos += 2;
    }
    targets
}

/// Patch `ForkInline` absolute branch targets in place.
pub(crate) fn patch_fork_inline_targets(
    code: &mut [u8],
    offset: usize,
    original_targets: &[usize],
    mut map_target: impl FnMut(usize) -> Option<usize>,
) {
    if offset + 2 >= code.len() || Opcode::from_byte(code[offset]) != Some(Opcode::ForkInline) {
        return;
    }

    let count = u16::from_be_bytes([code[offset + 1], code[offset + 2]]) as usize;
    let mut pos = offset + 3;
    for old_target in original_targets.iter().copied().take(count) {
        if pos + 1 >= code.len() {
            break;
        }
        if let Some(new_target) = map_target(old_target).and_then(|t| u16::try_from(t).ok()) {
            let bytes = new_target.to_be_bytes();
            code[pos] = bytes[0];
            code[pos + 1] = bytes[1];
        }
        pos += 2;
    }
}
