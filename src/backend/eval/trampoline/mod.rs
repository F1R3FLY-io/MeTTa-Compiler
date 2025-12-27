//! Trampoline-based Iterative Evaluation
//!
//! This module provides the core data structures and engine for iterative evaluation
//! using an explicit work stack instead of recursive function calls.
//! This approach prevents stack overflow for deeply nested expressions.

mod types;
mod engine;

pub use types::{Continuation, WorkItem, MAX_EVAL_DEPTH};
pub use engine::eval_trampoline;
