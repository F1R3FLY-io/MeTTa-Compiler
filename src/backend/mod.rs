// Backend module for MeTTa evaluation with Rust-based eval
//
// This module provides the new architecture where:
// - `compile`: MeTTa text → PathMap [parsed_sexprs, fact_db]
// - `eval`: Lazy evaluation with direct dispatch to Rholang interpreter built-ins
// - `run`: PathMap method to execute s-expressions (will be in Rholang)

pub mod compile;
pub mod environment;
pub mod eval;
pub mod fuzzy_match;
pub mod models;
pub mod mork_convert;

pub use compile::compile;
pub use environment::Environment;
pub use eval::{eval, pattern_match};
pub use fuzzy_match::FuzzyMatcher;
pub use models::*;
