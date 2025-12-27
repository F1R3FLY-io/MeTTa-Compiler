//! Dead Code Elimination for the bytecode optimizer.
//!
//! This module implements dead code elimination by:
//! 1. Building a control flow graph (CFG) from bytecode
//! 2. Marking all blocks reachable from the entry point
//! 3. Removing unreachable blocks
#![allow(clippy::needless_range_loop)]

use std::collections::{HashSet, VecDeque};

use crate::backend::bytecode::opcodes::Opcode;

use super::helpers::instruction_size;
use super::types::DceStats;

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
                Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpIfNil
                | Opcode::JumpIfError => {
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
            if matches!(
                opcode,
                Opcode::Jump
                    | Opcode::JumpShort
                    | Opcode::Return
                    | Opcode::ReturnMulti
                    | Opcode::Halt
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
                    Opcode::JumpIfFalse
                    | Opcode::JumpIfTrue
                    | Opcode::JumpIfNil
                    | Opcode::JumpIfError => {
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
                Opcode::Jump
                | Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpIfNil
                | Opcode::JumpIfError => {
                    if offset + 2 < code.len() {
                        let old_jump_offset =
                            i16::from_be_bytes([code[offset + 1], code[offset + 2]]);

                        // Find original position of this instruction
                        let old_instr_pos =
                            self.reverse_offset_dce(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 3;
                        let old_target =
                            (old_jump_from as isize + old_jump_offset as isize) as usize;

                        if old_target <= original_len {
                            // Calculate new positions
                            let new_jump_from = offset + 3;
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

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

                        let old_instr_pos =
                            self.reverse_offset_dce(offset, offset_map, original_len);
                        let old_jump_from = old_instr_pos + 2;
                        let old_target =
                            (old_jump_from as isize + old_jump_offset as isize) as usize;

                        if old_target <= original_len {
                            let new_jump_from = offset + 2;
                            let target_delta = offset_map.get(old_target).copied().unwrap_or(0);
                            let new_target = (old_target as isize + target_delta) as usize;

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

    /// Reverse lookup for DCE offset map
    fn reverse_offset_dce(
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
