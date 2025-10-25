// Compile function: MeTTa text → PathMap structure
//
// The compile function parses MeTTa source code and produces a PathMap structure
// containing [parsed_sexprs, fact_db] where:
// - parsed_sexprs: List of s-expressions as nested lists preserving original operator symbols
// - fact_db: PathMap instance representing the fact database (initially empty)
//
// Operator symbols like +, -, * are preserved as-is (not normalized to add, sub, mul)

use crate::backend::models::{MettaState, MettaValue};
use crate::ir::MettaExpr;
use crate::tree_sitter_parser::TreeSitterMettaParser;

/// Compile MeTTa source code into a MettaState ready for evaluation
/// Returns a compiled state with pending expressions and empty environment
pub fn compile(src: &str) -> Result<MettaState, String> {
    // Parse the source into s-expressions using Tree-Sitter
    let mut parser = TreeSitterMettaParser::new()
        .map_err(|e| format!("Failed to initialize parser: {}", e))?;
    let sexprs = parser.parse(src)?;

    // Convert s-expressions to MettaValue representation
    let mut metta_values = Vec::new();
    for sexpr in sexprs {
        metta_values.push(sexpr_to_metta_value(&sexpr)?);
    }

    Ok(MettaState::new_compiled(metta_values))
}

/// Convert a MettaExpr (SExpr) to a MettaValue
/// Operator symbols are preserved as-is (no normalization)
/// Position information is discarded during conversion (used only for LSP/tooling)
fn sexpr_to_metta_value(sexpr: &MettaExpr) -> Result<MettaValue, String> {
    match sexpr {
        MettaExpr::Atom(s, _span) => {
            // Parse literals
            if s == "true" {
                Ok(MettaValue::Bool(true))
            } else if s == "false" {
                Ok(MettaValue::Bool(false))
            } else {
                // Keep the original symbol as-is (including operators like +, -, *, etc.)
                Ok(MettaValue::Atom(s.clone()))
            }
        }
        MettaExpr::String(s, _span) => Ok(MettaValue::String(s.clone())),
        MettaExpr::Integer(n, _span) => Ok(MettaValue::Long(*n)),
        MettaExpr::Float(f, _span) => Ok(MettaValue::Float(*f)),
        MettaExpr::List(items, _span) => {
            if items.is_empty() {
                Ok(MettaValue::Nil)
            } else {
                let mut values = Vec::new();
                for item in items {
                    values.push(sexpr_to_metta_value(item)?);
                }
                Ok(MettaValue::SExpr(values))
            }
        }
        MettaExpr::Quoted(expr, _span) => {
            // For quoted expressions, wrap in a quote operator
            let inner = sexpr_to_metta_value(expr)?;
            Ok(MettaValue::SExpr(vec![
                MettaValue::Atom("quote".to_string()),
                inner,
            ]))
        }
    }
}

/// Helper function to create an error value
pub fn make_error(msg: &str, details: MettaValue) -> MettaValue {
    MettaValue::Error(msg.to_string(), Box::new(details))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_empty_input() {
        let result = compile("");
        assert!(result.is_ok());
        let state = result.unwrap();
        assert_eq!(state.source.len(), 0);
    }

    #[test]
    fn test_compile_simple() {
        let src = "(+ 1 2)";
        let result = compile(src);
        assert!(result.is_ok());

        let state = result.unwrap();
        assert_eq!(state.source.len(), 1);
        // Environment is empty at compile time (facts added during eval)
        assert_eq!(state.environment.rule_count(), 0);
        assert!(state.output.is_empty());

        // Should be: (+ 1 2) - operator symbol preserved
        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_compile_multiple_expressions() {
        let src = "(+ 1 2) (* 3 4)";
        let state = compile(src).unwrap();
        assert_eq!(state.source.len(), 2);
    }

    #[test]
    fn test_compile_operators() {
        // Operators should be preserved as-is (not normalized)
        let operators = vec![
            ("+", "+"),
            ("-", "-"),
            ("*", "*"),
            ("/", "/"),
            ("<", "<"),
            ("<=", "<="),
            ("==", "=="),
        ];

        for (op, expected) in operators {
            let src = format!("({} 1 2)", op);
            let state = compile(&src).unwrap();
            if let MettaValue::SExpr(items) = &state.source[0] {
                assert_eq!(
                    items[0],
                    MettaValue::Atom(expected.to_string()),
                    "Failed for operator {}",
                    op
                );
            }
        }
    }

    #[test]
    fn test_compile_gt() {
        // Test > operator - should be preserved as-is
        let src = "(> 1 2)";
        let state = compile(src).unwrap();
        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items[0], MettaValue::Atom(">".to_string()));
        }

        // Note: >= is tokenized by the lexer as two separate tokens: Symbol(">") and Equals
        // This would need to be fixed in sexpr.rs to handle >= as a single operator
        // For now, >= is not supported as a single operator
    }

    #[test]
    fn test_compile_negative_numbers() {
        let src = "(+ -5 -10)";
        let state = compile(src).unwrap();

        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            assert_eq!(items[1], MettaValue::Long(-5));
            assert_eq!(items[2], MettaValue::Long(-10));
        } else {
            panic!("Expected SExpr with negative numbers");
        }
    }

    #[test]
    fn test_compile_zero() {
        let src = "0";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);
        assert_eq!(state.source[0], MettaValue::Long(0));
    }

    #[test]
    fn test_compile_literals() {
        let src = "(true false 42 \"hello\")";
        let state = compile(src).unwrap();

        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
            assert_eq!(items[2], MettaValue::Long(42));
            assert_eq!(items[3], MettaValue::String("hello".to_string()));
        }
    }

    #[test]
    fn test_compile_mixed_literals() {
        let src = "(list 42 -7 0 true false \"text\" ())";
        let state = compile(src).unwrap();

        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items[0], MettaValue::Atom("list".to_string()));
            assert_eq!(items[1], MettaValue::Long(42));
            assert_eq!(items[2], MettaValue::Long(-7));
            assert_eq!(items[3], MettaValue::Long(0));
            assert_eq!(items[4], MettaValue::Bool(true));
            assert_eq!(items[5], MettaValue::Bool(false));
            assert_eq!(items[6], MettaValue::String("text".to_string()));
            assert_eq!(items[7], MettaValue::Nil);
        } else {
            panic!("Expected SExpr with mixed literals");
        }
    }

    #[test]
    fn test_compile_with_comments() {
        let src = r#"
            // Single line comment
            (+ 1 2)
            /* Block comment */
            (* 3 4)
        "#;
        let state = compile(src).unwrap();
        assert_eq!(state.source.len(), 2);
    }

    #[test]
    fn test_compile_type_assertion() {
        let src = "(: x Number)";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);

        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom(":".to_string()));
            assert_eq!(items[1], MettaValue::Atom("x".to_string()));
            assert_eq!(items[2], MettaValue::Atom("Number".to_string()));
        } else {
            panic!("Expected SExpr for type assertion");
        }
    }

    #[test]
    fn test_compile_exclaim_operator() {
        let src = "!(double 5)";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);

        if let MettaValue::SExpr(items) = &state.source[0] {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Atom("!".to_string()));

            if let MettaValue::SExpr(inner) = &items[1] {
                assert_eq!(inner[0], MettaValue::Atom("double".to_string()));
                assert_eq!(inner[1], MettaValue::Long(5));
            } else {
                panic!("Expected SExpr inside !");
            }
        } else {
            panic!("Expected SExpr for ! operator");
        }
    }

    #[test]
    fn test_compile_dollar_variable() {
        let src = "$x";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);
        assert_eq!(state.source[0], MettaValue::Atom("$x".to_string()));
    }

    #[test]
    fn test_compile_quote_variable() {
        let src = "'quoted";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);
        // Tree-Sitter parser treats 'quoted as a prefixed expression: (' quoted)
        assert_eq!(
            state.source[0],
            MettaValue::SExpr(vec![
                MettaValue::Atom("'".to_string()),
                MettaValue::Atom("quoted".to_string())
            ])
        );
    }

    #[test]
    fn test_compile_deeply_nested() {
        let src = "(+ 1 (+ 2 (+ 3 (+ 4 5))))";
        let state = compile(src).unwrap();

        assert_eq!(state.source.len(), 1);

        // Outer: (+ 1 ...)
        if let MettaValue::SExpr(outer) = &state.source[0] {
            assert_eq!(outer[0], MettaValue::Atom("+".to_string()));
            assert_eq!(outer[1], MettaValue::Long(1));

            // Level 2: (+ 2 ...)
            if let MettaValue::SExpr(level2) = &outer[2] {
                assert_eq!(level2[0], MettaValue::Atom("+".to_string()));
                assert_eq!(level2[1], MettaValue::Long(2));

                // Level 3: (+ 3 ...)
                if let MettaValue::SExpr(level3) = &level2[2] {
                    assert_eq!(level3[0], MettaValue::Atom("+".to_string()));
                    assert_eq!(level3[1], MettaValue::Long(3));

                    // Level 4: (+ 4 5)
                    if let MettaValue::SExpr(level4) = &level3[2] {
                        assert_eq!(level4[0], MettaValue::Atom("+".to_string()));
                        assert_eq!(level4[1], MettaValue::Long(4));
                        assert_eq!(level4[2], MettaValue::Long(5));
                    } else {
                        panic!("Expected SExpr at level 4");
                    }
                } else {
                    panic!("Expected SExpr at level 3");
                }
            } else {
                panic!("Expected SExpr at level 2");
            }
        } else {
            panic!("Expected SExpr for outer expression");
        }
    }

    #[test]
    fn test_invalid_syntax_unclosed_paren() {
        let input = "(+ 1 2";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_syntax_extra_close_paren() {
        let input = "(+ 1 2))";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_syntax_mismatched_parens() {
        let input = "((+ 1 2)";
        let result = compile(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_error_with_atom_message() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(error failure-code 42)"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        if let MettaValue::Error(msg, _) = &results[0] {
            assert_eq!(msg, "failure-code");
        } else {
            panic!("Expected error");
        }
    }

    // Note: URI literals with backticks are not yet supported by the lexer
    // They would need to be added to sexpr.rs
}
