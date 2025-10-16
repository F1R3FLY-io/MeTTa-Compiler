pub mod backend;
pub mod ffi;
pub mod pathmap_par_integration;
pub mod rholang_integration;
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

pub use backend::{
    compile, eval,
    types::{Environment, MettaState, MettaValue, Rule},
};
pub use pathmap_par_integration::{
    metta_error_to_par, metta_state_to_pathmap_par, metta_value_to_par, par_to_metta_value,
    pathmap_par_to_metta_state,
};
pub use rholang_integration::{compile_to_state_json, metta_state_to_json, run_state};
pub use sexpr::{Lexer, Parser, SExpr, Token};

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
        // Rule definition returns empty list
        assert!(results.is_empty());

        // Second expression: evaluation
        let (results, _env) = eval(state.pending_exprs[1].clone(), env);
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

        for expr in state.pending_exprs {
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

        for (i, expr) in state.pending_exprs.iter().enumerate() {
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
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::String(ref s) if s == "yes"));
    }

    #[test]
    fn test_if_with_equality_check() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(if (== 5 5) "equal" "not-equal")"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("both-true".to_string()));
    }

    #[test]
    fn test_if_with_computation_in_branches() {
        use crate::backend::compile::compile;
        use crate::backend::eval::eval;

        let input = r#"(if (< 5 10) (+ 2 3) (* 4 5))"#;
        let state = compile(input).unwrap();
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

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

        for expr in state.pending_exprs {
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
        let (results, _env) = eval(state.pending_exprs[0].clone(), state.environment);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], MettaValue::SExpr(_)));
    }

    // TODO -> more on error propagation and invalid syntax
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
            let (expr_results, new_env) = eval(expr, env);
            env = new_env;
            if let Some(result) = expr_results.last() {
                last_result = Some(result.clone());
            }
        }

        assert_eq!(last_result, Some(MettaValue::Long(3)));
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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

        for expr in state.pending_exprs {
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
}
