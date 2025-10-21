use std::collections::HashMap;

pub mod metta_state;
pub mod metta_value;

pub use metta_state::MettaState;
pub use metta_value::MettaValue;

use crate::backend::environment::Environment;

/// Variable bindings for pattern matching
pub type Bindings = HashMap<String, MettaValue>;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

/// Represents a pattern matching rule: (= lhs rhs)
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: MettaValue,
    pub rhs: MettaValue,
}
