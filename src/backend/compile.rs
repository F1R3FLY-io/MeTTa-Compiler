// Compile function: MeTTa text → PathMap structure
//
// The compile function parses MeTTa source code and produces a PathMap structure
// containing [parsed_sexprs, fact_db] where:
// - parsed_sexprs: List of s-expressions as nested lists with textual operator names
// - fact_db: PathMap instance representing the fact database (initially empty)
//
// Grounded operators like +, -, * are replaced with textual names like "add", "sub", "mul"

use crate::backend::types::{MettaValue, MettaState};
use crate::sexpr::{Lexer, Parser, SExpr};

/// Compile MeTTa source code into a MettaState ready for evaluation
/// Returns a compiled state with pending expressions and empty environment
pub fn compile(src: &str) -> Result<MettaState, String> {
    // Parse the source into s-expressions
    let mut lexer = Lexer::new(src);
    let tokens = lexer.tokenize()?;

    let mut parser = Parser::new(tokens);
    let sexprs = parser.parse()?;

    // Convert s-expressions to MettaValue representation
    let mut metta_values = Vec::new();
    for sexpr in sexprs {
        metta_values.push(sexpr_to_metta_value(&sexpr)?);
    }

    Ok(MettaState::new_compiled(metta_values))
}

/// Convert an SExpr to a MettaValue
/// This replaces grounded operators with their textual names
fn sexpr_to_metta_value(sexpr: &SExpr) -> Result<MettaValue, String> {
    match sexpr {
        SExpr::Atom(s) => {
            // Check if this is a grounded operator that needs to be renamed
            let normalized = match s.as_str() {
                "+" => "add",
                "-" => "sub",
                "*" => "mul",
                "/" => "div",
                "<" => "lt",
                "<=" => "lte",
                ">" => "gt",
                ">=" => "gte",
                "==" => "eq",
                "!=" => "neq",
                other => other,
            };

            // Parse literals
            if normalized == "true" {
                Ok(MettaValue::Bool(true))
            } else if normalized == "false" {
                Ok(MettaValue::Bool(false))
            } else {
                Ok(MettaValue::Atom(normalized.to_string()))
            }
        }
        SExpr::String(s) => Ok(MettaValue::String(s.clone())),
        SExpr::Integer(n) => Ok(MettaValue::Long(*n)),
        SExpr::List(items) => {
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
        SExpr::Quoted(expr) => {
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

/// Convert MettaValue to Rholang AST Proc
/// This is the toProcExpr function from the pseudocode
/// For now, this is a placeholder - will be implemented when we integrate with rholang-rs
pub fn to_proc_expr(_metta_value: &MettaValue) -> Result<String, String> {
    // TODO: Implement conversion to Rholang AST Proc type
    // This will require the rholang-rs dependency
    Err("toProcExpr not yet implemented - requires rholang-rs integration".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple() {
        let src = "(+ 1 2)";
        let result = compile(src);
        assert!(result.is_ok());

        let state = result.unwrap();
        assert_eq!(state.pending_exprs.len(), 1);
        // Environment is empty at compile time (facts added during eval)
        assert!(state.environment.rule_cache.is_empty());
        assert!(state.eval_outputs.is_empty());

        // Should be: (add 1 2)
        if let MettaValue::SExpr(items) = &state.pending_exprs[0] {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom("add".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_compile_operators() {
        let operators = vec![
            ("+", "add"),
            ("-", "sub"),
            ("*", "mul"),
            ("/", "div"),
            ("<", "lt"),
            ("<=", "lte"),
            ("==", "eq"),
        ];

        for (op, expected) in operators {
            let src = format!("({} 1 2)", op);
            let state = compile(&src).unwrap();
            if let MettaValue::SExpr(items) = &state.pending_exprs[0] {
                assert_eq!(items[0], MettaValue::Atom(expected.to_string()),
                    "Failed for operator {}", op);
            }
        }
    }

    #[test]
    fn test_compile_gt() {
        // Test > operator
        let src = "(> 1 2)";
        let state = compile(src).unwrap();
        if let MettaValue::SExpr(items) = &state.pending_exprs[0] {
            assert_eq!(items[0], MettaValue::Atom("gt".to_string()));
        }

        // Note: >= is tokenized by the lexer as two separate tokens: Symbol(">") and Equals
        // This would need to be fixed in sexpr.rs to handle >= as a single operator
        // For now, >= is not supported as a single operator
    }

    #[test]
    fn test_compile_literals() {
        let src = "(true false 42 \"hello\")";
        let state = compile(src).unwrap();

        if let MettaValue::SExpr(items) = &state.pending_exprs[0] {
            assert_eq!(items[0], MettaValue::Bool(true));
            assert_eq!(items[1], MettaValue::Bool(false));
            assert_eq!(items[2], MettaValue::Long(42));
            assert_eq!(items[3], MettaValue::String("hello".to_string()));
        }
    }

    // Note: URI literals with backticks are not yet supported by the lexer
    // They would need to be added to sexpr.rs
}
