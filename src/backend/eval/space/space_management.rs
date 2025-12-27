//! Space management operations.
//!
//! This module handles creating and modifying spaces:
//! - new-space: Create a new named space
//! - add-atom: Add an atom to a space
//! - remove-atom: Remove an atom from a space

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, SpaceHandle};

use super::super::eval;

/// new-space: Create a new named space
/// Returns a Space reference that can be used with add-atom, remove-atom, collapse
/// Usage: (new-space) or (new-space "name")
pub(crate) fn eval_new_space(items: Vec<MettaValue>, mut env: Environment) -> EvalResult {
    let args = &items[1..];

    // Get optional name, default to "space-N"
    let name = if !args.is_empty() {
        match &args[0] {
            MettaValue::String(s) => s.clone(),
            MettaValue::Atom(s) => s.clone(),
            other => {
                let err = MettaValue::Error(
                    format!(
                        "new-space: optional name must be a string, got {}. Usage: (new-space) or (new-space \"name\")",
                        super::super::friendly_value_repr(other)
                    ),
                    Arc::new(other.clone()),
                );
                return (vec![err], env);
            }
        }
    } else {
        "unnamed".to_string()
    };

    let space_id = env.create_named_space(&name);
    let handle = SpaceHandle::new(space_id, name);
    (vec![MettaValue::Space(handle)], env)
}

/// add-atom: Add an atom to a space
/// Usage: (add-atom space-ref atom)
pub(crate) fn eval_add_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("add-atom", items, 2, env, "(add-atom space atom)");

    let space_ref = &items[1];
    let atom = &items[2];

    // Evaluate both arguments
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "add-atom: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let (atom_results, env2) = eval(atom.clone(), env1);
    if atom_results.is_empty() {
        let err = MettaValue::Error(
            "add-atom: atom evaluated to empty".to_string(),
            Arc::new(atom.clone()),
        );
        return (vec![err], env2);
    }

    // Get the space ID
    let space_value = &space_results[0];
    let atom_value = &atom_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Use SpaceHandle's add_atom method directly (it has its own backing store)
            handle.add_atom(atom_value.clone());
            (vec![MettaValue::Unit], env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                    super::super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env2)
        }
    }
}

/// remove-atom: Remove an atom from a space
/// Usage: (remove-atom space-ref atom)
pub(crate) fn eval_remove_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("remove-atom", items, 2, env, "(remove-atom space atom)");

    let space_ref = &items[1];
    let atom = &items[2];

    // Evaluate both arguments
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "remove-atom: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let (atom_results, env2) = eval(atom.clone(), env1);
    if atom_results.is_empty() {
        let err = MettaValue::Error(
            "remove-atom: atom evaluated to empty".to_string(),
            Arc::new(atom.clone()),
        );
        return (vec![err], env2);
    }

    // Get the space ID
    let space_value = &space_results[0];
    let atom_value = &atom_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Use SpaceHandle's remove_atom method directly (it has its own backing store)
            handle.remove_atom(atom_value);
            (vec![MettaValue::Unit], env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                    super::super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env2)
        }
    }
}
