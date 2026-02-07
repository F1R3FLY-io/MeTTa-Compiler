//! Trampoline-based Iterative Evaluation
//!
//! This module provides the core data structures and engine for iterative evaluation
//! using an explicit work stack instead of recursive function calls.
//! This approach prevents stack overflow for deeply nested expressions.
//!
//! ## Generic Evaluation
//!
//! The evaluation engine is generic over the allocation strategy via the `EvalContext`
//! trait. The production implementation uses `StaticArenaContext` with arena-allocated
//! `ArenaValue<'static>` values.
//!
//! ## Unified Generic Engine
//!
//! The `generic_trampoline` module provides a truly generic engine that uses
//! `GenericWorkItem<V>` and `GenericContinuation<V>`, enabling the same evaluation
//! logic to work with any `EvalContext` implementation.
//!
//! ## Entry Points
//!
//! - `eval_trampoline_arena`: Arena-based evaluation (ArenaValue<'static> → ArenaValue<'static>)
//! - `eval_trampoline_generic`: Generic evaluation for any `EvalContext`
//!
//! ## Zero-Conversion Architecture
//!
//! The entire evaluation pipeline uses ArenaValue<'static>:
//!
//! ```text
//! compile_arena() → ArenaValue<'static> → eval_trampoline_arena() → ArenaValue<'static>
//! ```
//!
//! No conversions between value types are performed.

mod arena_engine;
mod context;
mod generic_engine;
mod generic_trampoline;
mod generic_types;
pub mod session_context;

// Primary entry points
pub use arena_engine::{
    eval_trampoline_arena, get_static_arena, get_static_factory,
    is_arena_mode_available, new_arena_env, ArenaEvalResult,
};

// Re-export evaluation context types
#[allow(unused_imports)]
pub use context::{
    ArenaEnvironment, ContextEnv, EvalContext, StaticArenaContext,
};

// Re-export generic types for the unified engine
#[allow(unused_imports)]
pub use generic_types::{
    GenericContinuation, GenericEvalResult, GenericWorkItem,
};

// Re-export generic engine functions (zero-conversion evaluation)
pub use generic_engine::{
    apply_bindings_generic, pattern_match_generic, pattern_specificity_generic,
    try_match_all_rules_generic,
};

// Re-export the unified generic trampoline engine
#[allow(unused_imports)]
pub use generic_trampoline::eval_trampoline_generic;

// Re-export session context (dual-arena model)
pub use session_context::SessionContext;
