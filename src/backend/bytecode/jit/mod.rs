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
//! - [`compiler`]: Bytecode-to-Cranelift IR translation
//! - [`codegen`]: Cranelift IR generation helpers
//! - [`runtime`]: Runtime support functions callable from JIT code
//! - [`tiered`]: Tiered compilation strategy and JIT cache management

pub mod types;
pub mod profile;
pub mod compiler;
pub mod codegen;
pub mod runtime;
pub mod tiered;
pub mod hybrid;
pub mod handlers;

// Re-export main types
pub use types::{
    JitValue, JitContext, JitResult, JitError, JitBailoutReason,
    JitChoicePoint, JitAlternative, JitAlternativeTag,
    // Binding/Environment support (Phase A)
    JitBindingEntry, JitBindingFrame,
    // Stage 2 JIT signal constants
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR, JIT_SIGNAL_HALT,
    JIT_SIGNAL_BAILOUT,
    // Optimization 5.2: Pre-allocation constants
    MAX_ALTERNATIVES_INLINE, STACK_SAVE_POOL_SIZE, MAX_STACK_SAVE_VALUES,
    // Optimization 5.3: Variable index cache
    VAR_INDEX_CACHE_SIZE,
};
// Space Ops Phase 4: Binding forking for nondeterminism
pub use runtime::JitSavedBindings;
pub use profile::{JitProfile, JitState, HOT_THRESHOLD};
pub use compiler::JitCompiler;
pub use tiered::{Tier, TieredCompiler, TieredStats, JitCache, ChunkId, CacheEntry, STAGE2_THRESHOLD};
pub use hybrid::{HybridExecutor, HybridConfig, HybridStats};

/// Feature flag for JIT compilation
/// When enabled, hot bytecode paths are compiled to native code
#[cfg(feature = "jit")]
pub const JIT_ENABLED: bool = true;

#[cfg(not(feature = "jit"))]
pub const JIT_ENABLED: bool = false;
