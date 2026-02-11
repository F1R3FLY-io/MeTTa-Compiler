//! Grounded Arguments Evaluation
//!
//! This module handles the identification of grounded arguments that need
//! evaluation in a hybrid lazy/eager evaluation strategy.

use crate::backend::environment::{MettaEnvironment, GenericEnvironment};
use crate::backend::models::{MettaValue, MettaValueInner, MettaValueTrait};

use super::super::{is_eager_special_form, is_grounded_op};

/// Find indices of arguments that need eager evaluation.
///
/// This function identifies which arguments in an S-expression should be
/// evaluated eagerly (before pattern matching) vs lazily (after pattern matching).
///
/// Returns empty vec if no arguments need eager evaluation (can proceed directly to rule matching).
///
/// ## Why This Exists
///
/// MeTTa uses hybrid lazy/eager evaluation:
/// - Grounded operations (like arithmetic) should be evaluated BEFORE pattern matching
/// - Special forms that produce values (like map-atom) should also be evaluated BEFORE
///   being passed to user-defined rules (for MeTTa HE semantic alignment)
/// - User-defined expressions should remain unevaluated for lazy pattern matching
///
/// Example: For `(countdown (- 3 1))`:
/// - The argument `(- 3 1)` is a grounded operation, so it needs evaluation to `2`
/// - Result: `(countdown 2)` - now pattern matching works correctly
///
/// Example: For `(get-expr-size (map-atom (a b) $v ($v x)))`:
/// - The argument `(map-atom ...)` is an eager special form, needs evaluation
/// - Result: `(get-expr-size ((a x) (b x)))` - now pattern matching receives the result
///
/// Example: For `(wrapper $a (add-atom &stack x))`:
/// - The argument `(add-atom &stack x)` is NOT grounded (user-defined side effect)
/// - Keep it unevaluated for lazy pattern matching
/// - Returns empty vec
pub fn find_grounded_arg_indices(items: &[MettaValue], _env: &MettaEnvironment) -> Vec<usize> {
    let mut indices = Vec::new();

    // Skip the first item (operator) - we only check arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if let MettaValueInner::SExpr(sub_items) = item.inner() {
            if let Some(first) = sub_items.first() {
                if let MettaValueInner::Atom(op) = first.inner() {
                    // Check if this is a grounded operation or a special form
                    // that produces values and needs eager evaluation
                    if is_grounded_op(op)
                        || is_eager_special_form(op)
                    {
                        indices.push(i); // Store actual index in items
                    }
                }
            }
        }
    }

    indices
}

/// Generic version of find_grounded_arg_indices.
///
/// This function works with any value type implementing `MettaValueTrait`,
/// enabling zero-conversion evaluation for both heap and arena allocation modes.
///
/// See `find_grounded_arg_indices` for detailed documentation on the purpose
/// and semantics of this function.
pub fn find_grounded_arg_indices_generic<V, F>(
    items: &[V],
    env: &GenericEnvironment<V, F>,
) -> Vec<usize>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: crate::backend::models::MettaValueFactory<V> + Clone,
{
    let _ = env; // env was previously used for TCO registry lookup, now unused
    let mut indices = Vec::new();

    // Skip the first item (operator) - we only check arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if let Some(sub_items) = item.as_sexpr() {
            if let Some(first) = sub_items.first() {
                if let Some(op) = first.as_atom() {
                    // Check if this is a grounded operation or a special form
                    // that produces values and needs eager evaluation
                    if is_grounded_op(op)
                        || is_eager_special_form(op)
                    {
                        indices.push(i); // Store actual index in items
                    }
                }
            }
        }
    }

    indices
}
