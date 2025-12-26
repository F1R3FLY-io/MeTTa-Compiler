//! Bytecode compilation and execution for MeTTa
//!
//! This module provides a bytecode representation for MeTTa expressions,
//! enabling faster execution through a virtual machine interpreter.
//!
//! # Module Structure
//!
//! - `opcodes`: Bytecode instruction definitions
//! - `optimizer`: Peephole optimization and dead code elimination
//! - `jit`: Just-in-time compilation infrastructure

pub mod jit;
pub mod opcodes;
pub mod optimizer;

pub use jit::{
    JitBindingEntry, JitBindingFrame, JitClosure, JitContext, JitError, JitResult, JitValue,
};
pub use opcodes::Opcode;
pub use optimizer::{
    optimize_bytecode_full, DeadCodeEliminator, OptimizationStats, PeepholeOptimizer,
};
