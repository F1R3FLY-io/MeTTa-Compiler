/// Test utilities for Rholang integration tests
///
/// This module provides shared utilities for integration tests, including:
/// - Finding the rholang-cli binary
/// - Parsing PathMap output structures
/// - Validating test expectations
/// - Test specification data structures

// Phase 2: Output validation modules
pub mod output_parser;
pub mod test_specs;
pub mod validators;

// Phase 3: Collection types and query system
pub mod collections;
pub mod query;

// Re-export commonly used types for convenience
pub use output_parser::{parse_pathmap, extract_all_outputs, extract_all_outputs_as_strings, PathMapOutput, MettaValueTestExt};
pub use mettatron::backend::types::MettaValue;
pub use test_specs::{RholangTestSpec, Expectation, OutputPattern, ValidationResult, TestReport};
pub use validators::validate;
pub use collections::CollectionValue;
pub use query::{PathMapQuery, OutputMatcher, QueryResult, ToMettaValue};

use std::env;
use std::path::PathBuf;

/// Find the rholang-cli binary
///
/// Searches in the following order:
/// 1. RHOLANG_CLI_PATH environment variable
/// 2. ../f1r3node/target/release/rholang-cli
/// 3. ../f1r3node/target/debug/rholang-cli
///
/// # Panics
/// Panics if rholang-cli cannot be found in any of the standard locations.
pub fn find_rholang_cli() -> PathBuf {
    // Check environment variable first
    if let Ok(path) = env::var("RHOLANG_CLI_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            return path;
        }
        eprintln!(
            "Warning: RHOLANG_CLI_PATH set to '{}' but file does not exist",
            path.display()
        );
    }

    // Check standard locations
    let base_dir = env::current_dir()
        .expect("Failed to get current directory")
        .parent()
        .expect("Failed to get parent directory")
        .to_path_buf();

    let candidates = vec![
        base_dir.join("f1r3node/target/release/rholang-cli"),
        base_dir.join("f1r3node/target/debug/rholang-cli"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    panic!(
        "rholang-cli not found. Tried:\n{}\n\nSet RHOLANG_CLI_PATH or build f1r3node:\n  cd ../f1r3node/rholang\n  RUSTFLAGS=\"-C target-cpu=native\" cargo build --release --bin rholang-cli",
        candidates.iter().map(|p| format!("  - {}", p.display())).collect::<Vec<_>>().join("\n")
    );
}

/// Get the integration tests directory
pub fn integration_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integration")
}

/// Check if a test output contains a success indicator
pub fn contains_success(output: &str) -> bool {
    output.contains("Test Suite Complete")
        || output.contains("All tests complete")
        || output.contains("success")
}

/// Check if a test output contains an error indicator
///
/// This checks for actual error messages, not just the word "error" appearing
/// in test descriptions or expected output. It looks for:
/// - "Errors received during evaluation:" prefix from Rholang
/// - "error in:" prefix from parser errors
/// - "ParserError" from compilation failures
/// - "FAIL" or "FAILED" in test output
/// - "panic" from Rust panics
pub fn contains_error(output: &str) -> bool {
    output.contains("Errors received during evaluation:")
        || output.contains("error in:")
        || output.contains("ParserError")
        || output.contains("FAIL")
        || output.contains("panic")
}

/// Extract eval_outputs from PathMap output
///
/// Looks for patterns like: ("output", [value1, value2, ...])
pub fn extract_eval_outputs(output: &str) -> Vec<String> {
    use regex::Regex;

    let mut results = Vec::new();

    // Pattern to match output array (updated field name from eval_outputs to output)
    // Example: ("output", [3, 12, 5])
    let re = Regex::new(r#"\("output",\s*\[(.*?)\]\)"#).unwrap();

    if let Some(caps) = re.captures(output) {
        if let Some(values) = caps.get(1) {
            let values_str = values.as_str();
            if !values_str.trim().is_empty() {
                // Split by comma and clean up
                for value in values_str.split(',') {
                    let cleaned = value.trim().to_string();
                    if !cleaned.is_empty() {
                        results.push(cleaned);
                    }
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_rholang_cli() {
        // This test will pass if rholang-cli is found, skip otherwise
        if let Ok(_) = std::panic::catch_unwind(|| {
            let cli = find_rholang_cli();
            assert!(cli.exists());
        }) {
            // Success
        } else {
            eprintln!("Skipping test_find_rholang_cli: rholang-cli not found");
        }
    }

    #[test]
    fn test_extract_eval_outputs() {
        let output = r#"{|("output", [3, 12, 5])|}"#;
        let results = extract_eval_outputs(output);
        assert_eq!(results, vec!["3", "12", "5"]);
    }

    #[test]
    fn test_extract_eval_outputs_empty() {
        let output = r#"{|("output", [])|}"#;
        let results = extract_eval_outputs(output);
        assert_eq!(results, Vec::<String>::new());
    }

    #[test]
    fn test_contains_success() {
        assert!(contains_success("Test Suite Complete - All Tests Passed"));
        assert!(contains_success("All tests complete!"));
        assert!(!contains_success("Test failed"));
    }

    #[test]
    fn test_contains_error() {
        // These should be detected as errors
        assert!(contains_error("Errors received during evaluation: something went wrong"));
        assert!(contains_error("error in: /path/to/file.rho"));
        assert!(contains_error("ParserError(\"Parse failed\")"));
        assert!(contains_error("FAIL"));
        assert!(contains_error("panic at location"));

        // These should NOT be detected as errors (false positives we want to avoid)
        assert!(!contains_error("Test passed successfully"));
        assert!(!contains_error("Expected: eval_outputs should be [12]"));
        assert!(!contains_error("Test: Error creation"));  // Test name containing "error"
        assert!(!contains_error("(error \"test\" 42)"));     // MeTTa error expression
    }
}
