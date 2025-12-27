//! Types and statistics for bytecode optimization.

use crate::backend::bytecode::opcodes::Opcode;

/// Result of a peephole scan at a given position
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeepholeAction {
    /// Keep the instruction(s) unchanged
    Keep,
    /// Remove bytes from `start` to `end` (exclusive)
    Remove { start: usize, end: usize },
    /// Replace bytes from `start` to `end` with a single opcode
    ReplaceWithOpcode {
        start: usize,
        end: usize,
        opcode: Opcode,
    },
    /// Replace bytes from `start` to `end` with multiple bytes
    ReplaceWithBytes {
        start: usize,
        end: usize,
        bytes: Vec<u8>,
    },
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
