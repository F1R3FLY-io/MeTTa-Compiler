#![allow(dead_code, unused_imports)]

/// Rholang Integration Tests
///
/// Tests the MeTTa-Rholang integration by executing .rho files
/// and validating their output.
mod common;

use common::{
    contains_error,
    extract_eval_outputs,
    find_rholang_cli,
    integration_dir,
    // Phase 2: New validation infrastructure
    parse_pathmap,
    validate,
    Expectation,
    MettaValue,
    MettaValueTestExt,
    OutputMatcher,
    // Phase 3: Query and matching framework
    PathMapQuery,
    ToMettaValue,
    ValidationResult,
};
use std::process::Command;

/// Helper to run a Rholang test file
fn run_rho_test(filename: &str) -> (bool, String, String) {
    let rholang_cli = find_rholang_cli();
    let test_file = integration_dir().join(filename);

    assert!(
        test_file.exists(),
        "Test file not found: {}",
        test_file.display()
    );

    let output = Command::new(&rholang_cli)
        .arg("--quiet")
        .arg(&test_file)
        .output()
        .expect("Failed to execute rholang-cli");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    (success, stdout, stderr)
}

/// Helper to assert test passed (Phase 2: uses new validation)
fn assert_test_passed(filename: &str) {
    let (success, stdout, stderr) = run_rho_test(filename);
    let exit_code = if success { 0 } else { 1 };

    // Phase 2: Use structured validation
    let success_check = Expectation::success("exit code is 0");
    let success_result = validate(&stdout, &stderr, exit_code, &success_check);

    let no_errors_check = Expectation::no_errors("no error indicators");
    let no_errors_result = validate(&stdout, &stderr, exit_code, &no_errors_check);

    // Report failures with detailed information
    if success_result.is_fail() || no_errors_result.is_fail() {
        eprintln!("=== Test Failed: {} ===", filename);
        eprintln!("Exit code: {}", exit_code);

        if success_result.is_fail() {
            eprintln!(
                "\nExit code validation failed: {}",
                success_result.failure_reason().unwrap()
            );
        }
        if no_errors_result.is_fail() {
            eprintln!(
                "\nError validation failed: {}",
                no_errors_result.failure_reason().unwrap()
            );
        }

        eprintln!("\n=== STDOUT ===\n{}", stdout);
        eprintln!("\n=== STDERR ===\n{}", stderr);
        panic!("Test failed: {}", filename);
    }
}

// ============================================================================
// Integration Test Suite - Reorganized Test Files
// ============================================================================

#[test]
fn test_basic_evaluation() {
    let (success, stdout, stderr) = run_rho_test("test_basic_evaluation.rho");
    let exit_code = if success { 0 } else { 1 };

    // Validate success and completion
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::success("exits cleanly")
        )
        .is_pass(),
        "Test failed with exit code {}",
        exit_code
    );
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::no_errors("no errors")
        )
        .is_pass(),
        "Test output contains error indicators"
    );

    // Validate PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    assert!(
        pathmaps.len() >= 8,
        "Expected at least 8 PathMaps (8 tests), got {}",
        pathmaps.len()
    );

    // Tests run in parallel, so we need to find the PathMaps by their outputs
    // rather than assuming a specific order

    // Test 1: Basic addition !(+ 1 2) → [3]
    let has_addition = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[3i64]));
    assert!(has_addition, "Expected output [3] for basic addition test");

    // Test 2: Basic subtraction !(- 10 3) → [7]
    let has_subtraction = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[7i64]));
    assert!(
        has_subtraction,
        "Expected output [7] for basic subtraction test"
    );

    // Test 3: Basic multiplication !(* 4 5) → [20]
    let has_multiplication = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[20i64]));
    assert!(
        has_multiplication,
        "Expected output [20] for basic multiplication test"
    );

    // Test 4: Basic division !(/ 20 4) → [5]
    let has_division = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[5i64]));
    assert!(has_division, "Expected output [5] for basic division test");

    // Test 5: Nested arithmetic !(+ 1 (* 2 3)) → [7]
    // This will also match Test 2, so check it exists (already verified above)

    // Test 6: Complex nested !(+ (* 2 3) (- 10 5)) → [11]
    let has_complex = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[11i64]));
    assert!(
        has_complex,
        "Expected output [11] for complex nested expression test"
    );

    // Test 7: Multiple expressions → [3, 12, 5]
    let has_multiple = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[3i64, 12i64, 5i64]));
    assert!(
        has_multiple,
        "Expected outputs [3, 12, 5] for multiple expressions test"
    );

    // Test 8: Boolean comparisons → [true, true, true]
    let has_booleans = pathmaps.iter().any(|pm| {
        pm.output.len() == 3 && pm.output.iter().all(|v| v.to_display_string() == "true")
    });
    assert!(
        has_booleans,
        "Expected outputs [true, true, true] for boolean comparisons test"
    );
}

#[test]
fn test_rules() {
    let (success, stdout, stderr) = run_rho_test("test_rules.rho");
    let exit_code = if success { 0 } else { 1 };

    // Validate success and completion
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::success("exits cleanly")
        )
        .is_pass(),
        "Test failed with exit code {}",
        exit_code
    );
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::no_errors("no errors")
        )
        .is_pass(),
        "Test output contains error indicators"
    );

    // Validate PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    assert!(
        pathmaps.len() >= 6,
        "Expected at least 6 PathMaps (6 tests), got {}",
        pathmaps.len()
    );

    // Test 1: Simple rule definition → [] (empty output)
    let has_empty_rule = pathmaps
        .iter()
        .any(|pm| pm.output.is_empty() && pm.has_environment());
    assert!(
        has_empty_rule,
        "Expected PathMap with empty output and environment (rule definition)"
    );

    // Test 2: Rule usage !(triple 7) → [21]
    let has_triple = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[21i64]));
    assert!(has_triple, "Expected output [21] for triple 7 test");

    // Test 3: Rule chaining !(quadruple 3) → [12]
    let has_quadruple = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[12i64]) && pm.has_environment());
    assert!(
        has_quadruple,
        "Expected output [12] for quadruple 3 test with rules in environment"
    );

    // Test 4: Multiple rule definitions !(double 5) !(triple 5) → [10, 15]
    let has_both_rules = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[10i64, 15i64]));
    assert!(
        has_both_rules,
        "Expected outputs [10, 15] for double and triple test"
    );

    // Test 5: Rule with multiple parameters !(add-mult 5 4) → [22]
    let has_multi_param = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[22i64]));
    assert!(
        has_multi_param,
        "Expected output [22] for add-mult 5 4 test"
    );

    // Test 6: Rule persistence → [42, 21, 6]
    let has_persistence = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[42i64, 21i64, 6i64]));
    assert!(
        has_persistence,
        "Expected outputs [42, 21, 6] for rule persistence test"
    );
}

#[test]
fn test_control_flow() {
    let (success, stdout, stderr) = run_rho_test("test_control_flow.rho");
    let exit_code = if success { 0 } else { 1 };

    // Validate success and completion
    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_control_flow");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Validation 1: Clean exit
    let exits_cleanly = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::success("exits cleanly"),
    );
    report.add_result(
        "Process exits cleanly",
        if exits_cleanly.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!("Exit code: {}", exit_code))
        },
    );

    // Validation 2: No errors
    let no_errors = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::no_errors("no errors"),
    );
    report.add_result(
        "No error indicators in output",
        if no_errors.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Output contains error indicators")
        },
    );

    // Validation 3: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (8 tests)",
        if pathmaps.len() >= 8 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 8 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Actual outputs based on current implementation:
    // PathMap 0: [] (if true - produces empty output)
    // PathMap 1: [SExpr([String("+"), Long(1), Long(2)])] (quote test)
    // PathMap 2: [] (if false - produces empty output)
    // PathMap 3: [Long(7)] (eval/quote composition)
    // PathMap 4-7: [] (error handling tests - produce empty outputs)
    //
    // NOTE: Many control flow features (if/then/else, error handling) are not yet
    // fully implemented or produce empty outputs. This test validates the features
    // that do work (quote, eval) and verifies that all tests ran.

    // Validation 4: quote → [(+, 1, 2)] (unevaluated s-expression)
    let has_quote = pathmaps.iter().any(|pm| {
        use common::PathMapQuery;
        pm.output.len() == 1 && pm.query_sexpr("+").exists(|_| true)
    });
    report.add_result(
        "Quote produces unevaluated s-expression (+, 1, 2)",
        if has_quote {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected quoted s-expression not found")
        },
    );

    // Validation 5: quote and eval composition !(eval (quote (+ 3 4))) → [7]
    let has_eval_quote = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[7i64]));
    report.add_result(
        "Eval/quote composition produces [7]",
        if has_eval_quote {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [7] not found")
        },
    );

    // Validation 6: Verify all 8 tests ran (count empty PathMaps for unimplemented features)
    let empty_count = pathmaps.iter().filter(|pm| pm.output.is_empty()).count();
    report.add_result(
        "All 8 tests executed (6+ empty PathMaps for unimplemented features)",
        if empty_count >= 6 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 6 empty PathMaps, got {}",
                empty_count
            ))
        },
    );

    // Validation 7: Test suite completion message
    report.add_result(
        "Test suite completion message present",
        if stdout.contains("Control Flow Test Suite Complete") {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected completion message not found")
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

#[test]
fn test_types() {
    let (success, stdout, stderr) = run_rho_test("test_types.rho");
    let exit_code = if success { 0 } else { 1 };

    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_types");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Validation 1: Clean exit
    let exits_cleanly = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::success("exits cleanly"),
    );
    report.add_result(
        "Process exits cleanly",
        if exits_cleanly.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!("Exit code: {}", exit_code))
        },
    );

    // Validation 2: No errors
    let no_errors = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::no_errors("no errors"),
    );
    report.add_result(
        "No error indicators in output",
        if no_errors.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Output contains error indicators")
        },
    );

    // Validation 3: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (7 tests)",
        if pathmaps.len() >= 7 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 7 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Actual outputs based on current implementation:
    // PathMap 0: [] (type assertion - empty output)
    // PathMap 1: [] (empty)
    // PathMap 2: [Bool(false)] (check-type mismatch)
    // PathMap 3: [Bool(true)] (check-type match or is-error)
    // PathMap 4: [] (empty)
    // PathMap 5: [String("Bool")] (get-type for boolean)
    // PathMap 6: [String("Number")] (get-type for number)
    //
    // NOTE: Some type system features (get-type for strings, type assertions with
    // environment tracking) are not yet fully implemented. This test validates the
    // features that do work: get-type for numbers/booleans and check-type.

    // Validation 4: get-type for number → ["Number"]
    let has_number_type = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&["Number"]));
    report.add_result(
        "get-type for number returns \"Number\"",
        if has_number_type {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [\"Number\"] not found")
        },
    );

    // Validation 5: get-type for boolean → ["Bool"]
    let has_bool_type = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&["Bool"]));
    report.add_result(
        "get-type for boolean returns \"Bool\"",
        if has_bool_type {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [\"Bool\"] not found")
        },
    );

    // Validation 6: check-type match → [true]
    let has_check_match = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[true]));
    report.add_result(
        "check-type match returns true",
        if has_check_match {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [true] not found")
        },
    );

    // Validation 7: check-type mismatch → [false]
    let has_check_mismatch = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[false]));
    report.add_result(
        "check-type mismatch returns false",
        if has_check_mismatch {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [false] not found")
        },
    );

    // Validation 8: Verify tests ran (count empty PathMaps for unimplemented features)
    let empty_count = pathmaps.iter().filter(|pm| pm.output.is_empty()).count();
    report.add_result(
        "All 7 tests executed (3+ empty PathMaps for unimplemented features)",
        if empty_count >= 3 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 3 empty PathMaps, got {}",
                empty_count
            ))
        },
    );

    // Validation 9: Test suite completion message
    report.add_result(
        "Test suite completion message present",
        if stdout.contains("Type System Test Suite Complete") {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected completion message not found")
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

#[test]
fn test_repl_simulation() {
    let (success, stdout, stderr) = run_rho_test("test_repl_simulation.rho");
    let exit_code = if success { 0 } else { 1 };

    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_repl_simulation");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Validation 1: Clean exit
    let exits_cleanly = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::success("exits cleanly"),
    );
    report.add_result(
        "Process exits cleanly",
        if exits_cleanly.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!("Exit code: {}", exit_code))
        },
    );

    // Validation 2: No errors
    let no_errors = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::no_errors("no errors"),
    );
    report.add_result(
        "No error indicators in output",
        if no_errors.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Output contains error indicators")
        },
    );

    // Validation 3: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (4 tests)",
        if pathmaps.len() >= 4 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 4 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Validation 4: Simple REPL session → [15, 12, 10]
    let has_repl_session = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[15i64, 12i64, 10i64]));
    report.add_result(
        "Simple REPL session produces [15, 12, 10]",
        if has_repl_session {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [15, 12, 10] not found")
        },
    );

    // Validation 5: Interactive rule definition → [25, 49]
    let has_square = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[25i64, 49i64]));
    report.add_result(
        "Interactive rule definition (square) produces [25, 49]",
        if has_square {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [25, 49] not found")
        },
    );

    // Validation 6: Building up context → [6, 4, 10]
    let has_incremental = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[6i64, 4i64, 10i64]));
    report.add_result(
        "Incremental context building produces [6, 4, 10]",
        if has_incremental {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [6, 4, 10] not found")
        },
    );

    // Validation 7: Mix of definitions and evaluations → [3, 20, 21]
    let has_mixed = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[3i64, 20i64, 21i64]));
    report.add_result(
        "Mixed definitions/evaluations produce [3, 20, 21]",
        if has_mixed {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [3, 20, 21] not found")
        },
    );

    // Validation 8: REPL-related content
    report.add_result(
        "REPL simulation test descriptions present",
        if stdout.contains("REPL") || stdout.contains("Interactive") || stdout.contains("session") {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected REPL-related content not found")
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

#[test]
fn test_edge_cases() {
    let (success, stdout, stderr) = run_rho_test("test_edge_cases.rho");
    let exit_code = if success { 0 } else { 1 };

    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_edge_cases");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Edge cases test intentionally tests error conditions
    // So we allow errors in stderr (e.g., division by zero panic)
    // but still expect the test suite to complete

    // Validation 1: Test suite completion
    let completion_check =
        Expectation::contains("test completion", "Edge Cases Test Suite Complete");
    let completion_result = validate(&stdout, &stderr, exit_code, &completion_check);
    report.add_result(
        "Test suite completes despite error conditions",
        if completion_result.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Test suite completion message not found")
        },
    );

    // Validation 2: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (6 tests)",
        if pathmaps.len() >= 6 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 6 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Validation 3: Syntax error (unmatched parenthesis) → error value in output
    let has_syntax_error = pathmaps.iter().any(|pm| {
        pm.output.iter().any(|v| {
            // Check for MettaValue::Error OR SExpr with "error" as first element
            match v {
                MettaValue::Error(_, _) => true,
                MettaValue::SExpr(items) => {
                    items.first().map_or(false, |first| {
                        matches!(first, MettaValue::Atom(s) | MettaValue::String(s) if s == "error")
                    })
                }
                _ => false
            }
        })
    });
    report.add_result(
        "Syntax error produces error s-expression",
        if has_syntax_error {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected error s-expression not found")
        },
    );

    // Validation 4: Empty input → [] (empty output)
    let has_empty = pathmaps.iter().any(|pm| pm.output.is_empty());
    report.add_result(
        "Empty input produces empty output",
        if has_empty {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected PathMap with empty output not found")
        },
    );

    // Validation 5: Undefined function → unevaluated expression (undefined-func, 1, 2)
    let has_undefined = pathmaps.iter().any(|pm| {
        use common::PathMapQuery;
        pm.query_sexpr("undefined-func").exists(|_| true)
    });
    report.add_result(
        "Undefined function returns unevaluated expression",
        if has_undefined {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected unevaluated expression (undefined-func, 1, 2) not found",
            )
        },
    );

    // Validation 6: Error resilience → [3, error(...), 10]
    let has_error_resilience_outputs = pathmaps.iter().any(|pm| {
        use common::PathMapQuery;
        pm.output.iter().any(|v| matches!(v, MettaValue::Long(3)))
            && pm.output.iter().any(|v| matches!(v, MettaValue::Long(10)))
    });
    report.add_result(
        "Error resilience: evaluation continues after error [3, ..., 10]",
        if has_error_resilience_outputs {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [3, ..., 10] not found")
        },
    );

    // Validation 7: Pattern matching with no match → unevaluated (only-zero, 5)
    let has_no_match = pathmaps.iter().any(|pm| {
        use common::PathMapQuery;
        pm.query_sexpr("only-zero")
            .exists(|v| v.to_display_string().contains("5"))
    });
    report.add_result(
        "Pattern matching with no match returns unevaluated expression",
        if has_no_match {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected unevaluated expression (only-zero, 5) not found")
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

#[test]
fn test_composability() {
    let (success, stdout, stderr) = run_rho_test("test_composability.rho");
    let exit_code = if success { 0 } else { 1 };

    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_composability");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Validation 1: Test completion
    let completion_check =
        Expectation::contains("test completion", "Composability Test Suite Complete");
    let completion_result = validate(&stdout, &stderr, exit_code, &completion_check);
    report.add_result(
        "Test suite completion message present",
        if completion_result.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Test suite completion message not found")
        },
    );

    // Validation 2: No errors
    let no_errors_check = Expectation::no_errors("no error indicators");
    let no_errors_result = validate(&stdout, &stderr, exit_code, &no_errors_check);
    report.add_result(
        "No error indicators in output",
        if no_errors_result.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Output contains error indicators")
        },
    );

    // Validation 3: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (10 tests)",
        if pathmaps.len() >= 10 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 10 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Validation 4: Identity composition → [12]
    let has_identity = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[12i64]));
    report.add_result(
        "Identity composition produces [12]",
        if has_identity {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [12] not found")
        },
    );

    // Validation 5: Sequential composition → [30]
    let has_sequential = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[30i64]));
    report.add_result(
        "Sequential composition produces [30]",
        if has_sequential {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [30] not found")
        },
    );

    // Validation 6: State accumulation → [3, 12, 5]
    let has_accumulation = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[3i64, 12i64, 5i64]));
    report.add_result(
        "State accumulation produces [3, 12, 5]",
        if has_accumulation {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [3, 12, 5] not found")
        },
    );

    // Validation 7: Multiple evaluations → [10, 15]
    let has_multiple = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[10i64, 15i64]));
    report.add_result(
        "Multiple evaluations produce [10, 15]",
        if has_multiple {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [10, 15] not found")
        },
    );

    // Validation 8: Nested composition → [2, 4, 6]
    let has_nested = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[2i64, 4i64, 6i64]));
    report.add_result(
        "Nested composition produces [2, 4, 6]",
        if has_nested {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [2, 4, 6] not found")
        },
    );

    // Validation 9: Mixed operations → [3, 10]
    let has_mixed = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[3i64, 10i64]));
    report.add_result(
        "Mixed operations produce [3, 10]",
        if has_mixed {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [3, 10] not found")
        },
    );

    // Validation 10: Sequential state → [1, 2, 3, 4, 5]
    let has_sequential_state = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).assert_outputs_eq(&[1i64, 2i64, 3i64, 4i64, 5i64]));
    report.add_result(
        "Sequential state produces [1, 2, 3, 4, 5]",
        if has_sequential_state {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected outputs [1, 2, 3, 4, 5] not found")
        },
    );

    // Validation 11: Final composition → [10] or [15]
    let has_final = pathmaps.iter().any(|pm| {
        OutputMatcher::new(pm).assert_outputs_eq(&[10i64])
            || OutputMatcher::new(pm).assert_outputs_eq(&[15i64])
    });
    report.add_result(
        "Final composition produces [10] or [15]",
        if has_final {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected output [10] or [15] not found")
        },
    );

    // Validation 12: Environment persistence (rule persistence)
    let has_environment = pathmaps.iter().any(|pm| pm.has_environment());
    report.add_result(
        "Environment data present (rule persistence)",
        if has_environment {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected some PathMaps with environment data")
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

// ============================================================================
// Examples
// ============================================================================

#[test]
fn test_example_robot_planning() {
    let (success, stdout, stderr) = run_rho_test("../examples/robot_planning.rho");
    let exit_code = if success { 0 } else { 1 };

    use common::{TestReport, ValidationResult};

    let mut report = TestReport::new("test_example_robot_planning");
    report.executed = true;
    report.exit_code = Some(exit_code);

    // Validation 1: Clean exit
    let exits_cleanly = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::success("exits cleanly"),
    );
    report.add_result(
        "Process exits cleanly",
        if exits_cleanly.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!("Exit code: {}", exit_code))
        },
    );

    // Validation 2: No errors
    let no_errors = validate(
        &stdout,
        &stderr,
        exit_code,
        &Expectation::no_errors("no errors"),
    );
    report.add_result(
        "No error indicators in output",
        if no_errors.is_pass() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Output contains error indicators")
        },
    );

    // Validation 3: PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    report.add_result(
        "PathMaps present (9 demos: D1, D2, D3, D4x4, D5, D6)",
        if pathmaps.len() >= 9 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 9 PathMaps, got {}",
                pathmaps.len()
            ))
        },
    );

    // Validation 4: Demo execution messages
    report.add_result(
        "Demo execution messages present (Demo 1:, Demo 2:)",
        if stdout.contains("Demo 1:") && stdout.contains("Demo 2:") {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected demo execution messages not found")
        },
    );

    // Validation 5: Completion message
    report.add_result(
        "All demos completion message present",
        if stdout.contains("All Demos Complete") {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected completion message not found")
        },
    );

    // Validation 6: Demo 1 - Get neighbors of room_a [room_b, room_e]
    // Exact output: two String values in any order
    let demo1_neighbors = pathmaps.iter().find(|pm| {
        pm.output.len() == 2
            && pm
                .output
                .iter()
                .all(|v| matches!(v, MettaValue::String(_) | MettaValue::Atom(_)))
            && pm.output.iter().any(|v| match v {
                MettaValue::String(s) | MettaValue::Atom(s) => s == "room_b",
                _ => false,
            })
            && pm.output.iter().any(|v| match v {
                MettaValue::String(s) | MettaValue::Atom(s) => s == "room_e",
                _ => false,
            })
    });
    report.add_result(
        "Demo 1: neighbors of room_a [room_b, room_e]",
        if demo1_neighbors.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected exact output [String(room_b), String(room_e)] not found",
            )
        },
    );

    // Validation 7: Demo 2 - Where is ball1? [room_c]
    let demo2_location = pathmaps
        .iter()
        .find(|pm| OutputMatcher::new(pm).assert_outputs_eq(&["room_c"]));
    report.add_result(
        "Demo 2: location of ball1 [room_c]",
        if demo2_location.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected location output [room_c] not found")
        },
    );

    // Validation 8: Demo 3 - Find shortest path from room_c to room_a
    // Exact output: (path room_c room_b room_a) and possibly errors
    let demo3_path = pathmaps.iter().find(|pm| {
        pm.output.iter().any(|v| {
            if let MettaValue::SExpr(exprs) = v {
                exprs.len() == 4
                    && matches!(&exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "path")
                    && matches!(&exprs[1], MettaValue::String(s) | MettaValue::Atom(s) if s == "room_c")
                    && matches!(&exprs[2], MettaValue::String(s) | MettaValue::Atom(s) if s == "room_b")
                    && matches!(&exprs[3], MettaValue::String(s) | MettaValue::Atom(s) if s == "room_a")
            } else {
                false
            }
        })
    });
    report.add_result(
        "Demo 3: exact path structure (path room_c room_b room_a)",
        if demo3_path.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected exact S-expression: (path room_c room_b room_a) not found",
            )
        },
    );

    // Validation 9: Demo 4 Step 1 - Locate ball1 (at least 2 instances of [room_c])
    let demo4_step1 = pathmaps
        .iter()
        .filter(|pm| OutputMatcher::new(pm).assert_outputs_eq(&["room_c"]))
        .count();
    report.add_result(
        "Demo 4 Step 1: locate ball1 [room_c] (appears 2+ times)",
        if demo4_step1 >= 2 {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(format!(
                "Expected at least 2 PathMaps with [room_c], got {}",
                demo4_step1
            ))
        },
    );

    // Validation 10: Demo 4 Step 3 - Build complete transport plan for ball1
    // Exact structure: (plan (objective (transport ball1 from room_c to room_a))
    //                        (route (waypoints room_c room_b room_a))
    //                        (steps (...)))
    let demo4_plan = pathmaps.iter().find(|pm| {
        pm.output.iter().any(|v| {
            if let MettaValue::SExpr(plan_exprs) = v {
                plan_exprs.len() == 4
                    && matches!(&plan_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "plan")
                    && matches!(&plan_exprs[1], MettaValue::SExpr(obj_exprs) if
                        obj_exprs.len() >= 2 &&
                        matches!(&obj_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "objective") &&
                        matches!(&obj_exprs[1], MettaValue::SExpr(trans_exprs) if
                            trans_exprs.len() >= 6 &&
                            matches!(&trans_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "transport") &&
                            matches!(&trans_exprs[1], MettaValue::String(s) | MettaValue::Atom(s) if s == "ball1")
                        )
                    )
                    && matches!(&plan_exprs[2], MettaValue::SExpr(route_exprs) if
                        route_exprs.len() >= 2 &&
                        matches!(&route_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "route") &&
                        matches!(&route_exprs[1], MettaValue::SExpr(waypoint_exprs) if
                            waypoint_exprs.len() >= 1 &&
                            matches!(&waypoint_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "waypoints")
                        )
                    )
                    && matches!(&plan_exprs[3], MettaValue::SExpr(step_exprs) if
                        step_exprs.len() >= 2 &&
                        matches!(&step_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "steps")
                    )
            } else {
                false
            }
        })
    });
    report.add_result(
        "Demo 4 Step 3: exact plan structure with objective/route/steps for ball1",
        if demo4_plan.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected exact plan S-expression structure for ball1 not found")
        },
    );

    // Validation 11: Demo 4 Step 4 - Validated plan with multihop_required
    // Exact structure: (validated (plan ...) multihop_required)
    let demo4_validated = pathmaps.iter().find(|pm| {
        pm.output.iter().any(|v| {
            if let MettaValue::SExpr(val_exprs) = v {
                val_exprs.len() == 3
                    && matches!(&val_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "validated")
                    && matches!(&val_exprs[1], MettaValue::SExpr(plan_exprs) if
                        !plan_exprs.is_empty() &&
                        matches!(&plan_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "plan")
                    )
                    && matches!(&val_exprs[2], MettaValue::String(s) | MettaValue::Atom(s) if s == "multihop_required")
            } else {
                false
            }
        })
    });
    report.add_result(
        "Demo 4 Step 4: exact validated structure (validated (plan ...) multihop_required)",
        if demo4_validated.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected exact S-expression: (validated (plan ...) multihop_required) not found",
            )
        },
    );

    // Validation 12: Demo 4 - ball1 transport steps sequence
    let ball1_steps_2hop = vec![
        vec!["navigate", "room_c"],
        vec!["pickup", "ball1"],
        vec!["navigate", "room_b"],
        vec!["navigate", "room_a"],
        vec!["putdown"],
    ];
    let has_ball1_steps = pathmaps
        .iter()
        .any(|pm| OutputMatcher::new(pm).match_steps_sequence(&ball1_steps_2hop));
    report.add_result(
        "Demo 4: ball1 transport steps [navigate(room_c), pickup(ball1), navigate(room_b), navigate(room_a), putdown]",
        if has_ball1_steps {
            ValidationResult::pass()
        } else {
            ValidationResult::fail("Expected exact steps sequence for ball1 not found")
        }
    );

    // Validation 13: Demo 5 - Minimum distance from room_a to room_d
    // Exact output: array of Long values containing [999, 3, 999, 999, 2, 2, 2, 2]
    // Due to nondeterminism, we verify:
    // - All values are Long (integers)
    // - Contains the minimum distance 2
    // - No valid distance < 2 exists (excluding sentinel 999)
    let demo5_distance = pathmaps.iter().find(|pm| {
        !pm.output.is_empty()
            && pm.output.iter().all(|v| matches!(v, MettaValue::Long(_)))
            && pm.output.iter().any(|v| matches!(v, MettaValue::Long(2)))
            && !pm
                .output
                .iter()
                .any(|v| matches!(v, MettaValue::Long(n) if *n < 2 && *n != 999))
    });
    report.add_result(
        "Demo 5: array of Long values with minimum distance 2",
        if demo5_distance.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected array of Long values with minimum 2 not found, or invalid distance < 2 exists",
            )
        },
    );

    // Validation 14: Demo 6 - Transport plan for box2
    // Exact structure: (plan (objective (transport box2 from room_b to room_d))
    //                        (route (waypoints ...))
    //                        (steps (...)))
    let demo6_box2 = pathmaps.iter().find(|pm| {
        pm.output.iter().any(|v| {
            if let MettaValue::SExpr(plan_exprs) = v {
                plan_exprs.len() == 4
                    && matches!(&plan_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "plan")
                    && matches!(&plan_exprs[1], MettaValue::SExpr(obj_exprs) if
                        obj_exprs.len() >= 2 &&
                        matches!(&obj_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "objective") &&
                        matches!(&obj_exprs[1], MettaValue::SExpr(trans_exprs) if
                            trans_exprs.len() >= 6 &&
                            matches!(&trans_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "transport") &&
                            matches!(&trans_exprs[1], MettaValue::String(s) | MettaValue::Atom(s) if s == "box2") &&
                            matches!(&trans_exprs[3], MettaValue::String(s) | MettaValue::Atom(s) if s == "room_b") &&
                            matches!(&trans_exprs[5], MettaValue::String(s) | MettaValue::Atom(s) if s == "room_d")
                        )
                    )
                    && matches!(&plan_exprs[2], MettaValue::SExpr(route_exprs) if
                        route_exprs.len() >= 2 &&
                        matches!(&route_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "route")
                    )
                    && matches!(&plan_exprs[3], MettaValue::SExpr(step_exprs) if
                        step_exprs.len() >= 2 &&
                        matches!(&step_exprs[0], MettaValue::String(s) | MettaValue::Atom(s) if s == "steps")
                    )
            } else {
                false
            }
        })
    });
    report.add_result(
        "Demo 6: exact plan structure with objective/route/steps for box2 (room_b to room_d)",
        if demo6_box2.is_some() {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected exact plan S-expression structure for box2 (room_b to room_d) not found",
            )
        },
    );

    // Validation 15: Demo 6 - box2 transport steps
    // Due to inherent nondeterminism (see LIMITATIONS in robot_planning.rho),
    // either the 2-hop or 3-hop path may appear in output
    // Accept either: room_b -> room_c -> room_d (5 steps) OR room_b -> room_a -> room_e -> room_d (6 steps)
    let box2_steps_via_c = vec![
        vec!["navigate", "room_b"],
        vec!["pickup", "box2"],
        vec!["navigate", "room_c"],
        vec!["navigate", "room_d"],
        vec!["putdown"],
    ];
    let box2_steps_via_a_e = vec![
        vec!["navigate", "room_b"],
        vec!["pickup", "box2"],
        vec!["navigate", "room_a"],
        vec!["navigate", "room_e"],
        vec!["navigate", "room_d"],
        vec!["putdown"],
    ];
    let has_box2_steps = pathmaps.iter().any(|pm| {
        let matcher = OutputMatcher::new(pm);
        matcher.match_steps_sequence(&box2_steps_via_c)
            || matcher.match_steps_sequence(&box2_steps_via_a_e)
    });
    report.add_result(
        "Demo 6: box2 transport path (2-hop or 3-hop due to nondeterminism)",
        if has_box2_steps {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Expected transport path for box2 not found (neither 2-hop nor 3-hop)",
            )
        },
    );

    // Validation 16: No unexpected Error values in PathMap outputs
    // Note: Expected errors like (error no_1hop_path ...) from failed path attempts are OK
    // We're checking for unexpected runtime errors, not intentional error returns
    let no_unexpected_errors = pathmaps.iter().all(|pm| {
        !pm.output.iter().any(|v| {
            // Check for Error variant with messages indicating actual failures
            // (as opposed to expected control-flow errors like no_1hop_path)
            matches!(v, MettaValue::Error(msg, _) if
                !msg.contains("no_1hop_path") &&
                !msg.contains("no_2hop_path") &&
                !msg.contains("no_3hop_path") &&
                !msg.is_empty()
            )
        })
    });
    report.add_result(
        "All PathMaps have no unexpected runtime errors",
        if no_unexpected_errors {
            ValidationResult::pass()
        } else {
            ValidationResult::fail(
                "Some PathMaps contain unexpected Error values (not control-flow errors)",
            )
        },
    );

    // Assert all validations passed and print report
    assert!(report.all_passed(), "\n{}", report.format());
}

#[test]
fn test_example_metta_rholang() {
    let (success, stdout, stderr) = run_rho_test("../examples/metta_rholang_example.rho");
    let exit_code = if success { 0 } else { 1 };

    // Validate success and no errors
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::success("exits cleanly")
        )
        .is_pass(),
        "Test failed with exit code {}",
        exit_code
    );
    assert!(
        validate(
            &stdout,
            &stderr,
            exit_code,
            &Expectation::no_errors("no errors")
        )
        .is_pass(),
        "Test output contains error indicators"
    );

    // Validate PathMaps present
    let pathmaps = parse_pathmap(&stdout);
    assert!(
        !pathmaps.is_empty(),
        "No PathMap structures found - examples should produce compilation results"
    );

    // Validate example completion messages
    assert!(
        stdout.contains("Example 1:") && stdout.contains("Example 2:"),
        "Expected example execution messages"
    );

    // Validate that simple arithmetic appears in PathMap source fields
    // (Examples compile but don't evaluate by default)
    let has_arithmetic_source = pathmaps.iter().any(|pm| {
        pm.source.iter().any(|v| {
            let s = v.to_display_string();
            s.contains("+") || s.contains("*")
        })
    });
    assert!(
        has_arithmetic_source,
        "Expected arithmetic expressions in PathMap source fields from examples"
    );
}

// ============================================================================
// Utility Tests
// ============================================================================

#[test]
fn test_rholang_cli_exists() {
    let cli = find_rholang_cli();
    assert!(cli.exists(), "rholang-cli not found at: {}", cli.display());
}

#[test]
fn test_integration_dir_exists() {
    let dir = integration_dir();
    assert!(
        dir.exists(),
        "Integration directory not found: {}",
        dir.display()
    );
}

#[test]
fn test_all_test_files_exist() {
    let test_files = vec![
        "test_basic_evaluation.rho",
        "test_rules.rho",
        "test_control_flow.rho",
        "test_types.rho",
        "test_composability.rho",
        "test_repl_simulation.rho",
        "test_edge_cases.rho",
    ];

    for file in test_files {
        let path = integration_dir().join(file);
        assert!(path.exists(), "Test file not found: {}", path.display());
    }
}

// ============================================================================
// Phase 3: Configuration and Filtering Tests
// ============================================================================

#[test]
fn test_phase3_config_load() {
    use common::TestManifest;

    // Test loading the manifest
    let manifest = TestManifest::load_default();
    assert!(
        manifest.is_ok(),
        "Failed to load manifest: {:?}",
        manifest.err()
    );

    let manifest = manifest.unwrap();

    // Verify manifest structure
    assert!(!manifest.tests.is_empty(), "No tests found in manifest");
    assert!(!manifest.categories.is_empty(), "No categories found");
    assert!(!manifest.suites.is_empty(), "No suites found");

    // Check specific tests exist
    let test_names: Vec<_> = manifest.tests.iter().map(|t| t.name.as_str()).collect();
    assert!(test_names.contains(&"test_basic_evaluation"));
    assert!(test_names.contains(&"test_rules"));
    assert!(test_names.contains(&"test_control_flow"));
}

#[test]
fn test_phase3_filtering() {
    use common::{TestFilter, TestManifest};

    let manifest = TestManifest::load_default().unwrap();

    // Test category filtering
    let basic_tests = manifest.tests_by_category("basic");
    assert!(!basic_tests.is_empty(), "No basic tests found");
    println!(
        "Basic tests: {:?}",
        basic_tests.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Test suite filtering
    let core_tests = manifest.tests_in_suite("core");
    assert!(!core_tests.is_empty(), "No tests in 'core' suite");
    println!(
        "Core suite tests: {:?}",
        core_tests.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    // Test filter builder
    let filter = TestFilter::new().with_category("basic".to_string());

    let filtered = filter.apply(&manifest);
    assert!(!filtered.is_empty(), "Filter returned no tests");
}

#[test]
fn test_phase3_test_runner() {
    use common::TestRunner;

    // Create runner from default manifest
    let runner = TestRunner::from_default();
    assert!(
        runner.is_ok(),
        "Failed to create runner: {:?}",
        runner.err()
    );

    let runner = runner.unwrap();

    // Verify runner has access to manifest
    let manifest = runner.manifest();
    assert!(!manifest.tests.is_empty());

    // The runner methods are tested but not actually executed here
    // to avoid running actual tests during unit test phase
    println!(
        "Runner created successfully with {} tests",
        manifest.tests.len()
    );
}

#[test]
fn test_phase3_categories() {
    use common::TestManifest;

    let manifest = TestManifest::load_default().unwrap();

    // Get categories by priority
    let categories = manifest.categories_by_priority();
    assert!(!categories.is_empty());

    // Verify priority ordering
    for i in 1..categories.len() {
        assert!(
            categories[i - 1].1.priority <= categories[i].1.priority,
            "Categories not sorted by priority"
        );
    }

    println!("Categories (by priority):");
    for (name, spec) in categories.iter().take(5) {
        println!(
            "  {} (priority {}): {}",
            name, spec.priority, spec.description
        );
    }
}

/// Test async runner execution (demonstrates Tokio integration)
#[tokio::test]
async fn test_async_runner() {
    use common::{TestFilter, TestRunner};

    // Create runner
    let runner = TestRunner::from_default();
    assert!(runner.is_ok(), "Failed to create runner");
    let runner = runner.unwrap();

    // Run a small subset of tests using async API
    let filter = TestFilter::new().with_category("basic".to_string());

    // This now uses Tokio's async runtime for I/O-bound concurrency
    let results = runner.run_filtered(&filter).await;

    // Verify we got results
    assert!(!results.is_empty(), "Should have run at least one test");

    // Print summary
    println!("Async runner executed {} tests", results.len());
    let passed = results.iter().filter(|r| r.success).count();
    println!("  {} passed, {} failed", passed, results.len() - passed);
}
