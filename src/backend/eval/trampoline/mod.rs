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
//! The `eval_loop` module provides a truly generic engine that uses
//! `WorkItem<V>` and `Continuation<V>`, enabling the same evaluation
//! logic to work with any `EvalContext` implementation.
//!
//! ## Entry Points
//!
//! - `eval_trampoline`: Arena-based evaluation (MettaValue → MettaValue)
//! - `eval_trampoline`: Generic evaluation for any `EvalContext`
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
pub(crate) mod dispatch_hints;
pub mod engine;
pub(crate) mod eval_loop;
pub mod session_context;
pub(crate) mod types;
pub mod unification;

// Primary entry points
pub use arena_engine::{eval_trampoline, get_static_factory, is_arena_mode_available, new_env};

#[cfg(feature = "trace")]
pub use arena_engine::eval_trampoline_with_trace;

// Re-export evaluation context types
#[allow(unused_imports)]
pub use context::{EvalContext, MettaEnvironment, StaticEvalContext};

// Re-export trampoline types
#[allow(unused_imports)]
pub use types::{Continuation, EvalResult, WorkItem};

// Re-export engine functions
pub use engine::{
    apply_bindings, pattern_match, try_deterministic_chain, try_match_all_rules,
    try_match_all_rules_with_outer,
};

// Internal: eval_loop::eval_trampoline(value, env, &ctx) is used by
// arena_engine and other entry points. Not re-exported — callers use
// arena_engine::eval_trampoline which wraps it with SessionContext.

// Phase 9.5: Normal-form memoization (check, insert, invalidate)
pub use dispatch_hints::{
    clear_normal_form_memo_for_new_query, invalidate_normal_form_memo, is_memoized_normal_form,
    memoize_normal_form,
};

// Expression-level eval memoization (clear on space mutation)
pub use dispatch_hints::clear_eval_memo;

// Match result cache (clear on space mutation)
pub use dispatch_hints::clear_match_result_cache;

// Re-export session context (dual-arena model)
pub use session_context::SessionContext;
