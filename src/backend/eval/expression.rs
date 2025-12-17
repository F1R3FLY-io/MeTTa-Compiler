use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};

use std::sync::Arc;
use tracing::trace;

// TODO -> provide docs/BUILTIN_FUNCTIONS_IMPLEMENTATION.md
// TODO -> update examples/ directory

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
}
