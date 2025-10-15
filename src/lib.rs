/// MeTTaTron - MeTTa Evaluator Library
///
/// This library provides a complete MeTTa language evaluator with lazy evaluation,
/// pattern matching, and special forms. MeTTa is a language with LISP-like syntax
/// supporting rules, pattern matching, control flow, and grounded functions.
///
/// # Architecture
///
/// The evaluation pipeline consists of two main stages:
///
/// 1. **Lexical Analysis & S-expression Parsing** (`sexpr` module)
///    - Tokenizes input text into structured tokens
///    - Parses tokens into S-expressions
///    - Handles comments: `//`, `/* */`, `;`
///    - Supports special operators: `!`, `?`, `<-`, etc.
///
/// 2. **Backend Evaluation** (`backend` module)
///    - Compiles MeTTa source to `MettaValue` expressions
///    - Evaluates expressions with lazy semantics
///    - Supports pattern matching with variables (`$x`, `&y`, `'z`)
///    - Implements special forms: `=`, `!`, `quote`, `if`, `error`
///    - Direct grounded function dispatch for arithmetic and comparisons
///
/// # Example
///
/// ```rust
/// use mettatron::backend::*;
///
/// // Define a rule and evaluate it
/// let input = r#"
///     (= (double $x) (* $x 2))
///     !(double 21)
/// "#;
///
/// let state = compile(input).unwrap();
/// let mut env = state.environment;
/// for sexpr in state.pending_exprs {
///     let (results, new_env) = eval(sexpr, env);
///     env = new_env;
///
///     for result in results {
///         println!("{:?}", result);
///     }
/// }
/// ```
///
/// # MeTTa Language Features
///
/// - **Rule Definition**: `(= pattern body)` - Define pattern matching rules
/// - **Evaluation**: `!(expr)` - Force evaluation with rule application
/// - **Pattern Matching**: Variables (`$x`, `&y`, `'z`) and wildcard (`_`)
/// - **Control Flow**: `(if cond then else)` - Conditional with lazy branches
/// - **Quote**: `(quote expr)` - Prevent evaluation
/// - **Error Handling**: `(error msg details)` - Create error values
/// - **Grounded Functions**: Arithmetic (`+`, `-`, `*`, `/`) and comparisons (`<`, `<=`, `>`, `==`)
///
/// # Evaluation Strategy
///
/// - **Lazy Evaluation**: Expressions evaluated only when needed
/// - **Pattern Matching**: Automatic variable binding in rule application
/// - **Error Propagation**: First error stops evaluation immediately
/// - **Environment**: Monotonic rule storage with union operations

pub mod sexpr;
pub mod backend;
pub mod rholang_integration;
pub mod pathmap_par_integration;
pub mod ffi;

pub use sexpr::{Lexer, Parser, SExpr, Token};
pub use backend::{
    compile, eval,
    types::{MettaValue, Environment, Rule, MettaState},
};
pub use rholang_integration::{
    run_state,
    metta_state_to_json,
    compile_to_state_json,
};
pub use pathmap_par_integration::{
    metta_value_to_par,
    metta_state_to_pathmap_par,
    metta_error_to_par,
    par_to_metta_value,
    pathmap_par_to_metta_state,
};

#[cfg(test)]
mod tests {
    use super::*;
    use backend::*;

    #[test]
    fn test_compile_simple() {
        let result = compile("(+ 1 2)");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_and_eval_arithmetic() {
        let input = "(+ 10 20)";
        let state = compile(input).unwrap();
        assert_eq!(state.pending_exprs.len(), 1);

        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Long(30)));
    }

    #[test]
    fn test_rule_definition_and_evaluation() {
        let input = r#"
            (= (double $x) (* $x 2))
            !(double 21)
        "#;

        let state = compile(input).unwrap();
        assert_eq!(state.pending_exprs.len(), 2);
        let mut env = state.environment;

        // First expression: rule definition
        let (results, new_env) = eval(state.pending_exprs[0].clone(), env);
        env = new_env;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Nil));

        // Second expression: evaluation
        let (results, _env) = eval(state.pending_exprs[1].clone(), env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Long(42)));
    }

    #[test]
    fn test_if_control_flow() {
        let input = r#"(if (< 5 10) "yes" "no")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::String(ref s) if s == "yes"));
    }

    #[test]
    fn test_quote() {
        let input = "(quote (+ 1 2))";
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::SExpr(_)));
    }

    #[test]
    fn test_error_propagation() {
        let input = r#"(error "test error" 42)"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_invalid_syntax() {
        let result = compile("(+ 1");
        assert!(result.is_err());
    }
}
