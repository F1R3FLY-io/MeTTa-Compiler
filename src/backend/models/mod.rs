use smallvec::SmallVec;

pub mod metta_state;
pub mod metta_value;

pub use metta_state::MettaState;
pub use metta_value::MettaValue;

use crate::backend::environment::Environment;

/// Variable bindings for pattern matching
/// Optimized with SmallVec to avoid heap allocation for <8 variables (90% of patterns)
pub type Bindings = SmallVec<[(String, MettaValue); 8]>;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

/// Represents a pattern matching rule: (= lhs rhs)
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: MettaValue,
    pub rhs: MettaValue,
}
