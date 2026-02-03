//! Trampoline-based Iterative Evaluation
//!
//! This module provides the core data structures and engine for iterative evaluation
//! using an explicit work stack instead of recursive function calls.
//! This approach prevents stack overflow for deeply nested expressions.
//!
//! ## Generic Evaluation
//!
//! The evaluation engine is generic over the allocation strategy via the `EvalContext`
//! trait. This enables writing evaluation logic once that works with both:
//! - `HeapContext`: Standard heap-allocated MettaValue (Arc-wrapped)
//! - `StaticArenaContext`: Arena-allocated ArenaValue with 'static lifetime
//!
//! ## Unified Generic Engine
//!
//! The `generic_trampoline` module provides a truly generic engine that uses
//! `GenericWorkItem<V>` and `GenericContinuation<V>`, enabling the same evaluation
//! logic to work with both heap and arena allocation strategies.
//!
//! ## Entry Points
//!
//! - `eval_trampoline`: Heap-based evaluation (MettaValue → MettaValue)
//! - `eval_trampoline_arena`: Arena-based evaluation (ArenaValue<'static> → ArenaValue<'static>)
//!
//! ## Zero-Conversion Architecture
//!
//! When `METTA_USE_ARENA=1`, the entire evaluation pipeline uses ArenaValue<'static>:
//!
//! ```text
//! compile_arena() → ArenaValue<'static> → eval_trampoline_arena() → ArenaValue<'static>
//! ```
//!
//! When unset (default), the entire pipeline uses MettaValue:
//!
//! ```text
//! compile() → MettaValue → eval_trampoline() → MettaValue
//! ```
//!
//! No conversions between value types are performed in either mode.

mod arena_engine;
mod context;
mod engine;
mod generic_engine;
mod generic_trampoline;
mod generic_types;
mod types;

// Primary entry points
pub use arena_engine::{
    create_arena_context, eval_trampoline_arena, get_static_arena, get_static_factory,
    is_arena_mode_available, ArenaEvalResult,
};
pub use engine::eval_trampoline;

// Re-export heap-based types (for backward compatibility)
#[allow(unused_imports)]
pub use types::{Continuation, WorkItem, MAX_EVAL_DEPTH};

// Re-export evaluation context types
#[allow(unused_imports)]
pub use context::{
    ArenaContext, ArenaEnvironment, ContextEnv, EvalContext, HeapContext, StaticArenaContext,
    is_arena_mode_enabled,
};

// Re-export generic types for the unified engine
#[allow(unused_imports)]
pub use generic_types::{
    GenericContinuation, GenericEvalResult, GenericWorkItem, HeapContinuation, HeapEvalResult,
    HeapWorkItem,
};

// Re-export generic engine functions (zero-conversion evaluation)
pub use generic_engine::{
    apply_bindings_generic, pattern_match_generic, pattern_specificity_generic,
    try_match_all_rules_generic,
};

// Re-export the unified generic trampoline engine
#[allow(unused_imports)]
pub use generic_trampoline::eval_trampoline_generic;
