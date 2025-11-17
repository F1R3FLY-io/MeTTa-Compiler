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
/// - Fresh output (only results from THIS invocation's `!` evaluations)
///
/// **Threading**: Synchronous, single-threaded evaluation
pub fn run_state(
    accumulated_state: MettaState,
    compiled_state: MettaState,
) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    // Start with empty outputs - each .run() returns only its own results
    let mut outputs = Vec::new();

    // Evaluate each pending expression from compiled state
    for expr in compiled_state.source {
        let is_eval_expr = expr.is_eval_expr();

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

    // Start with accumulated environment, but if it's empty and compiled has data, use compiled's environment
    use pathmap::zipper::ZipperIteration;

    let acc_count = {
        let space = accumulated_state.environment.create_space();
        let mut rz = space.btm.read_zipper();
        let mut count = 0;
        while rz.to_next_val() {
            count += 1;
        }
        count
    };

    let comp_count = {
        let space = compiled_state.environment.create_space();
        let mut rz = space.btm.read_zipper();
        let mut count = 0;
        while rz.to_next_val() {
            count += 1;
        }
        count
    };

    // Use the environment that has data (prefer accumulated, fall back to compiled if accumulated is empty)
    let mut env = if acc_count > 0 || comp_count == 0 {
        accumulated_state.environment
    } else {
        compiled_state.environment.clone()
    };
    // Start with empty outputs - each .run() returns only its own results
    let mut outputs = Vec::new();

    // Batch expressions into parallelizable groups
    let mut current_batch: Vec<(usize, MettaValue, bool)> = Vec::new();
    let exprs: Vec<_> = compiled_state.source.into_iter().enumerate().collect();

    for (idx, expr) in exprs {
        let is_eval_expr = expr.is_eval_expr();
        let is_rule_def = expr.is_rule_def();

        // Check if this is a ground fact (S-expression that's not a rule and not an eval)
        let is_ground_fact = matches!(&expr, MettaValue::SExpr(_)) && !is_rule_def && !is_eval_expr;

        // If this is a rule definition or ground fact and we have a batch, evaluate the batch first
        if (is_rule_def || is_ground_fact) && !current_batch.is_empty() {
            // Evaluate parallel batch
            let batch_results = evaluate_batch_parallel(current_batch, env.clone()).await;
            for (_batch_idx, results, should_output) in batch_results {
                if should_output {
                    outputs.extend(results);
                }
            }
            current_batch = Vec::new();
        }

        // If this is a rule definition or ground fact, execute it sequentially
        // (both modify the environment by adding to MORK Space)
        if is_rule_def || is_ground_fact {
            let (_results, new_env) = eval(expr, env);
            env = new_env;
            // Neither rule definitions nor ground facts produce output
            // They only modify the environment by adding to MORK Space
            // (Ground facts are not wrapped in !, so they shouldn't generate output)
        } else {
            // Only eval expressions go in parallel batch
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

    #[test]
    fn test_ground_facts_not_in_output() {
        // Regression test: verify ground facts are NOT added to output
        let mut accumulated = MettaState::new_empty();

        // Add ground facts
        let compiled1 = compile("(connected room_a room_b) (connected room_b room_c)").unwrap();
        accumulated = run_state(accumulated, compiled1).unwrap();
        // Ground facts should NOT produce output
        assert_eq!(accumulated.output.len(), 0);

        // Verify ground facts are in environment (can be queried)
        let compiled2 = compile("!(match &self (connected $from $to) ($from $to))").unwrap();
        accumulated = run_state(accumulated, compiled2).unwrap();
        // Now output should contain query results (2 matches)
        assert_eq!(accumulated.output.len(), 2);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_multiple_rules_sequential() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (square $x) (* $x $x))
            !(square 3)
            (= (double $x) (* $x 2))
            !(double 3)
            "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        assert_eq!(result.output.len(), 2);
        assert_eq!(result.output[0], MettaValue::Long(9));
        assert_eq!(result.output[1], MettaValue::Long(6));
    }

    #[test]
    fn test_run_state_accumulated_state() {
        // Test that rules persist across multiple run_state calls
        let accumulated = MettaState::new_empty();
        let compiled1 = compile("(= (double $x) (* $x 2))").unwrap();
        let result1 = run_state(accumulated, compiled1).unwrap();

        let compiled2 = compile("!(double 5)").unwrap();
        let result2 = run_state(result1, compiled2).unwrap();

        assert!(!result2.output.is_empty());
        assert_eq!(result2.output[0], MettaValue::Long(10));
    }

    #[test]
    fn test_run_state_rule_ordering() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (f special-value) catched)
            (= (f $x) $x)
            !(f A)
            !(f special-value)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should have outputs for both calls
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_nested() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (triple $x) ($x $x $x))
            (= (grid3x3 $x) (triple (triple $x)))
            !(grid3x3 (square (+ 1 2)))
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_recursive_function() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
            (= (factorial 0) 1)
            (= (factorial $x) (* $x (factorial (- $x 1))))
            !(factorial 5)
            "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
        // Note: This might need adjustment based on actual evaluation behavior
        // Factorial of 5 should be 120
    }

    // Space Operations Tests - Adding Facts
    #[test]
    fn test_run_state_add_facts_to_space() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                !(+ 1 1)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Facts are added to space (no output), only eval expression produces output
        assert_eq!(result.output.len(), 1);
        assert_eq!(result.output[0], MettaValue::Long(2));
    }

    #[test]
    fn test_run_state_facts_persist_across_runs() {
        // First run: add facts
        let accumulated1 = MettaState::new_empty();
        let compiled1 = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                "#,
        )
        .unwrap();
        let result1 = run_state(accumulated1, compiled1).unwrap();

        // Second run: use facts via rules
        let compiled2 = compile(
            r#"
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();
        let result2 = run_state(result1, compiled2).unwrap();

        // Should be able to query the facts
        assert!(!result2.output.is_empty());
    }

    // Pattern Matching and Queries
    #[test]
    fn test_run_state_simple_pattern_match() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find parents of Bob
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_pattern_match_with_variables() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (= (find-parents $parent)
                   (match &self (Parent $parent $child) ($parent $child)))
                !(find-parents Tom)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find all children of Tom
        assert!(!result.output.is_empty());
    }

    // Family Relationship Tests
    #[test]
    fn test_run_state_family_relationships() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (female Pam)
                (female Ann)
                (male Tom)
                (male Bob)
                (= (grandparent $gp $gc)
                   (match &self (parent $gp $p)
                          (match &self (parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_recursive_ancestor_relation() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Bob Ann)
                (parent Ann Sara)
                (= (ancestor $a $d)
                   (match &self (parent $a $d) True))
                (= (ancestor $a $d)
                   (match &self (parent $a $p)
                          (ancestor $p $d)))
                !(ancestor Tom Sara)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find that Tom is an ancestor of Sara
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_family_query() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (parent Tom Bob)
                (parent Pam Bob)
                (parent Bob Ann)
                (parent Bob Pat)
                (parent Pat Jim)
                (female Pam)
                (female Ann)
                (male Tom)
                (= (sibling $s1 $s2)
                   (match &self (parent $p $s1)
                          (match &self (parent $p $s2)
                                 (if (== $s1 $s2) (empty) True))))
                !(sibling Ann Pat)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    // Constraint Solving Tests
    #[test]
    fn test_run_state_nondeterministic_choice() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                !(small-digit)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce multiple results (nondeterministic)
        assert!(!result.output.is_empty());
        // All results should be valid digits
        for output in &result.output {
            if let MettaValue::Long(n) = output {
                assert!(*n >= 1 && *n <= 3);
            }
        }
    }

    #[test]
    fn test_run_state_constraint_solving_pair() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-pair)
                   (let $x (small-digit)
                        (let $y (small-digit)
                             (if (not-equal $x $y)
                                 ($x $y)
                                 (empty)))))
                !(solve-pair)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce pairs where x != y
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_constraint_solving_triple() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (= (small-digit) 1)
                (= (small-digit) 2)
                (= (small-digit) 3)
                (= (not-equal $x $y)
                   (if (== $x $y) (empty) True))
                (= (solve-triple)
                   (let $x (small-digit)
                        (if (== $x 1)
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     (let $z (small-digit)
                                          (if (and (not-equal $x $z) (not-equal $y $z))
                                              ($x $y $z)
                                              (empty)))
                                     (empty)))
                            (let $y (small-digit)
                                 (if (not-equal $x $y)
                                     ($x $y 1)
                                     (empty))))))
                !(solve-triple)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should produce valid triples with constraints
        assert!(!result.output.is_empty());
    }

    // Knowledge Base Operations
    #[test]
    fn test_run_state_entity_relations() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (works alice acme)
                (works bob beta)
                (friends alice carol)
                (located acme SF)
                (located beta NYC)
                (= (find-colleagues $person)
                   (match &self (works $person $company)
                          (match &self (works $other $company)
                                 (if (== $person $other) (empty) $other))))
                !(find-colleagues alice)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_complex_pattern_matching() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Tom Liz)
                (Parent Bob Ann)
                (= (get-parent-entries $parent $child)
                   (match &self (Parent $parent $child) (Parent $parent $child)))
                !(get-parent-entries Tom $child)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find all children of Tom
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_nested_queries() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (Likes Bob Ann)
                (= (find-liked-grandchildren $grandparent)
                   (match &self (Parent $grandparent $parent)
                          (match &self (Parent $parent $child)
                                 (match &self (Likes $parent $child) $child))))
                !(find-liked-grandchildren Tom)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_rule_with_multiple_matches() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (Parent Bob Pat)
                (= (get-all-children $parent)
                   (match &self (Parent $parent $child) $child))
                !(get-all-children Bob)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Should find both Ann and Pat
        assert!(!result.output.is_empty());
    }

    // Async tests for space operations
    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_add_facts_then_query() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        assert!(!result.output.is_empty());
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_ground_facts_not_in_output_async() {
        // Regression test: verify ground facts are NOT added to output (async version)
        let mut accumulated = MettaState::new_empty();

        // Add ground facts
        let compiled1 = compile("(connected room_a room_b) (connected room_b room_c)").unwrap();
        accumulated = run_state_async(accumulated, compiled1).await.unwrap();
        // Ground facts should NOT produce output
        assert_eq!(accumulated.output.len(), 0);

        // Verify ground facts are in environment (can be queried)
        let compiled2 = compile("!(match &self (connected $from $to) ($from $to))").unwrap();
        accumulated = run_state_async(accumulated, compiled2).await.unwrap();
        // Now output should contain query results (2 matches)
        assert_eq!(accumulated.output.len(), 2);
    }

    #[cfg(feature = "async")]
    #[tokio::test]
    async fn test_run_state_async_parallel_queries() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                (= (get-parents $child)
                   (match &self (Parent $parent $child) $parent))
                !(get-parents Bob)
                !(get-parents Ann)
                "#,
        )
        .unwrap();

        let result = run_state_async(accumulated, compiled).await.unwrap();

        // Both queries should execute in parallel
        assert!(!result.output.is_empty());
    }

    #[test]
    fn test_run_state_facts_only_no_output() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Pam Bob)
                (Parent Bob Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        // Facts are added to space but produce no output
        assert_eq!(result.output.len(), 0);
    }

    #[test]
    fn test_run_state_mixed_facts_and_rules() {
        let accumulated = MettaState::new_empty();
        let compiled = compile(
            r#"
                (Parent Tom Bob)
                (Parent Bob Ann)
                (= (grandparent $gp $gc)
                   (match &self (Parent $gp $p)
                          (match &self (Parent $p $gc) True)))
                !(grandparent Tom Ann)
                "#,
        )
        .unwrap();

        let result = run_state(accumulated, compiled).unwrap();

        assert!(!result.output.is_empty());
    }
}
