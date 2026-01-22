use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use tracing::trace;

use super::eval;

/// Quote: return argument wrapped in Quoted to prevent evaluation
/// Variables ARE substituted before wrapping (via normal evaluation flow)
pub(super) fn eval_quote(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_quote", ?items);
    require_args_with_usage!("quote", items, 1, env, "(quote expr)");

    // Wrap the expression in Quoted to prevent further evaluation
    // Variable substitution happens naturally through apply_bindings before this is called
    (vec![MettaValue::Quoted(Box::new(items[1].clone()))], env)
}

/// Unquote: extract the inner value from a quoted expression and evaluate it
/// Implements the MeTTa spec: (= (unquote (quote $atom)) $atom)
///
/// Semantics:
/// - (unquote (quote X)) → unwraps to X, evaluates X, returns result
/// - (unquote Y) where Y is not Quoted → returns (unquote Y) as unevaluated data
///
/// This is the inverse of quote, allowing extraction and evaluation of quoted expressions
pub(super) fn eval_unquote(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    trace!(target: "mettatron::eval::eval_unquote", ?items);
    require_args_with_usage!("unquote", items, 1, env, "(unquote expr)");

    // Evaluate the argument (handles pattern matching, rules, etc.)
    let (arg_results, arg_env) = eval(items[1].clone(), env);

    // Process each result (supports non-deterministic evaluation)
    let mut final_results = Vec::new();
    let mut current_env = arg_env;

    for result in arg_results {
        match result {
            MettaValue::Quoted(inner) => {
                // Pattern matched: (unquote (quote X)) → evaluate X
                // This is the core unquote semantics: unwrap and evaluate
                let (eval_results, new_env) = eval(*inner, current_env.clone());
                final_results.extend(eval_results);
                current_env = new_env;
            }
            other => {
                // Pattern didn't match: return (unquote Y) as unevaluated data
                // This matches MeTTa behavior when unquote receives non-quoted values
                final_results.push(MettaValue::SExpr(vec![
                    MettaValue::Atom("unquote".to_string()),
                    other,
                ]));
            }
        }
    }

    (final_results, current_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::eval;
    use crate::backend::models::Rule;
    use std::sync::Arc;

    #[test]
    fn test_quote_missing_argument() {
        let env = Environment::new();

        // (quote) - missing argument
        let value = MettaValue::SExpr(vec![MettaValue::Atom("quote".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("quote"));
                assert!(msg.contains("argument")); // Just check for "argument" - flexible
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_quote_prevents_evaluation() {
        let env = Environment::new();

        // (quote (+ 1 2))
        // Should return the expression unevaluated
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);

        // Should be the unevaluated s-expression wrapped in Quoted
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("+".to_string()));
                }
                _ => panic!("Expected SExpr inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_variable() {
        let env = Environment::new();

        // (quote $x)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Quoted(Box::new(MettaValue::Atom("$x".to_string())))
        );
    }

    #[test]
    fn test_quote_with_complex_nested_expressions() {
        let env = Environment::new();

        // Test quoting deeply nested expressions
        // (quote (+ 1 (* 2 (/ 6 3))))
        let nested_quote = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(2),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("/".to_string()),
                        MettaValue::Long(6),
                        MettaValue::Long(3),
                    ]),
                ]),
            ]),
        ]);

        let (results, _) = eval(nested_quote, env);
        assert_eq!(results.len(), 1);

        // Should return the exact structure without evaluation, wrapped in Quoted
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3); // +, 1, and the nested expression
                    assert_eq!(items[0], MettaValue::Atom("+".to_string()));
                    assert_eq!(items[1], MettaValue::Long(1));
                    match &items[2] {
                        MettaValue::SExpr(inner_items) => {
                            assert_eq!(inner_items.len(), 3); // *, 2, and the inner nested expression
                            assert_eq!(inner_items[0], MettaValue::Atom("*".to_string()));
                        }
                        _ => panic!("Expected nested S-expression"),
                    }
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_different_value_types() {
        let env = Environment::new();

        // Test quoting different types of values
        let test_cases = vec![
            // (quote 42)
            (MettaValue::Long(42), MettaValue::Long(42)),
            // (quote true)
            (MettaValue::Bool(true), MettaValue::Bool(true)),
            // (quote "hello")
            (
                MettaValue::String("hello".to_string()),
                MettaValue::String("hello".to_string()),
            ),
            // (quote nil)
            (MettaValue::Nil, MettaValue::Nil),
            // (quote foo)
            (
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("foo".to_string()),
            ),
        ];

        for (input, expected) in test_cases {
            let quote_expr = MettaValue::SExpr(vec![MettaValue::Atom("quote".to_string()), input]);
            let (results, _) = eval(quote_expr, env.clone());
            assert_eq!(results.len(), 1);
            // Now expect Quoted wrapper
            assert_eq!(results[0], MettaValue::Quoted(Box::new(expected)));
        }
    }

    #[test]
    fn test_quote_with_variables() {
        let env = Environment::new();

        // Test quoting variables with different prefixes
        let variable_cases = vec![
            "$x",
            "&y",
            "'z",
            "_",
            "$var123",
            "&long-name",
            "'special-char",
        ];

        for var_name in variable_cases {
            let quote_var = MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::Atom(var_name.to_string()),
            ]);
            let (results, _) = eval(quote_var, env.clone());
            assert_eq!(results.len(), 1);
            // Now expect Quoted wrapper
            assert_eq!(
                results[0],
                MettaValue::Quoted(Box::new(MettaValue::Atom(var_name.to_string())))
            );
        }
    }

    #[test]
    fn test_quote_with_expressions_containing_special_forms() {
        let env = Environment::new();

        // Test quoting expressions that contain special forms
        // (quote (if (> 5 3) "yes" "no"))
        let quote_if = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Long(5),
                    MettaValue::Long(3),
                ]),
                MettaValue::String("yes".to_string()),
                MettaValue::String("no".to_string()),
            ]),
        ]);

        let (results, _) = eval(quote_if, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 4);
                    assert_eq!(items[0], MettaValue::Atom("if".to_string()));
                    // Should not be evaluated, so condition remains as-is
                    match &items[1] {
                        MettaValue::SExpr(cond) => {
                            assert_eq!(cond[0], MettaValue::Atom(">".to_string()));
                            assert_eq!(cond[1], MettaValue::Long(5));
                            assert_eq!(cond[2], MettaValue::Long(3));
                        }
                        _ => panic!("Expected condition to remain as S-expression"),
                    }
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }

        // (quote (let $x 42 (+ $x 1)))
        let quote_let = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(42),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(1),
                ]),
            ]),
        ]);

        let (results, _) = eval(quote_let, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 4);
                    assert_eq!(items[0], MettaValue::Atom("let".to_string()));
                    assert_eq!(items[1], MettaValue::Atom("$x".to_string()));
                    assert_eq!(items[2], MettaValue::Long(42));
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_errors() {
        let env = Environment::new();

        // Test quoting error expressions
        // (quote (error "test" 42))
        let quote_error = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("test".to_string()),
                MettaValue::Long(42),
            ]),
        ]);

        let (results, _) = eval(quote_error, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("error".to_string()));
                    assert_eq!(items[1], MettaValue::String("test".to_string()));
                    assert_eq!(items[2], MettaValue::Long(42));
                }
                _ => panic!("Expected S-expression inside Quoted, not actual error"),
            },
            _ => panic!("Expected Quoted value"),
        }

        // Test quoting an actual error value
        let actual_error = MettaValue::Error("real-error".to_string(), Arc::new(MettaValue::Nil));
        let quote_actual_error = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            actual_error.clone(),
        ]);

        let (results, _) = eval(quote_actual_error, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Quoted(Box::new(actual_error)));
    }

    #[test]
    fn test_quote_with_empty_expressions() {
        let env = Environment::new();

        // Test quoting empty expressions
        // (quote ())
        let quote_empty = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![]),
        ]);

        let (results, _) = eval(quote_empty, env);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Quoted(Box::new(MettaValue::SExpr(vec![])))
        );
    }

    #[test]
    fn test_quote_preserves_exact_structure() {
        let env = Environment::new();

        // Test that quote preserves exact structure including nested quotes
        // (quote (quote (+ 1 2)))
        let nested_quotes = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
            ]),
        ]);

        let (results, _) = eval(nested_quotes, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(quoted_outer) => match quoted_outer.as_ref() {
                MettaValue::SExpr(outer) => {
                    assert_eq!(outer.len(), 2);
                    assert_eq!(outer[0], MettaValue::Atom("quote".to_string()));
                    match &outer[1] {
                        MettaValue::SExpr(inner) => {
                            assert_eq!(inner.len(), 3);
                            assert_eq!(inner[0], MettaValue::Atom("+".to_string()));
                            assert_eq!(inner[1], MettaValue::Long(1));
                            assert_eq!(inner[2], MettaValue::Long(2));
                        }
                        _ => panic!("Expected inner S-expression"),
                    }
                }
                _ => panic!("Expected outer S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_function_calls() {
        let env = Environment::new();

        // Test quoting function calls (should not be evaluated)
        // (quote (foo bar baz))
        let quote_function_call = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("bar".to_string()),
                MettaValue::Atom("baz".to_string()),
            ]),
        ]);

        let (results, _) = eval(quote_function_call, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("foo".to_string()));
                    assert_eq!(items[1], MettaValue::Atom("bar".to_string()));
                    assert_eq!(items[2], MettaValue::Atom("baz".to_string()));
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_arithmetic_operations() {
        let env = Environment::new();

        // Test that quoted arithmetic is not evaluated
        // (quote (* (+ 2 3) (- 10 4)))
        let quote_arithmetic = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(2),
                    MettaValue::Long(3),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("-".to_string()),
                    MettaValue::Long(10),
                    MettaValue::Long(4),
                ]),
            ]),
        ]);

        let (results, _) = eval(quote_arithmetic, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(quoted_inner) => match quoted_inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("*".to_string()));

                    // First sub-expression should remain unevaluated
                    match &items[1] {
                        MettaValue::SExpr(add_expr) => {
                            assert_eq!(add_expr[0], MettaValue::Atom("+".to_string()));
                            assert_eq!(add_expr[1], MettaValue::Long(2));
                            assert_eq!(add_expr[2], MettaValue::Long(3));
                        }
                        _ => panic!("Expected unevaluated addition"),
                    }

                    // Second sub-expression should remain unevaluated
                    match &items[2] {
                        MettaValue::SExpr(sub_expr) => {
                            assert_eq!(sub_expr[0], MettaValue::Atom("-".to_string()));
                            assert_eq!(sub_expr[1], MettaValue::Long(10));
                            assert_eq!(sub_expr[2], MettaValue::Long(4));
                        }
                        _ => panic!("Expected unevaluated subtraction"),
                    }
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_with_comparison_operations() {
        let env = Environment::new();

        // Test quoting comparison operations
        // (quote (< (+ 1 2) (* 2 2)))
        let quote_comparison = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("<".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(2),
                    MettaValue::Long(2),
                ]),
            ]),
        ]);

        let (results, _) = eval(quote_comparison, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("<".to_string()));
                    // Both operands should remain unevaluated
                }
                _ => panic!("Expected S-expression inside Quoted"),
            },
            _ => panic!("Expected Quoted value"),
        }
    }

    #[test]
    fn test_quote_integration_with_eval() {
        let env = Environment::new();

        // Test that eval can process quoted expressions
        // (eval (quote (+ 2 3)))
        let eval_quote = MettaValue::SExpr(vec![
            MettaValue::Atom("eval".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(2),
                    MettaValue::Long(3),
                ]),
            ]),
        ]);

        let (results, _) = eval(eval_quote, env);
        assert_eq!(results.len(), 1);
        // Quote prevents evaluation - should return Quoted expression
        assert_eq!(
            results[0],
            MettaValue::Quoted(Box::new(MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ])))
        );
    }

    #[test]
    fn test_quote_preserves_special_types() {
        let env = Environment::new();

        // Test quoting Type values
        let quote_type = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::Type(Arc::new(MettaValue::Atom("Number".to_string()))),
        ]);

        let (results, _) = eval(quote_type, env);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Quoted(Box::new(MettaValue::Type(Arc::new(MettaValue::Atom(
                "Number".to_string()
            )))))
        );
    }

    #[test]
    fn test_quote_in_complex_control_flow() {
        let env = Environment::new();

        // Test quote within if expressions
        // (if true (quote (+ 1 2)) (quote (+ 3 4)))
        let quote_in_if = MettaValue::SExpr(vec![
            MettaValue::Atom("if".to_string()),
            MettaValue::Bool(true),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(4),
                ]),
            ]),
        ]);

        let (results, _) = eval(quote_in_if, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Quoted(inner) => match inner.as_ref() {
                MettaValue::SExpr(items) => {
                    assert_eq!(items.len(), 3);
                    assert_eq!(items[0], MettaValue::Atom("+".to_string()));
                    assert_eq!(items[1], MettaValue::Long(1));
                    assert_eq!(items[2], MettaValue::Long(2));
                }
                _ => panic!("Expected S-expression inside Quoted from then branch"),
            },
            _ => panic!("Expected quoted expression from then branch"),
        }
    }

    #[test]
    fn test_quote_with_very_deep_nesting() {
        let env = Environment::new();

        // Test quote with deeply nested structure (stress test)
        // (quote (a (b (c (d (e (f 42)))))))
        let deep_nested = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("b".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("c".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("d".to_string()),
                            MettaValue::SExpr(vec![
                                MettaValue::Atom("e".to_string()),
                                MettaValue::SExpr(vec![
                                    MettaValue::Atom("f".to_string()),
                                    MettaValue::Long(42),
                                ]),
                            ]),
                        ]),
                    ]),
                ]),
            ]),
        ]);

        let (results, _) = eval(deep_nested, env);
        assert_eq!(results.len(), 1);

        // Verify the deep structure is preserved - first unwrap Quoted
        match &results[0] {
            MettaValue::Quoted(quoted_value) => {
                let mut current = quoted_value.as_ref();
                let expected_atoms = vec!["a", "b", "c", "d", "e", "f"];

                for expected_atom in expected_atoms {
                    match current {
                        MettaValue::SExpr(items) => {
                            assert_eq!(items.len(), 2);
                            assert_eq!(items[0], MettaValue::Atom(expected_atom.to_string()));
                            current = &items[1];
                        }
                        _ => panic!("Expected S-expression at level {}", expected_atom),
                    }
                }

                // At the deepest level, should find 42
                assert_eq!(*current, MettaValue::Long(42));
            }
            _ => panic!("Expected Quoted value"),
        }
    }

    // ============================================================================
    // UNQUOTE TESTS - Unwrapping and evaluating quoted expressions
    // ============================================================================

    #[test]
    fn test_unquote_basic() {
        // Test: (unquote (quote (+ 1 2)))
        // Should unwrap and evaluate to 3
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::Quoted(Box::new(MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]))),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_unquote_quoted_atom() {
        // Test: (unquote (quote foo))
        // Should unwrap and return atom foo
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::Quoted(Box::new(MettaValue::Atom("foo".to_string()))),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("foo".to_string()));
    }

    #[test]
    fn test_unquote_quoted_number() {
        // Test: (unquote (quote 42))
        // Should unwrap and return 42
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::Quoted(Box::new(MettaValue::Long(42))),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_unquote_non_quoted() {
        // Test: (unquote (+ 1 2))
        // (+ 1 2) evaluates to 3 (not Quoted)
        // Should return (unquote 3) as unevaluated data
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        // Should return (unquote 3) as unevaluated
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], MettaValue::Atom("unquote".to_string()));
                assert_eq!(items[1], MettaValue::Long(3));
            }
            other => panic!("Expected (unquote 3), got {:?}", other),
        }
    }

    #[test]
    fn test_unquote_with_rule_returning_quoted() {
        // Test: Rule that returns quoted value, then unquote
        // (= (add-numbers $x $y) (quote (+ $x $y)))
        // (unquote (add-numbers 51 6)) → evaluates to 57
        let mut env = Environment::new();

        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("add-numbers".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // (unquote (add-numbers 51 6))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("add-numbers".to_string()),
                MettaValue::Long(51),
                MettaValue::Long(6),
            ]),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(57));
    }

    #[test]
    fn test_unquote_nested_quoted() {
        // Test: (unquote (quote (quote x)))
        // Should unwrap once to (quote x)
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::Quoted(Box::new(MettaValue::Quoted(Box::new(MettaValue::Atom(
                "x".to_string(),
            ))))),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);

        // Result should be Quoted(Atom("x"))
        match &results[0] {
            MettaValue::Quoted(inner) => {
                assert_eq!(**inner, MettaValue::Atom("x".to_string()));
            }
            other => panic!("Expected Quoted(Atom(x)), got {:?}", other),
        }
    }

    #[test]
    fn test_unquote_complex_expression() {
        // Test: (unquote (quote (* (+ 2 3) (- 10 4))))
        // Should evaluate to (* 5 6) → 30
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::Quoted(Box::new(MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Long(2),
                    MettaValue::Long(3),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("-".to_string()),
                    MettaValue::Long(10),
                    MettaValue::Long(4),
                ]),
            ]))),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_quote_unquote_roundtrip() {
        // Test: (unquote (quote X)) should give back X evaluated
        let env = Environment::new();

        // Original expression
        let original = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]);

        // (quote (+ 10 20))
        let quoted = MettaValue::SExpr(vec![
            MettaValue::Atom("quote".to_string()),
            original.clone(),
        ]);

        // Evaluate quote
        let (quote_results, env1) = eval(quoted, env);
        assert_eq!(quote_results.len(), 1);
        assert!(matches!(&quote_results[0], MettaValue::Quoted(_)));

        // (unquote <quoted-result>)
        let unquoted = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            quote_results[0].clone(),
        ]);

        // Evaluate unquote
        let (unquote_results, _) = eval(unquoted, env1);
        assert_eq!(unquote_results.len(), 1);

        // Should evaluate to 30
        assert_eq!(unquote_results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_unquote_with_variable_substitution() {
        // Test that variables in quoted expressions are substituted correctly
        // This tests the interaction with the rule system
        let mut env = Environment::new();

        // (= (make-op $op $x $y) (quote ($op $x $y)))
        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("make-op".to_string()),
                MettaValue::Atom("$op".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("$op".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
            ]),
        };
        env.add_rule(rule);

        // (unquote (make-op + 15 25))
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unquote".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("make-op".to_string()),
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(15),
                MettaValue::Long(25),
            ]),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(40));
    }

    #[test]
    fn test_unquote_inside_let_with_rule() {
        // (= (add-numbers $x $y)
        //     (let $sum (unquote (quote (+ $x $y)))
        //         $sum))
        // ! (add-numbers 12 23)
        // Should evaluate to 35
        let mut env = Environment::new();

        let rule = Rule {
            lhs: MettaValue::SExpr(vec![
                MettaValue::Atom("add-numbers".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            rhs: MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$sum".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("unquote".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("quote".to_string()),
                        MettaValue::SExpr(vec![
                            MettaValue::Atom("+".to_string()),
                            MettaValue::Atom("$x".to_string()),
                            MettaValue::Atom("$y".to_string()),
                        ]),
                    ]),
                ]),
                MettaValue::Atom("$sum".to_string()),
            ]),
        };
        env.add_rule(rule);

        // Test: (add-numbers 12 23)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("add-numbers".to_string()),
            MettaValue::Long(12),
            MettaValue::Long(23),
        ]);

        let (results, _) = eval(expr, env);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0],
            MettaValue::Long(35),
            "BUG: (add-numbers 12 23) with unquote in let should evaluate to 35"
        );
    }
}
