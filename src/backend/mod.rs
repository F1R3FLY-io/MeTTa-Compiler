// Backend module for MeTTa evaluation with Rust-based eval
//
// This module provides the new architecture where:
// - `compile`: MeTTa text → PathMap [parsed_sexprs, fact_db]
// - `eval`: Lazy evaluation with direct dispatch to Rholang interpreter built-ins
// - `run`: PathMap method to execute s-expressions (will be in Rholang)

pub mod types;
pub mod compile;
pub mod eval;

pub use types::*;
pub use compile::compile;
pub use eval::eval;
