// Compile function: MeTTa text → PathMap structure
//
// The compile function parses MeTTa source code and produces a PathMap structure
// containing [parsed_sexprs, fact_db] where:
// - parsed_sexprs: List of s-expressions as nested lists preserving original operator symbols
// - fact_db: PathMap instance representing the fact database (initially empty)
//
// Operator symbols like +, -, * are preserved as-is (not normalized to add, sub, mul)

#[cfg(test)]
use crate::backend::models::MettaValueInner;
use crate::backend::models::{MettaState, MettaValue, MettaValueFactory, MettaValueTrait};
use crate::ir::MettaExpr;
use crate::tree_sitter_parser::{SyntaxError, SyntaxErrorKind, TreeSitterMettaParser};

use tracing::{debug, error, info, instrument, warn};

// ============================================================================
// Generic Compilation - Zero-Conversion Support
// ============================================================================

/// Convert a MettaExpr to any value type using a factory.
///
/// This generic function enables zero-conversion compilation by delegating
/// value construction to the provided factory (e.g., `GcFactory`).
pub fn expr_to_value_generic<V, F>(expr: &MettaExpr, factory: &F) -> Result<V, String>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    match expr {
        MettaExpr::Atom(s, _span) => {
            // Parse literals (MeTTa uses capitalized True/False per hyperon-experimental)
            match s.as_str() {
                "True" => Ok(factory.bool(true)),
                "False" => Ok(factory.bool(false)),
                _ => Ok(factory.atom(s)),
            }
        }
        MettaExpr::String(s, _span) => Ok(factory.string(s)),
        MettaExpr::Integer(n, _span) => Ok(factory.long(*n)),
        MettaExpr::Float(f, _span) => Ok(factory.float(*f)),
        MettaExpr::List(items, _span) => {
            if items.is_empty() {
                // HE-compatible: () is an empty S-expression, not unit
                Ok(factory.sexpr(vec![]))
            } else {
                // Check if this is a conjunction: (,) or (, expr1 expr2 ...)
                let is_conjunction = items
                    .first()
                    .is_some_and(|first| matches!(first, MettaExpr::Atom(s, _) if s == ","));

                if is_conjunction {
                    // Convert to Conjunction variant (skip the comma operator)
                    let goals: Result<Vec<V>, String> = items[1..]
                        .iter()
                        .map(|e| expr_to_value_generic(e, factory))
                        .collect();
                    Ok(factory.conjunction(goals?))
                } else {
                    // Regular S-expression
                    let values: Result<Vec<V>, String> = items
                        .iter()
                        .map(|e| expr_to_value_generic(e, factory))
                        .collect();
                    Ok(factory.sexpr(values?))
                }
            }
        }
        MettaExpr::Quoted(expr, _span) => {
            // For quoted expressions, wrap in a quote operator
            let inner = expr_to_value_generic(expr.as_ref(), factory)?;
            // Create (quote inner) as SExpr
            Ok(factory.sexpr(vec![factory.atom("quote"), inner]))
        }
    }
}

/// Compile MeTTa source code to a generic value type.
///
/// This is the zero-conversion compile function that works with any value type
/// implementing `MettaValueTrait`. It returns a vector of parsed expressions.
///
/// # Type Parameters
///
/// - `V`: The value type (e.g., `MettaValue`)
/// - `F`: The factory type for constructing values
///
/// # Arguments
///
/// - `src`: The MeTTa source code to compile
/// - `factory`: The factory for constructing values
///
/// # Returns
///
/// A vector of compiled expressions in the target value type, or a syntax error.
#[instrument(level = "info", skip(src, factory))]
pub fn compile_generic<V, F>(src: &str, factory: &F) -> Result<Vec<V>, SyntaxError>
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    info!(
        line_count = src.lines().count(),
        char_count = src.chars().count(),
        "Compiling MeTTa source (generic)"
    );

    // Parse the source into s-expressions using Tree-Sitter
    let mut parser = TreeSitterMettaParser::new().map_err(|e| SyntaxError {
        kind: SyntaxErrorKind::ParserInit(e),
        line: 0,
        column: 0,
        text: String::new(),
        file_path: None,
    })?;

    let sexprs = parser.parse(src).map_err(|e| {
        error!(
            kind = ?e.kind,
            text = %e,
            "Syntax error from parsing MeTTa source code"
        );
        debug!(src, %e);
        e
    })?;

    // Convert all expressions using the generic factory
    let values: Result<Vec<V>, String> = sexprs
        .iter()
        .map(|expr| expr_to_value_generic(expr, factory))
        .collect();

    let values = values.map_err(|e| {
        error!(
            text = %e,
            "Error during converting MeTTa expressions to generic values"
        );
        SyntaxError {
            kind: SyntaxErrorKind::UnknownNodeKind(e),
            line: 0,
            column: 0,
            text: String::new(),
            file_path: None,
        }
    })?;

    info!(expr_count = values.len(), "Generic compilation successful");

    Ok(values)
}

// ============================================================================
// Arena Compilation - Session-Based Dual-Arena Pipeline
// ============================================================================

/// Compile MeTTa source code to an MettaState for session-based evaluation.
///
/// This function creates a session-scoped MettaState, compiles the source
/// expressions into the session's storage arena, and returns the populated
/// state ready for evaluation with `eval` or `eval_trampoline`.
///
/// # Arguments
///
/// - `src`: The MeTTa source code to compile
///
/// # Returns
///
/// An `MettaState` containing the compiled expressions in its storage arena,
/// or a syntax error.
///
/// ## Dual-Arena Model
///
/// The returned `MettaState` owns a session-scoped storage arena:
/// - Source expressions are allocated in the storage arena
/// - O(1) bulk deallocation when MettaState is dropped
/// - Efficient arena pooling reduces allocation overhead
///
/// During evaluation:
/// - **Storage Arena** (session-owned): Holds source, rules, bindings, results
/// - **Eval Arena** (thread-local): Holds intermediates, reset between sessions
///
/// # Example
///
/// ```ignore
/// use mettatron::backend::compile::compile;
/// use mettatron::backend::eval::{eval, trampoline::new_env};
///
/// // Compile to MettaState
/// let state = compile("!(+ 1 2)").unwrap();
///
/// // Create arena environment
/// let mut env = new_env();
///
/// // Evaluate - zero conversions throughout
/// for &expr in state.source() {
///     let (results, new_env) = eval(expr, env, &state);
///     env = new_env;
/// }
///
/// // O(1) bulk deallocation on drop
/// drop(state);
/// ```
#[instrument(level = "info", skip(src))]
pub fn compile(src: &str) -> Result<MettaState, SyntaxError> {
    info!(
        line_count = src.lines().count(),
        char_count = src.chars().count(),
        "Compiling MeTTa source to MettaState"
    );

    // Create MettaState (acquires storage arena from pool)
    let mut state = MettaState::new();

    // Get factory for value allocation
    let factory = state.factory();

    // Parse the source into s-expressions using Tree-Sitter
    let mut parser = TreeSitterMettaParser::new().map_err(|e| SyntaxError {
        kind: SyntaxErrorKind::ParserInit(e),
        line: 0,
        column: 0,
        text: String::new(),
        file_path: None,
    })?;

    let sexprs = parser.parse(src).map_err(|e| {
        error!(
            kind = ?e.kind,
            text = %e,
            "Syntax error from parsing MeTTa source code"
        );
        debug!(src, %e);
        e
    })?;

    // Convert all expressions using the storage factory
    for expr in &sexprs {
        let value = expr_to_value_generic(expr, &factory).map_err(|e| {
            error!(
                text = %e,
                "Error during converting MeTTa expressions to arena values"
            );
            SyntaxError {
                kind: SyntaxErrorKind::UnknownNodeKind(e),
                line: 0,
                column: 0,
                text: String::new(),
                file_path: None,
            }
        })?;
        state.source_mut().push(value);
    }

    info!(expr_count = state.source().len(), "MettaState compilation successful");

    Ok(state)
}

/// Compile MeTTa source code to MettaState with a file path for error reporting.
///
/// Like `compile`, but includes the file path in any syntax error messages.
pub fn compile_with_path(
    src: &str,
    file_path: Option<&str>,
) -> Result<MettaState, SyntaxError> {
    compile(src).map_err(|e| match file_path {
        Some(path) => e.with_file_path(path),
        None => e,
    })
}

/// Helper function to create an error value
pub fn make_error(msg: &str, details: MettaValue) -> MettaValue {
    MettaValue::Error(msg.to_string(), details)
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
        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
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
            if let MettaValueInner::SExpr(items) = state.source[0].inner() {
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
        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
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

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
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
        let src = "(True False 42 \"hello\")";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
            assert_eq!(items[2], MettaValue::Long(42));
            assert_eq!(items[3], MettaValue::String("hello".to_string()));
        }
    }

    #[test]
    fn test_compile_mixed_literals() {
        let src = "(list 42 -7 0 True False \"text\" ())";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
            assert_eq!(items[0], MettaValue::Atom("list".to_string()));
            assert_eq!(items[1], MettaValue::Long(42));
            assert_eq!(items[2], MettaValue::Long(-7));
            assert_eq!(items[3], MettaValue::Long(0));
            assert_eq!(items[4], MettaValue::Bool(true));
            assert_eq!(items[5], MettaValue::Bool(false));
            assert_eq!(items[6], MettaValue::String("text".to_string()));
            // () compiles to empty SExpr for HE compatibility
            assert_eq!(items[7], MettaValue::SExpr(vec![]));
        } else {
            panic!("Expected SExpr with mixed literals");
        }
    }

    #[test]
    fn test_boolean_case_sensitivity() {
        // Lowercase should be treated as atoms, not booleans
        // MeTTa uses capitalized True/False per hyperon-experimental
        let src = "(true false)";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Atom("true".to_string()));
            assert_eq!(items[1], MettaValue::Atom("false".to_string()));
        } else {
            panic!("Expected SExpr with lowercase boolean atoms");
        }

        // Verify capitalized versions ARE treated as booleans
        let src = "(True False)";
        let state = compile(src).unwrap();

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
        } else {
            panic!("Expected SExpr with boolean values");
        }
    }

    #[test]
    fn test_compile_with_comments() {
        let src = r#"
            ; Single line comment
            (+ 1 2)
            ; Another comment
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

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
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

        if let MettaValueInner::SExpr(items) = state.source[0].inner() {
            assert_eq!(items.len(), 2);
            assert_eq!(items[0], MettaValue::Atom("!".to_string()));

            if let MettaValueInner::SExpr(inner) = items[1].inner() {
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
        if let MettaValueInner::SExpr(outer) = state.source[0].inner() {
            assert_eq!(outer[0], MettaValue::Atom("+".to_string()));
            assert_eq!(outer[1], MettaValue::Long(1));

            // Level 2: (+ 2 ...)
            if let MettaValueInner::SExpr(level2) = outer[2].inner() {
                assert_eq!(level2[0], MettaValue::Atom("+".to_string()));
                assert_eq!(level2[1], MettaValue::Long(2));

                // Level 3: (+ 3 ...)
                if let MettaValueInner::SExpr(level3) = level2[2].inner() {
                    assert_eq!(level3[0], MettaValue::Atom("+".to_string()));
                    assert_eq!(level3[1], MettaValue::Long(3));

                    // Level 4: (+ 4 5)
                    if let MettaValueInner::SExpr(level4) = level3[2].inner() {
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
        use crate::backend::eval::trampoline::new_env;
        use crate::backend::models::MettaValueInner;

        let input = r#"!(error failure-code 42)"#;
        let state = compile(input).expect("compile failed");
        let env = new_env();
        let (results, _env) = eval(state.source()[0], env, &state);

        assert_eq!(results.len(), 1);
        if let MettaValueInner::Error(msg, _) = results[0].inner() {
            assert_eq!(*msg, "failure-code");
        } else {
            panic!("Expected error, got: {:?}", results[0]);
        }
    }

    // =========================================================================
    // Arena Compilation Tests
    // =========================================================================

    #[test]
    fn test_compile_with_eval() {
        use crate::backend::compile::compile;
        use crate::backend::eval::trampoline::{eval_trampoline, new_env};

        let src = "!(+ 1 2)";
        let state = compile(src).unwrap();
        let env = new_env();

        // Evaluate the expression
        let (results, _env) = eval_trampoline(state.source()[0], env, &state);

        // Results should contain [3]
        assert_eq!(results.len(), 1);
        assert!(results[0].is_long());
        assert_eq!(results[0].as_long(), Some(3));
    }
}
