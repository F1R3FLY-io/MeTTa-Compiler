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

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::super::eval;
use super::super::EvalStep;

/// Step version of eval_collapse - defers evaluation to trampoline.
/// Usage: (collapse expr)
pub(crate) fn eval_collapse_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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
///
/// DEPRECATED: Use eval_collapse_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_collapse(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("collapse", items, 1, env, "(collapse expr)");

    let expr = &items[1];

    // Evaluate the expression - this may return multiple results (superposition)
    let (results, env1) = eval(expr.clone(), env);

    if results.is_empty() {
        // Empty superposition returns Unit () (HE-compatible)
        return (vec![MettaValue::Unit()], env1);
    }

    // Filter out Empty sentinels and Nil values from results
    // Empty represents "no result to report", Nil represents "no result" in nondeterministic evaluation
    let filtered: Vec<MettaValue> = results
        .into_iter()
        .filter(|v| !matches!(v.inner(), MettaValueInner::Empty | MettaValueInner::Nil))
        .collect();

    if filtered.is_empty() {
        // All results were Empty/Nil → return Unit () (HE-compatible)
        return (vec![MettaValue::Unit()], env1);
    }

    // Check if the single result is a space (direct space collapse)
    if filtered.len() == 1 {
        if let MettaValueInner::Space(handle) = filtered[0].inner() {
            // Use SpaceHandle's collapse method directly
            let atoms = handle.collapse();
            if atoms.is_empty() {
                return (vec![MettaValue::Unit()], env1);
            } else {
                return (vec![MettaValue::SExpr(atoms)], env1);
            }
        }
    }

    // For any other expression, gather all results into a list
    (vec![MettaValue::SExpr(filtered)], env1)
}

/// Step version of eval_collapse_bind - defers evaluation to trampoline.
/// Usage: (collapse-bind expr)
pub(crate) fn eval_collapse_bind_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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
///
/// DEPRECATED: Use eval_collapse_bind_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_collapse_bind(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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
pub(crate) fn eval_superpose(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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
pub(crate) fn eval_amb_step(items: Vec<MettaValue>, env: HeapEnvironment, depth: usize) -> EvalStep {
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
///
/// DEPRECATED: Use eval_amb_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_amb(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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

/// Step version of eval_guard - defers evaluation to trampoline.
/// Usage: (guard condition)
pub(crate) fn eval_guard_step(items: Vec<MettaValue>, env: HeapEnvironment, depth: usize) -> EvalStep {
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
///
/// DEPRECATED: Use eval_guard_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_guard(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("guard", items, 1, env, "(guard condition)");

    let condition = &items[1];

    // Evaluate the condition
    let (cond_results, env_after) = eval(condition.clone(), env);

    // Check the condition result
    match cond_results.first().map(|v| v.inner()) {
        Some(MettaValueInner::Bool(true)) => {
            // Guard passes - return Unit and continue
            (vec![MettaValue::Unit()], env_after)
        }
        Some(MettaValueInner::Bool(false)) => {
            // Guard fails - return empty (nondeterministic failure)
            (vec![], env_after)
        }
        Some(MettaValueInner::Error(msg, details)) => {
            // Error propagates
            (
                vec![MettaValue::Error(msg.clone(), details.clone())],
                env_after,
            )
        }
        Some(_) => {
            // Type error - guard requires a boolean
            let other = cond_results.first().expect("checked above");
            let err = MettaValue::Error(
                format!(
                    "guard: condition must evaluate to Bool, got {}",
                    super::super::friendly_type_name(other)
                ),
                other.clone(),
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
pub(crate) fn eval_commit(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
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
pub(crate) fn eval_backtrack(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    let _ = items; // Suppress unused warning
                   // Return empty to signal nondeterministic failure
    (vec![], env)
}

/// Step version of eval_get_atoms - defers evaluation to trampoline.
/// Usage: (get-atoms space)
pub(crate) fn eval_get_atoms_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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
///
/// DEPRECATED: Use eval_get_atoms_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_get_atoms(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("get-atoms", items, 1, env, "(get-atoms space)");

    let space_ref = &items[1];

    // Evaluate the space reference
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "get-atoms: space evaluated to empty".to_string(),
            space_ref.clone(),
        );
        return (vec![err], env1);
    }

    let space_value = &space_results[0];

    match space_value.inner() {
        MettaValueInner::Space(handle) => {
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
                space_value.clone(),
            );
            (vec![err], env1)
        }
    }
}
