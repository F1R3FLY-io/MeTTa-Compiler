use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use crate::backend::pathmap_converter::metta_expr_to_pathmap;

use pathmap::PathMap;
use tracing::trace;

// TODO -> comment about MeTTa lists being multisets

/*
  TODO -> serialization strategy options
  - Canonical String Serialization -> Recursive?
  - Structural Hash
  - PathMap with Structural Keys?


  TODO -> example with PathMap rationalization
  If you need several set operations, PathMap avoids repeated serialization:
  Convert once, use many times
  let pathmap1 = items_to_pathmap(list1);
  let pathmap2 = items_to_pathmap(list2);

  let results = vec![
      pathmap1.join(&pathmap2),     // union
      pathmap1.meet(&pathmap2),     // intersection
      pathmap1.subtract(&pathmap2), // difference
  ];


  TODO -> implementation checklist
  - [ ] Implement back and forth conversion between metta value and path map. Choose algorithm
        for serializatio nested structures. Make sure PathMap is used efficient. Use zippers?
  - [ ] Implemented main methods: eval_unique_atom, eval_union_atom, eval_intersection_atom, eval_subtraction_atom
  - [ ] ?Provide optimization for rust interpreter level manipulations for < 100 lists?

  - [ ] Provide benchmarks
  - [ ] Update examples and docs
*/

/// Unique atom: (unique-atom $list)
/// Returns only unique entities from a tuple
/// Example: (unique-atom (a b c d d)) -> (a b c d)
pub fn eval_unique_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // TODO -> just take an advantage from default PathMap semantics over Set?

    todo!()
}

/// Union atom: (union-atom $list1 $list2)
/// Returns the union of two tuples
/// Example: (union-atom (a b b c) (b c c d)) -> (a b b c b c c d)
pub fn eval_union_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_union-atom", ?items);
    require_args_with_usage!("union-atom", items, 2, env, "(union-atom list_a list_b)");

    let list_a = &items[1];
    let list_b = &items[2];

    // TODO -> need to check if list_a and list_b are S-expressions or Nil
    // TODO -> what about Conjunction?

    let pm1 = metta_expr_to_pathmap(list_a).unwrap();
    let pm2 = metta_expr_to_pathmap(list_b).unwrap();

    let union_result = pm1.join(&pm2);

    // println!("pm1 before join: {:?}", pm1);
    // println!("pm2 before join: {:?}", pm2);
    // let union_result = pm1.join(&pm2);
    // println!("union_result: {:?}", union_result);

    dbg!(union_result);

    // for (key_bytes, value) in union_result.iter() {
    //     let key_string = String::from_utf8(key_bytes.to_vec())
    //         .map_err(|_| "Invalid UTF-8 in PathMap key")
    //         .unwrap();
    //     println!("{} {:?}", key_string, value);
    // }

    /*


      let pathmap1 = items_to_pathmap(list1);
      let pathmap2 = items_to_pathmap(list2);

      let union_result = pathmap1.join(&pathmap2);  // Ring algebra!

      pathmap_to_items(union_result)
    */

    todo!()
}

/// Intersection atom: (intersection-atom $list1 $list2)
/// Returns the intersection of two tuples
/// Example: (intersection-atom (a b c c) (b c c c d)) -> (b c c)
pub fn eval_intersection_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    todo!()
}

/// Subtraction atom: (subtraction-atom $list1 $list2)
/// Returns the subtraction of two tuples
/// Example: (subtraction-atom (a b b c) (b c c d)) -> (a b)
pub fn eval_subtraction_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    todo!()
}

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
