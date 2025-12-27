//! Trampoline-based Iterative Evaluation
//!
//! This module provides the core data structures and engine for iterative evaluation
//! using an explicit work stack instead of recursive function calls.
//! This approach prevents stack overflow for deeply nested expressions.

mod engine;
mod types;

pub use engine::eval_trampoline;
pub use types::{Continuation, WorkItem, MAX_EVAL_DEPTH};
