/// Rholang Integration Tests
///
/// Tests the MeTTa-Rholang integration by executing .rho files
/// and validating their output.

mod utils;

use std::process::Command;
use utils::{find_rholang_cli, integration_dir, contains_error, extract_eval_outputs};

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
        .arg(&test_file)
        .output()
        .expect("Failed to execute rholang-cli");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let success = output.status.success();

    (success, stdout, stderr)
}

/// Helper to assert test passed
fn assert_test_passed(filename: &str) {
    let (success, stdout, stderr) = run_rho_test(filename);

    if !success || contains_error(&stdout) || contains_error(&stderr) {
        eprintln!("=== Test Failed: {} ===", filename);
        eprintln!("Exit code: {}", if success { "0" } else { "non-zero" });
        eprintln!("\n=== STDOUT ===\n{}", stdout);
        eprintln!("\n=== STDERR ===\n{}", stderr);
        panic!("Test failed: {}", filename);
    }
}

// ============================================================================
// Basic Integration Tests
// ============================================================================

#[test]
fn test_metta_integration() {
    assert_test_passed("test_metta_integration.rho");
}

#[test]
fn test_pathmap_simple() {
    assert_test_passed("test_pathmap_simple.rho");
}

#[test]
fn test_pathmap_state() {
    assert_test_passed("test_pathmap_state.rho");
}

#[test]
fn test_pathmap_run_method() {
    assert_test_passed("test_pathmap_run_method.rho");
}

// ============================================================================
// Test Harness Tests
// ============================================================================

#[test]
fn test_harness_simple() {
    let (success, stdout, stderr) = run_rho_test("test_harness_simple.rho");

    assert!(
        success,
        "test_harness_simple.rho failed to execute:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // Check for completion message
    assert!(
        stdout.contains("Test Suite Complete") || stdout.contains("All Tests Passed"),
        "Test suite did not complete successfully:\n{}",
        stdout
    );

    // Check that we don't have obvious errors
    assert!(
        !contains_error(&stdout),
        "Test output contains error indicators:\n{}",
        stdout
    );

    // Validate arithmetic outputs are present
    let outputs = extract_eval_outputs(&stdout);
    assert!(
        !outputs.is_empty(),
        "No arithmetic outputs found in test results"
    );
}

#[test]
fn test_harness_composability() {
    let (success, stdout, stderr) = run_rho_test("test_harness_composability.rho");

    assert!(
        success,
        "test_harness_composability.rho failed to execute:\nSTDOUT:\n{}\nSTDERR:\n{}",
        stdout,
        stderr
    );

    // Check for test completion
    assert!(
        stdout.contains("Test Suite Complete") || stdout.contains("TEST"),
        "Test suite output not found:\n{}",
        stdout
    );

    // Validate rule-related content appears
    assert!(
        stdout.contains("rule") || stdout.contains("double") || stdout.contains("triple"),
        "Rule-related content not found in output"
    );
}

#[test]
fn test_harness_validation() {
    assert_test_passed("test_harness_validation.rho");
}

// ============================================================================
// Examples
// ============================================================================

#[test]
fn test_example_robot_planning() {
    assert_test_passed("../examples/robot_planning.rho");
}

#[test]
fn test_example_metta_rholang() {
    assert_test_passed("../examples/metta_rholang_example.rho");
}

// ============================================================================
// Utility Tests
// ============================================================================

#[test]
fn test_rholang_cli_exists() {
    let cli = find_rholang_cli();
    assert!(
        cli.exists(),
        "rholang-cli not found at: {}",
        cli.display()
    );
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
        "test_metta_integration.rho",
        "test_pathmap_simple.rho",
        "test_pathmap_state.rho",
        "test_pathmap_run_method.rho",
        "test_harness_simple.rho",
        "test_harness_composability.rho",
        "test_harness_validation.rho",
    ];

    for file in test_files {
        let path = integration_dir().join(file);
        assert!(
            path.exists(),
            "Test file not found: {}",
            path.display()
        );
    }
}
