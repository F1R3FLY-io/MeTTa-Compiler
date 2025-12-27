//! Peephole optimizer for bytecode.
//!
//! Implements pattern-based optimizations that identify and eliminate
//! redundant instruction sequences.
//!
//! # Optimization Patterns
//!
//! | Pattern | Replacement | Rationale |
//! |---------|-------------|-----------|
//! | `PushLongSmall 0; Add` | (remove) | Adding 0 is identity |
//! | `PushLongSmall 0; Sub` | (remove) | Subtracting 0 is identity |
//! | `PushLongSmall 1; Mul` | (remove) | Multiplying by 1 is identity |
//! | `PushLongSmall 1; Div` | (remove) | Dividing by 1 is identity |
//! | `Push*; Pop` | (remove) | Dead push immediately popped |
//! | `Swap; Swap` | (remove) | Double swap is no-op |
//! | `Dup; Pop` | (remove) | Duplicate then pop is no-op |
//! | `Not; Not` | (remove) | Double negation is identity |
//! | `PushTrue; Not` | `PushFalse` | Constant fold |
//! | `PushFalse; Not` | `PushTrue` | Constant fold |
//! | `Nop` | (remove) | No-ops are unnecessary |

use crate::backend::bytecode::opcodes::Opcode;

use super::helpers::instruction_size;
use super::types::{OptimizationStats, PeepholeAction};

/// Peephole optimizer for bytecode
pub struct PeepholeOptimizer {
    /// Statistics about optimizations performed
    stats: OptimizationStats,
}

impl Default for PeepholeOptimizer {
    fn default() -> Self {
        Self::new()
    }
}

impl PeepholeOptimizer {
    /// Create a new peephole optimizer
    pub fn new() -> Self {
        Self {
            stats: OptimizationStats::new(),
        }
    }

    /// Get optimization statistics
    pub fn stats(&self) -> &OptimizationStats {
        &self.stats
    }

    /// Optimize bytecode in place
    ///
    /// Returns the optimized bytecode and updates statistics.
    pub fn optimize(&mut self, code: Vec<u8>) -> Vec<u8> {
        // Multiple passes until no more optimizations found
        let mut result = code;
        loop {
            let (optimized, changed) = self.optimize_pass(&result);
            if !changed {
                break;
            }
            result = optimized;
        }
        // Jump threading pass (runs once after peephole stabilizes)
        result = self.thread_jumps(result);
        result
    }

    /// Thread jumps: redirect jumps that target other jumps to final destination
    ///
    /// Detects patterns like:
    ///   Jump L1
    ///   ...
    /// L1: Jump L2
    ///
    /// And rewrites first jump to target L2 directly.
    fn thread_jumps(&mut self, mut code: Vec<u8>) -> Vec<u8> {
        let mut changed = true;
        // Limit iterations to prevent infinite loops
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 10;

        while changed && iterations < MAX_ITERATIONS {
            changed = false;
            iterations += 1;

            let mut offset = 0;
            while offset < code.len() {
                let Some(opcode) = Opcode::from_byte(code[offset]) else {
                    offset += 1;
                    continue;
                };

                match opcode {
                    // 2-byte signed offset jumps
                    Opcode::Jump => {
                        if offset + 2 < code.len() {
                            let jump_offset =
                                i16::from_be_bytes([code[offset + 1], code[offset + 2]]);
                            let jump_from = offset + 3;
                            let target = (jump_from as isize + jump_offset as isize) as usize;

                            // Check if target is also an unconditional jump
                            if target < code.len() && code[target] == Opcode::Jump.to_byte() {
                                if target + 2 < code.len() {
                                    let target_offset =
                                        i16::from_be_bytes([code[target + 1], code[target + 2]]);
                                    let target_from = target + 3;
                                    let final_target =
                                        (target_from as isize + target_offset as isize) as usize;

                                    // Avoid self-loops
                                    if final_target != offset {
                                        // Calculate new offset from original jump
                                        let new_offset =
                                            (final_target as isize - jump_from as isize) as i16;
                                        let new_bytes = new_offset.to_be_bytes();

                                        // Only update if offset changed
                                        if code[offset + 1] != new_bytes[0]
                                            || code[offset + 2] != new_bytes[1]
                                        {
                                            code[offset + 1] = new_bytes[0];
                                            code[offset + 2] = new_bytes[1];
                                            self.stats.jump_threaded += 1;
                                            changed = true;
                                        }
                                    }
                                }
                            }
                        }
                        offset += 3;
                    }

                    // Short jumps
                    Opcode::JumpShort => {
                        if offset + 1 < code.len() {
                            let jump_offset = code[offset + 1] as i8;
                            let jump_from = offset + 2;
                            let target = (jump_from as isize + jump_offset as isize) as usize;

                            // Check if target is an unconditional jump (any variant)
                            if target < code.len() {
                                if code[target] == Opcode::Jump.to_byte() && target + 2 < code.len()
                                {
                                    let target_offset =
                                        i16::from_be_bytes([code[target + 1], code[target + 2]]);
                                    let target_from = target + 3;
                                    let final_target =
                                        (target_from as isize + target_offset as isize) as usize;

                                    // Avoid self-loops
                                    if final_target != offset {
                                        let new_offset =
                                            (final_target as isize - jump_from as isize) as isize;

                                        // Check if fits in i8
                                        if new_offset >= i8::MIN as isize
                                            && new_offset <= i8::MAX as isize
                                        {
                                            let new_byte = new_offset as i8 as u8;
                                            if code[offset + 1] != new_byte {
                                                code[offset + 1] = new_byte;
                                                self.stats.jump_threaded += 1;
                                                changed = true;
                                            }
                                        }
                                    }
                                } else if code[target] == Opcode::JumpShort.to_byte()
                                    && target + 1 < code.len()
                                {
                                    let target_offset = code[target + 1] as i8;
                                    let target_from = target + 2;
                                    let final_target =
                                        (target_from as isize + target_offset as isize) as usize;

                                    // Avoid self-loops
                                    if final_target != offset {
                                        let new_offset =
                                            (final_target as isize - jump_from as isize) as isize;

                                        if new_offset >= i8::MIN as isize
                                            && new_offset <= i8::MAX as isize
                                        {
                                            let new_byte = new_offset as i8 as u8;
                                            if code[offset + 1] != new_byte {
                                                code[offset + 1] = new_byte;
                                                self.stats.jump_threaded += 1;
                                                changed = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        offset += 2;
                    }

                    _ => {
                        offset += instruction_size(&code, offset);
                    }
                }
            }
        }

        code
    }

    /// Single optimization pass
    ///
    /// Returns (optimized_code, whether_any_changes_made)
    fn optimize_pass(&mut self, code: &[u8]) -> (Vec<u8>, bool) {
        if code.is_empty() {
            return (Vec::new(), false);
        }

        // First pass: identify all patches
        let mut patches: Vec<PeepholeAction> = Vec::new();
        let mut offset = 0;

        while offset < code.len() {
            let action = self.scan_pattern(code, offset);
            match action {
                PeepholeAction::Keep => {
                    // Advance by instruction size
                    let size = instruction_size(code, offset);
                    offset += size;
                }
                PeepholeAction::Remove { start, end }
                | PeepholeAction::ReplaceWithOpcode { start, end, .. }
                | PeepholeAction::ReplaceWithBytes { start, end, .. } => {
                    patches.push(action);
                    // Skip past the matched pattern
                    offset = end;
                }
            }
        }

        if patches.is_empty() {
            return (code.to_vec(), false);
        }

        // Second pass: apply patches and build offset map
        let mut result = Vec::with_capacity(code.len());
        let mut offset_map: Vec<isize> = vec![0; code.len() + 1];
        let mut current_delta: isize = 0;
        let mut src_offset = 0;
        let mut patch_idx = 0;

        while src_offset < code.len() {
            offset_map[src_offset] = current_delta;

            // Check if we're at a patch location
            if patch_idx < patches.len() {
                match patches[patch_idx] {
                    PeepholeAction::Keep => {
                        patch_idx += 1;
                        continue;
                    }
                    PeepholeAction::Remove { start, end } => {
                        if src_offset == start {
                            let removed_bytes = end - start;
                            current_delta -= removed_bytes as isize;
                            self.stats.bytes_removed += removed_bytes;
                            // Fill offset_map for removed bytes
                            for i in start..end {
                                offset_map[i] = current_delta + (i - start) as isize;
                            }
                            src_offset = end;
                            patch_idx += 1;
                            continue;
                        }
                    }
                    PeepholeAction::ReplaceWithOpcode { start, end, opcode } => {
                        if src_offset == start {
                            result.push(opcode.to_byte());
                            let removed_bytes = (end - start) - 1; // We added 1 byte
                            current_delta -= removed_bytes as isize;
                            self.stats.bytes_removed += removed_bytes;
                            // Fill offset_map for replaced bytes
                            for i in start..end {
                                offset_map[i] = current_delta + (i - start) as isize;
                            }
                            src_offset = end;
                            patch_idx += 1;
                            continue;
                        }
                    }
                    PeepholeAction::ReplaceWithBytes {
                        start,
                        end,
                        ref bytes,
                    } => {
                        if src_offset == start {
                            result.extend_from_slice(bytes);
                            let original_len = end - start;
                            let new_len = bytes.len();
                            if original_len > new_len {
                                let removed_bytes = original_len - new_len;
                                current_delta -= removed_bytes as isize;
                                self.stats.bytes_removed += removed_bytes;
                            }
                            // Fill offset_map for replaced bytes
                            for i in start..end {
                                offset_map[i] = current_delta + (i - start) as isize;
                            }
                            src_offset = end;
                            patch_idx += 1;
                            continue;
                        }
                    }
                }
            }

            // Copy instruction
            let size = instruction_size(code, src_offset);
            result.extend_from_slice(&code[src_offset..src_offset + size]);
            src_offset += size;
        }
        offset_map[code.len()] = current_delta;

        // Third pass: fix up jump targets
        self.fixup_jumps(&mut result, &offset_map, code.len());

        (result, true)
    }

    /// Scan for an optimization pattern at the given offset
    fn scan_pattern(&mut self, code: &[u8], offset: usize) -> PeepholeAction {
        let remaining = code.len() - offset;
        if remaining == 0 {
            return PeepholeAction::Keep;
        }

        let op = code[offset];

        // Single instruction patterns
        match Opcode::from_byte(op) {
            Some(Opcode::Nop) => {
                self.stats.nops_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 1,
                };
            }
            _ => {}
        }

        // Two-instruction patterns
        if remaining >= 2 {
            let next_op = code[offset + 1];

            // Swap; Swap → remove both
            if op == Opcode::Swap.to_byte() && next_op == Opcode::Swap.to_byte() {
                self.stats.swap_swap_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 2,
                };
            }

            // Dup; Pop → remove both
            if op == Opcode::Dup.to_byte() && next_op == Opcode::Pop.to_byte() {
                self.stats.dup_pop_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 2,
                };
            }

            // Not; Not → remove both
            if op == Opcode::Not.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.not_not_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 2,
                };
            }

            // PushTrue; Not → PushFalse
            if op == Opcode::PushTrue.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.const_not_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::PushFalse,
                };
            }

            // PushFalse; Not → PushTrue
            if op == Opcode::PushFalse.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.const_not_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::PushTrue,
                };
            }

            // PushNil/PushUnit/PushTrue/PushFalse/PushEmpty; Pop → remove both
            let is_simple_push = matches!(
                Opcode::from_byte(op),
                Some(Opcode::PushNil)
                    | Some(Opcode::PushTrue)
                    | Some(Opcode::PushFalse)
                    | Some(Opcode::PushUnit)
                    | Some(Opcode::PushEmpty)
            );
            if is_simple_push && next_op == Opcode::Pop.to_byte() {
                self.stats.push_pop_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 2,
                };
            }

            // Neg; Neg → remove both (numeric double negation)
            if op == Opcode::Neg.to_byte() && next_op == Opcode::Neg.to_byte() {
                self.stats.neg_neg_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 2,
                };
            }

            // Abs; Abs → Abs (idempotent)
            if op == Opcode::Abs.to_byte() && next_op == Opcode::Abs.to_byte() {
                self.stats.idempotent_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 1, // Remove first Abs only
                };
            }

            // Comparison folding: Lt; Not → Ge
            if op == Opcode::Lt.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Ge,
                };
            }

            // Comparison folding: Le; Not → Gt
            if op == Opcode::Le.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Gt,
                };
            }

            // Comparison folding: Gt; Not → Le
            if op == Opcode::Gt.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Le,
                };
            }

            // Comparison folding: Ge; Not → Lt
            if op == Opcode::Ge.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Lt,
                };
            }

            // Comparison folding: Eq; Not → Ne
            if op == Opcode::Eq.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Ne,
                };
            }

            // Comparison folding: Ne; Not → Eq
            if op == Opcode::Ne.to_byte() && next_op == Opcode::Not.to_byte() {
                self.stats.comparison_folded += 1;
                return PeepholeAction::ReplaceWithOpcode {
                    start: offset,
                    end: offset + 2,
                    opcode: Opcode::Eq,
                };
            }
        }

        // Three-instruction patterns (PushLongSmall + value + op)
        if remaining >= 3 {
            let op2 = code[offset + 2];

            // PushLongSmall X; Pop → remove all
            if op == Opcode::PushLongSmall.to_byte() && op2 == Opcode::Pop.to_byte() {
                self.stats.push_pop_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }

            // PushLongSmall 0; Add → remove all (x + 0 = x)
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 0
                && op2 == Opcode::Add.to_byte()
            {
                self.stats.identity_ops_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }

            // PushLongSmall 0; Sub → remove all (x - 0 = x)
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 0
                && op2 == Opcode::Sub.to_byte()
            {
                self.stats.identity_ops_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }

            // PushLongSmall 1; Mul → remove all (x * 1 = x)
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 1
                && op2 == Opcode::Mul.to_byte()
            {
                self.stats.identity_ops_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }

            // PushLongSmall 1; Div → remove all (x / 1 = x)
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 1
                && op2 == Opcode::Div.to_byte()
            {
                self.stats.identity_ops_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }

            // Mul by 0: x * 0 = 0
            // PushLongSmall 0; Mul → Pop; PushLongSmall 0
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 0
                && op2 == Opcode::Mul.to_byte()
            {
                self.stats.mul_zero_folded += 1;
                return PeepholeAction::ReplaceWithBytes {
                    start: offset,
                    end: offset + 3,
                    bytes: vec![Opcode::Pop.to_byte(), Opcode::PushLongSmall.to_byte(), 0],
                };
            }

            // Pow by 0: x ^ 0 = 1
            // PushLongSmall 0; Pow → Pop; PushLongSmall 1
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 0
                && op2 == Opcode::Pow.to_byte()
            {
                self.stats.pow_folded += 1;
                return PeepholeAction::ReplaceWithBytes {
                    start: offset,
                    end: offset + 3,
                    bytes: vec![Opcode::Pop.to_byte(), Opcode::PushLongSmall.to_byte(), 1],
                };
            }

            // Pow by 1 (identity): x ^ 1 = x
            // PushLongSmall 1; Pow → remove all
            if op == Opcode::PushLongSmall.to_byte()
                && code[offset + 1] == 1
                && op2 == Opcode::Pow.to_byte()
            {
                self.stats.pow_folded += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 3,
                };
            }
        }

        // Four-byte patterns (PushConstant/PushAtom/etc with u16 + Pop)
        if remaining >= 4 {
            let op3 = code[offset + 3];

            // PushLong/PushAtom/PushString/etc; Pop → remove all
            let is_push_u16 = matches!(
                Opcode::from_byte(op),
                Some(Opcode::PushLong)
                    | Some(Opcode::PushAtom)
                    | Some(Opcode::PushString)
                    | Some(Opcode::PushUri)
                    | Some(Opcode::PushConstant)
                    | Some(Opcode::PushVariable)
            );
            if is_push_u16 && op3 == Opcode::Pop.to_byte() {
                self.stats.push_pop_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 4,
                };
            }

            // Constant branch folding: PushTrue; JumpIfTrue → Jump (always taken)
            if op == Opcode::PushTrue.to_byte() && code[offset + 1] == Opcode::JumpIfTrue.to_byte()
            {
                self.stats.const_branch_folded += 1;
                return PeepholeAction::ReplaceWithBytes {
                    start: offset,
                    end: offset + 4,
                    bytes: vec![
                        Opcode::Jump.to_byte(),
                        code[offset + 2], // offset high byte
                        code[offset + 3], // offset low byte
                    ],
                };
            }

            // Constant branch folding: PushFalse; JumpIfFalse → Jump (always taken)
            if op == Opcode::PushFalse.to_byte()
                && code[offset + 1] == Opcode::JumpIfFalse.to_byte()
            {
                self.stats.const_branch_folded += 1;
                return PeepholeAction::ReplaceWithBytes {
                    start: offset,
                    end: offset + 4,
                    bytes: vec![Opcode::Jump.to_byte(), code[offset + 2], code[offset + 3]],
                };
            }

            // Dead branch: PushTrue; JumpIfFalse → remove all (never taken, fall through)
            if op == Opcode::PushTrue.to_byte() && code[offset + 1] == Opcode::JumpIfFalse.to_byte()
            {
                self.stats.dead_branch_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 4,
                };
            }

            // Dead branch: PushFalse; JumpIfTrue → remove all (never taken, fall through)
            if op == Opcode::PushFalse.to_byte() && code[offset + 1] == Opcode::JumpIfTrue.to_byte()
            {
                self.stats.dead_branch_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 4,
                };
            }

            // Load deduplication: LoadLocal X; LoadLocal X → LoadLocal X; Dup
            if op == Opcode::LoadLocal.to_byte()
                && code[offset + 2] == Opcode::LoadLocal.to_byte()
                && code[offset + 1] == code[offset + 3]
            // Same slot
            {
                self.stats.load_deduplicated += 1;
                return PeepholeAction::ReplaceWithBytes {
                    start: offset,
                    end: offset + 4,
                    bytes: vec![
                        Opcode::LoadLocal.to_byte(),
                        code[offset + 1], // slot
                        Opcode::Dup.to_byte(),
                    ],
                };
            }
        }

        PeepholeAction::Keep
    }

    /// Fix up jump targets after code has been modified
    fn fixup_jumps(&self, code: &mut [u8], offset_map: &[isize], original_len: usize) {
        let mut offset = 0;

        while offset < code.len() {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            match opcode {
                // 2-byte signed offset jumps
                Opcode::Jump
                | Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpIfNil
                | Opcode::JumpIfError => {
                    if offset + 2 < code.len() {
                        let old_jump_offset =
                            i16::from_be_bytes([code[offset + 1], code[offset + 2]]);

                        // Find the original position of this jump instruction
                        let old_instr_pos = self.reverse_offset(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 3; // After the 3-byte instruction
                        let old_target =
                            (old_jump_from as isize + old_jump_offset as isize) as usize;

                        // Calculate new positions
                        let new_jump_from = offset + 3;

                        // Find where the original target is now
                        if old_target <= original_len {
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            // Calculate new offset
                            let new_jump_offset =
                                (new_target as isize - new_jump_from as isize) as i16;
                            let bytes = new_jump_offset.to_be_bytes();
                            code[offset + 1] = bytes[0];
                            code[offset + 2] = bytes[1];
                        }
                    }
                    offset += 3;
                }

                // 1-byte signed offset jumps
                Opcode::JumpShort | Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                    if offset + 1 < code.len() {
                        let old_jump_offset = code[offset + 1] as i8;

                        // Find the original position of this jump instruction
                        let old_instr_pos = self.reverse_offset(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 2; // After the 2-byte instruction
                        let old_target =
                            (old_jump_from as isize + old_jump_offset as isize) as usize;

                        // Calculate new positions
                        let new_jump_from = offset + 2;

                        // Find where the original target is now
                        if old_target <= original_len {
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            // Calculate new offset
                            let new_jump_offset =
                                (new_target as isize - new_jump_from as isize) as i8;
                            code[offset + 1] = new_jump_offset as u8;
                        }
                    }
                    offset += 2;
                }

                _ => {
                    offset += instruction_size(code, offset);
                }
            }
        }
    }

    /// Reverse lookup: given a new offset, find the original offset
    fn reverse_offset(
        &self,
        new_offset: usize,
        offset_map: &[isize],
        original_len: usize,
    ) -> usize {
        for old_pos in 0..=original_len {
            let delta = offset_map.get(old_pos).copied().unwrap_or(0);
            if (old_pos as isize + delta) as usize == new_offset {
                return old_pos;
            }
        }
        // Fallback: assume no change
        new_offset
    }
}

/// Optimize bytecode using the peephole optimizer
///
/// Convenience function for one-shot optimization.
pub fn optimize_bytecode(code: Vec<u8>) -> (Vec<u8>, OptimizationStats) {
    let mut optimizer = PeepholeOptimizer::new();
    let optimized = optimizer.optimize(code);
    (optimized, optimizer.stats().clone())
}
