//! # MeTTa Set Operations
//!
//! This module implements set operations for MeTTa tuples with **multiset semantics**,
//! aligned with the MeTTa HE (hyperon-experimental) reference implementation.
//!
//! ## Semantics Overview
//!
//! All operations treat tuples as **multisets** (bags) where element multiplicity matters:
//!
//! | Operation | Semantics | Example |
//! |-----------|-----------|---------|
//! | `unique-atom` | Remove duplicates, keep first occurrence | `(a b a c b)` → `(a b c)` |
//! | `union-atom` | Concatenate (preserves all multiplicities) | `(a b)` ∪ `(b c)` → `(a b b c)` |
//! | `intersection-atom` | min(left count, right count) | `(a b c c)` ∩ `(b c c c d)` → `(b c c)` |
//! | `subtraction-atom` | left count − right count (saturating) | `(a b b c)` − `(b c c d)` → `(a b)` |
//!
//! ## Order Preservation
//!
//! All operations preserve element order from the **left input**:
//! - `unique-atom`: Order of first occurrences preserved
//! - `union-atom`: Left elements first, then right elements
//! - `intersection-atom`: Order from left input
//! - `subtraction-atom`: Order from left input
//!
//! ## Comparison Function
//!
//! Elements are compared using **structural equality** (`PartialEq`/`Hash`), matching
//! MeTTa HE's use of `==` for intersection/subtraction operations.
//!
//! ## Reference
//!
//! See: `hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs`

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::collections::HashMap;
use std::sync::Arc;

use tracing::trace;

/// Extracts a `Vec<MettaValue>` from a list argument, with proper error handling.
/// Returns `Ok(Vec)` for `SExpr`/`Nil`, or `Err` with appropriate error `MettaValue`.
fn extract_list_arg(
    op_name: &str,
    arg: &MettaValue,
    arg_position: &str,
) -> Result<Vec<MettaValue>, MettaValue> {
    match arg {
        MettaValue::SExpr(vec) => Ok(vec.clone()),
        MettaValue::Nil => Ok(Vec::new()),
        _ => Err(MettaValue::Error(
            format!("{}: {} argument must be a list", op_name, arg_position),
            Arc::new(arg.clone()),
        )),
    }
}

/// Removes duplicate elements from a tuple.
///
/// # Syntax
/// ```text
/// (unique-atom $list)
/// ```
///
/// # Semantics
/// - **Deduplication**: Only the first occurrence of each element is kept
/// - **Order**: Preserved (order of first occurrences)
/// - **Comparison**: Structural equality via `PartialEq`/`Hash`
///
/// # Example
/// ```text
/// (unique-atom (a b c d d)) -> (a b c d)
/// (unique-atom (a b a c b)) -> (a b c)
/// ```
///
/// # Complexity
/// - Time: O(n)
/// - Space: O(n) for the HashSet
pub fn eval_unique_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("unique-atom", items, 1, env, "(unique-atom list)");

    let input = &items[1];
    let input_vec = match input {
        MettaValue::SExpr(vec) => vec.clone(),
        MettaValue::Nil => Vec::new(),
        _ => {
            return (
                vec![MettaValue::Error(
                    "unique-atom: argument must be a list".to_string(),
                    Arc::new(input.clone()),
                )],
                env,
            );
        }
    };

    let mut seen = std::collections::HashSet::new();
    let mut unique_items = Vec::new();

    for item in input_vec {
        if seen.insert(item.clone()) {
            unique_items.push(item);
        }
    }

    let result = MettaValue::SExpr(unique_items);
    (vec![result], env)
}

/// Computes the multiset union of two tuples (concatenation).
///
/// # Syntax
/// ```text
/// (union-atom $list1 $list2)
/// ```
///
/// # Semantics
/// - **Multiset union**: All elements from both lists are preserved (no deduplication)
/// - **Order**: Left elements first, then right elements
/// - **Multiplicities**: Sum of counts from both lists
///
/// # Example
/// ```text
/// (union-atom (a b b c) (b c c d)) -> (a b b c b c c d)
/// (union-atom (a b) (c d)) -> (a b c d)
/// ```
///
/// # Complexity
/// - Time: O(n + m)
/// - Space: O(n + m) for the result
pub fn eval_union_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_union-atom", ?items);
    require_args_with_usage!("union-atom", items, 2, env, "(union-atom left right)");

    let left = &items[1];
    let right = &items[2];

    let left_vec = match left {
        MettaValue::SExpr(vec) => vec.clone(),
        MettaValue::Nil => Vec::new(),
        _ => {
            return (
                vec![MettaValue::Error(
                    "union-atom: left argument must be a list".to_string(),
                    Arc::new(left.clone()),
                )],
                env,
            );
        }
    };

    let right_vec = match right {
        MettaValue::SExpr(vec) => vec.clone(),
        MettaValue::Nil => Vec::new(),
        _ => {
            return (
                vec![MettaValue::Error(
                    "union-atom: right argument must be a list".to_string(),
                    Arc::new(right.clone()),
                )],
                env,
            );
        }
    };

    let mut union_vec = left_vec;
    union_vec.extend(right_vec);

    let result = MettaValue::SExpr(union_vec);
    (vec![result], env)
}

/// Computes the multiset intersection of two tuples.
///
/// # Syntax
/// ```text
/// (intersection-atom $list1 $list2)
/// ```
///
/// # Semantics (MeTTa HE aligned)
/// - **Multiset intersection**: For each element, result contains min(left count, right count)
/// - **Order**: Preserved from the **left** input
/// - **Comparison**: Structural equality via `PartialEq`/`Hash` (matches MeTTa HE's `==`)
///
/// # Algorithm
/// 1. Build a count map from the right list
/// 2. Iterate left list in order, emit element if count > 0 and decrement
///
/// # Example
/// ```text
/// (intersection-atom (a b c c) (b c c c d)) -> (b c c)
///   ; 'a': left=1, right=0 → 0 (not in result)
///   ; 'b': left=1, right=0 → min(1,0)=0? No, 'b' is in right once
///   ; Actually: 'b' left=1, right=1 → 1; 'c' left=2, right=3 → min(2,3)=2
///
/// (intersection-atom (z y x) (x z)) -> (z x)
///   ; Order from left: z, then x (y not in right)
/// ```
///
/// # Complexity
/// - Time: O(n + m) where n = left size, m = right size
/// - Space: O(m) for the count map
pub fn eval_intersection_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_intersection-atom", ?items);
    require_args_with_usage!(
        "intersection-atom",
        items,
        2,
        env,
        "(intersection-atom left right)"
    );

    let left = &items[1];
    let right = &items[2];

    // Validate and extract left argument
    let left_vec = match extract_list_arg("intersection-atom", left, "left") {
        Ok(vec) => vec,
        Err(err) => return (vec![err], env),
    };

    // Validate and extract right argument
    let right_vec = match extract_list_arg("intersection-atom", right, "right") {
        Ok(vec) => vec,
        Err(err) => return (vec![err], env),
    };

    // Build count map from right list
    let mut right_counts: HashMap<MettaValue, usize> = HashMap::with_capacity(right_vec.len());
    for item in right_vec {
        *right_counts.entry(item).or_default() += 1;
    }

    // Filter left list, consuming counts from right (preserves left input order)
    let mut result = Vec::with_capacity(left_vec.len().min(right_counts.len()));
    for item in left_vec {
        if let Some(count) = right_counts.get_mut(&item) {
            if *count > 0 {
                *count -= 1;
                result.push(item);
            }
        }
    }

    (vec![MettaValue::SExpr(result)], env)
}

/// Computes the multiset difference of two tuples (left minus right).
///
/// # Syntax
/// ```text
/// (subtraction-atom $list1 $list2)
/// ```
///
/// # Semantics (MeTTa HE aligned)
/// - **Multiset subtraction**: For each element, result contains max(0, left count − right count)
/// - **Order**: Preserved from the **left** input
/// - **Comparison**: Structural equality via `PartialEq`/`Hash` (matches MeTTa HE's `==`)
/// - **Saturating**: Counts never go negative; if right count ≥ left count, element is removed
///
/// # Algorithm
/// 1. Build a count map from the right list
/// 2. Iterate left list in order, emit element only if not found in right (or count exhausted)
///
/// # Example
/// ```text
/// (subtraction-atom (a b b c) (b c c d)) -> (a b)
///   ; 'a': left=1, right=0 → 1-0=1 (kept)
///   ; 'b': left=2, right=1 → 2-1=1 (one kept)
///   ; 'c': left=1, right=2 → 1-2=0 (removed, saturating)
///   ; 'd': only in right, not in left
///
/// (subtraction-atom (z y x w v u) (w x)) -> (z y v u)
///   ; Order from left preserved: z, y, v, u (w and x removed)
/// ```
///
/// # Complexity
/// - Time: O(n + m) where n = left size, m = right size
/// - Space: O(m) for the count map
pub fn eval_subtraction_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_subtraction-atom", ?items);
    require_args_with_usage!(
        "subtraction-atom",
        items,
        2,
        env,
        "(subtraction-atom left right)"
    );

    let left = &items[1];
    let right = &items[2];

    // Validate and extract left argument
    let left_vec = match extract_list_arg("subtraction-atom", left, "left") {
        Ok(vec) => vec,
        Err(err) => return (vec![err], env),
    };

    // Validate and extract right argument
    let right_vec = match extract_list_arg("subtraction-atom", right, "right") {
        Ok(vec) => vec,
        Err(err) => return (vec![err], env),
    };

    // Build count map from right list
    let mut right_counts: HashMap<MettaValue, usize> = HashMap::with_capacity(right_vec.len());
    for item in right_vec {
        *right_counts.entry(item).or_default() += 1;
    }

    // Filter left list, removing items that exist in right (preserves left input order)
    let mut result = Vec::with_capacity(left_vec.len());
    for item in left_vec {
        if let Some(count) = right_counts.get_mut(&item) {
            if *count > 0 {
                *count -= 1;
                // Skip this item (subtract it)
                continue;
            }
        }
        result.push(item);
    }

    (vec![MettaValue::SExpr(result)], env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::Environment;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_unique_atom() {
        let env = Environment::new();

        // (unique-atom (a b c d d)) -> (a b c d)
        let items = vec![
            MettaValue::Atom("unique-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];

        let (results, _) = eval_unique_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(unique) => {
                assert_eq!(unique.len(), 4);
                assert_eq!(unique[0], MettaValue::Atom("a".to_string()));
                assert_eq!(unique[1], MettaValue::Atom("b".to_string()));
                assert_eq!(unique[2], MettaValue::Atom("c".to_string()));
                assert_eq!(unique[3], MettaValue::Atom("d".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_unique_atom_empty_list() {
        let env = Environment::new();

        // (unique-atom ()) -> ()
        let items = vec![
            MettaValue::Atom("unique-atom".to_string()),
            MettaValue::SExpr(vec![]),
        ];

        let (results, _) = eval_unique_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(unique) => {
                assert_eq!(unique.len(), 0);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_unique_atom_mixed_types() {
        let env = Environment::new();

        // (unique-atom (a 1 1 "hello" true false a)) -> (a 1 "hello" true false)
        let items = vec![
            MettaValue::Atom("unique-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
                MettaValue::Bool(true),
                MettaValue::Bool(false),
                MettaValue::Atom("a".to_string()),
            ]),
        ];

        let (results, _) = eval_unique_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(unique) => {
                assert_eq!(unique.len(), 5);
                // Check that all unique values are present
                assert!(unique.contains(&MettaValue::Atom("a".to_string())));
                assert!(unique.contains(&MettaValue::Long(1)));
                assert!(unique.contains(&MettaValue::String("hello".to_string())));
                assert!(unique.contains(&MettaValue::Bool(true)));
                assert!(unique.contains(&MettaValue::Bool(false)));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_union_atom() {
        let env = Environment::new();

        // (union-atom (a b b c) (b c c d)) -> (a b b c b c c d)
        let items = vec![
            MettaValue::Atom("union-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];

        let (results, _) = eval_union_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(union) => {
                assert_eq!(union.len(), 8);
                assert_eq!(union[0], MettaValue::Atom("a".to_string()));
                assert_eq!(union[1], MettaValue::Atom("b".to_string()));
                assert_eq!(union[2], MettaValue::Atom("b".to_string()));
                assert_eq!(union[3], MettaValue::Atom("c".to_string()));
                assert_eq!(union[4], MettaValue::Atom("b".to_string()));
                assert_eq!(union[5], MettaValue::Atom("c".to_string()));
                assert_eq!(union[6], MettaValue::Atom("c".to_string()));
                assert_eq!(union[7], MettaValue::Atom("d".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_union_atom_empty_lists() {
        let env = Environment::new();

        // (union-atom () ()) -> ()
        let items = vec![
            MettaValue::Atom("union-atom".to_string()),
            MettaValue::SExpr(vec![]),
            MettaValue::SExpr(vec![]),
        ];

        let (results, _) = eval_union_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(union) => {
                assert_eq!(union.len(), 0);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_union_atom_mixed_types() {
        let env = Environment::new();

        // (union-atom (a 1) (2 "hello")) -> (a 1 2 "hello")
        let items = vec![
            MettaValue::Atom("union-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string()), MettaValue::Long(1)]),
            MettaValue::SExpr(vec![
                MettaValue::Long(2),
                MettaValue::String("hello".to_string()),
            ]),
        ];

        let (results, _) = eval_union_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(union) => {
                assert_eq!(union.len(), 4);
                assert_eq!(union[0], MettaValue::Atom("a".to_string()));
                assert_eq!(union[1], MettaValue::Long(1));
                assert_eq!(union[2], MettaValue::Long(2));
                assert_eq!(union[3], MettaValue::String("hello".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_intersection_atom() {
        let env = Environment::new();

        // (intersection-atom (a b c c) (b c c c d)) -> (b c c)
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];

        let (results, _) = eval_intersection_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(intersection) => {
                assert_eq!(intersection.len(), 3);
                assert_eq!(intersection[0], MettaValue::Atom("b".to_string()));
                assert_eq!(intersection[1], MettaValue::Atom("c".to_string()));
                assert_eq!(intersection[2], MettaValue::Atom("c".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_intersection_atom_empty_result() {
        let env = Environment::new();

        // (intersection-atom (a b) (c d)) -> ()
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];

        let (results, _) = eval_intersection_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(intersection) => {
                assert_eq!(intersection.len(), 0);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_intersection_atom_mixed_types() {
        let env = Environment::new();

        // (intersection-atom (a 1 "hello" true) (1 "hello" 2 false)) -> (1 "hello")
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
                MettaValue::Bool(true),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
                MettaValue::Long(2),
                MettaValue::Bool(false),
            ]),
        ];

        let (results, _) = eval_intersection_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(intersection) => {
                assert_eq!(intersection.len(), 2);
                // Check that both common values are present
                assert!(intersection.contains(&MettaValue::Long(1)));
                assert!(intersection.contains(&MettaValue::String("hello".to_string())));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom() {
        let env = Environment::new();

        // (subtraction-atom (a b b c) (b c c d)) -> (a b)
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];

        let (results, _) = eval_subtraction_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 2);
                assert_eq!(subtraction[0], MettaValue::Atom("a".to_string()));
                assert_eq!(subtraction[1], MettaValue::Atom("b".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom_empty_result() {
        let env = Environment::new();

        // (subtraction-atom (a b) (a b)) -> ()
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
        ];

        let (results, _) = eval_subtraction_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 0);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom_mixed_types() {
        let env = Environment::new();

        // (subtraction-atom (a 1 "hello" true) (1 "hello")) -> (a true)
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
                MettaValue::Bool(true),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::String("hello".to_string()),
            ]),
        ];

        let (results, _) = eval_subtraction_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 2);
                // Check that remaining values are present
                assert!(subtraction.contains(&MettaValue::Atom("a".to_string())));
                assert!(subtraction.contains(&MettaValue::Bool(true)));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // ================== Error Handling Tests ==================

    #[test]
    fn test_intersection_atom_non_list_left() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::Atom("not-a-list".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        ];
        let (results, _) = eval_intersection_atom(items, env);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_intersection_atom_non_list_right() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
            MettaValue::Long(42),
        ];
        let (results, _) = eval_intersection_atom(items, env);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_subtraction_atom_non_list_left() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::Bool(true),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        ];
        let (results, _) = eval_subtraction_atom(items, env);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_subtraction_atom_non_list_right() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
            MettaValue::String("not-a-list".to_string()),
        ];
        let (results, _) = eval_subtraction_atom(items, env);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    // ================== Nested S-Expression Tests ==================

    #[test]
    fn test_intersection_atom_nested_sexpr() {
        let env = Environment::new();
        // Test with nested s-expressions: ((a b) (c d)) intersect ((a b) (e f)) -> ((a b))
        let nested_ab = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        let nested_cd = MettaValue::SExpr(vec![
            MettaValue::Atom("c".to_string()),
            MettaValue::Atom("d".to_string()),
        ]);
        let nested_ef = MettaValue::SExpr(vec![
            MettaValue::Atom("e".to_string()),
            MettaValue::Atom("f".to_string()),
        ]);

        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![nested_ab.clone(), nested_cd]),
            MettaValue::SExpr(vec![nested_ab.clone(), nested_ef]),
        ];

        let (results, _) = eval_intersection_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(intersection) => {
                assert_eq!(intersection.len(), 1);
                assert_eq!(intersection[0], nested_ab);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom_nested_sexpr() {
        let env = Environment::new();
        // Test with nested s-expressions: ((a b) (c d)) - ((a b)) -> ((c d))
        let nested_ab = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        let nested_cd = MettaValue::SExpr(vec![
            MettaValue::Atom("c".to_string()),
            MettaValue::Atom("d".to_string()),
        ]);

        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![nested_ab.clone(), nested_cd.clone()]),
            MettaValue::SExpr(vec![nested_ab]),
        ];

        let (results, _) = eval_subtraction_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 1);
                assert_eq!(subtraction[0], nested_cd);
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // ================== Order Preservation Tests ==================

    #[test]
    fn test_subtraction_atom_order_preserved_complex() {
        let env = Environment::new();
        // (subtraction-atom (z y x w v u) (w x)) -> (z y v u)
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("z".to_string()),
                MettaValue::Atom("y".to_string()),
                MettaValue::Atom("x".to_string()),
                MettaValue::Atom("w".to_string()),
                MettaValue::Atom("v".to_string()),
                MettaValue::Atom("u".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("w".to_string()),
                MettaValue::Atom("x".to_string()),
            ]),
        ];

        let (results, _) = eval_subtraction_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 4);
                assert_eq!(subtraction[0], MettaValue::Atom("z".to_string()));
                assert_eq!(subtraction[1], MettaValue::Atom("y".to_string()));
                assert_eq!(subtraction[2], MettaValue::Atom("v".to_string()));
                assert_eq!(subtraction[3], MettaValue::Atom("u".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_intersection_atom_order_preserved_complex() {
        let env = Environment::new();
        // (intersection-atom (z y x w v u) (u w y)) -> (y w u)
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("z".to_string()),
                MettaValue::Atom("y".to_string()),
                MettaValue::Atom("x".to_string()),
                MettaValue::Atom("w".to_string()),
                MettaValue::Atom("v".to_string()),
                MettaValue::Atom("u".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("u".to_string()),
                MettaValue::Atom("w".to_string()),
                MettaValue::Atom("y".to_string()),
            ]),
        ];

        let (results, _) = eval_intersection_atom(items, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(intersection) => {
                assert_eq!(intersection.len(), 3);
                // Order preserved from left input: y, w, u (not u, w, y from right)
                assert_eq!(intersection[0], MettaValue::Atom("y".to_string()));
                assert_eq!(intersection[1], MettaValue::Atom("w".to_string()));
                assert_eq!(intersection[2], MettaValue::Atom("u".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }

    // ================== Nil Input Tests ==================

    #[test]
    fn test_intersection_atom_nil_left() {
        let env = Environment::new();

        // (intersection-atom () (a b)) -> ()
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::Nil,
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
        ];
        let (results, _) = eval_intersection_atom(items, env);
        match &results[0] {
            MettaValue::SExpr(intersection) => assert_eq!(intersection.len(), 0),
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_intersection_atom_nil_right() {
        let env = Environment::new();

        // (intersection-atom (a b) ()) -> ()
        let items = vec![
            MettaValue::Atom("intersection-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::Nil,
        ];
        let (results, _) = eval_intersection_atom(items, env);
        match &results[0] {
            MettaValue::SExpr(intersection) => assert_eq!(intersection.len(), 0),
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom_nil_left() {
        let env = Environment::new();

        // (subtraction-atom () (a b)) -> ()
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::Nil,
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
        ];
        let (results, _) = eval_subtraction_atom(items, env);
        match &results[0] {
            MettaValue::SExpr(subtraction) => assert_eq!(subtraction.len(), 0),
            _ => panic!("Expected S-expression result"),
        }
    }

    #[test]
    fn test_subtraction_atom_nil_right() {
        let env = Environment::new();

        // (subtraction-atom (a b) ()) -> (a b)
        let items = vec![
            MettaValue::Atom("subtraction-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::Nil,
        ];
        let (results, _) = eval_subtraction_atom(items, env);
        match &results[0] {
            MettaValue::SExpr(subtraction) => {
                assert_eq!(subtraction.len(), 2);
                assert_eq!(subtraction[0], MettaValue::Atom("a".to_string()));
                assert_eq!(subtraction[1], MettaValue::Atom("b".to_string()));
            }
            _ => panic!("Expected S-expression result"),
        }
    }
}
