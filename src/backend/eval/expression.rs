use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use std::sync::Arc;
use tracing::trace;

/// Cons atom: (cons-atom head tail)
/// Constructs an expression using two arguments
/// Example: (cons-atom a (b c)) -> (a b c)
pub(super) fn eval_cons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_cons_atom", ?items);
    require_args_with_usage!("cons-atom", items, 2, env, "(cons-atom head tail)");

    let new_head = &items[1];
    let tail_expr = &items[2];

    match tail_expr {
        MettaValue::SExpr(expr_items) => {
            let mut new_items = vec![new_head.clone()];
            new_items.extend(expr_items.iter().cloned());
            let result = MettaValue::SExpr(new_items);
            (vec![result], env)
        }
        MettaValue::Nil => {
            // Treat Nil as empty expression: (cons-atom a ()) -> (a)
            let result = MettaValue::SExpr(vec![new_head.clone()]);
            (vec![result], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (cons-atom <head> (: <tail> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Decons atom: (decons-atom expr)
/// Works as a reverse to cons-atom function. It gets Expression as an input
/// and returns it splitted to head and tail.
/// Example: (decons-atom (Cons X Nil)) -> (Cons (X Nil))
pub(super) fn eval_decons_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_decons_atom", ?items);
    require_args_with_usage!("decons-atom", items, 1, env, "(decons-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => {
            let result = MettaValue::SExpr(vec![
                expr_items[0].clone(),
                MettaValue::SExpr(expr_items[1..].to_vec()),
            ]);
            (vec![result], env)
        }
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!(
                    "expected: (decons-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (decons-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Size atom: (size-atom expr)
/// Returns the size (number of elements) of an expression
/// Example: (size-atom (a b c)) -> 3
pub(super) fn eval_size_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_size_atom", ?items);
    require_args_with_usage!("size-atom", items, 1, env, "(size-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => {
            let size = expr_items.len() as i64;
            (vec![MettaValue::Long(size)], env)
        }
        MettaValue::Nil => (vec![MettaValue::Long(0)], env),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (size-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Index atom: (index-atom expr index)
/// Returns the atom at the given index in the expression, or error if index is out of bounds
/// Example: (index-atom (a b c) 1) -> b
pub(super) fn eval_index_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_index_atom", ?items);
    require_args_with_usage!("index-atom", items, 2, env, "(index-atom expr index)");

    let expr = &items[1];
    let index_val = &items[2];

    // Extract index as i64
    let index = match index_val {
        MettaValue::Long(n) => *n,
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (index-atom (: <expr> Expression) (: <index> Number)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            return (vec![err], env);
        }
    };

    match expr {
        MettaValue::SExpr(expr_items) => {
            if index < 0 || index as usize >= expr_items.len() {
                let err = MettaValue::Error(
                    format!(
                        "index {} out of bounds for expression of size {}",
                        index,
                        expr_items.len()
                    ),
                    Arc::new(MettaValue::SExpr(items.clone())),
                );
                return (vec![err], env);
            }
            (vec![expr_items[index as usize].clone()], env)
        }
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!("cannot index empty expression (index {} requested)", index),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (index-atom (: <expr> Expression) (: <index> Number)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Car atom: (car-atom expr)
/// Extracts the first atom of an expression
/// Example: (car-atom (a b c)) -> a
pub(super) fn eval_car_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_car_atom", ?items);
    require_args_with_usage!("car-atom", items, 1, env, "(car-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => (vec![expr_items[0].clone()], env),
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!(
                    "car-atom expects a non-empty expression as an argument, found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (car-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Cdr atom: (cdr-atom expr)
/// Extracts the tail of an expression (all except first atom)
/// Example: (cdr-atom (a b c)) -> (b c)
pub(super) fn eval_cdr_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_cdr_atom", ?items);
    require_args_with_usage!("cdr-atom", items, 1, env, "(cdr-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => {
            let tail = if expr_items.len() == 1 {
                // Single element: return empty expression (Nil)
                MettaValue::Nil
            } else {
                MettaValue::SExpr(expr_items[1..].to_vec())
            };
            (vec![tail], env)
        }
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!(
                    "cdr-atom expects a non-empty expression as an argument, found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (cdr-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Min atom: (min-atom expr)
/// Returns the atom with minimum value in the expression
/// Only numbers (Long or Float) are allowed
/// Example: (min-atom (5 2 8 1)) -> 1
pub(super) fn eval_min_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_min_atom", ?items);
    require_args_with_usage!("min-atom", items, 1, env, "(min-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => {
            let numbers_with_values: Result<Vec<(f64, &MettaValue)>, MettaValue> = expr_items
                .iter()
                .map(|item| {
                    match item {
                        MettaValue::Long(n) => Ok((*n as f64, item)),
                        MettaValue::Float(f) => Ok((*f, item)),
                        _ => Err(MettaValue::Error(
                            format!(
                                "min-atom expects expression containing only numbers, found non-numeric value: {}",
                                super::friendly_value_repr(item)
                            ),
                            Arc::new(MettaValue::SExpr(items.clone())),
                        )),
                    }
                })
                .collect();

            let numbers_with_values = match numbers_with_values {
                Ok(v) => v,
                Err(e) => return (vec![e], env),
            };

            let (_, min_value) = numbers_with_values
                .iter()
                .min_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap();

            (vec![(*min_value).clone()], env)
        }
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!(
                    "min-atom expects a non-empty expression containing numbers, found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (min-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

/// Max atom: (max-atom expr)
/// Returns the atom with maximum value in the expression
/// Only numbers (Long or Float) are allowed
/// Example: (max-atom (5 2 8 1)) -> 8
pub(super) fn eval_max_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_max_atom", ?items);
    require_args_with_usage!("max-atom", items, 1, env, "(max-atom expr)");

    let expr = &items[1];

    match expr {
        MettaValue::SExpr(expr_items) => {
            let numbers_with_values: Result<Vec<(f64, &MettaValue)>, MettaValue> = expr_items
                .iter()
                .map(|item| {
                    match item {
                        MettaValue::Long(n) => Ok((*n as f64, item)),
                        MettaValue::Float(f) => Ok((*f, item)),
                        _ => Err(MettaValue::Error(
                            format!(
                                "max-atom expects expression containing only numbers, found non-numeric value: {}",
                                super::friendly_value_repr(item)
                            ),
                            Arc::new(MettaValue::SExpr(items.clone())),
                        )),
                    }
                })
                .collect();

            let numbers_with_values = match numbers_with_values {
                Ok(v) => v,
                Err(e) => return (vec![e], env),
            };

            let (_, max_value) = numbers_with_values
                .iter()
                .max_by(|(a, _), (b, _)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap();

            (vec![(*max_value).clone()], env)
        }
        MettaValue::Nil => {
            let err = MettaValue::Error(
                format!(
                    "max-atom expects a non-empty expression containing numbers, found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "expected: (max-atom (: <expr> Expression)), found: {}",
                    super::friendly_value_repr(&MettaValue::SExpr(items.clone()))
                ),
                Arc::new(MettaValue::SExpr(items.clone())),
            );
            (vec![err], env)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;
    use crate::eval;

    #[test]
    fn test_cons_atom_basic() {
        let env = Environment::new();

        // Test: (cons-atom a (b c)) should produce (a b c)
        let source = "(cons-atom a (b c))";
        let state = compile(source).unwrap();
        assert_eq!(state.source.len(), 1);

        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(
            results.len(),
            1,
            "cons-atom should return exactly one result"
        );

        // Verify the result is (a b c)
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);
        assert_eq!(
            results[0], expected,
            "cons-atom should prepend head to tail expression"
        );
    }

    #[test]
    fn test_cons_atom_with_empty_expression() {
        let env = Environment::new();

        // Test: (cons-atom a ()) should produce (a)
        let source = "(cons-atom a ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]);
        assert_eq!(
            results[0], expected,
            "cons-atom with empty expression should produce single-element list"
        );
    }

    #[test]
    fn test_cons_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (cons-atom head (nested (deep (value)))) should produce (head nested (deep (value)))
        let source = "(cons-atom head (nested (deep (value))))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("head".to_string()),
            MettaValue::Atom("nested".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("deep".to_string()),
                MettaValue::SExpr(vec![MettaValue::Atom("value".to_string())]),
            ]),
        ]);
        assert_eq!(
            results[0], expected,
            "cons-atom should preserve nested structure"
        );
    }

    #[test]
    fn test_cons_atom_error_when_tail_is_atom() {
        let env = Environment::new();

        // Test: (cons-atom a b) should produce an error (tail must be Expression, not Atom)
        let source = "(cons-atom a b)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("expected"),
                    "Error should mention expected type"
                );
                assert!(
                    msg.contains("Expression"),
                    "Error should mention Expression type"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_cons_atom_wrong_argument_count() {
        let env = Environment::new();

        // Test: (cons-atom a) should produce an error (missing tail)
        let source = "(cons-atom a)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("cons-atom"));
                assert!(msg.contains("requires exactly 2 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_decons_atom_basic() {
        let env = Environment::new();

        // Test: (decons-atom (a b c)) should produce (a (b c))
        let source = "(decons-atom (a b c))";
        let state = compile(source).unwrap();
        assert_eq!(state.source.len(), 1);

        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(
            results.len(),
            1,
            "decons-atom should return exactly one result"
        );

        // Verify the result is (a (b c))
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ]);
        assert_eq!(
            results[0], expected,
            "decons-atom should split expression into head and tail"
        );
    }

    #[test]
    fn test_decons_atom_with_single_element() {
        let env = Environment::new();

        // Test: (decons-atom (a)) should produce (a ())
        let source = "(decons-atom (a))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        // Note: tail is empty SExpr, which represents ()
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::SExpr(vec![]),
        ]);
        assert_eq!(
            results[0], expected,
            "decons-atom with single element should return head and empty tail"
        );
    }

    #[test]
    fn test_decons_atom_with_empty_expression() {
        let env = Environment::new();

        // Test: (decons-atom ()) should produce an error
        let source = "(decons-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("expected"),
                    "Error should mention expected type"
                );
                assert!(
                    msg.contains("Expression"),
                    "Error should mention Expression type"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_decons_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (decons-atom (a (b c) d)) should produce (a ((b c) d))
        let source = "(decons-atom (a (b c) d))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::SExpr(vec![
                    MettaValue::Atom("b".to_string()),
                    MettaValue::Atom("c".to_string()),
                ]),
                MettaValue::Atom("d".to_string()),
            ]),
        ]);
        assert_eq!(
            results[0], expected,
            "decons-atom should preserve nested structure in tail"
        );
    }

    #[test]
    fn test_decons_atom_error_wrong_argument_count() {
        let env = Environment::new();

        // Test: (decons-atom) should produce an error (missing expr)
        let source = "(decons-atom)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("decons-atom"));
                assert!(msg.contains("requires exactly 1 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_size_atom_basic() {
        let env = Environment::new();

        // Test: (size-atom (a b c)) should produce 3
        let source = "(size-atom (a b c))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(3),
            "size-atom should return the number of elements in expression"
        );
    }

    #[test]
    fn test_size_atom_with_empty_expression() {
        let env = Environment::new();

        // Test: (size-atom ()) should produce 0
        let source = "(size-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(0),
            "size-atom with empty expression should return 0"
        );
    }

    #[test]
    fn test_size_atom_with_single_element() {
        let env = Environment::new();

        // Test: (size-atom (a)) should produce 1
        let source = "(size-atom (a))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(1),
            "size-atom with single element should return 1"
        );
    }

    #[test]
    fn test_size_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (size-atom (a (b c) d)) should produce 3 (nested expressions count as single elements)
        let source = "(size-atom (a (b c) d))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(3),
            "size-atom should count nested expressions as single elements"
        );
    }

    #[test]
    fn test_size_atom_error_wrong_argument_count() {
        let env = Environment::new();

        // Test: (size-atom) should produce an error (missing expr)
        let source = "(size-atom)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("size-atom"));
                assert!(msg.contains("requires exactly 1 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_index_atom_basic() {
        let env = Environment::new();

        // Test: (index-atom (a b c) 1) should produce b
        let source = "(index-atom (a b c) 1)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Atom("b".to_string()),
            "index-atom should return the element at the given index"
        );
    }

    #[test]
    fn test_index_atom_first_element() {
        let env = Environment::new();

        // Test: (index-atom (a b c) 0) should produce a
        let source = "(index-atom (a b c) 0)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Atom("a".to_string()),
            "index-atom should return the first element at index 0"
        );
    }

    #[test]
    fn test_index_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (index-atom (a (b c) d) 1) should produce (b c)
        let source = "(index-atom (a (b c) d) 1)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);
        assert_eq!(
            results[0], expected,
            "index-atom should return nested expressions as-is"
        );
    }

    #[test]
    fn test_index_atom_error_out_of_bounds() {
        let env = Environment::new();

        // Test: (index-atom (a b c) 5) should produce an error
        let source = "(index-atom (a b c) 5)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("out of bounds"),
                    "Error should mention out of bounds"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_index_atom_error_wrong_argument_count() {
        let env = Environment::new();

        // Test: (index-atom (a b c)) should produce an error (missing index)
        let source = "(index-atom (a b c))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("index-atom"));
                assert!(msg.contains("requires exactly 2 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_car_atom_basic() {
        let env = Environment::new();

        // Test: (car-atom (a b c)) should produce a
        let source = "(car-atom (a b c))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Atom("a".to_string()),
            "car-atom should return the first element of the expression"
        );
    }

    #[test]
    fn test_car_atom_with_single_element() {
        let env = Environment::new();

        // Test: (car-atom (a)) should produce a
        let source = "(car-atom (a))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Atom("a".to_string()),
            "car-atom with single element should return that element"
        );
    }

    #[test]
    fn test_car_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (car-atom ((a b) c d)) should produce (a b)
        let source = "(car-atom ((a b) c d))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        assert_eq!(
            results[0], expected,
            "car-atom should return nested expressions as-is"
        );
    }

    #[test]
    fn test_car_atom_error_with_empty_expression() {
        let env = Environment::new();

        // Test: (car-atom ()) should produce an error
        let source = "(car-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-empty expression"),
                    "Error should mention non-empty expression"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_car_atom_error_wrong_argument_count() {
        let env = Environment::new();

        // Test: (car-atom) should produce an error (missing expr)
        let source = "(car-atom)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("car-atom"));
                assert!(msg.contains("requires exactly 1 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_cdr_atom_basic() {
        let env = Environment::new();

        // Test: (cdr-atom (a b c)) should produce (b c)
        let source = "(cdr-atom (a b c))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);
        assert_eq!(
            results[0], expected,
            "cdr-atom should return the tail of the expression"
        );
    }

    #[test]
    fn test_cdr_atom_with_single_element() {
        let env = Environment::new();

        // Test: (cdr-atom (a)) should produce () (Nil)
        let source = "(cdr-atom (a))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Nil,
            "cdr-atom with single element should return empty expression (Nil)"
        );
    }

    #[test]
    fn test_cdr_atom_with_nested_expressions() {
        let env = Environment::new();

        // Test: (cdr-atom (a (b c) d)) should produce ((b c) d)
        let source = "(cdr-atom (a (b c) d))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        let expected = MettaValue::SExpr(vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::Atom("d".to_string()),
        ]);
        assert_eq!(
            results[0], expected,
            "cdr-atom should preserve nested structure in tail"
        );
    }

    #[test]
    fn test_cdr_atom_error_with_empty_expression() {
        let env = Environment::new();

        // Test: (cdr-atom ()) should produce an error
        let source = "(cdr-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-empty expression"),
                    "Error should mention non-empty expression"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_cdr_atom_error_wrong_argument_count() {
        let env = Environment::new();

        // Test: (cdr-atom) should produce an error (missing expr)
        let source = "(cdr-atom)";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("cdr-atom"));
                assert!(msg.contains("requires exactly 1 argument"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_min_atom_basic() {
        let env = Environment::new();

        // Test: (min-atom (5 2 8 1)) should produce 1
        let source = "(min-atom (5 2 8 1))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(1),
            "min-atom should return the minimum value"
        );
    }

    #[test]
    fn test_min_atom_with_single_element() {
        let env = Environment::new();

        // Test: (min-atom (42)) should produce 42
        let source = "(min-atom (42))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(42),
            "min-atom with single element should return that element"
        );
    }

    #[test]
    fn test_min_atom_with_floats() {
        let env = Environment::new();

        // Test: (min-atom (5.5 2.1 8.9 1.0)) should produce 1.0
        // Note: We need to create Float values manually since parser might not support floats
        let expr = MettaValue::SExpr(vec![
            MettaValue::Float(5.5),
            MettaValue::Float(2.1),
            MettaValue::Float(8.9),
            MettaValue::Float(1.0),
        ]);
        let items = vec![MettaValue::Atom("min-atom".to_string()), expr];
        let (results, _) = eval_min_atom(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Float(f) => {
                assert!((f - 1.0).abs() < 0.001, "min-atom should return 1.0");
            }
            other => panic!("Expected Float(1.0), got {:?}", other),
        }
    }

    #[test]
    fn test_min_atom_error_with_empty_expression() {
        let env = Environment::new();

        // Test: (min-atom ()) should produce an error
        let source = "(min-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-empty expression"),
                    "Error should mention non-empty expression"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_min_atom_error_with_non_numeric_value() {
        let env = Environment::new();

        // Test: (min-atom (5 2 hello 8)) should produce an error
        let source = "(min-atom (5 2 hello 8))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-numeric"),
                    "Error should mention non-numeric value"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_max_atom_basic() {
        let env = Environment::new();

        // Test: (max-atom (5 2 8 1)) should produce 8
        let source = "(max-atom (5 2 8 1))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(8),
            "max-atom should return the maximum value"
        );
    }

    #[test]
    fn test_max_atom_with_single_element() {
        let env = Environment::new();

        // Test: (max-atom (42)) should produce 42
        let source = "(max-atom (42))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(42),
            "max-atom with single element should return that element"
        );
    }

    #[test]
    fn test_max_atom_with_floats() {
        let env = Environment::new();

        // Test: (max-atom (5.5 2.1 8.9 1.0)) should produce 8.9
        let expr = MettaValue::SExpr(vec![
            MettaValue::Float(5.5),
            MettaValue::Float(2.1),
            MettaValue::Float(8.9),
            MettaValue::Float(1.0),
        ]);
        let items = vec![MettaValue::Atom("max-atom".to_string()), expr];
        let (results, _) = eval_max_atom(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Float(f) => {
                assert!((f - 8.9).abs() < 0.001, "max-atom should return 8.9");
            }
            other => panic!("Expected Float(8.9), got {:?}", other),
        }
    }

    #[test]
    fn test_max_atom_error_with_empty_expression() {
        let env = Environment::new();

        // Test: (max-atom ()) should produce an error
        let source = "(max-atom ())";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-empty expression"),
                    "Error should mention non-empty expression"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_max_atom_error_with_non_numeric_value() {
        let env = Environment::new();

        // Test: (max-atom (5 2 hello 8)) should produce an error
        let source = "(max-atom (5 2 hello 8))";
        let state = compile(source).unwrap();
        let (results, _) = eval(state.source[0].clone(), env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("non-numeric"),
                    "Error should mention non-numeric value"
                );
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
