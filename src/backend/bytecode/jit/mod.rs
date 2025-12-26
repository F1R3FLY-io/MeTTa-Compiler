//! Cranelift JIT Compilation Module
//!
//! This module provides JIT compilation for hot bytecode paths using Cranelift.
//! It implements a multi-tier execution strategy for optimal performance:
//!
//! ```text
//! Tier 0: Tree-walking interpreter (cold code, < 10 executions)
//! Tier 1: Bytecode VM (warm code, 10-99 executions, 40-750x speedup)
//! Tier 2: JIT Stage 1 (hot code, 100-499 executions, arithmetic/boolean native)
//! Tier 3: JIT Stage 2 (very hot code, 500+ executions, full native with runtime calls)
//! ```
//!
//! # Architecture
//!
//! The JIT uses NaN-boxing for efficient value representation and type checking.
//! Hot bytecode chunks are compiled to native code after reaching execution
//! thresholds (100 for Stage 1, 500 for Stage 2).
//!
//! # Modules
//!
//! - [`types`]: JitValue (NaN-boxed), JitContext, JitResult
//! - [`profile`]: Hotness tracking and compilation triggering
//! - [`compiler`]: Bytecode-to-Cranelift IR translation (not yet available)
//! - [`codegen`]: Cranelift IR generation helpers (not yet available)
//! - [`runtime`]: Runtime support functions callable from JIT code (not yet available)
//! - [`tiered`]: Tiered compilation strategy and JIT cache management (not yet available)

pub mod types;
pub mod profile;

// Re-export main types
pub use types::{
    JitValue, JitContext, JitResult, JitError, JitBailoutReason,
    JitChoicePoint, JitAlternative, JitAlternativeTag,
    // Binding/Environment support (Phase A)
    JitBindingEntry, JitBindingFrame,
    // Lambda closure support
    JitClosure,
    // Stage 2 JIT signal constants
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR, JIT_SIGNAL_HALT,
    JIT_SIGNAL_BAILOUT,
    // Optimization 5.2: Pre-allocation constants
    MAX_ALTERNATIVES_INLINE, STACK_SAVE_POOL_SIZE, MAX_STACK_SAVE_VALUES,
    // Optimization 5.3: Variable index cache
    VAR_INDEX_CACHE_SIZE,
};
pub use profile::{JitProfile, JitState, HOT_THRESHOLD};

/// JIT compilation is always enabled with tiered compilation
pub const JIT_ENABLED: bool = true;
