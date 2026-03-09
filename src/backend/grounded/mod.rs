//! Grounded operations for lazy evaluation.
//!
//! This module provides the `GroundedOperation` trait and implementations for
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

mod arithmetic;
mod comparison;
pub mod generic_arithmetic;
pub mod generic_comparison;
pub mod generic_logical;
pub mod generic_registry;
pub mod generic_state;
pub mod generic_traits;
mod logical;
mod state;
#[cfg(test)]
mod tests;
mod traits;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::environment::MettaEnvironment;
use super::models::{MettaValue, ValueView};

// Re-export generic types (active code path)
pub use generic_arithmetic::{
    AddOpGeneric, ClampOpGeneric, DivOpGeneric, ModOpGeneric, MulOpGeneric, SafeDivOpGeneric,
    SubOpGeneric,
};
pub use generic_comparison::{
    EqualOpGeneric, GreaterEqOpGeneric, GreaterOpGeneric, LessEqOpGeneric, LessOpGeneric,
    NotEqualOpGeneric,
};
pub use generic_logical::{AndOpGeneric, NotOpGeneric, OrOpGeneric, XorOpGeneric};
pub use generic_registry::{
    execute_generic_grounded_op, get_generic_registry, has_generic_grounded_op,
    GenericGroundedRegistry,
};
pub use generic_state::{
    find_error_generic, friendly_type_name_generic, GenericGroundedState, GenericGroundedWork,
};
pub use generic_traits::GenericGroundedOperationTCO;

// Re-export legacy types (used by proptests for multi-tier correctness verification)
pub use arithmetic::{AddOp, DivOp, ModOp, MulOp, SubOp};
pub use comparison::{EqualOp, GreaterEqOp, GreaterOp, LessEqOp, LessOp, NotEqualOp};
pub use logical::{AndOp, NotOp, OrOp};
pub use state::{GroundedState, GroundedWork};
pub use traits::{EvalFn, GroundedOperation, GroundedOperationTCO};

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

/// Check if any result is an error and return it if so
/// Used for error propagation through grounded operations
pub(crate) fn find_error(results: &[MettaValue]) -> Option<&MettaValue> {
    results.iter().find(|v| v.is_error())
}

/// Helper function to get a friendly type name for error messages
pub(crate) fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value.view() {
        ValueView::Long(_) => "Number (integer)",
        ValueView::Float(_) => "Number (float)",
        ValueView::Bool(_) => "Bool",
        ValueView::Unit => "Expression",
        ValueView::Empty => "Empty",
        ValueView::String(_) => "String",
        ValueView::Atom(_) => "Symbol",
        ValueView::SExpr(_) => "Expression",
        ValueView::Error(_, _) => "Error",
        ValueView::Type(_) => "Type",
        ValueView::Conjunction(_) => "Conjunction",
        ValueView::Space(_) => "Space",
        ValueView::State(_) => "State",
        ValueView::Quoted(_) => "Quoted expression",
        ValueView::Memo(_) => "Memo",
    }
}

/// Registry of grounded operations, keyed by name.
///
/// Legacy: Only used by proptests for multi-tier correctness verification.
/// Active evaluation uses `GenericGroundedRegistry` with static dispatch.
pub struct GroundedRegistry {
    operations: HashMap<String, Arc<dyn GroundedOperation>>,
}

impl GroundedRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        GroundedRegistry {
            operations: HashMap::new(),
        }
    }

    /// Register a grounded operation
    pub fn register(&mut self, op: Arc<dyn GroundedOperation>) {
        self.operations.insert(op.name().to_string(), op);
    }

    /// Look up a grounded operation by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn GroundedOperation>> {
        self.operations.get(name).cloned()
    }
}

impl Default for GroundedRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for GroundedRegistry {
    fn clone(&self) -> Self {
        GroundedRegistry {
            operations: self.operations.clone(),
        }
    }
}
