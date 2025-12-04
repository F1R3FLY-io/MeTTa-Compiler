// Backend module for MeTTa evaluation with Rust-based eval
//
// This module provides the new architecture where:
// - `compile`: MeTTa text → PathMap [parsed_sexprs, fact_db]
// - `eval`: Lazy evaluation with direct dispatch to Rholang interpreter built-ins
// - `run`: PathMap method to execute s-expressions (will be in Rholang)

pub mod builtin_signatures;
pub mod compile;
pub mod environment;
pub mod eval;
pub mod fuzzy_match;
pub mod grounded;
pub mod models;
pub mod modules;
pub mod mork_convert;
pub mod symbol;
pub mod varint_encoding;

pub use builtin_signatures::{
    get_arg_types, get_return_type, get_signature, is_builtin, BuiltinSignature, TypeExpr,
};
pub use compile::{compile, compile_with_path};
pub use environment::{Environment, ScopeTracker};
pub use eval::{eval, pattern_match};
pub use fuzzy_match::FuzzyMatcher;
pub use grounded::{ExecError, GroundedOperation, GroundedRegistry, GroundedResult};
pub use models::*;
pub use symbol::{intern, intern_string, Symbol};
