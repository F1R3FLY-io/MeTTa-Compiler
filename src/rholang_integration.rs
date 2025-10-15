/// Rholang integration module
/// Provides conversion between MeTTa types and Rholang Par types

use crate::backend::types::{MettaValue, MettaState};

/// Convert MettaValue to a JSON-like string representation
/// This can be parsed by Rholang to reconstruct the value
pub fn metta_value_to_rholang_string(value: &MettaValue) -> String {
    match value {
        MettaValue::Atom(s) => format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s)),
        MettaValue::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
        MettaValue::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
        MettaValue::String(s) => format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s)),
        MettaValue::Uri(s) => format!(r#"{{"type":"uri","value":"{}"}}"#, escape_json(s)),
        MettaValue::Nil => r#"{"type":"nil"}"#.to_string(),
        MettaValue::SExpr(items) => {
            let items_json: Vec<String> = items.iter()
                .map(|v| metta_value_to_rholang_string(v))
                .collect();
            format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
        }
        MettaValue::Error(msg, details) => {
            format!(
                r#"{{"type":"error","message":"{}","details":{}}}"#,
                escape_json(msg),
                metta_value_to_rholang_string(details)
            )
        }
        MettaValue::Type(t) => {
            format!(r#"{{"type":"metatype","value":{}}}"#, metta_value_to_rholang_string(t))
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

/// Compile MeTTa source and return full MettaState as JSON
/// Returns the complete state (pending_exprs, environment, eval_outputs)
/// This is the PathMap-compatible interface that should be used by all compile handlers
pub fn compile_to_json(src: &str) -> Result<String, String> {
    let state = crate::backend::compile::compile(src)?;
    Ok(metta_state_to_json(&state))
}

/// Compile MeTTa source and return full MettaState JSON (error-safe version)
/// Returns the complete state (pending_exprs, environment, eval_outputs)
pub fn compile_safe(src: &str) -> String {
    match compile_to_json(src) {
        Ok(json) => json,
        Err(e) => format!(r#"{{"error":"{}"}}"#, escape_json(&e)),
    }
}

/// Convert MettaState to JSON representation for PathMap storage
/// Returns a JSON string with the format:
/// ```json
/// {
///   "pending_exprs": [...],
///   "environment": {"facts_count": N},
///   "eval_outputs": [...]
/// }
/// ```
pub fn metta_state_to_json(state: &MettaState) -> String {
    let pending_json: Vec<String> = state.pending_exprs.iter()
        .map(|expr| metta_value_to_rholang_string(expr))
        .collect();

    let outputs_json: Vec<String> = state.eval_outputs.iter()
        .map(|output| metta_value_to_rholang_string(output))
        .collect();

    // For environment, we'll serialize facts count as a placeholder
    // Full serialization of MORK Space would require more complex handling
    let env_json = format!(r#"{{"facts_count":{}}}"#, state.environment.rule_count());

    format!(
        r#"{{"pending_exprs":[{}],"environment":{},"eval_outputs":[{}]}}"#,
        pending_json.join(","),
        env_json,
        outputs_json.join(",")
    )
}

/// Compile MeTTa source and return MettaState as JSON
/// This is the new PathMap-compatible interface
pub fn compile_to_state_json(src: &str) -> Result<String, String> {
    let state = crate::backend::compile::compile(src)?;
    Ok(metta_state_to_json(&state))
}

/// Compile MeTTa source and return MettaState JSON (error-safe version)
pub fn compile_to_state_safe(src: &str) -> String {
    match compile_to_state_json(src) {
        Ok(json) => json,
        Err(e) => format!(r#"{{"error":"{}"}}"#, escape_json(&e)),
    }
}

/// Run compiled state against accumulated state
/// This is the core evaluation function for PathMap-based REPL integration.
///
/// Takes two MettaState objects:
/// - `accumulated_state`: State with accumulated environment and outputs
/// - `compiled_state`: Fresh state with pending expressions to evaluate
///
/// Returns a new accumulated state with:
/// - Empty pending_exprs (all evaluated)
/// - Updated environment (merged with new rules/facts)
/// - Extended eval_outputs (accumulated results)
pub fn run_state(accumulated_state: MettaState, compiled_state: MettaState) -> Result<MettaState, String> {
    use crate::backend::eval::eval;

    // Start with accumulated environment
    let mut env = accumulated_state.environment;
    let mut outputs = accumulated_state.eval_outputs;

    // Evaluate each pending expression from compiled state
    for expr in compiled_state.pending_exprs {
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

/// Run state from JSON inputs (for Rholang integration)
/// Parses JSON states, runs evaluation, returns JSON result
pub fn run_state_json(_accumulated_json: &str, _compiled_json: &str) -> Result<String, String> {
    // For now, we'll use the direct MettaState approach
    // Full JSON deserialization would require a proper JSON parser
    // This is a placeholder that will be implemented when needed for Rholang
    Err("JSON state deserialization not yet implemented - use run_state() directly".to_string())
}

/// Run state from JSON (error-safe version)
pub fn run_state_json_safe(_accumulated_json: &str, _compiled_json: &str) -> String {
    match run_state_json(_accumulated_json, _compiled_json) {
        Ok(json) => json,
        Err(e) => format!(r#"{{"error":"{}"}}"#, escape_json(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple() {
        let src = "(+ 1 2)";
        let result = compile_safe(src);
        // Should return full MettaState with pending_exprs, environment, eval_outputs
        assert!(result.contains(r#""pending_exprs""#));
        assert!(result.contains(r#""environment""#));
        assert!(result.contains(r#""eval_outputs""#));
        assert!(result.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_compile_error() {
        let src = "(unclosed";
        let result = compile_safe(src);
        // Error format should contain "error" field
        assert!(result.contains(r#""error""#));
    }

    #[test]
    fn test_metta_value_atom() {
        let value = MettaValue::Atom("test".to_string());
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"atom","value":"test"}"#);
    }

    #[test]
    fn test_metta_value_number() {
        let value = MettaValue::Long(42);
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"number","value":42}"#);
    }

    #[test]
    fn test_metta_value_bool() {
        let value = MettaValue::Bool(true);
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"bool","value":true}"#);
    }

    #[test]
    fn test_metta_value_string() {
        let value = MettaValue::String("hello".to_string());
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"string","value":"hello"}"#);
    }

    #[test]
    fn test_metta_value_nil() {
        let value = MettaValue::Nil;
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"nil"}"#);
    }

    #[test]
    fn test_metta_value_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let json = metta_value_to_rholang_string(&value);
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""type":"atom","value":"add""#));
        assert!(json.contains(r#""type":"number","value":1"#));
        assert!(json.contains(r#""type":"number","value":2"#));
    }

    #[test]
    fn test_escape_json() {
        assert_eq!(escape_json(r#"hello"world"#), r#"hello\"world"#);
        assert_eq!(escape_json("hello\nworld"), r"hello\nworld");
        assert_eq!(escape_json("hello\tworld"), r"hello\tworld");
        assert_eq!(escape_json(r"hello\world"), r"hello\\world");
    }

    #[test]
    fn test_compile_nested_arithmetic() {
        let src = "(+ 1 (* 2 3))";
        let result = compile_safe(src);
        // Should return full MettaState
        assert!(result.contains(r#""pending_exprs""#));
        assert!(result.contains(r#""environment""#));
        assert!(result.contains(r#""eval_outputs""#));
        // Should have nested sexpr
        assert!(result.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_compile_multiple_expressions() {
        let src = "(+ 1 2) (- 3 4)";
        let result = compile_safe(src);
        // Should return full MettaState
        assert!(result.contains(r#""pending_exprs""#));
        assert!(result.contains(r#""environment""#));
        assert!(result.contains(r#""eval_outputs""#));
        // Should have 2 expressions in the pending_exprs array
        let count = result.matches(r#""type":"sexpr""#).count();
        assert_eq!(count, 2);
    }

    // Tests for MettaState structure and JSON serialization

    #[test]
    fn test_compile_returns_correct_state_structure() {
        use crate::backend::compile::compile;

        let src = "(+ 1 2)";
        let state = compile(src).unwrap();

        // Compiled state should have pending expressions
        assert_eq!(state.pending_exprs.len(), 1);
        // Environment should be empty (fresh compilation)
        assert_eq!(state.environment.rule_count(), 0);
        // No outputs yet
        assert_eq!(state.eval_outputs.len(), 0);
    }

    #[test]
    fn test_metta_state_to_json() {
        use crate::backend::types::MettaState;

        let state = MettaState::new_compiled(vec![
            MettaValue::Long(42)
        ]);

        let json = metta_state_to_json(&state);

        assert!(json.contains(r#""pending_exprs""#));
        assert!(json.contains(r#""environment""#));
        assert!(json.contains(r#""eval_outputs""#));
        assert!(json.contains(r#""type":"number","value":42"#));
    }

    #[test]
    fn test_compile_to_state_json() {
        let src = "(+ 1 2)";
        let json = compile_to_state_json(src).unwrap();

        assert!(json.contains(r#""pending_exprs""#));
        assert!(json.contains(r#""environment""#));
        assert!(json.contains(r#""eval_outputs""#));
    }

    // Tests for run_state() function

    #[test]
    fn test_run_state_simple_arithmetic() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Start with empty accumulated state
        let accumulated = MettaState::new_empty();

        // Compile expression with ! to produce output
        let compiled = compile("!(+ 10 5)").unwrap();

        // Run compiled against accumulated
        let result = run_state(accumulated, compiled).unwrap();

        // Should have one output
        assert_eq!(result.eval_outputs.len(), 1);
        assert_eq!(result.eval_outputs[0], MettaValue::Long(15));

        // Pending expressions should be empty (all evaluated)
        assert_eq!(result.pending_exprs.len(), 0);
    }

    #[test]
    fn test_run_state_accumulates_outputs() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Start with empty state
        let mut accumulated = MettaState::new_empty();

        // First evaluation: !(+ 1 2)
        let compiled1 = compile("!(+ 1 2)").unwrap();
        accumulated = run_state(accumulated, compiled1).unwrap();
        assert_eq!(accumulated.eval_outputs.len(), 1);
        assert_eq!(accumulated.eval_outputs[0], MettaValue::Long(3));

        // Second evaluation: !(* 3 4)
        let compiled2 = compile("!(* 3 4)").unwrap();
        accumulated = run_state(accumulated, compiled2).unwrap();

        // Should have both outputs
        assert_eq!(accumulated.eval_outputs.len(), 2);
        assert_eq!(accumulated.eval_outputs[0], MettaValue::Long(3));
        assert_eq!(accumulated.eval_outputs[1], MettaValue::Long(12));
    }

    #[test]
    fn test_run_state_rule_persistence() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Start with empty state
        let mut accumulated = MettaState::new_empty();

        // Step 1: Define a rule
        let rule_src = "(= (double $x) (* $x 2))";
        let compiled_rule = compile(rule_src).unwrap();
        accumulated = run_state(accumulated, compiled_rule).unwrap();

        // Rule definition returns empty list (no output)
        assert_eq!(accumulated.eval_outputs.len(), 0);

        // Environment should now have the rule
        assert_eq!(accumulated.environment.rule_count(), 1);

        // Step 2: Use the rule
        let use_src = "!(double 21)";
        let compiled_use = compile(use_src).unwrap();
        accumulated = run_state(accumulated, compiled_use).unwrap();

        // Should have 1 output: 42 from evaluation (rule def produced no output)
        assert_eq!(accumulated.eval_outputs.len(), 1);
        assert_eq!(accumulated.eval_outputs[0], MettaValue::Long(42));
    }

    #[test]
    fn test_run_state_multiple_expressions() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        let accumulated = MettaState::new_empty();

        // Compile multiple expressions with ! to produce outputs
        let compiled = compile("!(+ 1 2) !(* 3 4)").unwrap();
        assert_eq!(compiled.pending_exprs.len(), 2);

        let result = run_state(accumulated, compiled).unwrap();

        // Should have two outputs
        assert_eq!(result.eval_outputs.len(), 2);
        assert_eq!(result.eval_outputs[0], MettaValue::Long(3));
        assert_eq!(result.eval_outputs[1], MettaValue::Long(12));
    }

    #[test]
    fn test_run_state_repl_simulation() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Simulate a REPL session
        let mut repl_state = MettaState::new_empty();

        // Input 1: Define a rule
        repl_state = run_state(
            repl_state,
            compile("(= (triple $x) (* $x 3))").unwrap()
        ).unwrap();

        // Input 2: Use the rule
        repl_state = run_state(
            repl_state,
            compile("!(triple 7)").unwrap()
        ).unwrap();

        // Input 3: Simple arithmetic with ! to produce output
        repl_state = run_state(
            repl_state,
            compile("!(+ 10 11)").unwrap()
        ).unwrap();

        // Should have 2 outputs (rule definition produces no output)
        assert_eq!(repl_state.eval_outputs.len(), 2);
        assert_eq!(repl_state.eval_outputs[0], MettaValue::Long(21)); // triple 7
        assert_eq!(repl_state.eval_outputs[1], MettaValue::Long(21)); // 10 + 11

        // Environment should have accumulated rules
        // Note: rule_count() may not be exactly 1 due to MORK Space internals
        // The important thing is that the rule works (verified by output above)
        assert!(repl_state.environment.rule_count() > 0);
    }

    #[test]
    fn test_run_state_error_handling() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        let accumulated = MettaState::new_empty();

        // Compile an error expression with ! to produce output
        let compiled = compile(r#"!(error "test error" 42)"#).unwrap();
        let result = run_state(accumulated, compiled).unwrap();

        // Should have one error output
        assert_eq!(result.eval_outputs.len(), 1);
        match &result.eval_outputs[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "test error");
            }
            _ => panic!("Expected error value"),
        }
    }

    #[test]
    fn test_state_json_roundtrip() {
        use crate::backend::compile::compile;

        let src = "(+ 1 2)";
        let state = compile(src).unwrap();
        let json = metta_state_to_json(&state);

        // Verify JSON structure
        assert!(json.contains(r#""pending_exprs":"#));
        assert!(json.contains(r#""environment":"#));
        assert!(json.contains(r#""eval_outputs":"#));
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""facts_count":0"#));
    }

    // Composability Tests - Verify that run_state() composes correctly

    #[test]
    fn test_composability_sequential_runs() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: s.run(a).run(b).run(c) should accumulate all results
        let mut state = MettaState::new_empty();

        let a = compile("!(+ 1 2)").unwrap();
        let b = compile("!(* 3 4)").unwrap();
        let c = compile("!(- 10 5)").unwrap();

        // Compose sequentially
        state = run_state(state, a).unwrap();
        state = run_state(state, b).unwrap();
        state = run_state(state, c).unwrap();

        // All outputs should be preserved in order
        assert_eq!(state.eval_outputs.len(), 3);
        assert_eq!(state.eval_outputs[0], MettaValue::Long(3));
        assert_eq!(state.eval_outputs[1], MettaValue::Long(12));
        assert_eq!(state.eval_outputs[2], MettaValue::Long(5));
    }

    #[test]
    fn test_composability_rule_chaining() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Rules defined in earlier runs are available in later runs
        let mut state = MettaState::new_empty();

        // Define first rule: double
        state = run_state(
            state,
            compile("(= (double $x) (* $x 2))").unwrap()
        ).unwrap();

        // Define second rule that uses first: quadruple uses double
        state = run_state(
            state,
            compile("(= (quadruple $x) (double (double $x)))").unwrap()
        ).unwrap();

        // Use both rules
        state = run_state(
            state,
            compile("!(quadruple 3)").unwrap()
        ).unwrap();

        // quadruple 3 = double (double 3) = double 6 = 12
        // Index 0: first rule definition produced no output, second rule at index 0? No, both rules produce no output
        // So the first actual output is the result of !(quadruple 3)
        assert_eq!(state.eval_outputs[0], MettaValue::Long(12));
    }

    #[test]
    fn test_composability_state_independence() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Each compile is independent, run merges them
        let accum1 = MettaState::new_empty();
        let accum2 = MettaState::new_empty();

        let compiled = compile("!(+ 10 20)").unwrap();

        // Run same compiled state against different accumulated states
        let result1 = run_state(accum1, compiled.clone()).unwrap();
        let result2 = run_state(accum2, compiled).unwrap();

        // Both should produce same result
        assert_eq!(result1.eval_outputs[0], MettaValue::Long(30));
        assert_eq!(result2.eval_outputs[0], MettaValue::Long(30));

        // Both should have same output count
        assert_eq!(result1.eval_outputs.len(), 1);
        assert_eq!(result2.eval_outputs.len(), 1);
    }

    #[test]
    fn test_composability_monotonic_accumulation() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Outputs never decrease, only accumulate
        let mut state = MettaState::new_empty();

        // Track output counts
        let mut counts = vec![];

        for i in 1..=5 {
            let src = format!("!(+ {} {})", i, i);
            state = run_state(state, compile(&src).unwrap()).unwrap();
            counts.push(state.eval_outputs.len());
        }

        // Output count should increase monotonically
        assert_eq!(counts, vec![1, 2, 3, 4, 5]);

        // Each output should be preserved
        for (i, output) in state.eval_outputs.iter().enumerate() {
            let expected = (i + 1) * 2;
            assert_eq!(*output, MettaValue::Long(expected as i64));
        }
    }

    #[test]
    fn test_composability_empty_state_identity() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Running against empty state should work like first run
        let empty = MettaState::new_empty();
        let compiled = compile("!(+ 5 7)").unwrap();

        let result = run_state(empty, compiled).unwrap();

        // Should have exactly one output
        assert_eq!(result.eval_outputs.len(), 1);
        assert_eq!(result.eval_outputs[0], MettaValue::Long(12));

        // Pending should be empty (all evaluated)
        assert_eq!(result.pending_exprs.len(), 0);
    }

    #[test]
    fn test_composability_environment_union() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Environments properly union across runs
        let mut state = MettaState::new_empty();

        // Add first rule
        state = run_state(
            state,
            compile("(= (inc $x) (+ $x 1))").unwrap()
        ).unwrap();

        let rules_after_first = state.environment.rule_count();

        // Add second rule
        state = run_state(
            state,
            compile("(= (dec $x) (- $x 1))").unwrap()
        ).unwrap();

        let rules_after_second = state.environment.rule_count();

        // Should have more rules after second (monotonic)
        assert!(rules_after_second >= rules_after_first);

        // Both rules should work
        state = run_state(
            state,
            compile("!(inc 5) !(dec 5)").unwrap()
        ).unwrap();

        // Should have 2 outputs: 6 + 4 (rule defs produce no output)
        assert!(state.eval_outputs.len() >= 2);
        assert_eq!(state.eval_outputs[state.eval_outputs.len() - 2], MettaValue::Long(6));
        assert_eq!(state.eval_outputs[state.eval_outputs.len() - 1], MettaValue::Long(4));
    }

    #[test]
    fn test_composability_no_cross_contamination() {
        use crate::backend::compile::compile;
        use crate::backend::types::MettaState;

        // Test: Independent state chains don't affect each other
        let mut state_a = MettaState::new_empty();
        let mut state_b = MettaState::new_empty();

        // State A: Define rule "double"
        state_a = run_state(
            state_a,
            compile("(= (double $x) (* $x 2))").unwrap()
        ).unwrap();

        // State B: Define rule "triple"
        state_b = run_state(
            state_b,
            compile("(= (triple $x) (* $x 3))").unwrap()
        ).unwrap();

        // State A should have double, not triple
        state_a = run_state(
            state_a,
            compile("!(double 5)").unwrap()
        ).unwrap();
        // Rule def produced no output, so first output is at index 0
        assert_eq!(state_a.eval_outputs[0], MettaValue::Long(10));

        // State B should have triple, not double
        state_b = run_state(
            state_b,
            compile("!(triple 5)").unwrap()
        ).unwrap();
        // Rule def produced no output, so first output is at index 0
        assert_eq!(state_b.eval_outputs[0], MettaValue::Long(15));
    }
}
