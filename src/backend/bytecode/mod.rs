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
//! - `memo_cache`: Thread-safe memoization cache
//! - `native_registry`: Native function registry
//! - `external_registry`: External function registry
//! - `cache`: Bytecode compilation cache with LRU eviction
//! - `mork_bridge`: Bridge to MORK for rule dispatch

pub mod cache;
pub mod chunk;
pub mod compiler;
pub mod external_registry;
pub mod jit;
pub mod memo_cache;
pub mod mork_bridge;
pub mod native_registry;
pub mod opcodes;
pub mod optimizer;

pub use cache::{cache_sizes, clear_caches, get_stats as cache_stats, BytecodeCacheStats};
pub use chunk::{BytecodeChunk, ChunkBuilder};
pub use compiler::{CompileContext, CompileError, CompileResult, Compiler, Upvalue};
pub use external_registry::{
    ExternalContext, ExternalError, ExternalFn, ExternalRegistry, ExternalResult,
};
pub use jit::{
    JitBindingEntry, JitBindingFrame, JitClosure, JitContext, JitError, JitResult, JitValue,
};
pub use memo_cache::{CacheStats, MemoCache};
pub use mork_bridge::{BridgeStats, CompiledRule, MorkBridge};
pub use native_registry::{NativeContext, NativeError, NativeFn, NativeRegistry, NativeResult};
pub use opcodes::Opcode;
pub use optimizer::{
    optimize_bytecode_full, DeadCodeEliminator, OptimizationStats, PeepholeOptimizer,
};
