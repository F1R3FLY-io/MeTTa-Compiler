//! Helper functions for list operations.
//!
//! This module provides utility functions for list operations including
//! variable substitution and variable format suggestions.

use std::sync::Arc;

use crate::backend::models::MettaValue;

/// Suggest variable format when user provides a plain atom instead of `$var`
/// Returns a suggestion string if the atom looks like it should be a variable
pub(super) fn suggest_variable_format(atom: &str) -> Option<String> {
    // If it's already a variable, no suggestion needed
    if atom.starts_with('$') || atom.starts_with('&') || atom.starts_with('\'') {
        return None;
    }

    // Don't suggest for obvious non-variables (operators, keywords, etc.)
    if atom.contains('(') || atom.contains(')') || atom.is_empty() {
        return None;
    }

    // Short, lowercase identifiers are likely intended as variables
    let first_char = atom.chars().next()?;
    if first_char.is_lowercase() && atom.len() <= 10 {
        Some(format!(
            "Did you mean: ${}? (variables must start with $)",
            atom
        ))
    } else {
        None
    }
}

/// Substitute a variable in an expression with a value
/// This is a simplified version of atom-subst
pub(super) fn substitute_variable(
    expr: &MettaValue,
    var_name: &str,
    value: &MettaValue,
) -> MettaValue {
    match expr {
        MettaValue::Atom(name) if name == var_name => value.clone(),
        MettaValue::SExpr(items) => {
            let substituted_items: Vec<MettaValue> = items
                .iter()
                .map(|item| substitute_variable(item, var_name, value))
                .collect();
            MettaValue::SExpr(substituted_items)
        }
        MettaValue::Error(msg, details) => {
            let substituted_details = substitute_variable(details, var_name, value);
            MettaValue::Error(msg.clone(), Arc::new(substituted_details))
        }
        _ => expr.clone(),
    }
}
