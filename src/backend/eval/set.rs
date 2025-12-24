use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use crate::backend::pathmap_converter::{
    metta_expr_to_pathmap_multiset, pathmap_multiset_to_metta_expr,
};
use std::sync::Arc;

use tracing::trace;

/// Unique atom: (unique-atom $list)
/// Returns only unique entities from a tuple
/// Example: (unique-atom (a b c d d)) -> (a b c d)
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

/// Union atom: (union-atom $list1 $list2)
/// Returns the union of two tuples
/// Example: (union-atom (a b b c) (b c c d)) -> (a b b c b c c d)
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

/// Intersection atom: (intersection-atom $list1 $list2)
/// Returns the intersection of two tuples
/// Example: (intersection-atom (a b c c) (b c c c d)) -> (b c c)
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

    // TODO -> check if left and right are lists

    // TODO -> fix unwraps
    let left_pm = metta_expr_to_pathmap_multiset(left).unwrap();
    let right_pm = metta_expr_to_pathmap_multiset(right).unwrap();
    let intersection_pm = left_pm.meet(&right_pm);
    let res = pathmap_multiset_to_metta_expr(intersection_pm).unwrap();

    (vec![res], env)
}

/// Subtraction atom: (subtraction-atom $list1 $list2)
/// Returns the subtraction of two tuples
/// Example: (subtraction-atom (a b b c) (b c c d)) -> (a b)
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

    // TODO -> check if left and right are lists

    // TODO -> fix unwraps
    let left_pm = metta_expr_to_pathmap_multiset(left).unwrap();
    let right_pm = metta_expr_to_pathmap_multiset(right).unwrap();
    let subtraction_pm = left_pm.subtract(&right_pm);
    let res = pathmap_multiset_to_metta_expr(subtraction_pm).unwrap();

    (vec![res], env)
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
}
