pub mod bindings;
pub mod metta_state;
pub mod metta_value;
pub mod space_handle;

pub use bindings::SmartBindings as Bindings;
pub use metta_state::MettaState;
pub use metta_value::MettaValue;
pub use space_handle::SpaceHandle;

use crate::backend::environment::Environment;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

/// Represents a pattern matching rule: (= lhs rhs)
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: MettaValue,
    pub rhs: MettaValue,
}
