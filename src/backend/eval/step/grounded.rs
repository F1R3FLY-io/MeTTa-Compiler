//! Grounded Arguments Evaluation
//!
//! This module handles the identification of grounded arguments that need
//! evaluation in a hybrid lazy/eager evaluation strategy.

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::MettaValueTrait;

use super::super::{is_eager_special_form, is_grounded_op};

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
    let mut indices = Vec::new();

    // Skip the first item (operator) - we only check arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if let Some(sub_items) = item.as_sexpr() {
            if let Some(first) = sub_items.first() {
                if let Some(op) = first.as_atom() {
                    // Check if this is a grounded operation or a special form
                    // that produces values and needs eager evaluation
                    if is_grounded_op(op) || is_eager_special_form(op) {
                        indices.push(i);
                    }
                    // MeTTa HE alignment: pre-evaluate S-expr args whose head
                    // has user-defined rules (function calls, not data constructors).
                    // Bloom filter: O(1), no false negatives. ~1% false positives
                    // cause harmless extra evaluation (data constructors eval to self).
                    // Use sub_items.len() - 1 to match get_arity() convention
                    // (arity excludes the head/operator itself).
                    else if env.may_have_rules_for(op, sub_items.len() - 1) {
                        indices.push(i);
                    }
                }
            }
        }
    }

    indices
}
