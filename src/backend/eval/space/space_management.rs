//! Space management operations.
//!
//! This module handles creating and modifying spaces:
//! - new-space: Create a new named space
//! - add-atom: Add an atom to a space
//! - remove-atom: Remove an atom from a space

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner, SpaceHandle};

use super::super::EvalStep;

/// Step version of eval_add_atom - defers evaluation to trampoline.
/// Usage: (add-atom space-ref atom)
pub(crate) fn eval_add_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "add-atom requires 2 arguments, got {}. Usage: (add-atom space atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let space_ref = items[1].clone();
    let atom = items[2].clone();

    EvalStep::StartAddAtom {
        space_ref,
        atom,
        env,
        depth,
    }
}

/// Step version of eval_remove_atom - defers evaluation to trampoline.
/// Usage: (remove-atom space-ref atom)
pub(crate) fn eval_remove_atom_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "remove-atom requires 2 arguments, got {}. Usage: (remove-atom space atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let space_ref = items[1].clone();
    let atom = items[2].clone();

    EvalStep::StartRemoveAtom {
        space_ref,
        atom,
        env,
        depth,
    }
}

/// new-space: Create a new named space
/// Returns a Space reference that can be used with add-atom, remove-atom, collapse
/// Usage: (new-space) or (new-space "name")
pub(crate) fn eval_new_space(items: Vec<MettaValue>, mut env: Environment) -> EvalResult {
    let args = &items[1..];

    // Get optional name, default to "space-N"
    let name = if !args.is_empty() {
        match args[0].inner() {
            MettaValueInner::String(s) => s.clone(),
            MettaValueInner::Atom(s) => s.clone(),
            _ => {
                let other = &args[0];
                let err = MettaValue::Error(
                    format!(
                        "new-space: optional name must be a string, got {}. Usage: (new-space) or (new-space \"name\")",
                        super::super::friendly_value_repr(other)
                    ),
                    other.clone(),
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

