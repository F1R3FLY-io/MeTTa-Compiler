pub mod backend;
pub mod config;
pub mod ir;
pub mod pathmap_par_integration;
pub mod repl;
pub mod rholang_integration;
pub mod tree_sitter_parser;

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
///    - Handles comments: `;` (semicolon line comments)
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
/// for sexpr in state.source {
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
pub use backend::{
    compile,
    environment::Environment,
    eval,
    models::{MettaState, MettaValue, Rule},
};
pub use ir::{MettaExpr, Position, SExpr, Span};
pub use rholang_integration::run_state;
pub use tree_sitter_parser::TreeSitterMettaParser;

// Export run_state_async when async feature is enabled (which is by default)
#[cfg(feature = "async")]
pub use rholang_integration::run_state_async;

pub use pathmap_par_integration::{
    metta_error_to_par, metta_state_to_pathmap_par, metta_value_to_par, par_to_metta_value,
    pathmap_par_to_metta_state,
};

pub use config::{configure_eval, get_eval_config, EvalConfig};

// Export commonly used REPL components
pub use repl::{MettaHelper, PatternHistory, QueryHighlighter, ReplStateMachine, SmartIndenter};

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
        assert_eq!(state.source.len(), 1);

        let (results, _env) = eval(state.source[0].clone(), state.environment);
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
        assert_eq!(state.source.len(), 2);
        let mut env = state.environment;

        // First expression: rule definition
        let (results, new_env) = eval(state.source[0].clone(), env);
        env = new_env;
        // Rule definition returns empty list
        assert!(results.is_empty());

        // Second expression: evaluation
        let (results, _env) = eval(state.source[1].clone(), env);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Long(42)));
    }

    #[test]
    fn test_multiple_evaluations() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (double $x) (* $x 2))
            !(double 5)
            !(double 10)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut all_results = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;

            if !expr_results.is_empty() {
                all_results.extend(expr_results);
            }
        }

        assert_eq!(all_results.len(), 2);
        assert_eq!(all_results[0], MettaValue::Long(10));
        assert_eq!(all_results[1], MettaValue::Long(20));
    }

    #[test]
    fn test_evaluation_steps() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (add1 $x) (+ $x 1))
            (= (add2 $x) (+ $x 2))
            !(add1 5)
            !(add2 5)
            !(add1 (add2 10))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut evaluations = Vec::new();

        for (i, expr) in state.source.iter().enumerate() {
            let (expr_results, new_env) = eval(expr.clone(), env);
            env = new_env;

            if !expr_results.is_empty() {
                evaluations.push((i, expr_results[0].clone()));
            }
        }

        assert_eq!(evaluations.len(), 3);
        assert_eq!(evaluations[0].1, MettaValue::Long(6));
        assert_eq!(evaluations[1].1, MettaValue::Long(7));
        assert_eq!(evaluations[2].1, MettaValue::Long(13));
    }

    #[test]
    fn test_if_control_flow() {
        let input = r#"(if (< 5 10) "yes" "no")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::String(ref s) if s == "yes"));
    }

    #[test]
    fn test_if_with_equality_check() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(if (== 5 5) "equal" "not-equal")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("equal".to_string()));
    }

    #[test]
    fn test_if_lazy_evaluation_true_branch() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (boom) (error "should not evaluate" 0))
            (if true success (boom))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Atom("success".to_string())));
    }

    #[test]
    fn test_if_prevents_infinite_loop() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (loop) (loop))
            (if true success (loop))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Atom("success".to_string())));
    }

    #[test]
    fn test_factorial_with_if() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (factorial $x)
            (if (> $x 0)
                (* $x (factorial (- $x 1)))
                1))
            !(factorial 5)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(120)));
    }

    #[test]
    fn test_factorial_base_case() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (factorial $x)
            (if (> $x 0)
                (* $x (factorial (- $x 1)))
                1))
            !(factorial 0)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(1)));
    }

    #[test]
    fn test_nested_if() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (if (> 10 5)
                (if (< 3 7) "both-true" "outer-true-inner-false")
                "outer-false")
        "#;

        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("both-true".to_string()));
    }

    #[test]
    fn test_if_with_computation_in_branches() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(if (< 5 10) (+ 2 3) (* 4 5))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_if_with_function_calls_in_branches() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (double $x) (* $x 2))
            (= (triple $x) (* $x 3))
            !(if (> 10 5) (double 7) (triple 7))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(14)));
    }

    #[test]
    fn test_quote() {
        let input = "(quote (+ 1 2))";
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::SExpr(_)));
    }

    #[test]
    fn test_error_propagation() {
        let input = r#"(error "test error" 42)"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::Error(_, _)));
    }

    #[test]
    fn test_error_in_nested_expression() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(+ 1 (+ 2 (+ 3 (error "deep error" nested))))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        if let MettaValue::Error(msg, _) = &results[0] {
            assert_eq!(msg, "deep error");
        } else {
            panic!("Expected error propagation from nested expression");
        }
    }

    #[test]
    fn test_error_in_function_call() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (safe-op $x) (if (< $x 0) (error "negative value" $x) (* $x 2)))
            !(safe-op -5)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::Error(msg, details)) = result {
            assert_eq!(msg, "negative value");
            assert_eq!(*details, MettaValue::Long(-5));
        } else {
            panic!("Expected error from function call");
        }
    }

    #[test]
    fn test_error_in_recursive_function() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (div-by-zero $n)
                (if (== $n 0)
                    (error "division by zero" $n)
                    (div-by-zero (- $n 1))))
            !(div-by-zero 3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::Error(msg, _)) = result {
            assert_eq!(msg, "division by zero");
        } else {
            panic!("Expected error from recursive function");
        }
    }

    #[test]
    fn test_error_with_catch() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(catch (error "caught" 42) "default-value")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("default-value".to_string()));
    }

    #[test]
    fn test_catch_without_error() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(catch (+ 5 7) "default-value")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(12));
    }

    #[test]
    fn test_nested_catch() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
        (catch
                (catch (error "inner" 1) (error "middle" 2))
                "outer-default")
        "#;

        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("outer-default".to_string()));
    }

    #[test]
    fn test_error_in_condition() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(if (error "condition failed" cond) yes no)"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        if let MettaValue::Error(msg, _) = &results[0] {
            assert_eq!(msg, "condition failed");
        } else {
            panic!("Expected error from condition evaluation");
        }
    }

    #[test]
    fn test_is_error_check() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(is-error (error "test" 0))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_is_error_with_normal_value() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(is-error (+ 1 2))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_error_recovery_pattern() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (safe-div $x $y)
                (if (== $y 0)
                    (error "division by zero" $y)
                    (/ $x $y)))
            (= (try-div $x $y)
                (catch (safe-div $x $y) -1))
            !(try-div 10 0)
            !(try-div 10 2)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut results = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        assert_eq!(results[0], MettaValue::Long(-1));
        assert_eq!(results[1], MettaValue::Long(5));
    }

    #[test]
    fn test_multiple_errors_in_sequence() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (error "first" 1)
            (error "second" 2)
            (error "third" 3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut errors = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(MettaValue::Error(msg, _)) = expr_results.first() {
                errors.push(msg.clone());
            }
        }

        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0], "first");
        assert_eq!(errors[1], "second");
        assert_eq!(errors[2], "third");
    }

    #[test]
    fn test_error_stops_evaluation_in_expression() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (side-effect) (error "should not see this" 0))
            (+ (error "first-error" 1) (side-effect))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::Error(msg, _)) = result {
            assert_eq!(msg, "first-error");
        } else {
            panic!("Expected first error to propagate");
        }
    }

    #[test]
    fn test_error_with_complex_details() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(error "complex" (+ 1 (+ 2 3)))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.source[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        if let MettaValue::Error(msg, details) = &results[0] {
            assert_eq!(msg, "complex");
            assert!(matches!(**details, MettaValue::SExpr(_)));
        } else {
            panic!("Expected error with complex details");
        }
    }

    #[test]
    fn test_catch_in_recursive_context() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (safe-fact $n)
                (if (< $n 0)
                    (catch (error "negative" $n) 0)
                    (if (== $n 0)
                        1
                        (* $n (safe-fact (- $n 1))))))
            !(safe-fact 5)
            !(safe-fact -3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut results = Vec::new();

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        assert_eq!(results[0], MettaValue::Long(120));
        assert_eq!(results[1], MettaValue::Long(0));
    }

    #[test]
    fn test_invalid_syntax() {
        let result = compile("(+ 1");
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_recursion() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (countdown 0) done)
            (= (countdown $n) (countdown (- $n 1)))
            !(countdown 3)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut last_result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);

            env = new_env;
            if let Some(result) = expr_results.last() {
                last_result = Some(result.clone());
            }
        }

        assert_eq!(last_result, Some(MettaValue::Atom("done".to_string())));
    }

    #[test]
    fn test_recursive_list_length_safe() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (len nil) 0)
            (= (len (cons $x $xs)) (+ 1 (len $xs)))
            !(len (cons a (cons b (cons c nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(3)));
    }

    #[test]
    fn test_recursive_list_sum() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (sum nil) 0)
            (= (sum (cons $x $xs)) (+ $x (sum $xs)))
            !(sum (cons 10 (cons 20 (cons 30 nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(60)));
    }

    #[test]
    fn test_recursive_fibonacci() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (fib 0) 0)
            (= (fib 1) 1)
            (= (fib $n) (+ (fib (- $n 1)) (fib (- $n 2))))
            !(fib 6)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(8)));
    }

    #[test]
    fn test_higher_order_apply_twice() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (apply-twice $f $x) ($f ($f $x)))
            (= (square $x) (* $x $x))
            !(apply-twice square 2)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut last_result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(result) = expr_results.last() {
                last_result = Some(result.clone());
            }
        }

        assert_eq!(last_result, Some(MettaValue::Long(16)));
    }

    #[test]
    fn test_apply_twice_with_constructor() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (apply-twice $f $x) ($f ($f $x)))
            !(apply-twice 1 2)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::SExpr(outer)) = result {
            assert_eq!(outer[0], MettaValue::Long(1));
            if let MettaValue::SExpr(inner) = &outer[1] {
                assert_eq!(inner[0], MettaValue::Long(1));
                assert_eq!(inner[1], MettaValue::Long(2));
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_apply_three_times() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (apply-three $f $x) ($f ($f ($f $x))))
            (= (inc $x) (+ $x 1))
            !(apply-three inc 10)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(13)));
    }

    #[test]
    fn test_compose_functions() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (compose $f $g $x) ($f ($g $x)))
            (= (double $x) (* $x 2))
            (= (inc $x) (+ $x 1))
            !(compose double inc 5)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        assert_eq!(result, Some(MettaValue::Long(12)));
    }

    #[test]
    fn test_map_with_square() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (mymap $f nil) nil)
            (= (mymap $f (cons $x $xs)) (cons ($f $x) (mymap $f $xs)))
            (= (square $x) (* $x $x))
            !(mymap square (cons 1 (cons 2 (cons 3 nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::SExpr(items)) = result {
            assert_eq!(items[0], MettaValue::Atom("cons".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));

            if let MettaValue::SExpr(rest1) = &items[2] {
                assert_eq!(rest1[0], MettaValue::Atom("cons".to_string()));
                assert_eq!(rest1[1], MettaValue::Long(4));

                if let MettaValue::SExpr(rest2) = &rest1[2] {
                    assert_eq!(rest2[0], MettaValue::Atom("cons".to_string()));
                    assert_eq!(rest2[1], MettaValue::Long(9));
                }
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_filter_positive_numbers() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (filter $pred nil) nil)
            (= (filter $pred (cons $x $xs))
               (if ($pred $x)
                   (cons $x (filter $pred $xs))
                   (filter $pred $xs)))
            (= (positive $x) (> $x 0))
            !(filter positive (cons 5 (cons -3 (cons 7 nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        // Should keep only 5 and 7: (cons 5 (cons 7 nil))
        if let Some(MettaValue::SExpr(items)) = result {
            assert_eq!(items[0], MettaValue::Atom("cons".to_string()));
            assert_eq!(items[1], MettaValue::Long(5));

            if let MettaValue::SExpr(rest) = &items[2] {
                assert_eq!(rest[0], MettaValue::Atom("cons".to_string()));
                assert_eq!(rest[1], MettaValue::Long(7));
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_fold_left() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (foldl $f $acc nil) $acc)
            (= (foldl $f $acc (cons $x $xs))
            (foldl $f ($f $acc $x) $xs))
            !(foldl + 0 (cons 1 (cons 2 (cons 3 nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        // foldl(+, 0, [1,2,3]) = ((0+1)+2)+3 = 6
        assert_eq!(result, Some(MettaValue::Long(6)));
    }

    #[test]
    fn test_append_lists() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (append nil $ys) $ys)
            (= (append (cons $x $xs) $ys) (cons $x (append $xs $ys)))
            !(append (cons 1 (cons 2 nil)) (cons 3 (cons 4 nil)))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(r) = expr_results.last() {
                result = Some(r.clone());
            }
        }

        if let Some(MettaValue::SExpr(items)) = result {
            assert_eq!(items[0], MettaValue::Atom("cons".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_simple_list_length() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (len nil) 0)
            (= (len (cons $x $xs)) (+ 1 (len $xs)))
            !(len (cons a (cons b (cons c nil))))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut last_result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(result) = expr_results.last() {
                last_result = Some(result.clone());
            }
        }

        assert_eq!(last_result, Some(MettaValue::Long(3)));
    }

    #[test]
    fn test_compile_nested_lists() {
        let src = "(a (b (c d)))";
        let state = compile(src).unwrap();

        if let MettaValue::SExpr(outer) = &state.source[0] {
            assert_eq!(outer[0], MettaValue::Atom("a".to_string()));

            if let MettaValue::SExpr(middle) = &outer[1] {
                assert_eq!(middle[0], MettaValue::Atom("b".to_string()));

                if let MettaValue::SExpr(inner) = &middle[1] {
                    assert_eq!(inner[0], MettaValue::Atom("c".to_string()));
                    assert_eq!(inner[1], MettaValue::Atom("d".to_string()));
                } else {
                    panic!("Expected SExpr for innermost");
                }
            } else {
                panic!("Expected SExpr for middle");
            }
        } else {
            panic!("Expected SExpr for outer");
        }
    }

    #[test]
    fn test_basic_nondeterminism() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (coin) heads)
            (= (coin) tails)
            !(coin)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results.contains(&MettaValue::Atom("heads".to_string())));
            assert!(results.contains(&MettaValue::Atom("tails".to_string())));
        } else {
            panic!("Expected nondeterministic results");
        }
    }

    #[test]
    fn test_binary_bit_nondeterminism() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (bin) 0)
            (= (bin) 1)
            !(bin)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results.contains(&MettaValue::Long(0)));
            assert!(results.contains(&MettaValue::Long(1)));
        } else {
            panic!("Expected binary nondeterministic results");
        }
    }

    #[test]
    fn test_working_nondeterminism() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (= (pair) (cons 0 0))
            (= (pair) (cons 0 1))
            (= (pair) (cons 1 0))
            (= (pair) (cons 1 1))
            !(pair)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 4);
        } else {
            panic!("Expected 4 pair results");
        }
    }

    #[test]
    fn test_nondeterministic_nested_application() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        // Test lazy/call-by-name semantics with nondeterministic functions
        // (f) -> [1, 2, 3]
        // (g $x) -> (* $x $x)
        // With lazy evaluation: $x binds to (f) as an expression, not a value.
        // (g (f)) → (* (f) (f)) → each (f) evaluates independently to [1,2,3]
        // Result: 3×3 = 9 combinations (Cartesian product of both (f) evaluations)
        let input = r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (= (g $x) (* $x $x))
            !(g (f))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            // Lazy semantics: (f)*(f) gives 9 results (all combinations)
            // 1*1, 1*2, 1*3, 2*1, 2*2, 2*3, 3*1, 3*2, 3*3
            assert_eq!(results.len(), 9);
            // All products of pairs from [1,2,3] × [1,2,3]
            assert!(results.contains(&MettaValue::Long(1))); // 1*1
            assert!(results.contains(&MettaValue::Long(2))); // 1*2, 2*1
            assert!(results.contains(&MettaValue::Long(3))); // 1*3, 3*1
            assert!(results.contains(&MettaValue::Long(4))); // 2*2
            assert!(results.contains(&MettaValue::Long(6))); // 2*3, 3*2
            assert!(results.contains(&MettaValue::Long(9))); // 3*3
        } else {
            panic!("Expected 9 results from lazy nondeterministic evaluation");
        }
    }

    #[test]
    fn test_nondeterministic_cartesian_product() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        // Test Cartesian product: when BOTH operands are nondeterministic
        // (a) -> [1, 2]
        // (b) -> [10, 20]
        // (+ (a) (b)) should -> [11, 21, 12, 22]
        let input = r#"
            (= (a) 1)
            (= (a) 2)
            (= (b) 10)
            (= (b) 20)
            !(+ (a) (b))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 4);
            assert!(results.contains(&MettaValue::Long(11))); // 1 + 10
            assert!(results.contains(&MettaValue::Long(21))); // 1 + 20
            assert!(results.contains(&MettaValue::Long(12))); // 2 + 10
            assert!(results.contains(&MettaValue::Long(22))); // 2 + 20
        } else {
            panic!("Expected Cartesian product of nondeterministic operands");
        }
    }

    #[test]
    fn test_nondeterministic_triple_product() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        // Test triple Cartesian product
        // (x) -> [1, 2]
        // (y) -> [10, 20]
        // (z) -> [100, 200]
        // (cons (x) (cons (y) (z))) should produce 2*2*2 = 8 results
        let input = r#"
            (= (x) 1)
            (= (x) 2)
            (= (y) 10)
            (= (y) 20)
            (= (z) 100)
            (= (z) 200)
            !(cons (x) (cons (y) (z)))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 8);
        } else {
            panic!("Expected 8 results from triple Cartesian product");
        }
    }

    #[test]
    fn test_nondeterministic_deeply_nested() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        // Test deeply nested nondeterministic application
        // (f) -> [1, 2]
        // (g $x) -> (* $x 10)
        // (h $x) -> (+ $x 5)
        // (h (g (f))) should -> [15, 25]
        let input = r#"
            (= (f) 1)
            (= (f) 2)
            (= (g $x) (* $x 10))
            (= (h $x) (+ $x 5))
            !(h (g (f)))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results.contains(&MettaValue::Long(15))); // h(g(1)) = h(10) = 15
            assert!(results.contains(&MettaValue::Long(25))); // h(g(2)) = h(20) = 25
        } else {
            panic!("Expected [15, 25] from deeply nested nondeterministic application");
        }
    }

    #[test]
    fn test_nondeterministic_with_pattern_matching() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        // Test nondeterminism combined with pattern matching
        // (color) -> [red, green, blue]
        // (intensity $c) matches all colors and returns different values
        let input = r#"
            (= (color) red)
            (= (color) green)
            (= (color) blue)
            (= (intensity red) 100)
            (= (intensity green) 150)
            (= (intensity blue) 200)
            !(intensity (color))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 3);
            assert!(results.contains(&MettaValue::Long(100)));
            assert!(results.contains(&MettaValue::Long(150)));
            assert!(results.contains(&MettaValue::Long(200)));
        } else {
            panic!("Expected [100, 150, 200] from pattern matching with nondeterminism");
        }
    }

    #[test]
    fn test_match_basic_pattern() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (leaf1 leaf2)
            (leaf0 leaf1)
            !(match &self ($x leaf2) $x)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 1);
            // Should match (leaf1 leaf2) with $x = leaf1
            assert_eq!(results[0], MettaValue::Atom("leaf1".to_string()));
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_multiple_bindings() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (Sam is a frog)
            (Tom is a cat)
            (Sophia is a robot)
            !(match &self ($who is a $what) ($who the $what))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 3);
            // Should match all three facts
            let expected = vec![
                MettaValue::SExpr(vec![
                    MettaValue::Atom("Sam".to_string()),
                    MettaValue::Atom("the".to_string()),
                    MettaValue::Atom("frog".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("Tom".to_string()),
                    MettaValue::Atom("the".to_string()),
                    MettaValue::Atom("cat".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("Sophia".to_string()),
                    MettaValue::Atom("the".to_string()),
                    MettaValue::Atom("robot".to_string()),
                ]),
            ];
            for expected_result in expected {
                assert!(results.contains(&expected_result));
            }
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_nested_structure() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            ((nested value) result)
            !(match &self (($x $y) result) (found $x and $y))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 1);
            let expected = MettaValue::SExpr(vec![
                MettaValue::Atom("found".to_string()),
                MettaValue::Atom("nested".to_string()),
                MettaValue::Atom("and".to_string()),
                MettaValue::Atom("value".to_string()),
            ]);
            assert_eq!(results[0], expected);
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_no_results() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (foo bar)
            !(match &self (nonexistent $x) $x)
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut last_result = Vec::new();

        for (i, expr) in state.source.iter().enumerate() {
            let (expr_results, new_env) = eval(expr.clone(), env);
            env = new_env;
            // Capture results from the last expression (the match)
            if i == state.source.len() - 1 {
                last_result = expr_results;
            }
        }

        // Should return empty list when no matches found
        assert!(
            last_result.is_empty(),
            "Expected empty results for no match, got: {:?}",
            last_result
        );
    }

    #[test]
    fn test_match_with_numbers() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (number 42)
            (number 100)
            !(match &self (number $n) (value $n))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut result = None;

        for expr in state.source {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results.contains(&MettaValue::SExpr(vec![
                MettaValue::Atom("value".to_string()),
                MettaValue::Long(42),
            ])));
            assert!(results.contains(&MettaValue::SExpr(vec![
                MettaValue::Atom("value".to_string()),
                MettaValue::Long(100),
            ])));
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_wildcard() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"
            (a b c)
            (x y z)
            !(match &self ($first $middle $last) (middle $middle))
        "#;

        let state = compile(input).unwrap();
        let mut env = state.environment;
        let mut last_result = Vec::new();

        for (i, expr) in state.source.iter().enumerate() {
            let (expr_results, new_env) = eval(expr.clone(), env);
            env = new_env;
            // Capture results from the last expression (the match)
            if i == state.source.len() - 1 {
                last_result = expr_results;
            }
        }

        // Should match both (a b c) and (x y z), extracting middle elements
        assert!(
            last_result.len() >= 2,
            "Expected at least 2 match results, got: {} - {:?}",
            last_result.len(),
            last_result
        );
        assert!(last_result
            .iter()
            .any(|r| matches!(r, MettaValue::SExpr(items)
            if items.len() == 2 && items[0] == MettaValue::Atom("middle".to_string())
            && items[1] == MettaValue::Atom("b".to_string()))));
        assert!(last_result
            .iter()
            .any(|r| matches!(r, MettaValue::SExpr(items)
            if items.len() == 2 && items[0] == MettaValue::Atom("middle".to_string())
            && items[1] == MettaValue::Atom("y".to_string()))));
    }
}
