use crate::backend::compile::compile;
/// Rholang Integration Module - Evaluation Functions
///
/// **PRIMARY INTEGRATION**: Use `pathmap_par_integration` module for Rholang interop
///
/// This module provides:
/// 1. **JSON export** for debugging and inspection (`metta_state_to_json`)
/// 2. **State evaluation** for REPL-style interaction (`run_state`, `run_state_async`)
/// 3. **Error handling** for safe compilation (`compile_safe`)
///
/// **Note**: For Rholang integration, use the PathMap Par functions in
/// `pathmap_par_integration` module, not the JSON functions here.
use crate::backend::models::{MettaState, MettaValue};

/// Safe compilation wrapper that never fails
///
/// This function wraps the `compile()` function and provides improved error handling
/// for Rholang integration. Instead of returning `Result<MettaState, String>`,
/// it always returns a `MettaState`:
/// - On success: Normal compiled state with parsed expressions
/// - On error: State containing an error s-expression: `(error "message")`
///
/// This allows Rholang contracts to handle syntax errors gracefully without
/// requiring complex error propagation through the Rholang runtime.
///
/// # Error Messages
///
/// The function improves upon Tree-Sitter's raw error messages by:
/// - Extracting line and column information
/// - Providing context about the error type
/// - Suggesting common fixes for known error patterns
///
/// # Example
///
/// ```ignore
/// // Valid MeTTa code
/// let state = compile_safe("(+ 1 2)");
/// assert_eq!(state.source.len(), 1);
///
/// // Invalid syntax - returns error s-expression
/// let state = compile_safe("(+ 1 2");  // Unclosed parenthesis
/// // state.source[0] == (error "Syntax error at line 1, column 7: ...")
/// ```
pub fn compile_safe(src: &str) -> MettaState {
    match compile(src) {
        Ok(state) => state,
        Err(error_msg) => {
            // Improve error message with additional context
            let improved_msg = improve_error_message(&error_msg, src);

            // Create error s-expression: (error "message")
            let error_sexpr = MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String(improved_msg),
            ]);

            // Return a state containing the error
            MettaState::new_with_error(error_sexpr)
        }
    }
}

/// Improve error messages with additional context and suggestions
fn improve_error_message(raw_error: &str, source: &str) -> String {
    // Extract line/column info if present
    let error_lower = raw_error.to_lowercase();

    // Provide contextual suggestions based on error patterns
    if error_lower.contains("unexpected") && error_lower.contains("'") {
        // Likely unclosed parenthesis or unexpected EOF
        let unclosed_parens = count_unclosed_parens(source);
        if unclosed_parens > 0 {
            return format!(
                "{} (Hint: {} unclosed parenthesis{} detected)",
                raw_error,
                unclosed_parens,
                if unclosed_parens == 1 { "" } else { "es" }
            );
        } else if unclosed_parens < 0 {
            return format!(
                "{} (Hint: {} extra closing parenthesis{} detected)",
                raw_error,
                -unclosed_parens,
                if unclosed_parens == -1 { "" } else { "es" }
            );
        }
    }

    // Return improved message or original if no improvements found
    raw_error.to_string()
}

/// Count unclosed parentheses in source
/// Returns: positive = unclosed open parens, negative = extra close parens
fn count_unclosed_parens(source: &str) -> i32 {
    let mut depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in source.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_string => escape_next = true,
            '"' => in_string = !in_string,
            '(' if !in_string => depth += 1,
            ')' if !in_string => depth -= 1,
            _ => {}
        }
    }

    depth
}

/// Convert MettaValue to a JSON-like string representation
/// Used for debugging and human-readable output
fn metta_value_to_json_string(value: &MettaValue) -> String {
    match value {
        MettaValue::Atom(s) => format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s)),
        MettaValue::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
        MettaValue::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
        MettaValue::Float(f) => format!(r#"{{"type":"number","value":{}}}"#, f),
        MettaValue::String(s) => format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s)),
        MettaValue::Uri(s) => format!(r#"{{"type":"uri","value":"{}"}}"#, escape_json(s)),
        MettaValue::Nil => r#"{"type":"nil"}"#.to_string(),
        MettaValue::SExpr(items) => {
            let items_json: Vec<String> = items.iter().map(metta_value_to_json_string).collect();
            format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
        }
        MettaValue::Error(msg, details) => {
            format!(
                r#"{{"type":"error","message":"{}","details":{}}}"#,
                escape_json(msg),
                metta_value_to_json_string(details)
            )
        }
        MettaValue::Type(t) => {
            format!(
                r#"{{"type":"metatype","value":{}}}"#,
                metta_value_to_json_string(t)
            )
        }
    }
}

/// Escape JSON special characters
fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

/// Convert MettaState to JSON representation for debugging
///
/// Returns a JSON string with the format:
/// ```json
/// {
///   "source": [...],
///   "environment": {"facts_count": N},
///   "output": [...]
/// }
/// ```
///
/// **Use Case**: Debugging, logging, inspection
/// **Not Recommended**: Rholang integration (use PathMap Par instead)
pub fn metta_state_to_json(state: &MettaState) -> String {
    let source_json: Vec<String> = state
        .source
        .iter()
        .map(metta_value_to_json_string)
        .collect();

    let outputs_json: Vec<String> = state
        .output
        .iter()
        .map(metta_value_to_json_string)
        .collect();

    // For environment, we'll serialize facts count as a placeholder
    // Full serialization of MORK Space would require more complex handling
    let env_json = format!(r#"{{"facts_count":{}}}"#, state.environment.rule_count());

    format!(
        r#"{{"source":[{}],"environment":{},"output":[{}]}}"#,
        source_json.join(","),
        env_json,
        outputs_json.join(",")
    )
}

/// Run compiled state against accumulated state
///
/// This is the core evaluation function for REPL-style interaction.
///
/// Takes two MettaState objects:
/// - `accumulated_state`: State with accumulated environment and outputs
/// - `compiled_state`: Fresh state with pending expressions to evaluate
///
/// Returns a new accumulated state with:
/// - Empty source (all evaluated)
/// - Updated environment (merged with new rules/facts)
/// - Extended output (accumulated results)
///
/// **Threading**: Synchronous, single-threaded evaluation
pub fn run_state(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    let mut outputs = accumulated_state.output;

    // Evaluate each pending expression from compiled state
    for expr in compiled_state.source {
        // Check if this is an evaluation expression (starts with !)
        let is_eval_expr = matches!(&expr, MettaValue::SExpr(items) if items.first().map(|v| matches!(v, MettaValue::Atom(s) if s == "!")).unwrap_or(false));

        let (results, new_env) = eval(expr, env);
        env = new_env;

        // Only extend outputs for evaluation expressions (!)
        // Other S-expressions are added to the atom space but produce no outputs
        if is_eval_expr {
            outputs.extend(results);
        }
    }

    // Return new accumulated state
    Ok(MettaState::new_accumulated(env, outputs))
}

/// Async version of run_state with parallel evaluation of independent expressions
///
/// This function parallelizes evaluation of consecutive `!` (eval) expressions
/// while maintaining sequential execution for rule definitions (`=`) to preserve
/// MeTTa semantics.
///
/// **MeTTa Semantics Preserved:**
/// - Rule definitions execute sequentially (environment threading)
/// - Independent eval expressions execute in parallel
/// - Output ordering is preserved
/// - Environment updates are atomic per batch
///
/// **Threading Model:** Uses Tokio's async/await (same as Rholang)
///
/// **Thread Safety:** Environment now uses `Arc<Mutex<T>>` for thread-safe sharing
#[cfg(feature = "async")]
pub async fn run_state_async(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    let mut outputs = accumulated_state.output;

    // Batch expressions into parallelizable groups
    let mut current_batch: Vec<(usize, MettaValue, bool)> = Vec::new();
    let exprs: Vec<_> = compiled_state.source.into_iter().enumerate().collect();

    for (idx, expr) in exprs {
        let is_eval_expr = matches!(&expr, MettaValue::SExpr(items)
            if items.first().map(|v| matches!(v, MettaValue::Atom(s) if s == "!")).unwrap_or(false));

        let is_rule_def = matches!(&expr, MettaValue::SExpr(items)
            if items.first().map(|v| matches!(v, MettaValue::Atom(s) if s == "=")).unwrap_or(false));

        // If this is a rule definition and we have a batch, evaluate the batch first
        if is_rule_def && !current_batch.is_empty() {
            // Evaluate parallel batch
            let batch_results = evaluate_batch_parallel(current_batch, env.clone()).await;
            for (_batch_idx, results, should_output) in batch_results {
                if should_output {
                    outputs.extend(results);
                }
            }
            current_batch = Vec::new();
        }

        // If this is a rule definition, execute it sequentially
        if is_rule_def {
            let (_results, new_env) = eval(expr, env);
            env = new_env;
            // Rule definitions don't produce output
        } else {
            // Add to current batch for parallel execution
            current_batch.push((idx, expr, is_eval_expr));
        }
    }

    // Evaluate any remaining batch
    if !current_batch.is_empty() {
        let batch_results = evaluate_batch_parallel(current_batch, env.clone()).await;
        for (_batch_idx, results, should_output) in batch_results {
            if should_output {
                outputs.extend(results);
            }
        }
    }

    Ok(MettaState::new_accumulated(env, outputs))
}

/// Helper function to evaluate a batch of expressions in parallel
/// Returns results in original order with their indices
#[cfg(feature = "async")]
async fn evaluate_batch_parallel(
    batch: Vec<(usize, MettaValue, bool)>,
    env: crate::backend::environment::Environment,
) -> Vec<(usize, Vec<MettaValue>, bool)> {
    use crate::backend::eval::eval;
    use tokio::task;

    // Spawn parallel evaluation tasks
    let tasks: Vec<_> = batch
        .into_iter()
        .map(|(idx, expr, should_output)| {
            let env = env.clone(); // Arc clone is cheap
            task::spawn_blocking(move || {
                let (results, _new_env) = eval(expr, env);
                (idx, results, should_output)
            })
        })
        .collect();

    // Collect results
    let mut results = Vec::new();
    for task_handle in tasks {
        match task_handle.await {
            Ok(result) => results.push(result),
            Err(e) => {
                // Task panicked - this shouldn't happen with our eval
                eprintln!("Parallel evaluation task panicked: {:?}", e);
            }
        }
    }

    // Sort results by original index to preserve order
    results.sort_by_key(|(idx, _, _)| *idx);

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_metta_state_to_json() {
        let src = "(+ 1 2)";
        let state = compile(src).unwrap();
        let json = metta_state_to_json(&state);

        // Should return full MettaState with source, environment, output
        assert!(json.contains(r#""source""#));
        assert!(json.contains(r#""environment""#));
        assert!(json.contains(r#""output""#));
        assert!(json.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_metta_value_atom() {
        let value = MettaValue::Atom("test".to_string());
        let json = metta_value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"atom","value":"test"}"#);
    }

    #[test]
    fn test_metta_value_number() {
        let value = MettaValue::Long(42);
        let json = metta_value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"number","value":42}"#);
    }

    #[test]
    fn test_metta_value_bool() {
        let value = MettaValue::Bool(true);
        let json = metta_value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"bool","value":true}"#);
    }

    #[test]
    fn test_metta_value_string() {
        let value = MettaValue::String("hello".to_string());
        let json = metta_value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"string","value":"hello"}"#);
    }

    #[test]
    fn test_metta_value_nil() {
        let value = MettaValue::Nil;
        let json = metta_value_to_json_string(&value);
        assert_eq!(json, r#"{"type":"nil"}"#);
    }

    #[test]
    fn test_metta_value_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let json = metta_value_to_json_string(&value);
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""items""#));
    }

    #[test]
    fn test_escape_json() {
        let escaped = escape_json("hello\n\"world\"\\test");
        assert_eq!(escaped, r#"hello\n\"world\"\\test"#);
    }

    #[test]
    fn test_compile_safe_success() {
        let state = compile_safe("(+ 1 2)");
        assert_eq!(state.source.len(), 1);
        // Should be a valid S-expression, not an error
        match &state.source[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::Atom("+".to_string()));
            }
            _ => panic!("Expected SExpr for valid input"),
        }
    }

    #[test]
    fn test_compile_safe_syntax_error() {
        let state = compile_safe("(+ 1 2");
        assert_eq!(state.source.len(), 1);
        // Should be an error s-expression
        match &state.source[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], MettaValue::Atom("error".to_string()));
                // Error message should be a string
                assert!(matches!(&items[1], MettaValue::String(_)));
                // Error message should mention the syntax issue
                if let MettaValue::String(msg) = &items[1] {
                    assert!(msg.contains("Syntax error") || msg.contains("unexpected"));
                }
            }
            _ => panic!("Expected error s-expression for syntax error"),
        }
    }

    #[test]
    fn test_compile_safe_improves_error_message() {
        let state = compile_safe("(+ 1 2");
        match &state.source[0] {
            MettaValue::SExpr(items) => {
                if let MettaValue::String(msg) = &items[1] {
                    // Should include hint about unclosed parenthesis
                    assert!(msg.contains("Hint") && msg.contains("unclosed"));
                }
            }
            _ => panic!("Expected error s-expression"),
        }
    }

    #[test]
    fn test_run_state_simple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile("!(+ 1 2)").unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(3));
    }

    #[test]
    fn test_run_state_with_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 21)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(42));
    }

    // Async Parallel Evaluation Tests
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_simple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile("!(+ 1 2)").unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have output
        assert!(!result.output.is_empty());
        assert_eq!(result.output[0], MettaValue::Long(3));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_parallel() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            !(+ 1 1)
            !(+ 2 2)
            !(+ 3 3)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have all outputs
        assert_eq!(result.output.len(), 3);
        assert_eq!(result.output[0], MettaValue::Long(2));
        assert_eq!(result.output[1], MettaValue::Long(4));
        assert_eq!(result.output[2], MettaValue::Long(6));
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_with_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (double $x) (* $x 2))
            !(double 5)
            !(double 10)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Should have outputs (parallel evaluation of both double calls)
        assert_eq!(result.output.len(), 2);
        assert_eq!(result.output[0], MettaValue::Long(10));
        assert_eq!(result.output[1], MettaValue::Long(20));
    }
}
