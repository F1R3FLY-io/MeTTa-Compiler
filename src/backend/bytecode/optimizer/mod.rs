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

mod dce;
mod helpers;
mod peephole;
mod types;

#[cfg(test)]
mod tests;

// Re-export public types
pub use dce::{eliminate_dead_code, DeadCodeEliminator};
pub use peephole::{optimize_bytecode, PeepholeOptimizer};
pub use types::{DceStats, OptimizationStats, PeepholeAction};

// Re-export helpers for internal use
pub(crate) use helpers::instruction_size;

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
