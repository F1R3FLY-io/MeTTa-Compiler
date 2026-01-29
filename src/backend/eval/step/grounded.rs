//! Grounded Arguments Evaluation
//!
//! This module handles the identification of grounded arguments that need
//! evaluation in a hybrid lazy/eager evaluation strategy.

use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::is_grounded_op;

/// Find indices of arguments that are grounded operations needing evaluation.
///
/// This function identifies which arguments in an S-expression should be
/// evaluated eagerly (before pattern matching) vs lazily (after pattern matching).
///
/// Returns empty vec if no grounded args (can proceed directly to rule matching).
///
/// ## Why This Exists
///
/// MeTTa uses hybrid lazy/eager evaluation:
/// - Grounded operations (like arithmetic) should be evaluated BEFORE pattern matching
/// - User-defined expressions should remain unevaluated for lazy pattern matching
///
/// Example: For `(countdown (- 3 1))`:
/// - The argument `(- 3 1)` is a grounded operation, so it needs evaluation to `2`
/// - Result: `(countdown 2)` - now pattern matching works correctly
///
/// Example: For `(wrapper $a (add-atom &stack x))`:
/// - The argument `(add-atom &stack x)` is NOT grounded (user-defined side effect)
/// - Keep it unevaluated for lazy pattern matching
/// - Returns empty vec
pub fn find_grounded_arg_indices(items: &[MettaValue], env: &Environment) -> Vec<usize> {
    let mut indices = Vec::new();

    // Skip the first item (operator) - we only check arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if let MettaValueInner::SExpr(sub_items) = item.inner() {
            if let Some(first) = sub_items.first() {
                if let MettaValueInner::Atom(op) = first.inner() {
                    // Check if this is a grounded operation (built-in or TCO)
                    if is_grounded_op(op) || env.get_grounded_operation_tco(op).is_some() {
                        indices.push(i); // Store actual index in items
                    }
                }
            }
        }
    }

    indices
}
