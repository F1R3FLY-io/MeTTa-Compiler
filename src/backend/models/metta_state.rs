use super::MettaValue;
use crate::backend::environment::Environment;

/// MeTTa compilation/evaluation state for PathMap-based REPL integration
/// This structure represents the state of a MeTTa computation session.
///
/// # State Composition
/// - **Compiled state** (fresh from `compile`):
///   - `source`: S-expressions to evaluate
///   - `environment`: Empty atom space
///   - `output`: Empty (no evaluations yet)
///
/// - **Accumulated state** (built over multiple REPL iterations):
///   - `source`: Empty (already evaluated)
///   - `environment`: Accumulated atom space (MORK facts/rules)
///   - `output`: Accumulated evaluation results
///
/// # Usage Pattern
/// ```ignore
/// // Compile MeTTa source
/// let compiled_state = compile(source)?;
///
/// // Run against accumulated state
/// let new_accumulated = accumulated_state.run(&compiled_state)?;
/// ```
#[derive(Clone, Debug)]
pub struct MettaState {
    /// Source s-expressions to be evaluated
    pub source: Vec<MettaValue>,
    /// The atom space (MORK fact database) containing rules and facts
    pub environment: Environment,
    /// Evaluation output results
    pub output: Vec<MettaValue>,
}

impl MettaState {
    /// Create a fresh compiled state from parse results
    pub fn new_compiled(source: Vec<MettaValue>) -> Self {
        MettaState {
            source,
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an empty accumulated state (for REPL initialization)
    pub fn new_empty() -> Self {
        MettaState {
            source: Vec::new(),
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an accumulated state with existing environment and output
    pub fn new_accumulated(environment: Environment, output: Vec<MettaValue>) -> Self {
        MettaState {
            source: Vec::new(),
            environment,
            output,
        }
    }

    /// Create a compiled state containing an error s-expression
    /// Used when parsing fails to allow error handling at the evaluation level
    pub fn new_with_error(error_sexpr: MettaValue) -> Self {
        MettaState {
            source: vec![error_sexpr],
            environment: Environment::new(),
            output: Vec::new(),
        }
    }
}
