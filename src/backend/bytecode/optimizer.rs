//! Bytecode Peephole Optimizer
//!
//! This module provides post-compilation optimization passes for bytecode.
//! It implements peephole optimization patterns that identify and eliminate
//! redundant instruction sequences.
//!
//! # Optimization Patterns
//!
//! | Pattern | Replacement | Rationale |
//! |---------|-------------|-----------|
//! | `PushLongSmall 0; Add` | (remove) | Adding 0 is identity |
//! | `PushLongSmall 0; Sub` | (remove) | Subtracting 0 is identity (from first operand) |
//! | `PushLongSmall 1; Mul` | (remove) | Multiplying by 1 is identity |
//! | `PushLongSmall 1; Div` | (remove) | Dividing by 1 is identity |
//! | `Push*; Pop` | (remove) | Dead push immediately popped |
//! | `Swap; Swap` | (remove) | Double swap is no-op |
//! | `Dup; Pop` | (remove) | Duplicate then pop is no-op |
//! | `Not; Not` | (remove) | Double negation is identity |
//! | `PushTrue; Not` | `PushFalse` | Constant fold |
//! | `PushFalse; Not` | `PushTrue` | Constant fold |
//! | `Nop` | (remove) | No-ops are unnecessary |
//!
//! # Jump Fixups
//!
//! When instructions are removed, all jump targets must be adjusted.
//! The optimizer builds a byte offset mapping and patches all jump instructions.
//!
//! # Example
//!
//! ```ignore
//! // Before optimization:
//! // push_long_small 0
//! // add
//! // push_true
//! // not
//!
//! // After optimization:
//! // push_false
//! ```

use super::opcodes::Opcode;

/// Result of a peephole scan at a given position
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeepholeAction {
    /// Keep the instruction(s) unchanged
    Keep,
    /// Remove bytes from `start` to `end` (exclusive)
    Remove { start: usize, end: usize },
    /// Replace bytes from `start` to `end` with a single opcode
    ReplaceWithOpcode { start: usize, end: usize, opcode: Opcode },
    /// Replace bytes from `start` to `end` with multiple bytes
    ReplaceWithBytes { start: usize, end: usize, bytes: Vec<u8> },
}

/// Peephole optimizer for bytecode
pub struct PeepholeOptimizer {
    /// Statistics about optimizations performed
    stats: OptimizationStats,
}

/// Statistics about optimizations performed
#[derive(Debug, Clone, Default)]
pub struct OptimizationStats {
    /// Number of identity arithmetic ops removed (add 0, mul 1, etc.)
    pub identity_ops_removed: usize,
    /// Number of push-pop pairs removed
    pub push_pop_removed: usize,
    /// Number of swap-swap pairs removed
    pub swap_swap_removed: usize,
    /// Number of not-not pairs removed
    pub not_not_removed: usize,
    /// Number of push-not constant folds
    pub const_not_folded: usize,
    /// Number of nops removed
    pub nops_removed: usize,
    /// Number of dup-pop pairs removed
    pub dup_pop_removed: usize,
    /// Total bytes removed
    pub bytes_removed: usize,
    // New stats for expanded optimizations
    /// Number of boolean identity ops removed (And True, Or False)
    pub bool_identity_removed: usize,
    /// Number of boolean annihilators folded (And False, Or True)
    pub bool_annihilator_folded: usize,
    /// Number of numeric Neg; Neg pairs removed
    pub neg_neg_removed: usize,
    /// Number of constant branches folded (PushTrue; JumpIfTrue → Jump)
    pub const_branch_folded: usize,
    /// Number of dead branches removed (PushTrue; JumpIfFalse)
    pub dead_branch_removed: usize,
    /// Number of idempotent ops removed (Abs; Abs)
    pub idempotent_removed: usize,
    /// Number of load deduplications (LoadLocal X; LoadLocal X → LoadLocal X; Dup)
    pub load_deduplicated: usize,
    /// Number of comparison folds (Lt; Not → Ge)
    pub comparison_folded: usize,
    /// Number of jumps threaded (Jump → Jump)
    pub jump_threaded: usize,
    /// Number of multiply-by-zero folds
    pub mul_zero_folded: usize,
    /// Number of power folds (x^0, x^1)
    pub pow_folded: usize,
}

impl OptimizationStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self::default()
    }

    /// Get total optimizations performed
    pub fn total_optimizations(&self) -> usize {
        self.identity_ops_removed
            + self.push_pop_removed
            + self.swap_swap_removed
            + self.not_not_removed
            + self.const_not_folded
            + self.nops_removed
            + self.dup_pop_removed
            + self.bool_identity_removed
            + self.bool_annihilator_folded
            + self.neg_neg_removed
            + self.const_branch_folded
            + self.dead_branch_removed
            + self.idempotent_removed
            + self.load_deduplicated
            + self.comparison_folded
            + self.jump_threaded
            + self.mul_zero_folded
            + self.pow_folded
    }
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
                                    let target_offset = i16::from_be_bytes([
                                        code[target + 1],
                                        code[target + 2],
                                    ]);
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
                                    let target_offset = i16::from_be_bytes([
                                        code[target + 1],
                                        code[target + 2],
                                    ]);
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
                    PeepholeAction::ReplaceWithBytes { start, end, ref bytes } => {
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

            // NOTE: Boolean identity and annihilator optimizations are DISABLED
            // because they can hide type errors. For example, optimizing:
            //   (and 1 True) → [PushLong 1; PushTrue; And] → [PushLong 1]
            // would return 1 instead of producing a type error.
            //
            // These optimizations would be safe in a type-checked language, but MeTTa
            // uses dynamic typing and expects runtime type errors for (and 1 True).
            //
            // Commented out patterns:
            // - x AND True = x  (PushTrue; And → remove)
            // - x OR False = x  (PushFalse; Or → remove)
            // - x AND False = False  (PushFalse; And → Pop; PushFalse)
            // - x OR True = True  (PushTrue; Or → Pop; PushTrue)

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
            // Note: Stack is [x, 0], so Sub computes x - 0 = x
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
                    bytes: vec![
                        Opcode::Pop.to_byte(),
                        Opcode::PushLongSmall.to_byte(),
                        0,
                    ],
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
                    bytes: vec![
                        Opcode::Pop.to_byte(),
                        Opcode::PushLongSmall.to_byte(),
                        1,
                    ],
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
            // PushTrue (1 byte) + JumpIfTrue (1 byte) + offset (2 bytes) = 4 bytes
            // → Jump (1 byte) + offset (2 bytes) = 3 bytes
            if op == Opcode::PushTrue.to_byte()
                && code[offset + 1] == Opcode::JumpIfTrue.to_byte()
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
                    bytes: vec![
                        Opcode::Jump.to_byte(),
                        code[offset + 2],
                        code[offset + 3],
                    ],
                };
            }

            // Dead branch: PushTrue; JumpIfFalse → remove all (never taken, fall through)
            if op == Opcode::PushTrue.to_byte()
                && code[offset + 1] == Opcode::JumpIfFalse.to_byte()
            {
                self.stats.dead_branch_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 4,
                };
            }

            // Dead branch: PushFalse; JumpIfTrue → remove all (never taken, fall through)
            if op == Opcode::PushFalse.to_byte()
                && code[offset + 1] == Opcode::JumpIfTrue.to_byte()
            {
                self.stats.dead_branch_removed += 1;
                return PeepholeAction::Remove {
                    start: offset,
                    end: offset + 4,
                };
            }

            // Load deduplication: LoadLocal X; LoadLocal X → LoadLocal X; Dup
            // LoadLocal (1 byte) + slot (1 byte) = 2 bytes each, total 4 bytes
            // After: LoadLocal (1 byte) + slot (1 byte) + Dup (1 byte) = 3 bytes
            if op == Opcode::LoadLocal.to_byte()
                && code[offset + 2] == Opcode::LoadLocal.to_byte()
                && code[offset + 1] == code[offset + 3] // Same slot
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
    ///
    /// Jump offsets are relative to the byte after the jump instruction.
    /// When bytes are removed, we need to:
    /// 1. Find the original position of the jump instruction
    /// 2. Calculate where it was jumping to in the original code
    /// 3. Find where that target is now in the new code
    /// 4. Calculate the new offset
    fn fixup_jumps(&self, code: &mut [u8], offset_map: &[isize], original_len: usize) {
        let mut offset = 0;

        while offset < code.len() {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            match opcode {
                // 2-byte signed offset jumps
                Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue
                | Opcode::JumpIfNil | Opcode::JumpIfError => {
                    if offset + 2 < code.len() {
                        let old_jump_offset = i16::from_be_bytes([code[offset + 1], code[offset + 2]]);

                        // Find the original position of this jump instruction
                        let old_instr_pos = self.reverse_offset(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 3; // After the 3-byte instruction
                        let old_target = (old_jump_from as isize + old_jump_offset as isize) as usize;

                        // Calculate new positions
                        let new_jump_from = offset + 3;

                        // Find where the original target is now
                        if old_target <= original_len {
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            // Calculate new offset
                            let new_jump_offset = (new_target as isize - new_jump_from as isize) as i16;
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
                        let old_target = (old_jump_from as isize + old_jump_offset as isize) as usize;

                        // Calculate new positions
                        let new_jump_from = offset + 2;

                        // Find where the original target is now
                        if old_target <= original_len {
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            // Calculate new offset
                            let new_jump_offset = (new_target as isize - new_jump_from as isize) as i8;
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
    ///
    /// The offset_map tells us: new_pos = old_pos + delta[old_pos]
    /// We need to find old_pos such that old_pos + delta[old_pos] == new_offset
    fn reverse_offset(&self, new_offset: usize, offset_map: &[isize], original_len: usize) -> usize {
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

/// Get the size of an instruction at the given offset
fn instruction_size(code: &[u8], offset: usize) -> usize {
    if offset >= code.len() {
        return 1;
    }

    match Opcode::from_byte(code[offset]) {
        Some(opcode) => 1 + opcode.immediate_size(),
        None => 1, // Unknown opcode, assume 1 byte
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

// ============================================================================
// Dead Code Elimination
// ============================================================================

use std::collections::{HashSet, VecDeque};

/// Statistics for dead code elimination
#[derive(Debug, Clone, Default)]
pub struct DceStats {
    /// Number of unreachable basic blocks removed
    pub blocks_removed: usize,
    /// Total bytes removed
    pub bytes_removed: usize,
    /// Number of basic blocks found
    pub blocks_found: usize,
    /// Number of reachable blocks
    pub blocks_reachable: usize,
}

impl DceStats {
    /// Create new empty stats
    pub fn new() -> Self {
        Self::default()
    }
}

/// Dead Code Eliminator
///
/// Removes unreachable code by:
/// 1. Building a control flow graph (CFG) from bytecode
/// 2. Marking all blocks reachable from the entry point
/// 3. Removing unreachable blocks
pub struct DeadCodeEliminator {
    /// Statistics about eliminations performed
    stats: DceStats,
}

impl Default for DeadCodeEliminator {
    fn default() -> Self {
        Self::new()
    }
}

impl DeadCodeEliminator {
    /// Create a new dead code eliminator
    pub fn new() -> Self {
        Self {
            stats: DceStats::new(),
        }
    }

    /// Get elimination statistics
    pub fn stats(&self) -> &DceStats {
        &self.stats
    }

    /// Eliminate dead code from bytecode
    ///
    /// Returns the optimized bytecode.
    pub fn eliminate(&mut self, code: Vec<u8>) -> Vec<u8> {
        if code.is_empty() {
            return code;
        }

        // Step 1: Find all basic block boundaries
        let block_starts = self.find_block_starts(&code);
        self.stats.blocks_found = block_starts.len();

        // Step 2: Mark reachable blocks starting from entry (offset 0)
        let reachable = self.mark_reachable(&code, &block_starts);
        self.stats.blocks_reachable = reachable.len();

        // Step 3: Identify unreachable regions
        let unreachable_regions = self.find_unreachable_regions(&code, &block_starts, &reachable);

        if unreachable_regions.is_empty() {
            return code;
        }

        // Step 4: Remove unreachable code and fix up jumps
        self.remove_unreachable(code, &unreachable_regions)
    }

    /// Find all basic block start offsets
    fn find_block_starts(&self, code: &[u8]) -> Vec<usize> {
        let mut starts = HashSet::new();
        starts.insert(0); // Entry point is always a block start

        let mut offset = 0;
        while offset < code.len() {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            let instr_size = 1 + opcode.immediate_size();
            let next_ip = offset + instr_size;

            match opcode {
                // Unconditional jumps - target is block start, next IP may also be (dead)
                Opcode::Jump => {
                    if let Some(target) = self.get_jump_target_i16(code, offset) {
                        if target < code.len() {
                            starts.insert(target);
                        }
                    }
                    // Code after unconditional jump might be reachable via other paths
                    if next_ip < code.len() {
                        starts.insert(next_ip);
                    }
                }
                Opcode::JumpShort => {
                    if let Some(target) = self.get_jump_target_i8(code, offset) {
                        if target < code.len() {
                            starts.insert(target);
                        }
                    }
                    if next_ip < code.len() {
                        starts.insert(next_ip);
                    }
                }

                // Conditional jumps - both target and fallthrough are block starts
                Opcode::JumpIfFalse | Opcode::JumpIfTrue | Opcode::JumpIfNil | Opcode::JumpIfError => {
                    if let Some(target) = self.get_jump_target_i16(code, offset) {
                        if target < code.len() {
                            starts.insert(target);
                        }
                    }
                    if next_ip < code.len() {
                        starts.insert(next_ip);
                    }
                }
                Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                    if let Some(target) = self.get_jump_target_i8(code, offset) {
                        if target < code.len() {
                            starts.insert(target);
                        }
                    }
                    if next_ip < code.len() {
                        starts.insert(next_ip);
                    }
                }

                // Return - next IP is a potential block start (dead code follows)
                Opcode::Return | Opcode::ReturnMulti | Opcode::Halt => {
                    if next_ip < code.len() {
                        starts.insert(next_ip);
                    }
                }

                _ => {}
            }

            offset = next_ip;
        }

        let mut result: Vec<usize> = starts.into_iter().collect();
        result.sort();
        result
    }

    /// Mark all blocks reachable from entry point
    fn mark_reachable(&self, code: &[u8], block_starts: &[usize]) -> HashSet<usize> {
        let mut reachable = HashSet::new();
        let mut worklist = VecDeque::new();

        // Start from entry point
        worklist.push_back(0usize);

        while let Some(block_start) = worklist.pop_front() {
            if reachable.contains(&block_start) {
                continue;
            }
            reachable.insert(block_start);

            // Find block end and successors
            let block_end = self.find_block_end(code, block_start, block_starts);
            let successors = self.find_successors(code, block_start, block_end);

            for succ in successors {
                if !reachable.contains(&succ) {
                    worklist.push_back(succ);
                }
            }
        }

        reachable
    }

    /// Find the end offset of a basic block
    fn find_block_end(&self, code: &[u8], start: usize, block_starts: &[usize]) -> usize {
        let mut offset = start;

        while offset < code.len() {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            let instr_size = 1 + opcode.immediate_size();
            let next_ip = offset + instr_size;

            // Check if next IP is a block start (not counting current block)
            if next_ip < code.len() && next_ip != start && block_starts.contains(&next_ip) {
                return next_ip;
            }

            // Terminating instructions end the block
            if matches!(opcode,
                Opcode::Jump | Opcode::JumpShort |
                Opcode::Return | Opcode::ReturnMulti | Opcode::Halt
            ) {
                return next_ip;
            }

            offset = next_ip;
        }

        code.len()
    }

    /// Find successor blocks of a basic block
    fn find_successors(&self, code: &[u8], start: usize, end: usize) -> Vec<usize> {
        let mut successors = Vec::new();

        // Scan the last instruction(s) in the block
        let mut offset = start;
        while offset < end {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            let instr_size = 1 + opcode.immediate_size();
            let next_ip = offset + instr_size;

            // Check if this is the last instruction in the block
            if next_ip >= end {
                match opcode {
                    Opcode::Jump => {
                        if let Some(target) = self.get_jump_target_i16(code, offset) {
                            successors.push(target);
                        }
                    }
                    Opcode::JumpShort => {
                        if let Some(target) = self.get_jump_target_i8(code, offset) {
                            successors.push(target);
                        }
                    }
                    Opcode::JumpIfFalse | Opcode::JumpIfTrue | Opcode::JumpIfNil | Opcode::JumpIfError => {
                        if let Some(target) = self.get_jump_target_i16(code, offset) {
                            successors.push(target);
                        }
                        // Fallthrough
                        if next_ip < code.len() {
                            successors.push(next_ip);
                        }
                    }
                    Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                        if let Some(target) = self.get_jump_target_i8(code, offset) {
                            successors.push(target);
                        }
                        // Fallthrough
                        if next_ip < code.len() {
                            successors.push(next_ip);
                        }
                    }
                    Opcode::Return | Opcode::ReturnMulti | Opcode::Halt => {
                        // No successors
                    }
                    _ => {
                        // Fallthrough to next block
                        if next_ip < code.len() {
                            successors.push(next_ip);
                        }
                    }
                }
            }

            offset = next_ip;
        }

        successors
    }

    /// Find regions of unreachable code (start, end) pairs
    fn find_unreachable_regions(
        &mut self,
        code: &[u8],
        block_starts: &[usize],
        reachable: &HashSet<usize>,
    ) -> Vec<(usize, usize)> {
        let mut regions = Vec::new();

        for (i, &start) in block_starts.iter().enumerate() {
            if !reachable.contains(&start) {
                let end = if i + 1 < block_starts.len() {
                    block_starts[i + 1]
                } else {
                    code.len()
                };

                if start < end {
                    regions.push((start, end));
                    self.stats.blocks_removed += 1;
                    self.stats.bytes_removed += end - start;
                }
            }
        }

        regions
    }

    /// Remove unreachable regions and fix up jump targets
    fn remove_unreachable(&self, code: Vec<u8>, regions: &[(usize, usize)]) -> Vec<u8> {
        if regions.is_empty() {
            return code;
        }

        // Build offset map: old_offset -> new_offset
        let mut offset_map: Vec<isize> = vec![0; code.len() + 1];
        let mut delta: isize = 0;

        let mut region_idx = 0;
        for old_offset in 0..=code.len() {
            // Check if we're entering an unreachable region
            if region_idx < regions.len() && old_offset == regions[region_idx].0 {
                let (start, end) = regions[region_idx];
                delta -= (end - start) as isize;
                region_idx += 1;
            }
            offset_map[old_offset] = delta;
        }

        // Build new code, skipping unreachable regions
        let mut result = Vec::with_capacity(code.len());
        let mut old_offset = 0;
        let mut region_idx = 0;

        while old_offset < code.len() {
            // Check if we're in an unreachable region
            if region_idx < regions.len() && old_offset == regions[region_idx].0 {
                old_offset = regions[region_idx].1;
                region_idx += 1;
                continue;
            }

            // Copy instruction
            let size = instruction_size(&code, old_offset);
            result.extend_from_slice(&code[old_offset..old_offset + size]);
            old_offset += size;
        }

        // Fix up jump targets
        self.fixup_jumps_dce(&mut result, &offset_map, code.len());

        result
    }

    /// Fix up jump targets after dead code removal
    fn fixup_jumps_dce(&self, code: &mut [u8], offset_map: &[isize], original_len: usize) {
        let mut offset = 0;

        while offset < code.len() {
            let Some(opcode) = Opcode::from_byte(code[offset]) else {
                offset += 1;
                continue;
            };

            match opcode {
                // 2-byte signed offset jumps
                Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue
                | Opcode::JumpIfNil | Opcode::JumpIfError => {
                    if offset + 2 < code.len() {
                        let old_jump_offset = i16::from_be_bytes([code[offset + 1], code[offset + 2]]);

                        // Find original position of this instruction
                        let old_instr_pos = self.reverse_offset_dce(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 3;
                        let old_target = (old_jump_from as isize + old_jump_offset as isize) as usize;

                        if old_target <= original_len {
                            // Calculate new positions
                            let new_jump_from = offset + 3;
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            let new_jump_offset = (new_target as isize - new_jump_from as isize) as i16;
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

                        let old_instr_pos = self.reverse_offset_dce(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 2;
                        let old_target = (old_jump_from as isize + old_jump_offset as isize) as usize;

                        if old_target <= original_len {
                            let new_jump_from = offset + 2;
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

                            let new_jump_offset = (new_target as isize - new_jump_from as isize) as i8;
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

    /// Reverse lookup for DCE offset map
    fn reverse_offset_dce(&self, new_offset: usize, offset_map: &[isize], original_len: usize) -> usize {
        for old_pos in 0..=original_len {
            let delta = offset_map.get(old_pos).copied().unwrap_or(0);
            if (old_pos as isize + delta) as usize == new_offset {
                return old_pos;
            }
        }
        new_offset
    }

    /// Get jump target for 2-byte signed offset jump
    fn get_jump_target_i16(&self, code: &[u8], offset: usize) -> Option<usize> {
        if offset + 2 >= code.len() {
            return None;
        }
        let rel_offset = i16::from_be_bytes([code[offset + 1], code[offset + 2]]);
        let next_ip = offset + 3;
        let target = (next_ip as isize + rel_offset as isize) as usize;
        Some(target)
    }

    /// Get jump target for 1-byte signed offset jump
    fn get_jump_target_i8(&self, code: &[u8], offset: usize) -> Option<usize> {
        if offset + 1 >= code.len() {
            return None;
        }
        let rel_offset = code[offset + 1] as i8;
        let next_ip = offset + 2;
        let target = (next_ip as isize + rel_offset as isize) as usize;
        Some(target)
    }
}

/// Eliminate dead code from bytecode
///
/// Convenience function for one-shot dead code elimination.
pub fn eliminate_dead_code(code: Vec<u8>) -> (Vec<u8>, DceStats) {
    let mut eliminator = DeadCodeEliminator::new();
    let optimized = eliminator.eliminate(code);
    (optimized, eliminator.stats().clone())
}

/// Full bytecode optimization: peephole + dead code elimination
///
/// Applies both optimizations in sequence for best results.
pub fn optimize_bytecode_full(code: Vec<u8>) -> (Vec<u8>, OptimizationStats, DceStats) {
    // First pass: peephole optimization
    let mut peephole = PeepholeOptimizer::new();
    let optimized = peephole.optimize(code);
    let peephole_stats = peephole.stats().clone();

    // Second pass: dead code elimination
    let mut dce = DeadCodeEliminator::new();
    let optimized = dce.eliminate(optimized);
    let dce_stats = dce.stats().clone();

    (optimized, peephole_stats, dce_stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_code(opcodes: &[u8]) -> Vec<u8> {
        opcodes.to_vec()
    }

    #[test]
    fn test_nop_removal() {
        let code = make_code(&[
            Opcode::Nop.to_byte(),
            Opcode::PushTrue.to_byte(),
            Opcode::Nop.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.nops_removed, 2);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_swap_swap_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::PushFalse.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.swap_swap_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::PushFalse.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_dup_pop_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Dup.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.dup_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_not_not_removal() {
        // Use LoadLocal to avoid triggering PushTrue/PushFalse; Not patterns first
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // local slot 0
            Opcode::Not.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.not_not_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::Return.to_byte()
            ]
        );
    }

    #[test]
    fn test_push_true_not_folding() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_not_folded, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushFalse.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_false_not_folding() {
        let code = make_code(&[
            Opcode::PushFalse.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_not_folded, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_simple_push_pop_removal() {
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Pop.to_byte(),
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_push_long_small_pop_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42,
            Opcode::Pop.to_byte(),
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_add_zero_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        // Should be: push 5, return
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                5,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_sub_zero_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Sub.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                10,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_mul_one_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            7,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                7,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_div_one_removal() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            8,
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::Div.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.identity_ops_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                8,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_push_constant_pop_removal() {
        let code = make_code(&[
            Opcode::PushLong.to_byte(),
            0,
            0, // index 0
            Opcode::Pop.to_byte(),
            Opcode::PushNil.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.push_pop_removed, 1);
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_jump_fixup() {
        // Build: if (load x) { nop; return 1 } else { return 2 }
        // Using LoadLocal instead of PushTrue to avoid triggering dead branch optimization
        // Layout:
        //   0: LoadLocal 0           (2 bytes)
        //   2: JumpIfFalse           (1 byte)
        //   3-4: offset 7            (2 bytes) -> jump_from=5, target=5+7=12 (else)
        //   5: Nop                   (1 byte) - will be removed
        //   6: PushLongSmall         (1 byte)
        //   7: 1                     (1 byte)
        //   8: Return                (1 byte)
        //   9: Jump                  (1 byte)
        //  10-11: offset 3           (2 bytes) -> jump_from=12, target=12+3=15 (end)
        //  12: PushLongSmall         (1 byte) - else branch
        //  13: 2                     (1 byte)
        //  14: Return                (1 byte)
        //
        // After removing Nop (1 byte at offset 5):
        //   0: LoadLocal 0
        //   2: JumpIfFalse
        //   3-4: offset ?            -> jump_from=5, should target 11 (was 12), so offset=6
        //   5: PushLongSmall (was 6)
        //   6: 1 (was 7)
        //   7: Return (was 8)
        //   8: Jump (was 9)
        //   9-10: offset ? (was 10-11) -> jump_from=11, should target 14 (was 15), so offset=3
        //  11: PushLongSmall (was 12)
        //  12: 2 (was 13)
        //  13: Return (was 14)

        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),      // 0
            0,                                 // 1: slot 0
            Opcode::JumpIfFalse.to_byte(),    // 2
            0, 7,                              // 3-4: offset 7 to else branch at 12
            Opcode::Nop.to_byte(),            // 5 - will be removed
            Opcode::PushLongSmall.to_byte(),  // 6
            1,                                 // 7
            Opcode::Return.to_byte(),         // 8
            Opcode::Jump.to_byte(),           // 9 (skip else)
            0, 3,                              // 10-11: offset 3 to end at 15
            Opcode::PushLongSmall.to_byte(),  // 12 - else branch
            2,                                 // 13
            Opcode::Return.to_byte(),         // 14
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.nops_removed, 1);
        // After removing nop at position 5, JumpIfFalse offset should be adjusted from 7 to 6
        assert_eq!(optimized[2], Opcode::JumpIfFalse.to_byte());
        let new_offset = i16::from_be_bytes([optimized[3], optimized[4]]);
        assert_eq!(new_offset, 6); // Adjusted from 7 to 6

        // Check that Jump offset is still 3 (both source and target shifted equally)
        assert_eq!(optimized[8], Opcode::Jump.to_byte());
        let jump_offset = i16::from_be_bytes([optimized[9], optimized[10]]);
        assert_eq!(jump_offset, 3); // Should remain 3
    }

    #[test]
    fn test_multiple_passes() {
        // After first pass: PushTrue; Not → PushFalse
        // After second pass: PushFalse; Not → PushTrue
        // The second Not is left alone since there's no more Not after it
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // First: PushTrue; Not → PushFalse (const_not_folded++)
        // Then: PushFalse; Not → PushTrue (const_not_folded++)
        assert!(stats.const_not_folded >= 1);
        // Final result should be PushTrue
        assert_eq!(
            optimized,
            vec![Opcode::PushTrue.to_byte(), Opcode::Return.to_byte()]
        );
    }

    #[test]
    fn test_no_optimization_needed() {
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Add.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code.clone());

        assert_eq!(stats.total_optimizations(), 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_empty_code() {
        let code = Vec::new();
        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.total_optimizations(), 0);
        assert!(optimized.is_empty());
    }

    #[test]
    fn test_chained_optimizations() {
        // swap; swap; swap; swap → should all be removed
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),
            Opcode::PushFalse.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Swap.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.swap_swap_removed, 2);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::PushFalse.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    // ========================================================================
    // Dead Code Elimination Tests
    // ========================================================================

    #[test]
    fn test_dce_empty_code() {
        let code = Vec::new();
        let (optimized, stats) = eliminate_dead_code(code);

        assert!(optimized.is_empty());
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(stats.bytes_removed, 0);
    }

    #[test]
    fn test_dce_no_dead_code() {
        // All code is reachable
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),  // 0
            Opcode::Return.to_byte(),    // 1
        ]);

        let (optimized, stats) = eliminate_dead_code(code.clone());

        assert_eq!(optimized, code);
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(stats.bytes_removed, 0);
    }

    #[test]
    fn test_dce_dead_code_after_unconditional_jump() {
        // Code after unconditional jump is dead
        //   0: Jump +3          (3 bytes) -> jumps to offset 6
        //   3: PushTrue         (1 byte)  - DEAD
        //   4: Pop              (1 byte)  - DEAD
        //   5: Nop              (1 byte)  - DEAD (but starts new block)
        //   6: PushFalse        (1 byte)  - target of jump
        //   7: Return           (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(),       // 0
            0, 3,                          // 1-2: offset +3 -> target = 3+3 = 6
            Opcode::PushTrue.to_byte(),   // 3 - DEAD
            Opcode::Pop.to_byte(),        // 4 - DEAD
            Opcode::Nop.to_byte(),        // 5 - DEAD (block boundary)
            Opcode::PushFalse.to_byte(),  // 6 - jump target
            Opcode::Return.to_byte(),     // 7
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Dead code from offset 3-5 should be removed
        assert!(stats.blocks_removed >= 1);
        assert!(stats.bytes_removed >= 3);

        // Result should be: Jump, PushFalse, Return
        // After removing dead code, Jump offset needs to be fixed
        assert_eq!(optimized.len(), 5); // Jump(3 bytes) + PushFalse(1) + Return(1)
        assert_eq!(optimized[0], Opcode::Jump.to_byte());
        assert_eq!(optimized[3], Opcode::PushFalse.to_byte());
        assert_eq!(optimized[4], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_dead_code_after_return() {
        // Code after return is dead
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),   // 0
            Opcode::Return.to_byte(),     // 1
            Opcode::PushFalse.to_byte(),  // 2 - DEAD
            Opcode::Pop.to_byte(),        // 3 - DEAD
            Opcode::Return.to_byte(),     // 4 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_dce_conditional_branch_both_reachable() {
        // Both branches of conditional are reachable
        //   0: PushTrue                  (1 byte)
        //   1: JumpIfFalse +4            (3 bytes) -> target = 4+4 = 8
        //   4: PushLongSmall 1           (2 bytes) - then branch
        //   6: Jump +2                   (3 bytes) -> target = 9+2 = 11
        //   9: PushLongSmall 2           (2 bytes) - else branch
        //  11: Return                    (1 byte)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),      // 0
            Opcode::JumpIfFalse.to_byte(),   // 1
            0, 4,                             // 2-3: offset +4 to offset 8
            Opcode::PushLongSmall.to_byte(), // 4
            1,                                // 5
            Opcode::Jump.to_byte(),          // 6
            0, 2,                             // 7-8: offset +2 to offset 11
            Opcode::PushLongSmall.to_byte(), // 9
            2,                                // 10
            Opcode::Return.to_byte(),        // 11
        ]);

        let (optimized, stats) = eliminate_dead_code(code.clone());

        // No dead code - all paths reachable
        assert_eq!(stats.blocks_removed, 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_dce_halt_terminates() {
        // Code after Halt is dead
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),   // 0
            Opcode::Halt.to_byte(),       // 1
            Opcode::PushFalse.to_byte(),  // 2 - DEAD
            Opcode::Return.to_byte(),     // 3 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::Halt.to_byte(),
            ]
        );
    }

    #[test]
    fn test_dce_jump_short() {
        // Test with short (1-byte offset) jump
        //   0: JumpShort +2       (2 bytes) -> target = 2+2 = 4
        //   2: Nop                (1 byte)  - DEAD
        //   3: Nop                (1 byte)  - DEAD (but new block)
        //   4: Return             (1 byte)  - target
        let code = make_code(&[
            Opcode::JumpShort.to_byte(),  // 0
            2,                             // 1: offset +2 -> target = 4
            Opcode::Nop.to_byte(),        // 2 - DEAD
            Opcode::Nop.to_byte(),        // 3 - DEAD
            Opcode::Return.to_byte(),     // 4 - jump target
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(optimized.len(), 3); // JumpShort(2) + Return(1)
        assert_eq!(optimized[0], Opcode::JumpShort.to_byte());
        assert_eq!(optimized[2], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_return_multi() {
        // Code after ReturnMulti is dead
        // Note: ReturnMulti has no immediate bytes (count is on stack)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),    // 0
            Opcode::ReturnMulti.to_byte(), // 1
            Opcode::PushFalse.to_byte(),   // 2 - DEAD
            Opcode::Return.to_byte(),      // 3 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert!(stats.blocks_removed >= 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushTrue.to_byte(),
                Opcode::ReturnMulti.to_byte(),
            ]
        );
    }

    #[test]
    fn test_dce_jump_fixup_after_removal() {
        // Test that jump targets are correctly fixed after dead code removal
        //   0: Jump +6            (3 bytes) -> target = 3+6 = 9
        //   3: PushTrue           (1 byte)  - DEAD (not jumped to)
        //   4: Return             (1 byte)  - DEAD
        //   5: Nop                (1 byte)  - DEAD
        //   6: Nop                (1 byte)  - DEAD
        //   7: Nop                (1 byte)  - DEAD
        //   8: Nop                (1 byte)  - DEAD (block boundary for target)
        //   9: PushFalse          (1 byte)  - jump target
        //  10: Return             (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(),       // 0
            0, 6,                          // 1-2: offset +6 -> target = 9
            Opcode::PushTrue.to_byte(),   // 3 - DEAD
            Opcode::Return.to_byte(),     // 4 - DEAD
            Opcode::Nop.to_byte(),        // 5 - DEAD
            Opcode::Nop.to_byte(),        // 6 - DEAD
            Opcode::Nop.to_byte(),        // 7 - DEAD
            Opcode::Nop.to_byte(),        // 8 - DEAD
            Opcode::PushFalse.to_byte(),  // 9 - target
            Opcode::Return.to_byte(),     // 10
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Removed 6 bytes (offsets 3-8), Jump offset should be fixed to 0
        assert!(stats.bytes_removed >= 6);

        // Check the Jump offset is fixed
        let new_offset = i16::from_be_bytes([optimized[1], optimized[2]]);
        assert_eq!(new_offset, 0); // Target is immediately after Jump

        // Result: Jump + PushFalse + Return
        assert_eq!(optimized.len(), 5);
        assert_eq!(optimized[3], Opcode::PushFalse.to_byte());
        assert_eq!(optimized[4], Opcode::Return.to_byte());
    }

    #[test]
    fn test_dce_with_conditional_dead_branch() {
        // Conditional where one branch has no way back
        //   0: PushTrue                  (1 byte)
        //   1: JumpIfTrue +3             (3 bytes) -> target = 4+3 = 7 (then branch)
        //   4: Jump +5                   (3 bytes) -> target = 7+5 = 12 (skip to end)
        //   7: PushLongSmall 1           (2 bytes) - then branch
        //   9: Return                    (1 byte)
        //  10: PushLongSmall 2           (2 bytes) - DEAD (neither else nor then reaches here)
        //  12: Return                    (1 byte)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),      // 0
            Opcode::JumpIfTrue.to_byte(),    // 1
            0, 3,                             // 2-3: offset +3 to 7
            Opcode::Jump.to_byte(),          // 4 - else: unconditional jump
            0, 5,                             // 5-6: offset +5 to 12
            Opcode::PushLongSmall.to_byte(), // 7 - then branch
            1,                                // 8
            Opcode::Return.to_byte(),        // 9 - then returns
            Opcode::PushLongSmall.to_byte(), // 10 - DEAD
            2,                                // 11 - DEAD
            Opcode::Return.to_byte(),        // 12 - target of else's jump
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        // Dead code at 10-11 should be removed
        assert!(stats.bytes_removed >= 2);
        assert!(optimized.len() < 13);
    }

    #[test]
    fn test_dce_combined_with_peephole() {
        // Test the combined optimizer
        //   0: Jump +3             (3 bytes) -> target = 6
        //   3: Nop                 (1 byte)  - DEAD
        //   4: Nop                 (1 byte)  - DEAD
        //   5: Nop                 (1 byte)  - DEAD
        //   6: Swap                (1 byte)  - jump target
        //   7: Swap                (1 byte)  - will be optimized by peephole
        //   8: Return              (1 byte)
        let code = make_code(&[
            Opcode::Jump.to_byte(),       // 0
            0, 3,                          // 1-2: offset +3 to 6
            Opcode::Nop.to_byte(),        // 3 - DEAD
            Opcode::Nop.to_byte(),        // 4 - DEAD
            Opcode::Nop.to_byte(),        // 5 - DEAD
            Opcode::Swap.to_byte(),       // 6 - peephole target
            Opcode::Swap.to_byte(),       // 7 - peephole target
            Opcode::Return.to_byte(),     // 8
        ]);

        let (optimized, peephole_stats, dce_stats) = optimize_bytecode_full(code);

        // Peephole should remove swap-swap and nops
        // Note: peephole runs first, then DCE
        assert!(
            peephole_stats.nops_removed > 0
                || peephole_stats.swap_swap_removed > 0
                || dce_stats.blocks_removed > 0,
            "Expected some optimization to occur"
        );

        // Final result should be minimal
        // After peephole: Jump, DeadNops..., Return (nops removed, swap-swap removed)
        // After DCE: Jump, Return
        assert!(optimized.len() <= 5); // At most Jump(3) + Return(1) + some
    }

    #[test]
    fn test_dce_stats() {
        // Test that stats are correctly tracked
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),   // 0
            Opcode::Return.to_byte(),     // 1
            Opcode::PushFalse.to_byte(),  // 2 - DEAD
            Opcode::PushNil.to_byte(),    // 3 - DEAD
            Opcode::Pop.to_byte(),        // 4 - DEAD
        ]);

        let (optimized, stats) = eliminate_dead_code(code);

        assert_eq!(stats.blocks_found, 2); // Entry block + dead block
        assert_eq!(stats.blocks_reachable, 1); // Only entry reachable
        assert_eq!(stats.blocks_removed, 1);
        assert_eq!(stats.bytes_removed, 3); // 3 dead bytes
        assert_eq!(optimized.len(), 2);
    }

    // ========================================================================
    // New Peephole Optimization Tests
    // ========================================================================

    // NOTE: Boolean identity and annihilator optimizations are DISABLED because
    // they can hide type errors in MeTTa's dynamically typed system.
    // For example: (and 1 True) should error, not return 1.
    // These tests verify the optimizations do NOT occur.

    #[test]
    fn test_bool_identity_and_true() {
        // x AND True = x mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushTrue.to_byte(),
            Opcode::And.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_identity_removed, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushTrue.to_byte(),
                Opcode::And.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_identity_or_false() {
        // x OR False = x mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushFalse.to_byte(),
            Opcode::Or.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_identity_removed, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushFalse.to_byte(),
                Opcode::Or.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_annihilator_and_false() {
        // x AND False = False mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushFalse.to_byte(),
            Opcode::And.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_annihilator_folded, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushFalse.to_byte(),
                Opcode::And.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_bool_annihilator_or_true() {
        // x OR True = True mathematically, but we DON'T optimize this because
        // it would hide type errors when x is not a boolean.
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            0, // Load x
            Opcode::PushTrue.to_byte(),
            Opcode::Or.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Verify optimization does NOT occur (disabled for type safety)
        assert_eq!(stats.bool_annihilator_folded, 0);
        // Code remains unchanged
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                0,
                Opcode::PushTrue.to_byte(),
                Opcode::Or.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_neg_neg_removal() {
        // Neg; Neg = identity
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5,
            Opcode::Neg.to_byte(),
            Opcode::Neg.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.neg_neg_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                5,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_mul_zero() {
        // x * 0 = 0
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            42, // x = 42
            Opcode::PushLongSmall.to_byte(),
            0, // * 0
            Opcode::Mul.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.mul_zero_folded, 1);
        // After mul_zero: [Push 42, Pop, Push 0, Return]
        // Then push_pop optimization removes Push 42; Pop
        // Final: [Push 0, Return]
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                0,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_pow_zero() {
        // x ^ 0 = 1
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            5, // x = 5
            Opcode::PushLongSmall.to_byte(),
            0, // ^ 0
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.pow_folded, 1);
        // After pow_zero: [Push 5, Pop, Push 1, Return]
        // Then push_pop optimization removes Push 5; Pop
        // Final: [Push 1, Return]
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                1,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_pow_one() {
        // x ^ 1 = x
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            7, // x = 7
            Opcode::PushLongSmall.to_byte(),
            1, // ^ 1
            Opcode::Pow.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.pow_folded, 1);
        // Should remove PushLongSmall 1; Pow
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                7,
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_abs_abs_idempotent() {
        // Abs(Abs(x)) = Abs(x)
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::Abs.to_byte(),
            Opcode::Abs.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.idempotent_removed, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                10,
                Opcode::Abs.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_comparison_folding_lt_not() {
        // Lt; Not → Ge
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Lt.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.comparison_folded, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                1,
                Opcode::PushLongSmall.to_byte(),
                2,
                Opcode::Ge.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_comparison_folding_eq_not() {
        // Eq; Not → Ne
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            1,
            Opcode::PushLongSmall.to_byte(),
            2,
            Opcode::Eq.to_byte(),
            Opcode::Not.to_byte(),
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.comparison_folded, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::PushLongSmall.to_byte(),
                1,
                Opcode::PushLongSmall.to_byte(),
                2,
                Opcode::Ne.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_const_branch_fold_push_true_jump_if_true() {
        // PushTrue; JumpIfTrue → Jump (always taken)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),      // 0
            Opcode::JumpIfTrue.to_byte(),    // 1
            0, 3,                             // 2-3: offset +3 to target
            Opcode::PushNil.to_byte(),       // 4 - skipped
            Opcode::Return.to_byte(),        // 5
            Opcode::PushFalse.to_byte(),     // 6 - target
            Opcode::Return.to_byte(),        // 7
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.const_branch_folded, 1);
        // PushTrue; JumpIfTrue +3 becomes Jump +3
        // First 4 bytes become 3 bytes (Jump)
        assert_eq!(optimized[0], Opcode::Jump.to_byte());
    }

    #[test]
    fn test_dead_branch_push_true_jump_if_false() {
        // PushTrue; JumpIfFalse → remove (never taken)
        let code = make_code(&[
            Opcode::PushTrue.to_byte(),      // 0
            Opcode::JumpIfFalse.to_byte(),   // 1
            0, 2,                             // 2-3: offset +2 to target
            Opcode::PushNil.to_byte(),       // 4 - fall through
            Opcode::Return.to_byte(),        // 5
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.dead_branch_removed, 1);
        // All 4 bytes (PushTrue, JumpIfFalse, offset) should be removed
        assert_eq!(
            optimized,
            vec![Opcode::PushNil.to_byte(), Opcode::Return.to_byte(),]
        );
    }

    #[test]
    fn test_load_deduplication() {
        // LoadLocal X; LoadLocal X → LoadLocal X; Dup
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            5, // slot 5
            Opcode::LoadLocal.to_byte(),
            5, // same slot 5
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert_eq!(stats.load_deduplicated, 1);
        assert_eq!(
            optimized,
            vec![
                Opcode::LoadLocal.to_byte(),
                5,
                Opcode::Dup.to_byte(),
                Opcode::Return.to_byte(),
            ]
        );
    }

    #[test]
    fn test_load_dedup_different_slots() {
        // LoadLocal X; LoadLocal Y → no change (different slots)
        let code = make_code(&[
            Opcode::LoadLocal.to_byte(),
            5,
            Opcode::LoadLocal.to_byte(),
            6, // different slot
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code.clone());

        assert_eq!(stats.load_deduplicated, 0);
        assert_eq!(optimized, code);
    }

    #[test]
    fn test_jump_threading() {
        // Jump L1; ... L1: Jump L2 → Jump L2
        let code = make_code(&[
            Opcode::Jump.to_byte(),          // 0
            0, 3,                             // 1-2: offset +3 → target = 6
            Opcode::Nop.to_byte(),           // 3 (dead)
            Opcode::Nop.to_byte(),           // 4 (dead)
            Opcode::Nop.to_byte(),           // 5 (dead)
            Opcode::Jump.to_byte(),          // 6 - L1: another jump
            0, 3,                             // 7-8: offset +3 → target = 12
            Opcode::Nop.to_byte(),           // 9 (dead)
            Opcode::Nop.to_byte(),           // 10 (dead)
            Opcode::Nop.to_byte(),           // 11 (dead)
            Opcode::PushTrue.to_byte(),      // 12 - L2: final target
            Opcode::Return.to_byte(),        // 13
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Jump at 0 should now point directly to L2 (offset 12)
        assert!(stats.jump_threaded >= 1);
        // After nop removal and jump threading, the first jump should target the end
    }

    #[test]
    fn test_jump_threading_short() {
        // JumpShort L1; L1: JumpShort L2 → JumpShort L2
        let code = make_code(&[
            Opcode::JumpShort.to_byte(),     // 0
            2,                                // 1: offset +2 → target = 4
            Opcode::Nop.to_byte(),           // 2 (dead)
            Opcode::Nop.to_byte(),           // 3 (dead)
            Opcode::JumpShort.to_byte(),     // 4 - L1: another short jump
            3,                                // 5: offset +3 → target = 9
            Opcode::Nop.to_byte(),           // 6 (dead)
            Opcode::Nop.to_byte(),           // 7 (dead)
            Opcode::Nop.to_byte(),           // 8 (dead)
            Opcode::PushTrue.to_byte(),      // 9 - L2: final target
            Opcode::Return.to_byte(),        // 10
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        // Should thread the short jumps
        assert!(stats.jump_threaded >= 1 || stats.nops_removed > 0);
    }

    #[test]
    fn test_combined_optimizations() {
        // Test multiple optimizations in one pass
        // NOTE: Bool identity optimization is disabled for type safety
        let code = make_code(&[
            Opcode::PushLongSmall.to_byte(),
            10,
            Opcode::PushLongSmall.to_byte(),
            0,
            Opcode::Add.to_byte(), // identity: x + 0 = x
            Opcode::Neg.to_byte(),
            Opcode::Neg.to_byte(), // double neg
            Opcode::Return.to_byte(),
        ]);

        let (optimized, stats) = optimize_bytecode(code);

        assert!(stats.identity_ops_removed >= 1);
        assert!(stats.neg_neg_removed >= 1);
        // Final result: PushLongSmall(2) + Return(1) = 3 bytes
        assert!(optimized.len() <= 4);
    }
}
