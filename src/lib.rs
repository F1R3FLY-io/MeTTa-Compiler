// Enable cfg(sanitize = "address") for ASAN integration with the slab allocator.
// Requires nightly Rust (same as -Zsanitizer=address).
#![feature(cfg_sanitize)]

pub mod backend;
pub mod config;
pub mod ir;
pub mod parser;
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
/// use mettatron::{compile, eval, new_env, MettaValue};
///
/// // Define a rule and evaluate it
/// let input = r#"
///     (= (double $x) (* $x 2))
///     !(double 21)
/// "#;
///
/// let state = compile(input).unwrap();
/// let mut env = new_env();
/// let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
/// for expr in source_exprs {
///     let (results, new_env) = eval(expr, env, &state);
///     env = new_env;
///
///     for result in results {
///         println!("{:?}", result.inner());
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
pub use ir::{MettaExpr, Position, SExpr, Span};
pub use tree_sitter_parser::TreeSitterMettaParser;

// ============================================================================
// Slab-Allocated API (Primary — GC-backed allocation)
// ============================================================================

pub use backend::{
    // Compilation (MeTTa source → MettaState with MettaValue expressions)
    compile, compile_with_path,
    // Evaluation
    eval, new_env,
    // Core types
    SessionContext, MettaEnvironment, EvalResult,
    MettaValue, MettaValueInner,
    // Allocator utilities
    global_factory, global_allocator, GcFactory,
    // Data model types (used by bytecode VM, JIT, and tiered cache internals)
    models::MettaState,
    // Thread pool initialization (eager startup from main)
    init_thread_pools,
    // Signal-triggered diagnostic dump (SIGTERM/SIGUSR1)
    diagnostics::install_signal_handlers,
};

// State evaluation API
pub use rholang_integration::run_state;
#[cfg(feature = "async")]
pub use rholang_integration::run_state_async;

// Session-based evaluation API
pub use rholang_integration::{state_to_json, eval_metta_session, eval_metta_session_raw};

pub use pathmap_par_integration::{
    decode_large_exprs_bytes_to_pars,
    decode_space_bytes_to_pars, has_metta_state_structure, metta_error_to_par,
    metta_run_error_expr, metta_run_error_par, metta_state_to_pathmap_par,
    metta_value_to_par, par_to_metta_value, pathmap_par_to_metta_state,
    pathmap_par_to_metta_state_lenient,
};

pub use config::{configure_eval, get_eval_config, EvalConfig};

// Export commonly used REPL components
pub use repl::{MettaHelper, PatternHistory, QueryHighlighter, ReplStateMachine, SmartIndenter};

// Evaluation trace system (zero-cost when disabled)
#[cfg(feature = "eval-trace")]
pub use backend::trace;
#[cfg(feature = "eval-trace")]
pub use backend::eval_with_trace;

#[cfg(test)]
mod tests {
    use smallvec::SmallVec;
    use crate::backend::compile::compile;
    use crate::backend::eval::eval;
    use crate::backend::eval::trampoline::new_env;
    use crate::backend::models::{MettaValue, MettaValueInner};

    /// Helper to check if results contain a Long value
    fn results_contain_long(results: &[MettaValue], n: i64) -> bool {
        results
            .iter()
            .any(|r| matches!(r.inner(), MettaValueInner::Long(v) if *v == n))
    }

    /// Helper to check if results contain an Atom value
    fn results_contain_atom(results: &[MettaValue], s: &str) -> bool {
        results
            .iter()
            .any(|r| matches!(r.inner(), MettaValueInner::Atom(v) if *v == s))
    }

    /// Helper to assert a result is a Long
    fn assert_long(val: MettaValue, expected: i64) {
        assert!(
            matches!(val.inner(), MettaValueInner::Long(n) if *n == expected),
            "Expected Long({}), got {:?}",
            expected,
            val.inner()
        );
    }

    /// Helper to assert a result is an Atom
    fn assert_atom(val: MettaValue, expected: &str) {
        assert!(
            matches!(val.inner(), MettaValueInner::Atom(s) if *s == expected),
            "Expected Atom({:?}), got {:?}",
            expected,
            val.inner()
        );
    }

    /// Helper to assert a result is a String
    fn assert_string(val: MettaValue, expected: &str) {
        assert!(
            matches!(val.inner(), MettaValueInner::String(s) if *s == expected),
            "Expected String({:?}), got {:?}",
            expected,
            val.inner()
        );
    }

    /// Helper to assert a result is a Bool
    fn assert_bool(val: MettaValue, expected: bool) {
        assert!(
            matches!(val.inner(), MettaValueInner::Bool(b) if *b == expected),
            "Expected Bool({}), got {:?}",
            expected,
            val.inner()
        );
    }

    #[test]
    fn test_compile_simple() {
        let result = compile("(+ 1 2)");
        assert!(result.is_ok());
    }

    #[test]
    fn test_compile_and_eval_arithmetic() {
        let input = "(+ 10 20)";
        let state = compile(input).expect("compile failed");
        assert_eq!(state.source().len(), 1);

        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Long(30)));
    }

    #[test]
    fn test_rule_definition_and_evaluation() {
        let input = r#"
            (= (double $x) (* $x 2))
            !(double 21)
        "#;

        let state = compile(input).expect("compile failed");
        assert_eq!(state.source().len(), 2);
        let mut env = new_env();

        // First expression: rule definition
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        // Rule definition returns empty list
        assert!(results.is_empty());

        // Second expression: evaluation
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[1];
        let (results, _env) = eval(expr, env, &state);
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Long(42)));
    }

    #[test]
    fn test_multiple_evaluations() {
        let input = r#"
            (= (double $x) (* $x 2))
            !(double 5)
            !(double 10)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut all_results = Vec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;

            if !expr_results.is_empty() {
                all_results.extend(expr_results);
            }
        }

        assert_eq!(all_results.len(), 2);
        assert_long(all_results[0], 10);
        assert_long(all_results[1], 20);
    }

    #[test]
    fn test_evaluation_steps() {
        let input = r#"
            (= (add1 $x) (+ $x 1))
            (= (add2 $x) (+ $x 2))
            !(add1 5)
            !(add2 5)
            !(add1 (add2 10))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut evaluations = Vec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for (i, expr) in source_exprs.iter().copied().enumerate() {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;

            if !expr_results.is_empty() {
                evaluations.push((i, expr_results[0]));
            }
        }

        assert_eq!(evaluations.len(), 3);
        assert_long(evaluations[0].1, 6);
        assert_long(evaluations[1].1, 7);
        assert_long(evaluations[2].1, 13);
    }

    #[test]
    fn test_if_control_flow() {
        let input = r#"(if (< 5 10) "yes" "no")"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::String(s) if *s == "yes"));
    }

    #[test]
    fn test_if_with_equality_check() {
        let input = r#"(if (== 5 5) "equal" "not-equal")"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_string(results[0], "equal");
    }

    #[test]
    fn test_if_lazy_evaluation_true_branch() {
        let input = r#"
            (= (boom) (error "should not evaluate" 0))
            (if True success (boom))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_atom(r, "success");
    }

    #[test]
    fn test_if_prevents_infinite_loop() {
        let input = r#"
            (= (loop) (loop))
            (if True success (loop))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_atom(r, "success");
    }

    #[test]
    fn test_factorial_with_if() {
        let input = r#"
            (= (factorial $x)
            (if (> $x 0)
                (* $x (factorial (- $x 1)))
                1))
            !(factorial 5)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 120);
    }

    #[test]
    fn test_factorial_base_case() {
        let input = r#"
            (= (factorial $x)
            (if (> $x 0)
                (* $x (factorial (- $x 1)))
                1))
            !(factorial 0)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 1);
    }

    #[test]
    fn test_nested_if() {
        let input = r#"
            (if (> 10 5)
                (if (< 3 7) "both-true" "outer-true-inner-false")
                "outer-false")
        "#;

        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_string(results[0], "both-true");
    }

    #[test]
    fn test_if_with_computation_in_branches() {
        let input = r#"(if (< 5 10) (+ 2 3) (* 4 5))"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_long(results[0], 5);
    }

    #[test]
    fn test_if_with_function_calls_in_branches() {
        let input = r#"
            (= (double $x) (* $x 2))
            (= (triple $x) (* $x 3))
            !(if (> 10 5) (double 7) (triple 7))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 14);
    }

    #[test]
    fn test_quote() {
        // (quote X) is self-evaluating — it preserves the Quoted wrapper.
        // This matches HE behavior: !(quote (+ 1 2)) → (quote (+ 1 2))
        let input = "(quote (+ 1 2))";
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert!(
            matches!(results[0].inner(), MettaValueInner::Quoted(_)),
            "Expected Quoted variant, got: {:?}", results[0]
        );
        // The inner value should be the unevaluated S-expression (+ 1 2)
        let inner = results[0].as_quoted().expect("Expected Quoted");
        assert!(
            matches!(inner.inner(), MettaValueInner::SExpr(_)),
            "Expected inner SExpr, got: {:?}", inner
        );
    }

    #[test]
    fn test_error_propagation() {
        let input = r#"(error "test error" 42)"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert!(matches!(results[0].inner(), MettaValueInner::Error(_, _)));
    }

    #[test]
    fn test_error_in_nested_expression() {
        let input = r#"(+ 1 (+ 2 (+ 3 (error "deep error" nested))))"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        if let MettaValueInner::Error(msg, _) = results[0].inner() {
            assert_eq!(*msg, "deep error");
        } else {
            panic!("Expected error propagation from nested expression");
        }
    }

    #[test]
    fn test_error_in_function_call() {
        let input = r#"
            (= (safe-op $x) (if (< $x 0) (error "negative value" $x) (* $x 2)))
            !(safe-op -5)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::Error(msg, details) = r.inner() {
                assert_eq!(*msg, "negative value");
                assert!(matches!(details.inner(), MettaValueInner::Long(-5)));
            } else {
                panic!("Expected error from function call");
            }
        } else {
            panic!("Expected error from function call");
        }
    }

    #[test]
    fn test_error_in_recursive_function() {
        let input = r#"
            (= (div-by-zero $n)
                (if (== $n 0)
                    (error "division by zero" $n)
                    (div-by-zero (- $n 1))))
            !(div-by-zero 3)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::Error(msg, _) = r.inner() {
                assert_eq!(*msg, "division by zero");
            } else {
                panic!("Expected error from recursive function");
            }
        } else {
            panic!("Expected error from recursive function");
        }
    }

    #[test]
    fn test_error_with_catch() {
        let input = r#"(catch (error "caught" 42) "default-value")"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_string(results[0], "default-value");
    }

    #[test]
    fn test_catch_without_error() {
        let input = r#"(catch (+ 5 7) "default-value")"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_long(results[0], 12);
    }

    #[test]
    fn test_nested_catch() {
        let input = r#"
        (catch
                (catch (error "inner" 1) (error "middle" 2))
                "outer-default")
        "#;

        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_string(results[0], "outer-default");
    }

    #[test]
    fn test_error_in_condition() {
        let input = r#"(if (error "condition failed" cond) yes no)"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        if let MettaValueInner::Error(msg, _) = results[0].inner() {
            assert_eq!(*msg, "condition failed");
        } else {
            panic!("Expected error from condition evaluation");
        }
    }

    #[test]
    fn test_is_error_check() {
        let input = r#"(is-error (error "test" 0))"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        assert_bool(results[0], true);
    }

    #[test]
    fn test_is_error_with_normal_value() {
        let input = r#"(is-error (+ 1 2))"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        // Arena evaluator may return multiple results; check that at least one is Bool(false)
        assert!(
            !results.is_empty(),
            "Expected at least one result"
        );
        assert!(
            results.iter().any(|r| matches!(r.inner(), MettaValueInner::Bool(false))),
            "Expected Bool(false) in results, got {:?}",
            results.iter().map(|r| format!("{:?}", r.inner())).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_error_recovery_pattern() {
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut results = Vec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        assert_long(results[0], -1);
        assert_long(results[1], 5);
    }

    #[test]
    fn test_multiple_errors_in_sequence() {
        let input = r#"
            (error "first" 1)
            (error "second" 2)
            (error "third" 3)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut errors = Vec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(r) = expr_results.first() {
                if let MettaValueInner::Error(msg, _) = r.inner() {
                    errors.push(msg.to_string());
                }
            }
        }

        assert_eq!(errors.len(), 3);
        assert_eq!(errors[0], "first");
        assert_eq!(errors[1], "second");
        assert_eq!(errors[2], "third");
    }

    #[test]
    fn test_error_stops_evaluation_in_expression() {
        let input = r#"
            (= (side-effect) (error "should not see this" 0))
            (+ (error "first-error" 1) (side-effect))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::Error(msg, _) = r.inner() {
                assert_eq!(*msg, "first-error");
            } else {
                panic!("Expected first error to propagate");
            }
        } else {
            panic!("Expected first error to propagate");
        }
    }

    #[test]
    fn test_error_with_complex_details() {
        let input = r#"(error "complex" (+ 1 (+ 2 3)))"#;
        let state = compile(input).expect("compile failed");
        // SAFE: MutexGuard dropped at semicolon, before eval() runs.
        // Prevents ABBA deadlock between source mutex and GC_IN_PROGRESS.
        let expr = state.source()[0];
        let (results, _env) = eval(expr, new_env(), &state);

        assert_eq!(results.len(), 1);
        if let MettaValueInner::Error(msg, details) = results[0].inner() {
            assert_eq!(*msg, "complex");
            assert!(matches!(details.inner(), MettaValueInner::SExpr(_)));
        } else {
            panic!("Expected error with complex details");
        }
    }

    #[test]
    fn test_catch_in_recursive_context() {
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut results = Vec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                results.extend(expr_results);
            }
        }

        assert_long(results[0], 120);
        assert_long(results[1], 0);
    }

    #[test]
    fn test_invalid_syntax() {
        let result = compile("(+ 1");
        assert!(result.is_err());
    }

    #[test]
    fn test_simple_recursion() {
        // Uses if-guard instead of overlapping base-case pattern because MeTTa HE
        // fires all matching rules nondeterministically (no specificity filter).
        let input = r#"
            (= (countdown $n) (if (== $n 0) done (countdown (- $n 1))))
            !(countdown 3)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut last_result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                last_result = Some(r);
            }
        }

        let r = last_result.expect("Expected a result");
        assert_atom(r, "done");
    }

    #[test]
    fn test_recursive_list_length_safe() {
        let input = r#"
            (= (len nil) 0)
            (= (len (cons $x $xs)) (+ 1 (len $xs)))
            !(len (cons a (cons b (cons c nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 3);
    }

    #[test]
    fn test_recursive_list_sum() {
        let input = r#"
            (= (sum nil) 0)
            (= (sum (cons $x $xs)) (+ $x (sum $xs)))
            !(sum (cons 10 (cons 20 (cons 30 nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 60);
    }

    #[test]
    fn test_recursive_fibonacci() {
        // Uses if-guards instead of overlapping base-case patterns because MeTTa HE
        // fires all matching rules nondeterministically (no specificity filter).
        let input = r#"
            (= (fib $n) (if (== $n 0) 0 (if (== $n 1) 1 (+ (fib (- $n 1)) (fib (- $n 2))))))
            !(fib 6)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 8);
    }

    #[test]
    fn test_higher_order_apply_twice() {
        let input = r#"
            (= (apply-twice $f $x) ($f ($f $x)))
            (= (square $x) (* $x $x))
            !(apply-twice square 2)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut last_result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                last_result = Some(r);
            }
        }

        let r = last_result.expect("Expected a result");
        assert_long(r, 16);
    }

    #[test]
    fn test_apply_twice_with_constructor() {
        let input = r#"
            (= (apply-twice $f $x) ($f ($f $x)))
            !(apply-twice 1 2)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::SExpr(outer) = r.inner() {
                assert!(matches!(outer[0].inner(), MettaValueInner::Long(1)));
                if let MettaValueInner::SExpr(inner) = outer[1].inner() {
                    assert!(matches!(inner[0].inner(), MettaValueInner::Long(1)));
                    assert!(matches!(inner[1].inner(), MettaValueInner::Long(2)));
                }
            } else {
                panic!("Expected SExpr result");
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_apply_three_times() {
        let input = r#"
            (= (apply-three $f $x) ($f ($f ($f $x))))
            (= (inc $x) (+ $x 1))
            !(apply-three inc 10)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 13);
    }

    #[test]
    fn test_compose_functions() {
        let input = r#"
            (= (compose $f $g $x) ($f ($g $x)))
            (= (double $x) (* $x 2))
            (= (inc $x) (+ $x 1))
            !(compose double inc 5)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        let r = result.expect("Expected a result");
        assert_long(r, 12);
    }

    #[test]
    fn test_map_with_square() {
        let input = r#"
            (= (mymap $f nil) nil)
            (= (mymap $f (cons $x $xs)) (cons ($f $x) (mymap $f $xs)))
            (= (square $x) (* $x $x))
            !(mymap square (cons 1 (cons 2 (cons 3 nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::SExpr(items) = r.inner() {
                assert!(matches!(items[0].inner(), MettaValueInner::Atom("cons")));
                assert!(matches!(items[1].inner(), MettaValueInner::Long(1)));

                if let MettaValueInner::SExpr(rest1) = items[2].inner() {
                    assert!(matches!(rest1[0].inner(), MettaValueInner::Atom("cons")));
                    assert!(matches!(rest1[1].inner(), MettaValueInner::Long(4)));

                    if let MettaValueInner::SExpr(rest2) = rest1[2].inner() {
                        assert!(matches!(rest2[0].inner(), MettaValueInner::Atom("cons")));
                        assert!(matches!(rest2[1].inner(), MettaValueInner::Long(9)));
                    }
                }
            } else {
                panic!("Expected SExpr result");
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_filter_positive_numbers() {
        let input = r#"
            (= (filter $pred nil) nil)
            (= (filter $pred (cons $x $xs))
               (if ($pred $x)
                   (cons $x (filter $pred $xs))
                   (filter $pred $xs)))
            (= (positive $x) (> $x 0))
            !(filter positive (cons 5 (cons -3 (cons 7 nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        // Should keep only 5 and 7: (cons 5 (cons 7 nil))
        if let Some(r) = result {
            if let MettaValueInner::SExpr(items) = r.inner() {
                assert!(matches!(items[0].inner(), MettaValueInner::Atom("cons")));
                assert!(matches!(items[1].inner(), MettaValueInner::Long(5)));

                if let MettaValueInner::SExpr(rest) = items[2].inner() {
                    assert!(matches!(rest[0].inner(), MettaValueInner::Atom("cons")));
                    assert!(matches!(rest[1].inner(), MettaValueInner::Long(7)));
                }
            } else {
                panic!("Expected SExpr result");
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_fold_left() {
        let input = r#"
            (= (foldl $f $acc nil) $acc)
            (= (foldl $f $acc (cons $x $xs))
            (foldl $f ($f $acc $x) $xs))
            !(foldl + 0 (cons 1 (cons 2 (cons 3 nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        // foldl(+, 0, [1,2,3]) = ((0+1)+2)+3 = 6
        let r = result.expect("Expected a result");
        assert_long(r, 6);
    }

    #[test]
    fn test_append_lists() {
        let input = r#"
            (= (append nil $ys) $ys)
            (= (append (cons $x $xs) $ys) (cons $x (append $xs $ys)))
            !(append (cons 1 (cons 2 nil)) (cons 3 (cons 4 nil)))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                result = Some(r);
            }
        }

        if let Some(r) = result {
            if let MettaValueInner::SExpr(items) = r.inner() {
                assert!(matches!(items[0].inner(), MettaValueInner::Atom("cons")));
                assert!(matches!(items[1].inner(), MettaValueInner::Long(1)));
            } else {
                panic!("Expected SExpr result");
            }
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_simple_list_length() {
        let input = r#"
            (= (len nil) 0)
            (= (len (cons $x $xs)) (+ 1 (len $xs)))
            !(len (cons a (cons b (cons c nil))))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut last_result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if let Some(&r) = expr_results.last() {
                last_result = Some(r);
            }
        }

        let r = last_result.expect("Expected a result");
        assert_long(r, 3);
    }

    #[test]
    fn test_compile_nested_lists() {
        let src = "(a (b (c d)))";
        let state = compile(src).expect("compile failed");

        let first = {let s = state.source(); s[0]};
        if let MettaValueInner::SExpr(outer) = first.inner() {
            assert!(matches!(outer[0].inner(), MettaValueInner::Atom("a")));

            if let MettaValueInner::SExpr(middle) = outer[1].inner() {
                assert!(matches!(middle[0].inner(), MettaValueInner::Atom("b")));

                if let MettaValueInner::SExpr(inner) = middle[1].inner() {
                    assert!(matches!(inner[0].inner(), MettaValueInner::Atom("c")));
                    assert!(matches!(inner[1].inner(), MettaValueInner::Atom("d")));
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
        let input = r#"
            (= (coin) heads)
            (= (coin) tails)
            !(coin)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results_contain_atom(&results, "heads"));
            assert!(results_contain_atom(&results, "tails"));
        } else {
            panic!("Expected nondeterministic results");
        }
    }

    #[test]
    fn test_binary_bit_nondeterminism() {
        let input = r#"
            (= (bin) 0)
            (= (bin) 1)
            !(bin)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results_contain_long(&results, 0));
            assert!(results_contain_long(&results, 1));
        } else {
            panic!("Expected binary nondeterministic results");
        }
    }

    #[test]
    fn test_working_nondeterminism() {
        let input = r#"
            (= (pair) (cons 0 0))
            (= (pair) (cons 0 1))
            (= (pair) (cons 1 0))
            (= (pair) (cons 1 1))
            !(pair)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
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
        // Test applicative evaluation with nondeterministic functions.
        // (f) -> [1, 2, 3]
        // (g $x) -> (* $x $x)
        //
        // With bloom filter applicative eval:
        // (g (f)) → (f) is pre-evaluated to {1, 2, 3} (nondeterministic)
        // → (g 1), (g 2), (g 3) are evaluated independently
        // → (* 1 1) = 1, (* 2 2) = 4, (* 3 3) = 9
        // Result: {1, 4, 9} — 3 results (call-by-value semantics)
        let input = r#"
            (= (f) 1)
            (= (f) 2)
            (= (f) 3)
            (= (g $x) (* $x $x))
            !(g (f))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            // Applicative eval: (f) pre-evaluated to {1,2,3}, each fed to (g $x)
            assert_eq!(results.len(), 3);
            assert!(results_contain_long(&results, 1)); // g(1) = 1*1
            assert!(results_contain_long(&results, 4)); // g(2) = 2*2
            assert!(results_contain_long(&results, 9)); // g(3) = 3*3
        } else {
            panic!("Expected 3 results from applicative nondeterministic evaluation");
        }
    }

    #[test]
    fn test_nondeterministic_cartesian_product() {
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 4);
            assert!(results_contain_long(&results, 11)); // 1 + 10
            assert!(results_contain_long(&results, 21)); // 1 + 20
            assert!(results_contain_long(&results, 12)); // 2 + 10
            assert!(results_contain_long(&results, 22)); // 2 + 20
        } else {
            panic!("Expected Cartesian product of nondeterministic operands");
        }
    }

    #[test]
    fn test_nondeterministic_triple_product() {
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            assert!(results_contain_long(&results, 15)); // h(g(1)) = h(10) = 15
            assert!(results_contain_long(&results, 25)); // h(g(2)) = h(20) = 25
        } else {
            panic!("Expected [15, 25] from deeply nested nondeterministic application");
        }
    }

    #[test]
    fn test_nondeterministic_with_pattern_matching() {
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

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 3);
            assert!(results_contain_long(&results, 100));
            assert!(results_contain_long(&results, 150));
            assert!(results_contain_long(&results, 200));
        } else {
            panic!("Expected [100, 150, 200] from pattern matching with nondeterminism");
        }
    }

    #[test]
    fn test_match_basic_pattern() {
        let input = r#"
            (leaf1 leaf2)
            (leaf0 leaf1)
            !(match &self ($x leaf2) $x)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 1);
            // Should match (leaf1 leaf2) with $x = leaf1
            assert_atom(results[0], "leaf1");
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_multiple_bindings() {
        let input = r#"
            (Sam is a frog)
            (Tom is a cat)
            (Sophia is a robot)
            !(match &self ($who is a $what) ($who the $what))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 3);
            // Should match all three facts - check via string representation
            let result_strs: Vec<String> = results.iter().map(|r| r.to_string()).collect();
            assert!(
                result_strs.contains(&"(Sam the frog)".to_string()),
                "Expected (Sam the frog), got {:?}",
                result_strs
            );
            assert!(
                result_strs.contains(&"(Tom the cat)".to_string()),
                "Expected (Tom the cat), got {:?}",
                result_strs
            );
            assert!(
                result_strs.contains(&"(Sophia the robot)".to_string()),
                "Expected (Sophia the robot), got {:?}",
                result_strs
            );
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_nested_structure() {
        let input = r#"
            ((nested value) result)
            !(match &self (($x $y) result) (found $x and $y))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 1);
            let result_str = results[0].to_string();
            assert_eq!(
                result_str, "(found nested and value)",
                "Expected (found nested and value), got {:?}",
                result_str
            );
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_no_results() {
        let input = r#"
            (foo bar)
            !(match &self (nonexistent $x) $x)
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut last_result = SmallVec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for (i, expr) in source_exprs.iter().copied().enumerate() {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            // Capture results from the last expression (the match)
            if i == source_exprs.len() - 1 {
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
        let input = r#"
            (number 42)
            (number 100)
            !(match &self (number $n) (value $n))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut result = None;

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in source_exprs {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            if !expr_results.is_empty() {
                result = Some(expr_results);
            }
        }

        if let Some(results) = result {
            assert_eq!(results.len(), 2);
            let result_strs: Vec<String> = results.iter().map(|r| r.to_string()).collect();
            assert!(
                result_strs.contains(&"(value 42)".to_string()),
                "Expected (value 42), got {:?}",
                result_strs
            );
            assert!(
                result_strs.contains(&"(value 100)".to_string()),
                "Expected (value 100), got {:?}",
                result_strs
            );
        } else {
            panic!("Expected match results");
        }
    }

    #[test]
    fn test_match_wildcard() {
        let input = r#"
            (a b c)
            (x y z)
            !(match &self ($first $middle $last) (middle $middle))
        "#;

        let state = compile(input).expect("compile failed");
        let mut env = new_env();
        let mut last_result = SmallVec::new();

        let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for (i, expr) in source_exprs.iter().copied().enumerate() {
            let (expr_results, new_env) = eval(expr, env, &state);
            env = new_env;
            // Capture results from the last expression (the match)
            if i == source_exprs.len() - 1 {
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
            .any(|r| matches!(r.inner(), MettaValueInner::SExpr(items)
            if items.len() == 2
            && matches!(items[0].inner(), MettaValueInner::Atom("middle"))
            && matches!(items[1].inner(), MettaValueInner::Atom("b")))));
        assert!(last_result
            .iter()
            .any(|r| matches!(r.inner(), MettaValueInner::SExpr(items)
            if items.len() == 2
            && matches!(items[0].inner(), MettaValueInner::Atom("middle"))
            && matches!(items[1].inner(), MettaValueInner::Atom("y")))));
    }
}
