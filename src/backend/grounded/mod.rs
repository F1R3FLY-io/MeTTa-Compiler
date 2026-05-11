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
pub mod logical;
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
pub use logical::{AndOp, NotOp, OrOp, XorOp};
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
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::NoReduce => write!(f, "NoReduce"),
            ExecError::Runtime(msg) => write!(f, "Runtime error: {}", msg),
            ExecError::Arithmetic(msg) => write!(f, "Arithmetic error: {}", msg),
            ExecError::IncorrectArgument(msg) => write!(f, "Incorrect argument: {}", msg),
        }
    }
}

impl std::error::Error for ExecError {}
