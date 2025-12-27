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

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use super::super::eval;

/// collapse: Gather all nondeterministic results into a list
/// Usage: (collapse expr)
///
/// HE-compatible behavior:
/// - If expr evaluates to multiple results (superposition), gathers them into a list
/// - If expr is a space, returns all atoms in the space as a list
/// - If expr is empty, returns Nil
///
/// Example:
/// ```metta
/// !(collapse (get-atoms &self))  ; Wraps atoms in a list
/// !(collapse &myspace)           ; Gets atoms from space as list
/// ```
pub(crate) fn eval_collapse(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("collapse", items, 1, env, "(collapse expr)");

    let expr = &items[1];

    // Evaluate the expression - this may return multiple results (superposition)
    let (results, env1) = eval(expr.clone(), env);

    if results.is_empty() {
        // Empty superposition returns Unit () (HE-compatible)
        return (vec![MettaValue::Unit], env1);
    }

    // Filter out Empty sentinels and Nil values from results
    // Empty represents "no result to report", Nil represents "no result" in nondeterministic evaluation
    let filtered: Vec<MettaValue> = results
        .into_iter()
        .filter(|v| !matches!(v, MettaValue::Empty | MettaValue::Nil))
        .collect();

    if filtered.is_empty() {
        // All results were Empty/Nil → return Unit () (HE-compatible)
        return (vec![MettaValue::Unit], env1);
    }

    // Check if the single result is a space (direct space collapse)
    if filtered.len() == 1 {
        if let MettaValue::Space(handle) = &filtered[0] {
            // Use SpaceHandle's collapse method directly
            let atoms = handle.collapse();
            if atoms.is_empty() {
                return (vec![MettaValue::Unit], env1);
            } else {
                return (vec![MettaValue::SExpr(atoms)], env1);
            }
        }
    }

    // For any other expression, gather all results into a list
    (vec![MettaValue::SExpr(filtered)], env1)
}

/// collapse-bind: Gather all nondeterministic results into a list WITHOUT filtering
/// Usage: (collapse-bind expr)
///
/// Unlike `collapse` which filters out Empty/Nil values, `collapse-bind` preserves
/// ALL results including Empty. This is important for introspection and checking
/// whether an expression produced Empty as a result.
///
/// HE-compatible behavior:
/// - Returns ALL alternatives from nondeterministic evaluation
/// - Does NOT filter Empty/Nil values
///
/// Example:
/// ```metta
/// !(collapse-bind (superpose (1 2 3)))  ; Returns [(1 2 3)]
/// !(collapse-bind Empty)                 ; Returns [Empty] not []
/// ```
pub(crate) fn eval_collapse_bind(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("collapse-bind", items, 1, env, "(collapse-bind expr)");

    let expr = &items[1];

    // Evaluate the expression - this may return multiple results (superposition)
    let (results, env1) = eval(expr.clone(), env);

    // Unlike collapse, do NOT filter Empty/Nil values
    // Return ALL results as a single list, preserving everything
    (vec![MettaValue::SExpr(results)], env1)
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
    match expr {
        MettaValue::SExpr(elements) => {
            if elements.is_empty() {
                // Empty superpose returns empty (no results) - nondeterministic failure
                (vec![], env)
            } else {
                // Return each element as a separate result (nondeterministic)
                (elements.clone(), env)
            }
        }
        MettaValue::Nil => {
            // Nil superposes to empty (no results)
            (vec![], env)
        }
        other => {
            // Single value superposes to itself
            (vec![other.clone()], env)
        }
    }
}

// =============================================================================
// Phase G: Advanced Nondeterminism Operations
// =============================================================================

/// amb: Ambiguous choice (inline nondeterministic choice)
/// Usage: (amb alt1 alt2 ... altN)
///
/// Returns each alternative as a separate result, similar to `superpose` but
/// evaluates each alternative before returning.
///
/// Example:
/// ```metta
/// !(amb 1 2 3)  ; Returns 1, 2, 3 as separate results (after evaluation)
/// !(amb)        ; Returns empty (no results) - nondeterministic failure
/// ```
pub(crate) fn eval_amb(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    if args.is_empty() {
        // Empty amb returns empty (nondeterministic failure)
        return (vec![], env);
    }

    // Evaluate each alternative and collect all results
    let mut all_results = Vec::new();
    let mut current_env = env;

    for alt in args {
        let (results, new_env) = eval(alt.clone(), current_env);
        all_results.extend(results);
        current_env = new_env;
    }

    (all_results, current_env)
}

/// guard: Guarded choice - continue if condition is true, fail otherwise
/// Usage: (guard condition)
///
/// If condition evaluates to True, returns Unit and execution continues.
/// If condition evaluates to False, returns empty (nondeterministic failure).
///
/// Example:
/// ```metta
/// !(if (guard True) "passed" "failed")   ; Returns "passed"
/// !(if (guard False) "passed" "failed")  ; Returns "failed"
/// ```
pub(crate) fn eval_guard(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("guard", items, 1, env, "(guard condition)");

    let condition = &items[1];

    // Evaluate the condition
    let (cond_results, env_after) = eval(condition.clone(), env);

    // Check the condition result
    match cond_results.first() {
        Some(MettaValue::Bool(true)) => {
            // Guard passes - return Unit and continue
            (vec![MettaValue::Unit], env_after)
        }
        Some(MettaValue::Bool(false)) => {
            // Guard fails - return empty (nondeterministic failure)
            (vec![], env_after)
        }
        Some(MettaValue::Error(msg, details)) => {
            // Error propagates
            (
                vec![MettaValue::Error(msg.clone(), details.clone())],
                env_after,
            )
        }
        Some(other) => {
            // Type error - guard requires a boolean
            let err = MettaValue::Error(
                format!(
                    "guard: condition must evaluate to Bool, got {}",
                    super::super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env_after)
        }
        None => {
            // Empty evaluation result - treat as guard failure
            (vec![], env_after)
        }
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
    (vec![MettaValue::Unit], env)
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

/// get-atoms: Get all atoms from a space as a superposition
/// Usage: (get-atoms space)
///
/// Unlike `collapse` which returns atoms wrapped in a list, `get-atoms` returns
/// atoms as a superposition (multiple values). This is HE-compatible behavior.
///
/// Example:
/// ```metta
/// !(get-atoms &self)  ; Returns each atom as a separate result
/// ```
pub(crate) fn eval_get_atoms(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-atoms", items, 1, env, "(get-atoms space)");

    let space_ref = &items[1];

    // Evaluate the space reference
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "get-atoms: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let space_value = &space_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Return atoms as superposition (multiple results), not wrapped in list
            let atoms = handle.collapse();
            if atoms.is_empty() {
                // Empty space returns empty results
                (vec![], env1)
            } else {
                // Return all atoms as separate results (superposition semantics)
                (atoms, env1)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "get-atoms: argument must be a space, got {}. Usage: (get-atoms space)",
                    super::super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env1)
        }
    }
}
