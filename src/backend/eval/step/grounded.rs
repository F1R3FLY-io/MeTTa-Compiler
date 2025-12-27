//! Grounded Arguments Evaluation
//!
//! This module handles the evaluation of grounded arguments in a hybrid
//! lazy/eager evaluation strategy.

use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

use super::super::{eval, is_grounded_op};

/// Evaluate arguments that are grounded operations (hybrid lazy/eager evaluation).
///
/// This function implements a key insight for MeTTa evaluation:
/// - Grounded operations (like arithmetic) should be evaluated BEFORE pattern matching
/// - User-defined expressions should remain unevaluated for lazy pattern matching
///
/// Example: For `(countdown (- 3 1))`:
/// - The argument `(- 3 1)` is a grounded operation, so evaluate it to `2`
/// - Result: `(countdown 2)` - now pattern matching works correctly
///
/// Example: For `(wrapper $a (add-atom &stack x))`:
/// - The argument `(add-atom &stack x)` is NOT grounded (user-defined side effect)
/// - Keep it unevaluated for lazy pattern matching
pub fn evaluate_grounded_args(items: &[MettaValue], env: &Environment) -> Vec<MettaValue> {
    if items.is_empty() {
        return items.to_vec();
    }

    let mut result = Vec::with_capacity(items.len());

    // Keep the first item (operator) as-is
    result.push(items[0].clone());

    // Process arguments (items after the first)
    for item in &items[1..] {
        match item {
            MettaValue::SExpr(sub_items) if !sub_items.is_empty() => {
                // Check if this is a grounded operation
                if let Some(MettaValue::Atom(op)) = sub_items.first() {
                    if is_grounded_op(op) {
                        // This is a grounded operation - evaluate it eagerly
                        // Recursively evaluate grounded args in sub-expression first
                        let evaluated_sub = evaluate_grounded_args(sub_items, env);
                        let (results, _) = eval(MettaValue::SExpr(evaluated_sub), env.clone());

                        // Use the first result (deterministic evaluation for grounded ops)
                        if let Some(first_result) = results.first() {
                            result.push(first_result.clone());
                        } else {
                            // Evaluation returned nothing - keep original
                            result.push(item.clone());
                        }
                        continue;
                    }
                }
                // Not a grounded operation - keep unevaluated (lazy)
                result.push(item.clone());
            }
            _ => {
                // Not an S-expression - keep as-is
                result.push(item.clone());
            }
        }
    }

    result
}
