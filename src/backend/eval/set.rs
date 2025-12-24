use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use crate::backend::pathmap_converter::{
    metta_expr_to_pathmap_multiset, metta_expr_to_pathmap_set, pathmap_to_metta_expr,
};

use tracing::trace;

/*
    TODO -> impl checklist

    - [ ] Finish serialization
    - [ ] Implement eval_unique_atom
    - [ ] Finish eval_union_atom
    - [ ] Double check Lattice and DistributiveLattice impls
    - [ ] Custom errors; fix unwraps

    - [ ] Tests for eval/set.rs
    - [ ] Documentation (MeTTa lists being multisets etc.)
    - [ ] Examples

*/

/// Unique atom: (unique-atom $list)
/// Returns only unique entities from a tuple
/// Example: (unique-atom (a b c d d)) -> (a b c d)
pub fn eval_unique_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // TODO -> just take an advantage from default PathMap semantics over Set?

    // metta_expr_to_pathmap_set();

    todo!()
}

/// Union atom: (union-atom $list1 $list2)
/// Returns the union of two tuples
/// Example: (union-atom (a b b c) (b c c d)) -> (a b b c b c c d)
pub fn eval_union_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_union-atom", ?items);
    require_args_with_usage!("union-atom", items, 2, env, "(union-atom left right)");

    let left = &items[1];
    let right = &items[2];

    // TODO -> for union, perhaps just have simple Vec joining?

    // TODO -> fix unwraps
    // let left_pm = metta_expr_to_pathmap(left).unwrap();
    // let right_pm = metta_expr_to_pathmap(right).unwrap();
    // let union_pm = left_pm.join(&right_pm);

    // dbg!(&union_pm);

    // let res = pathmap_to_metta_expr(union_pm).unwrap();
    // dbg!(res);

    todo!()
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

    // TODO -> fix unwraps
    let left_pm = metta_expr_to_pathmap_multiset(left).unwrap();
    let right_pm = metta_expr_to_pathmap_multiset(right).unwrap();
    let intersection_pm = left_pm.meet(&right_pm);
    let res = pathmap_to_metta_expr(intersection_pm).unwrap();

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

    // TODO -> fix unwraps
    let left_pm = metta_expr_to_pathmap_multiset(left).unwrap();
    let right_pm = metta_expr_to_pathmap_multiset(right).unwrap();
    let subtraction_pm = left_pm.subtract(&right_pm);
    let res = pathmap_to_metta_expr(subtraction_pm).unwrap();

    (vec![res], env)
}

// TODO -> tests for each method
#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;
    use crate::backend::environment::Environment;
    use crate::backend::models::MettaValue;
    use crate::eval;

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
    fn test_union_atom() {
        let env = Environment::new();

        // let source = "(union-atom (a b (1 2 3)) (d e f))";
        // let state = compile(source).unwrap();

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
        // assert_eq!(results.len(), 1);
        // match &results[0] {
        //     MettaValue::SExpr(union) => {
        //         assert_eq!(union.len(), 8);
        //         assert_eq!(union[0], MettaValue::Atom("a".to_string()));
        //         assert_eq!(union[1], MettaValue::Atom("b".to_string()));
        //         assert_eq!(union[2], MettaValue::Atom("b".to_string()));
        //         assert_eq!(union[3], MettaValue::Atom("c".to_string()));
        //         assert_eq!(union[4], MettaValue::Atom("b".to_string()));
        //         assert_eq!(union[5], MettaValue::Atom("c".to_string()));
        //         assert_eq!(union[6], MettaValue::Atom("c".to_string()));
        //         assert_eq!(union[7], MettaValue::Atom("d".to_string()));
        //     }
        //     _ => panic!("Expected S-expression result"),
        // }
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
}
