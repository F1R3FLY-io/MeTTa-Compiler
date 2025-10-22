/// Output validation logic for integration tests
///
/// Validates test outputs against expectations using various validation strategies.
use super::output_parser::{parse_pathmap, MettaValueTestExt, PathMapOutput};
use super::test_specs::{Expectation, OutputPattern, ValidationResult};
use regex::Regex;

/// Validate test output against an expectation
pub fn validate(
    stdout: &str,
    stderr: &str,
    exit_code: i32,
    expectation: &Expectation,
) -> ValidationResult {
    match &expectation.pattern {
        OutputPattern::Contains { text } => validate_contains(stdout, text),
        OutputPattern::Regex { pattern } => validate_regex(stdout, pattern),
        OutputPattern::Outputs { values } => validate_outputs(stdout, values),
        OutputPattern::PathMapStructure {
            has_source,
            has_environment,
            output_count,
        } => validate_pathmap_structure(stdout, *has_source, *has_environment, *output_count),
        OutputPattern::NoErrors => validate_no_errors(stdout, stderr),
        OutputPattern::Success => validate_success(exit_code),
        OutputPattern::Custom { validator } => match validator(stdout, stderr) {
            Ok(()) => ValidationResult::pass(),
            Err(reason) => ValidationResult::fail(reason),
        },
    }
}

/// Validate that stdout contains specific text
fn validate_contains(stdout: &str, text: &str) -> ValidationResult {
    if stdout.contains(text) {
        ValidationResult::pass()
    } else {
        ValidationResult::fail(format!(
            "Expected stdout to contain '{}', but it was not found",
            text
        ))
    }
}

/// Validate that stdout matches a regex pattern
fn validate_regex(stdout: &str, pattern: &str) -> ValidationResult {
    match Regex::new(pattern) {
        Ok(re) => {
            if re.is_match(stdout) {
                ValidationResult::pass()
            } else {
                ValidationResult::fail(format!(
                    "Expected stdout to match pattern '{}', but it did not",
                    pattern
                ))
            }
        }
        Err(e) => ValidationResult::fail(format!("Invalid regex pattern '{}': {}", pattern, e)),
    }
}

/// Validate specific evaluation outputs
fn validate_outputs(stdout: &str, expected_values: &[String]) -> ValidationResult {
    let pathmaps = parse_pathmap(stdout);

    if pathmaps.is_empty() {
        return ValidationResult::fail("No PathMap structures found in output".to_string());
    }

    // Collect all outputs from all PathMaps
    let mut actual_outputs = Vec::new();
    for pathmap in &pathmaps {
        actual_outputs.extend(pathmap.output.clone());
    }

    if actual_outputs.is_empty() && !expected_values.is_empty() {
        return ValidationResult::fail(format!(
            "Expected outputs {:?}, but PathMap output field is empty",
            expected_values
        ));
    }

    // Check if expected values are present (using matches_str for flexible comparison)
    for expected in expected_values {
        if !actual_outputs.iter().any(|v| v.matches_str(expected)) {
            let actual_strs: Vec<String> = actual_outputs
                .iter()
                .map(|v| v.to_display_string())
                .collect();
            return ValidationResult::fail(format!(
                "Expected output '{}' not found. Actual outputs: {:?}",
                expected, actual_strs
            ));
        }
    }

    ValidationResult::pass()
}

/// Validate PathMap structure
fn validate_pathmap_structure(
    stdout: &str,
    has_source: bool,
    has_environment: bool,
    output_count: usize,
) -> ValidationResult {
    let pathmaps = parse_pathmap(stdout);

    if pathmaps.is_empty() {
        return ValidationResult::fail("No PathMap structures found in output".to_string());
    }

    // Validate the first PathMap
    let pathmap = &pathmaps[0];

    // Check source field
    if has_source && !pathmap.has_source() {
        return ValidationResult::fail(
            "Expected PathMap to have source expressions, but field is empty".to_string(),
        );
    }
    if !has_source && pathmap.has_source() {
        return ValidationResult::fail(
            "Expected PathMap to have empty source field, but it contains expressions".to_string(),
        );
    }

    // Check environment field
    if has_environment && !pathmap.has_environment() {
        return ValidationResult::fail(
            "Expected PathMap to have environment data, but field is empty".to_string(),
        );
    }

    // Check output count
    if pathmap.output.len() != output_count {
        return ValidationResult::fail(format!(
            "Expected {} outputs, but found {}. Actual outputs: {:?}",
            output_count,
            pathmap.output.len(),
            pathmap.output
        ));
    }

    ValidationResult::pass()
}

/// Validate that there are no errors in output
fn validate_no_errors(stdout: &str, stderr: &str) -> ValidationResult {
    let error_indicators = [
        "Errors received during evaluation:",
        "error in:",
        "ParserError",
        "FAIL",
        "panic",
    ];

    for indicator in &error_indicators {
        if stdout.contains(indicator) {
            return ValidationResult::fail(format!(
                "Found error indicator '{}' in stdout",
                indicator
            ));
        }
        if stderr.contains(indicator) {
            return ValidationResult::fail(format!(
                "Found error indicator '{}' in stderr",
                indicator
            ));
        }
    }

    ValidationResult::pass()
}

/// Validate exit code is 0 (success)
fn validate_success(exit_code: i32) -> ValidationResult {
    if exit_code == 0 {
        ValidationResult::pass()
    } else {
        ValidationResult::fail(format!("Expected exit code 0, but got {}", exit_code))
    }
}

/// Validate environment persistence across multiple operations
pub fn validate_environment_persistence(pathmaps: &[PathMapOutput]) -> ValidationResult {
    if pathmaps.len() < 2 {
        return ValidationResult::fail(
            "Need at least 2 PathMaps to validate environment persistence".to_string(),
        );
    }

    // Check that all PathMaps have environment data
    for (i, pathmap) in pathmaps.iter().enumerate() {
        if !pathmap.has_environment() {
            return ValidationResult::fail(format!(
                "PathMap {} is missing environment data",
                i + 1
            ));
        }
    }

    ValidationResult::pass()
}

/// Validate source field handling
pub fn validate_source_handling(
    pathmaps: &[PathMapOutput],
    expected_empty: bool,
) -> ValidationResult {
    if pathmaps.is_empty() {
        return ValidationResult::fail("No PathMap structures found".to_string());
    }

    for (i, pathmap) in pathmaps.iter().enumerate() {
        if expected_empty && pathmap.has_source() {
            return ValidationResult::fail(format!(
                "PathMap {} has source expressions, but expected empty. Found: {:?}",
                i + 1,
                pathmap.source
            ));
        }
        if !expected_empty && !pathmap.has_source() {
            return ValidationResult::fail(format!(
                "PathMap {} is missing source expressions",
                i + 1
            ));
        }
    }

    ValidationResult::pass()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_contains_pass() {
        let expectation = Expectation::contains("test", "hello");
        let result = validate("hello world", "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_contains_fail() {
        let expectation = Expectation::contains("test", "goodbye");
        let result = validate("hello world", "", 0, &expectation);
        assert!(result.is_fail());
    }

    #[test]
    fn test_validate_regex_pass() {
        let expectation = Expectation::regex("test", r"\d+");
        let result = validate("result: 123", "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_regex_fail() {
        let expectation = Expectation::regex("test", r"\d+");
        let result = validate("no numbers here", "", 0, &expectation);
        assert!(result.is_fail());
    }

    #[test]
    fn test_validate_outputs_pass() {
        let stdout = r#"{|(("source", []), ("output", [3, 7]))|}  "#;
        let expectation = Expectation::outputs("test", vec!["3".to_string(), "7".to_string()]);
        let result = validate(stdout, "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_outputs_fail() {
        let stdout = r#"{|(("source", []), ("output", [3]))|}  "#;
        let expectation = Expectation::outputs("test", vec!["7".to_string()]);
        let result = validate(stdout, "", 0, &expectation);
        assert!(result.is_fail());
    }

    #[test]
    fn test_validate_pathmap_structure_pass() {
        let stdout = r#"{|(("source", [(+ 1 2)]), ("environment", ...), ("output", [3]))|}  "#;
        let expectation = Expectation::pathmap_structure("test", true, true, 1);
        let result = validate(stdout, "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_pathmap_structure_fail_output_count() {
        let stdout = r#"{|(("source", []), ("environment", ...), ("output", [3, 7]))|}  "#;
        let expectation = Expectation::pathmap_structure("test", false, true, 1);
        let result = validate(stdout, "", 0, &expectation);
        assert!(result.is_fail());
        assert!(result
            .failure_reason()
            .unwrap()
            .contains("Expected 1 outputs, but found 2"));
    }

    #[test]
    fn test_validate_no_errors_pass() {
        let expectation = Expectation::no_errors("test");
        let result = validate("all good", "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_no_errors_fail() {
        let expectation = Expectation::no_errors("test");
        let result = validate("ParserError: syntax error", "", 0, &expectation);
        assert!(result.is_fail());
    }

    #[test]
    fn test_validate_success_pass() {
        let expectation = Expectation::success("test");
        let result = validate("", "", 0, &expectation);
        assert!(result.is_pass());
    }

    #[test]
    fn test_validate_success_fail() {
        let expectation = Expectation::success("test");
        let result = validate("", "", 1, &expectation);
        assert!(result.is_fail());
    }
}
