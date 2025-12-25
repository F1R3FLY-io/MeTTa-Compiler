// Backend module for MeTTa evaluation with Rust-based eval
//
// This module provides the new architecture where:
// - `compile`: MeTTa text → PathMap [parsed_sexprs, fact_db]
// - `eval`: Lazy evaluation with direct dispatch to Rholang interpreter built-ins
// - `run`: PathMap method to execute s-expressions (will be in Rholang)
// - `bytecode`: Stack-based bytecode VM for faster execution (WIP)

pub mod builtin_signatures;
pub mod bytecode;
pub mod compile;
pub mod environment;
pub mod eval;
pub mod fuzzy_match;
pub mod grounded;
pub mod models;
pub mod modules;
pub mod mork_convert;
pub mod priority_scheduler;
pub mod symbol;
pub mod thread_pool;
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
pub use priority_scheduler::{
    global_priority_eval_pool, priority_levels, P2MedianEstimator, PriorityEvalThreadPool,
    PriorityPoolStats, RuntimeTracker, SchedulerConfig, TaskTypeId,
};
pub use symbol::{intern, intern_string, Symbol};
pub use thread_pool::global_eval_pool;
