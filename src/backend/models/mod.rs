pub mod bindings;
pub mod memo_handle;
pub mod metta_state;
pub mod metta_value;
pub mod space_handle;

pub use bindings::SmartBindings as Bindings;
pub use memo_handle::MemoHandle;
pub use metta_state::MettaState;
pub use metta_value::MettaValue;
pub use space_handle::SpaceHandle;

use crate::backend::environment::Environment;
use std::sync::Arc;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

/// Represents a pattern matching rule: (= lhs rhs)
/// Uses Arc for efficient sharing and COW semantics
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: Arc<MettaValue>,
    pub rhs: Arc<MettaValue>,
}

impl Rule {
    /// Create a new rule from owned MettaValues (wraps in Arc)
    pub fn new(lhs: MettaValue, rhs: MettaValue) -> Self {
        Rule {
            lhs: Arc::new(lhs),
            rhs: Arc::new(rhs),
        }
    }

    /// Create a new rule from Arc-wrapped MettaValues
    pub fn from_arc(lhs: Arc<MettaValue>, rhs: Arc<MettaValue>) -> Self {
        Rule { lhs, rhs }
    }

    /// Get a reference to the LHS pattern
    #[inline]
    pub fn lhs_ref(&self) -> &MettaValue {
        &self.lhs
    }

    /// Get a reference to the RHS template
    #[inline]
    pub fn rhs_ref(&self) -> &MettaValue {
        &self.rhs
    }

    /// Clone the RHS as an Arc (O(1) operation)
    #[inline]
    pub fn rhs_arc(&self) -> Arc<MettaValue> {
        Arc::clone(&self.rhs)
    }

    /// Clone the LHS as an Arc (O(1) operation)
    #[inline]
    pub fn lhs_arc(&self) -> Arc<MettaValue> {
        Arc::clone(&self.lhs)
    }
}

impl Rule {
    /// Create a new rule from LHS and RHS expressions
    pub fn new(lhs: MettaValue, rhs: MettaValue) -> Self {
        Rule { lhs, rhs }
    }
}
