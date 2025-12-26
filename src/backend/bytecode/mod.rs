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
//! - `chunk`: Compiled bytecode container
//! - `compiler`: MeTTa to bytecode compiler

pub mod chunk;
pub mod compiler;
pub mod jit;
pub mod opcodes;
pub mod optimizer;

pub use chunk::{BytecodeChunk, ChunkBuilder};
pub use compiler::{CompileContext, CompileError, CompileResult, Compiler, Upvalue};
pub use jit::{
    JitBindingEntry, JitBindingFrame, JitClosure, JitContext, JitError, JitResult, JitValue,
};
pub use opcodes::Opcode;
pub use optimizer::{
    optimize_bytecode_full, DeadCodeEliminator, OptimizationStats, PeepholeOptimizer,
};
