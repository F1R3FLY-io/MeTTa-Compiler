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
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
pub(super) fn substitute_variable(
    expr: &MettaValue,
    var_name: &str,
    value: &MettaValue,
) -> MettaValue {
    // Fast path for leaf nodes
    match expr {
        MettaValue::Atom(name) if name == var_name => return value.clone(),
        MettaValue::Atom(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::Bool(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Unit
        | MettaValue::Space(_)
        | MettaValue::State(_)
        | MettaValue::Type(_)
        | MettaValue::Memo(_)
        | MettaValue::Empty => return expr.clone(),
        // Compound types need iterative processing
        MettaValue::SExpr(_) | MettaValue::Conjunction(_) | MettaValue::Error(_, _) => {}
    }

    // Iterative implementation using explicit work stack
    substitute_variable_iterative(expr, var_name, value)
}

/// Work item for iterative substitute_variable
enum SubstituteWork<'a> {
    /// Process a value - may push more work
    Process(&'a MettaValue),
    /// Build an SExpr from the last N results
    BuildSExpr(usize),
    /// Build a Conjunction from the last N results
    BuildConjunction(usize),
    /// Build an Error from the last result
    BuildError(String),
}

/// Iterative implementation of substitute_variable using explicit work stack.
fn substitute_variable_iterative(
    expr: &MettaValue,
    var_name: &str,
    value: &MettaValue,
) -> MettaValue {
    // Work stack: items to process
    let mut work_stack: Vec<SubstituteWork> = Vec::with_capacity(32);
    // Result stack: processed results
    let mut result_stack: Vec<MettaValue> = Vec::with_capacity(32);

    work_stack.push(SubstituteWork::Process(expr));

    while let Some(work) = work_stack.pop() {
        match work {
            SubstituteWork::Process(val) => {
                match val {
                    // Variable substitution
                    MettaValue::Atom(name) if name == var_name => {
                        result_stack.push(value.clone());
                    }
                    // S-expression: push build marker, then push children in reverse order
                    MettaValue::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push(val.clone());
                        } else {
                            work_stack.push(SubstituteWork::BuildSExpr(items.len()));
                            for item in items.iter().rev() {
                                work_stack.push(SubstituteWork::Process(item));
                            }
                        }
                    }
                    // Conjunction: similar to SExpr
                    MettaValue::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push(val.clone());
                        } else {
                            work_stack.push(SubstituteWork::BuildConjunction(goals.len()));
                            for goal in goals.iter().rev() {
                                work_stack.push(SubstituteWork::Process(goal));
                            }
                        }
                    }
                    // Error: push build marker, then push details
                    MettaValue::Error(msg, details) => {
                        work_stack.push(SubstituteWork::BuildError(msg.clone()));
                        work_stack.push(SubstituteWork::Process(details));
                    }
                    // All other types: no substitution, clone as-is
                    _ => {
                        result_stack.push(val.clone());
                    }
                }
            }
            SubstituteWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<MettaValue> = result_stack.drain(start..).collect();
                result_stack.push(MettaValue::SExpr(children));
            }
            SubstituteWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<MettaValue> = result_stack.drain(start..).collect();
                result_stack.push(MettaValue::Conjunction(children));
            }
            SubstituteWork::BuildError(msg) => {
                let details = result_stack
                    .pop()
                    .expect("BuildError should have details on result stack");
                result_stack.push(MettaValue::Error(msg, Arc::new(details)));
            }
        }
    }

    // Final result should be on the stack
    debug_assert_eq!(
        result_stack.len(),
        1,
        "substitute_variable should produce exactly one result"
    );
    result_stack.pop().expect("Result stack should not be empty")
}
