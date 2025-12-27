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
mod arithmetic_tco;
mod comparison;
mod comparison_tco;
mod logical;
mod logical_tco;
mod state;
#[cfg(test)]
mod tests;
mod traits;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::environment::Environment;
use super::models::MettaValue;

// Re-export all public types
pub use arithmetic::{AddOp, DivOp, ModOp, MulOp, SubOp};
pub use arithmetic_tco::{AddOpTCO, DivOpTCO, ModOpTCO, MulOpTCO, SubOpTCO};
pub use comparison::{EqualOp, GreaterEqOp, GreaterOp, LessEqOp, LessOp, NotEqualOp};
pub use comparison_tco::{
    EqualOpTCO, GreaterEqOpTCO, GreaterOpTCO, LessEqOpTCO, LessOpTCO, NotEqualOpTCO,
};
pub use logical::{AndOp, NotOp, OrOp};
pub use logical_tco::{AndOpTCO, NotOpTCO, OrOpTCO};
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
    results
        .iter()
        .find(|v| matches!(v, MettaValue::Error(_, _)))
}

/// Helper function to get a friendly type name for error messages
pub(crate) fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value {
        MettaValue::Long(_) => "Number (integer)",
        MettaValue::Float(_) => "Number (float)",
        MettaValue::Bool(_) => "Bool",
        MettaValue::String(_) => "String",
        MettaValue::Atom(_) => "Symbol",
        MettaValue::SExpr(_) => "Expression",
        MettaValue::Nil => "Nil",
        MettaValue::Unit => "Unit",
        MettaValue::Error(_, _) => "Error",
        MettaValue::Type(_) => "Type",
        MettaValue::Conjunction(_) => "Conjunction",
        MettaValue::Space(_) => "Space",
        MettaValue::State(_) => "State",
        MettaValue::Memo(_) => "Memo",
        MettaValue::Empty => "Empty",
    }
}

/// Registry of grounded operations, keyed by name
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

    /// Create a registry with standard operations (+, -, *, /, comparisons, logical)
    pub fn with_standard_ops() -> Self {
        let mut registry = Self::new();

        // Arithmetic operations
        registry.register(Arc::new(AddOp));
        registry.register(Arc::new(SubOp));
        registry.register(Arc::new(MulOp));
        registry.register(Arc::new(DivOp));
        registry.register(Arc::new(ModOp));

        // Comparison operations
        registry.register(Arc::new(LessOp));
        registry.register(Arc::new(LessEqOp));
        registry.register(Arc::new(GreaterOp));
        registry.register(Arc::new(GreaterEqOp));
        registry.register(Arc::new(EqualOp));
        registry.register(Arc::new(NotEqualOp));

        // Logical operations
        registry.register(Arc::new(AndOp));
        registry.register(Arc::new(OrOp));
        registry.register(Arc::new(NotOp));

        registry
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
        Self::with_standard_ops()
    }
}

impl Clone for GroundedRegistry {
    fn clone(&self) -> Self {
        GroundedRegistry {
            operations: self.operations.clone(),
        }
    }
}

/// Registry of TCO-compatible grounded operations
pub struct GroundedRegistryTCO {
    operations: HashMap<String, Arc<dyn GroundedOperationTCO>>,
}

impl GroundedRegistryTCO {
    /// Create a new empty registry
    pub fn new() -> Self {
        GroundedRegistryTCO {
            operations: HashMap::new(),
        }
    }

    /// Create a registry with standard TCO operations
    pub fn with_standard_ops() -> Self {
        let mut registry = Self::new();

        // Arithmetic operations
        registry.register(Arc::new(AddOpTCO));
        registry.register(Arc::new(SubOpTCO));
        registry.register(Arc::new(MulOpTCO));
        registry.register(Arc::new(DivOpTCO));
        registry.register(Arc::new(ModOpTCO));

        // Comparison operations
        registry.register(Arc::new(LessOpTCO));
        registry.register(Arc::new(LessEqOpTCO));
        registry.register(Arc::new(GreaterOpTCO));
        registry.register(Arc::new(GreaterEqOpTCO));
        registry.register(Arc::new(EqualOpTCO));
        registry.register(Arc::new(NotEqualOpTCO));

        // Logical operations
        registry.register(Arc::new(AndOpTCO));
        registry.register(Arc::new(OrOpTCO));
        registry.register(Arc::new(NotOpTCO));

        registry
    }

    /// Register a TCO grounded operation
    pub fn register(&mut self, op: Arc<dyn GroundedOperationTCO>) {
        self.operations.insert(op.name().to_string(), op);
    }

    /// Look up a TCO grounded operation by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn GroundedOperationTCO>> {
        self.operations.get(name).cloned()
    }
}

impl Default for GroundedRegistryTCO {
    fn default() -> Self {
        Self::with_standard_ops()
    }
}

impl Clone for GroundedRegistryTCO {
    fn clone(&self) -> Self {
        GroundedRegistryTCO {
            operations: self.operations.clone(),
        }
    }
}
