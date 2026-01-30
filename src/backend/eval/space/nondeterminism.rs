//! Nondeterminism operations.
//!
//! This module handles nondeterministic evaluation operations:
//! - collapse: Gather all nondeterministic results into a list
//! - collapse-bind: Gather results without filtering
//! - superpose: Convert a list to nondeterministic results
//! - amb: Ambiguous choice (inline nondeterministic choice)
//! - guard: Guarded choice
//! - commit: Remove choice points (soft cut)
//! - backtrack: Force immediate backtracking
//! - get-atoms: Get all atoms from a space as a superposition

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

use super::super::EvalStep;

/// Step version of eval_collapse - defers evaluation to trampoline.
/// Usage: (collapse expr)
pub(crate) fn eval_collapse_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "collapse requires 1 argument. Usage: (collapse expr)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartCollapse {
        expr: items[1].clone(),
        env,
        depth,
    }
}

/// Step version of eval_collapse_bind - defers evaluation to trampoline.
/// Usage: (collapse-bind expr)
pub(crate) fn eval_collapse_bind_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "collapse-bind requires 1 argument. Usage: (collapse-bind expr)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartCollapseBind {
        expr: items[1].clone(),
        env,
        depth,
    }
}

/// superpose: Convert a list to nondeterministic results (superposition)
/// Usage: (superpose list)
///
/// HE-compatible behavior:
/// - Takes a list and returns each element as a separate result (nondeterministic)
/// - This is the inverse of `collapse` - it explicitly introduces nondeterminism
/// - Essential for HE-compatible deterministic-first evaluation model
///
/// Example:
/// ```metta
/// !(superpose (1 2 3))  ; Returns 1, 2, 3 as separate results
/// !(superpose ())       ; Returns empty (no results)
/// ```
pub(crate) fn eval_superpose(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("superpose", items, 1, env, "(superpose list)");

    let expr = &items[1];

    // DON'T evaluate the argument - treat it as a data list (HE-compatible)
    // This is different from most operations that evaluate their arguments
    match expr.inner() {
        MettaValueInner::SExpr(elements) => {
            if elements.is_empty() {
                // Empty superpose returns empty (no results) - nondeterministic failure
                (vec![], env)
            } else {
                // Return each element as a separate result (nondeterministic)
                (elements.clone(), env)
            }
        }
        MettaValueInner::Nil => {
            // Nil superposes to empty (no results)
            (vec![], env)
        }
        _ => {
            // Single value superposes to itself
            (vec![expr.clone()], env)
        }
    }
}

// =============================================================================
// Phase G: Advanced Nondeterminism Operations
// =============================================================================

/// Step version of eval_amb - defers evaluation to trampoline.
/// Usage: (amb alt1 alt2 ... altN)
pub(crate) fn eval_amb_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    let alternatives = items[1..].to_vec();

    if alternatives.is_empty() {
        // Empty amb returns empty (nondeterministic failure)
        return EvalStep::Done((vec![], env));
    }

    EvalStep::StartAmb {
        alternatives,
        env,
        depth,
    }
}

/// Step version of eval_guard - defers evaluation to trampoline.
/// Usage: (guard condition)
pub(crate) fn eval_guard_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "guard requires 1 argument. Usage: (guard condition)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartGuard {
        condition: items[1].clone(),
        env,
        depth,
    }
}

/// commit: Remove choice points (soft cut)
/// Usage: (commit) or (commit N)
///
/// In the tree-walker, this is mostly a no-op since choice points are not
/// tracked the same way as in the bytecode VM. It returns Unit.
///
/// Example:
/// ```metta
/// !(commit)    ; Remove all choice points
/// !(commit 1)  ; Remove 1 choice point
/// ```
pub(crate) fn eval_commit(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // In tree-walker evaluation, commit is a no-op since we don't maintain
    // explicit choice points. The nondeterminism is handled through result lists.
    // Just return Unit to indicate success.
    let _ = items; // Suppress unused warning
    (vec![MettaValue::Unit()], env)
}

/// backtrack: Force immediate backtracking (nondeterministic failure)
/// Usage: (backtrack)
///
/// Returns empty (no results), causing nondeterministic failure.
/// This is equivalent to `(amb)` with no alternatives.
///
/// Example:
/// ```metta
/// !(backtrack)  ; Returns empty (no results)
/// ```
pub(crate) fn eval_backtrack(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let _ = items; // Suppress unused warning
                   // Return empty to signal nondeterministic failure
    (vec![], env)
}

/// Step version of eval_get_atoms - defers evaluation to trampoline.
/// Usage: (get-atoms space)
pub(crate) fn eval_get_atoms_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "get-atoms requires 1 argument. Usage: (get-atoms space)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartGetAtoms {
        space_ref: items[1].clone(),
        env,
        depth,
    }
}

