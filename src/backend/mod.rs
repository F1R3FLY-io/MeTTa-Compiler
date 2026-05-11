// Backend module for MeTTa evaluation with Rust-based eval
//
// This module provides the new architecture where:
// - `compile`: MeTTa text → PathMap [parsed_sexprs, fact_db]
// - `eval`: Lazy evaluation with direct dispatch to Rholang interpreter built-ins
// - `run`: PathMap method to execute s-expressions (will be in Rholang)
// - `bytecode`: Stack-based bytecode VM for faster execution (WIP)

pub mod analysis;
pub mod builtin_signatures;
pub mod bytecode;
pub mod compile;
pub mod diagnostics;
pub mod environment;
pub mod eval;
pub mod fuzzy_match;
pub mod grounded;
pub(crate) mod hash_utils;
pub mod interrupt;
pub mod literal_classifier;
pub mod models;
pub mod modules;
pub mod mork_convert;
pub mod priority_scheduler;
pub mod scheduler;
pub mod symbol;
pub mod varint_encoding;
pub mod wide_mork;

#[cfg(feature = "trace")]
pub mod trace;

pub use builtin_signatures::{
    get_arg_types, get_return_type, get_signature, is_builtin, BuiltinSignature, TypeExpr,
};
pub use compile::{compile, compile_generic, compile_with_path};
pub use environment::rule_management::activate_analysis;
pub use environment::{GenericEnvironment, MettaEnvironment, ScopeTracker};
#[cfg(feature = "trace")]
pub use eval::eval_with_trace;
pub use eval::trampoline::{
    eval_trampoline,
    get_static_factory,
    new_env,
    // Session-based evaluation context
    SessionContext,
    StaticEvalContext,
};
pub use eval::EvalResult;
pub use eval::{eval, pattern_match};
pub use fuzzy_match::FuzzyMatcher;
pub use grounded::ExecError;
pub use models::*;
pub use priority_scheduler::{
    priority_levels, P2MedianEstimator, PriorityPoolStats, PriorityQueue, RuntimeTracker,
    SchedulerConfig, TaskTypeId,
};
pub use symbol::{intern, intern_string, Symbol};
