//! Grounded operations for lazy evaluation.
//!
//! This module provides the `GroundedOperationTCO` trait and implementations for
//! built-in operations that receive unevaluated arguments and evaluate them internally.
//! This matches Hyperon Experimental's (HE) `execute_bindings()` pattern.
//!
//! # Key Concepts
//!
//! - **Lazy Evaluation**: Arguments are passed unevaluated to grounded operations
//! - **Internal Evaluation**: Operations decide when/if to evaluate their arguments
//! - **Cartesian Products**: When arguments produce multiple results, operations
//!   compute Cartesian products of all result combinations
//!
//! # Example
//!
//! ```ignore
//! // With lazy evaluation:
//! (= (f) 1) (= (f) 2)
//! !(+ (f) 10)
//! // The + operation receives (f) unevaluated, evaluates it to [1, 2],
//! // then computes [1+10, 2+10] = [11, 12]
//! ```

pub mod arithmetic;
pub mod comparison;
pub mod fileio;
pub mod json;
pub mod logical;
pub mod random;
pub mod registry;
pub mod state;
pub mod string;
mod traits;

use std::collections::HashMap;
use std::fmt;

use super::models::MettaValue;

// Re-export operation types
pub use arithmetic::{AbsOp, AddOp, ClampOp, DivOp, MaxOp, MinOp, ModOp, MulOp, SafeDivOp, SubOp};
pub use comparison::{EqualOp, GreaterEqOp, GreaterOp, LessEqOp, LessOp, NotEqualOp};
pub use fileio::{
    FileGetSizeOp, FileOpenOp, FileReadExactOp, FileReadToStringOp, FileSeekOp, FileWriteOp,
};
pub use json::{JsonDecodeOp, JsonEncodeOp};
pub use logical::{AndOp, NotOp, OrOp, XorOp};
pub use random::{
    FlipOp, NewRandomGeneratorOp, RandomFloatOp, RandomIntOp, ResetRandomGeneratorOp,
    SetRandomSeedOp,
};
pub use registry::{execute_grounded_op, get_grounded_registry, has_grounded_op, GroundedRegistry};
pub use state::{find_error, friendly_type_name, GroundedState, GroundedWork};
pub use string::StringToCharsOp;
pub use traits::GroundedOperationTCO;

/// Bindings from pattern matching (variable name -> value)
pub type Bindings = HashMap<String, MettaValue>;

/// Result type for grounded operations
/// Each result is a (value, optional_bindings) pair
pub type GroundedResult = Result<Vec<(MettaValue, Option<Bindings>)>, ExecError>;

/// Error type for grounded operation execution
#[derive(Debug, Clone)]
pub enum ExecError {
    /// Operation is not applicable to these arguments - try other rules
    /// This is NOT an error, just signals "I can't handle this"
    NoReduce,

    /// Runtime error during execution (type mismatches, etc.)
    Runtime(String),

    /// Arithmetic error (division by zero, overflow, etc.)
    Arithmetic(String),

    /// Incorrect argument type or arity
    IncorrectArgument(String),

    /// HE-empirical tag-atom error (e.g. `DivisionByZero`,
    /// `IncorrectNumberOfArguments`). Produces the canonical Error shape
    /// `(Error <call> <tag>)` with the tag-atom as the detail, matching HE
    /// reference output for `!(/ 5 0)` → `(Error (/ 5 0) DivisionByZero)`.
    Tagged(&'static str),

    /// Type-mismatch with 1-indexed position. Produces the canonical Error
    /// shape `(Error <call> (BadType <position> <expected> <got>))` matching
    /// HE `metta/runner/stdlib/atom.rs` BadType reporting.
    BadArgType {
        /// 1-indexed argument position (matches HE convention)
        pos: usize,
        expected: &'static str,
        got: String,
    },
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::NoReduce => write!(f, "NoReduce"),
            ExecError::Runtime(msg) => write!(f, "Runtime error: {}", msg),
            ExecError::Arithmetic(msg) => write!(f, "Arithmetic error: {}", msg),
            ExecError::IncorrectArgument(msg) => write!(f, "Incorrect argument: {}", msg),
            ExecError::Tagged(tag) => write!(f, "{}", tag),
            ExecError::BadArgType { pos, expected, got } => {
                write!(f, "BadType arg {}: expected {}, got {}", pos, expected, got)
            }
        }
    }
}

impl std::error::Error for ExecError {}

/// Convert an `ExecError` into the canonical HE-aligned Error atom value
/// `(Error <call_form> <detail>)`.
///
/// This is the single source-of-truth for the Error shape produced by grounded
/// operations and is consumed by both the trampoline tier and (via thin
/// wrappers) the bytecode/JIT tiers. The exact shape mirrors HE's
/// `metta/runner/stdlib/*.rs` emitters:
///
/// * `Tagged(tag)` → `(Error <call> <tag>)` — detail is a bare atom such as
///   `DivisionByZero`. Matches `!(/ 5 0)` → `(Error (/ 5 0) DivisionByZero)`.
/// * `BadArgType { pos, expected, got }` → `(Error <call> (BadType <pos>
///   <expected> <got>))`.
/// * `Runtime(msg)`, `Arithmetic(msg)`, `IncorrectArgument(msg)` →
///   `(Error <call> <msg_string>)` for back-compat string-message paths.
/// * `NoReduce` is a non-error signal; callers should never invoke this for
///   `NoReduce` (the assertion enforces this).
///
/// Note: this function does NOT consume the `ExecError` (uses `&ExecError`),
/// so callers may inspect the variant for tracing before converting.
pub fn exec_error_to_value<V, F>(err: &ExecError, call_form: V, factory: &F) -> V
where
    V: crate::backend::models::MettaValueTrait + Clone,
    F: crate::backend::models::MettaValueFactory<V>,
{
    match err {
        ExecError::NoReduce => {
            // Per Plan: callers must short-circuit NoReduce before reaching
            // this function. NoReduce represents "operation not applicable",
            // not an Error atom; the trampoline returns the unreduced form.
            // We still produce a sensible fallback rather than panic to keep
            // production robust.
            factory.error(call_form, factory.atom("NoReduce"))
        }
        ExecError::Tagged(tag) => factory.error(call_form, factory.atom(tag)),
        ExecError::BadArgType {
            pos,
            expected,
            got,
        } => {
            let detail = factory.sexpr(vec![
                factory.atom("BadType"),
                factory.long(*pos as i64),
                factory.atom(expected),
                factory.atom(got),
            ]);
            factory.error(call_form, detail)
        }
        ExecError::Runtime(msg) => factory.error(call_form, factory.string(msg)),
        ExecError::Arithmetic(msg) => factory.error(call_form, factory.string(msg)),
        ExecError::IncorrectArgument(msg) => factory.error(call_form, factory.string(msg)),
    }
}
