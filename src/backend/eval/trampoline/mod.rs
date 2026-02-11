//! Trampoline-based Iterative Evaluation
//!
//! This module provides the core data structures and engine for iterative evaluation
//! using an explicit work stack instead of recursive function calls.
//! This approach prevents stack overflow for deeply nested expressions.
//!
//! ## Generic Evaluation
//!
//! The evaluation engine is generic over the allocation strategy via the `EvalContext`
//! trait. The production implementation uses `StaticEvalContext` with arena-allocated
//! `MettaValue` values.
//!
//! ## Unified Generic Engine
//!
//! The `generic_trampoline` module provides a truly generic engine that uses
//! `GenericWorkItem<V>` and `GenericContinuation<V>`, enabling the same evaluation
//! logic to work with any `EvalContext` implementation.
//!
//! ## Entry Points
//!
//! - `eval_trampoline`: Arena-based evaluation (MettaValue → MettaValue)
//! - `eval_trampoline_generic`: Generic evaluation for any `EvalContext`
//!
//! ## Zero-Conversion Architecture
//!
//! The entire evaluation pipeline uses MettaValue:
//!
//! ```text
//! compile() → MettaValue → eval_trampoline() → MettaValue
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
    eval_trampoline, get_static_factory,
    is_arena_mode_available, new_env, EvalResult,
};

// Re-export evaluation context types
#[allow(unused_imports)]
pub use context::{
    MettaEnvironment, ContextEnv, EvalContext, StaticEvalContext,
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
