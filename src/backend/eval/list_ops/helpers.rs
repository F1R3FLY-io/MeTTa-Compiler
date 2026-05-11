//! Helper functions for list operations.
//!
//! This module provides utility functions for list operations including
//! generic variable substitution and variable format suggestions.

use crate::backend::models::{MettaValueFactory, MettaValueInner, MettaValueTrait};

/// Suggest variable format when user provides a plain atom instead of `$var`
/// Returns a suggestion string if the atom looks like it should be a variable
pub(crate) fn suggest_variable_format(atom: &str) -> Option<String> {
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

// ============================================================================
// Generic Variable Substitution
// ============================================================================

/// Generic substitute variable - works with any value type implementing MettaValueTrait.
///
/// This function replaces all occurrences of a variable (identified by `var_name`)
/// with the provided `value` in the expression tree. It uses an iterative approach
/// to avoid stack overflow on deeply nested expressions.
///
/// # Type Parameters
///
/// - `V`: The value type (must implement `MettaValueTrait + Clone`)
/// - `F`: The factory type (must implement `MettaValueFactory<V>`)
///
/// # Arguments
///
/// - `expr`: The expression to perform substitution on
/// - `var_name`: The name of the variable to substitute (e.g., "$x")
/// - `value`: The value to substitute in place of the variable
/// - `factory`: The factory for constructing new values
///
/// # Returns
///
/// A new value with all occurrences of `var_name` replaced by `value`.
///
/// # Performance
///
/// This generic version eliminates boundary conversions by operating directly
/// on the generic value type. When used with `MettaValue`, clone is O(1) due
/// to Arc wrapping. When used with `MettaValue`, clone is O(1) pointer copy.
pub(crate) fn substitute_variable_generic<V, F>(
    expr: &V,
    var_name: &str,
    value: &V,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Fast path for leaf nodes using trait methods
    if let Some(name) = expr.as_atom() {
        if name == var_name {
            return value.clone(); // O(1) clone for Arc-wrapped or arena values
        }
        return expr.clone();
    }

    // Check other leaf types - these don't contain variables
    if matches!(
        expr.inner_raw(),
        MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Unit
            | MettaValueInner::Space(_)
            | MettaValueInner::State(_)
            | MettaValueInner::Type(_)
            | MettaValueInner::Memo(_)
            | MettaValueInner::Empty
    ) {
        return expr.clone();
    }

    // Compound types need iterative processing
    substitute_variable_iterative_generic(expr, var_name, value, factory)
}

/// Work item for iterative generic substitution
enum SubstituteWorkGeneric<V> {
    /// Process a value - may push more work
    Process(V),
    /// Build an SExpr from the last N results
    BuildSExpr(usize),
    /// Build a Conjunction from the last N results
    BuildConjunction(usize),
    /// Build an Error from the last result
    BuildError(String),
}

/// Iterative implementation of substitute_variable_generic using explicit work stack.
fn substitute_variable_iterative_generic<V, F>(
    expr: &V,
    var_name: &str,
    value: &V,
    factory: &F,
) -> V
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    // Work stack: items to process
    let mut work_stack: Vec<SubstituteWorkGeneric<V>> =
        vec![SubstituteWorkGeneric::Process(expr.clone())];

    // Result stack: processed results
    let mut result_stack: Vec<V> = Vec::with_capacity(32);

    while let Some(work) = work_stack.pop() {
        match work {
            SubstituteWorkGeneric::Process(val) => {
                // Handle atom/variable
                if let Some(name) = val.as_atom() {
                    if name == var_name {
                        result_stack.push(value.clone());
                    } else {
                        result_stack.push(val);
                    }
                    continue;
                }

                // Handle S-expression
                if let Some(items) = val.as_sexpr() {
                    if items.is_empty() {
                        result_stack.push(val);
                    } else {
                        work_stack.push(SubstituteWorkGeneric::BuildSExpr(items.len()));
                        for item in items.iter().rev() {
                            work_stack.push(SubstituteWorkGeneric::Process(item.clone()));
                        }
                    }
                    continue;
                }

                // Handle conjunction
                if let Some(goals) = val.as_conjunction() {
                    if goals.is_empty() {
                        result_stack.push(val);
                    } else {
                        work_stack.push(SubstituteWorkGeneric::BuildConjunction(goals.len()));
                        for goal in goals.iter().rev() {
                            work_stack.push(SubstituteWorkGeneric::Process(goal.clone()));
                        }
                    }
                    continue;
                }

                // Handle error
                if let Some((msg, details)) = val.as_error() {
                    work_stack.push(SubstituteWorkGeneric::BuildError(msg.to_string()));
                    work_stack.push(SubstituteWorkGeneric::Process(details.clone()));
                    continue;
                }

                // All other types: return as-is (leaf nodes)
                result_stack.push(val);
            }

            SubstituteWorkGeneric::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.sexpr(children));
            }

            SubstituteWorkGeneric::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<V> = result_stack.drain(start..).collect();
                result_stack.push(factory.conjunction(children));
            }

            SubstituteWorkGeneric::BuildError(msg) => {
                let details = result_stack
                    .pop()
                    .expect("BuildError should have details on result stack");
                result_stack.push(factory.error(&msg, details));
            }
        }
    }

    // Final result should be on the stack
    debug_assert_eq!(
        result_stack.len(),
        1,
        "substitute_variable_generic should produce exactly one result"
    );
    result_stack
        .pop()
        .expect("Result stack should not be empty")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_substitute_variable_generic_atom() {
        let factory = GcFactory::default();
        let expr = MettaValue::Atom("$x".to_string());
        let value = MettaValue::Long(42);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert_eq!(result.as_long(), Some(42));
    }

    #[test]
    fn test_substitute_variable_generic_no_match() {
        let factory = GcFactory::default();
        let expr = MettaValue::Atom("$y".to_string());
        let value = MettaValue::Long(42);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert_eq!(result.as_atom(), Some("$y"));
    }

    #[test]
    fn test_substitute_variable_generic_sexpr() {
        let factory = GcFactory::default();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        let value = MettaValue::Long(10);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert!(result.is_sexpr());
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items[0].as_atom(), Some("+"));
        assert_eq!(items[1].as_long(), Some(10));
        assert_eq!(items[2].as_long(), Some(1));
    }

    #[test]
    fn test_substitute_variable_generic_nested() {
        let factory = GcFactory::default();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("inner".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);
        let value = MettaValue::Long(99);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert!(result.is_sexpr());
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);

        // Check outer $x was replaced
        assert_eq!(items[2].as_long(), Some(99));

        // Check inner $x was replaced
        let inner = items[1].as_sexpr().expect("should be inner sexpr");
        assert_eq!(inner[1].as_long(), Some(99));
    }

    #[test]
    fn test_substitute_variable_generic_ground_types() {
        let factory = GcFactory::default();
        let value = MettaValue::Long(42);

        // Long should be unchanged
        let expr = MettaValue::Long(100);
        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert_eq!(result.as_long(), Some(100));

        // Bool should be unchanged
        let expr = MettaValue::Bool(true);
        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert_eq!(result.as_bool(), Some(true));

        // String should be unchanged
        let expr = MettaValue::String("hello".to_string());
        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert_eq!(result.as_string(), Some("hello"));
    }

    #[test]
    fn test_substitute_variable_generic_error() {
        let factory = GcFactory::default();
        let expr = MettaValue::Error("test error".to_string(), MettaValue::Atom("$x".to_string()));
        let value = MettaValue::Long(42);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert!(result.is_error());
        let (msg, details) = result.as_error().expect("should be error");
        assert_eq!(msg, "test error");
        assert_eq!(details.as_long(), Some(42));
    }

    #[test]
    fn test_substitute_variable_generic_conjunction() {
        let factory = GcFactory::default();
        let expr = MettaValue::Conjunction(vec![
            MettaValue::Atom("$x".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        let value = MettaValue::Long(42);

        let result = substitute_variable_generic(&expr, "$x", &value, &factory);
        assert!(result.is_conjunction());
        let goals = result.as_conjunction().expect("should be conjunction");
        assert_eq!(goals[0].as_long(), Some(42));
        assert_eq!(goals[1].as_atom(), Some("$y"));
    }
}
