//! Conjunction Evaluation
//!
//! This module implements evaluation of MORK-style conjunction expressions
//! with left-to-right goal evaluation and binding threading.

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::EvalStep;

/// Step version of eval_conjunction that defers evaluation to trampoline.
/// This prevents stack overflow for deeply nested conjunction goals.
pub fn eval_conjunction_step(goals: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    EvalStep::StartConjunction { goals, env, depth }
}
