/// Test specification data structures for integration tests
///
/// Defines expectations for Rholang test execution, including
/// expected outputs, environment state, and PathMap structure.
use std::path::PathBuf;

/// Specification for a Rholang integration test
#[derive(Debug, Clone)]
pub struct RholangTestSpec {
    /// Test name (for reporting)
    pub name: String,
    /// Path to the .rho test file
    pub file: PathBuf,
    /// Timeout in seconds (default: 30)
    pub timeout_secs: u64,
    /// Expected outputs from the test
    pub expectations: Vec<Expectation>,
}

impl RholangTestSpec {
    /// Create a new test spec with default timeout
    pub fn new<S: Into<String>, P: Into<PathBuf>>(name: S, file: P) -> Self {
        RholangTestSpec {
            name: name.into(),
            file: file.into(),
            timeout_secs: 30,
            expectations: Vec::new(),
        }
    }

    /// Set custom timeout
    pub fn with_timeout(mut self, timeout_secs: u64) -> Self {
        self.timeout_secs = timeout_secs;
        self
    }

    /// Add an expectation
    pub fn expect(mut self, expectation: Expectation) -> Self {
        self.expectations.push(expectation);
        self
    }

    /// Add multiple expectations
    pub fn expect_all(mut self, expectations: Vec<Expectation>) -> Self {
        self.expectations.extend(expectations);
        self
    }
}

/// An expectation for test output
#[derive(Debug, Clone)]
pub struct Expectation {
    /// Description of what's being tested
    pub description: String,
    /// The pattern to match
    pub pattern: OutputPattern,
}

impl Expectation {
    /// Create a new expectation
    pub fn new<S: Into<String>>(description: S, pattern: OutputPattern) -> Self {
        Expectation {
            description: description.into(),
            pattern,
        }
    }

    /// Expect stdout contains specific text
    pub fn contains<S: Into<String>>(description: S, text: S) -> Self {
        Expectation::new(description, OutputPattern::Contains { text: text.into() })
    }

    /// Expect stdout matches regex
    pub fn regex<S: Into<String>>(description: S, pattern: S) -> Self {
        Expectation::new(
            description,
            OutputPattern::Regex {
                pattern: pattern.into(),
            },
        )
    }

    /// Expect specific evaluation outputs
    pub fn outputs<S: Into<String>>(description: S, values: Vec<String>) -> Self {
        Expectation::new(description, OutputPattern::Outputs { values })
    }

    /// Expect PathMap structure
    pub fn pathmap_structure<S: Into<String>>(
        description: S,
        has_source: bool,
        has_environment: bool,
        output_count: usize,
    ) -> Self {
        Expectation::new(
            description,
            OutputPattern::PathMapStructure {
                has_source,
                has_environment,
                output_count,
            },
        )
    }

    /// Expect no errors in output
    pub fn no_errors<S: Into<String>>(description: S) -> Self {
        Expectation::new(description, OutputPattern::NoErrors)
    }

    /// Expect success exit code
    pub fn success<S: Into<String>>(description: S) -> Self {
        Expectation::new(description, OutputPattern::Success)
    }
}

/// Pattern for matching test output
#[derive(Debug, Clone)]
pub enum OutputPattern {
    /// Stdout contains the specified text
    Contains { text: String },

    /// Stdout matches the regex pattern
    Regex { pattern: String },

    /// Specific values in the "output" field of PathMap
    Outputs { values: Vec<String> },

    /// PathMap has specific structure
    PathMapStructure {
        has_source: bool,
        has_environment: bool,
        output_count: usize,
    },

    /// No error messages in output
    NoErrors,

    /// Test exits successfully (exit code 0)
    Success,

    /// Custom validation function
    Custom {
        validator: fn(&str, &str) -> Result<(), String>,
    },
}

/// Result of validating a test expectation
#[derive(Debug, Clone, PartialEq)]
pub enum ValidationResult {
    /// Expectation passed
    Pass,
    /// Expectation failed with reason
    Fail { reason: String },
}

impl ValidationResult {
    /// Create a passing result
    pub fn pass() -> Self {
        ValidationResult::Pass
    }

    /// Create a failing result
    pub fn fail<S: Into<String>>(reason: S) -> Self {
        ValidationResult::Fail {
            reason: reason.into(),
        }
    }

    /// Check if the result is a pass
    pub fn is_pass(&self) -> bool {
        matches!(self, ValidationResult::Pass)
    }

    /// Check if the result is a failure
    pub fn is_fail(&self) -> bool {
        !self.is_pass()
    }

    /// Get the failure reason if failed
    pub fn failure_reason(&self) -> Option<&str> {
        match self {
            ValidationResult::Fail { reason } => Some(reason),
            _ => None,
        }
    }
}

/// Report for a test execution
#[derive(Debug, Clone)]
pub struct TestReport {
    /// Test name
    pub name: String,
    /// Whether test executed successfully
    pub executed: bool,
    /// Exit code (if test ran)
    pub exit_code: Option<i32>,
    /// Validation results for each expectation
    pub results: Vec<(String, ValidationResult)>,
    /// Execution time in milliseconds
    pub duration_ms: u128,
}

impl TestReport {
    /// Create a new test report
    pub fn new<S: Into<String>>(name: S) -> Self {
        TestReport {
            name: name.into(),
            executed: false,
            exit_code: None,
            results: Vec::new(),
            duration_ms: 0,
        }
    }

    /// Check if all expectations passed
    pub fn all_passed(&self) -> bool {
        self.executed && self.results.iter().all(|(_, r)| r.is_pass())
    }

    /// Get the number of passing expectations
    pub fn passed_count(&self) -> usize {
        self.results.iter().filter(|(_, r)| r.is_pass()).count()
    }

    /// Get the number of failing expectations
    pub fn failed_count(&self) -> usize {
        self.results.iter().filter(|(_, r)| r.is_fail()).count()
    }

    /// Add a validation result
    pub fn add_result<S: Into<String>>(&mut self, description: S, result: ValidationResult) {
        self.results.push((description.into(), result));
    }

    /// Format report as string
    pub fn format(&self) -> String {
        let mut output = String::new();
        output.push_str(&format!("\n=== Test Report: {} ===\n", self.name));

        if !self.executed {
            output.push_str("Status: Failed to execute\n");
            return output;
        }

        output.push_str(&format!(
            "Status: {}\n",
            if self.all_passed() {
                "PASSED ✓"
            } else {
                "FAILED ✗"
            }
        ));
        output.push_str(&format!(
            "Results: {} passed, {} failed\n",
            self.passed_count(),
            self.failed_count()
        ));
        output.push_str(&format!("Duration: {}ms\n", self.duration_ms));

        if !self.results.is_empty() {
            output.push_str("\nExpectations:\n");
            for (desc, result) in &self.results {
                match result {
                    ValidationResult::Pass => {
                        output.push_str(&format!("  ✓ {}\n", desc));
                    }
                    ValidationResult::Fail { reason } => {
                        output.push_str(&format!("  ✗ {}\n", desc));
                        output.push_str(&format!("    Reason: {}\n", reason));
                    }
                }
            }
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_test_spec() {
        let spec = RholangTestSpec::new("test_simple", "test.rho")
            .with_timeout(60)
            .expect(Expectation::contains("has output", "result"));

        assert_eq!(spec.name, "test_simple");
        assert_eq!(spec.timeout_secs, 60);
        assert_eq!(spec.expectations.len(), 1);
    }

    #[test]
    fn test_validation_result() {
        let pass = ValidationResult::pass();
        assert!(pass.is_pass());
        assert!(!pass.is_fail());

        let fail = ValidationResult::fail("error message");
        assert!(!fail.is_pass());
        assert!(fail.is_fail());
        assert_eq!(fail.failure_reason(), Some("error message"));
    }

    #[test]
    fn test_test_report() {
        let mut report = TestReport::new("test_example");
        report.executed = true;
        report.add_result("check 1", ValidationResult::pass());
        report.add_result("check 2", ValidationResult::fail("mismatch"));

        assert!(report.executed);
        assert!(!report.all_passed());
        assert_eq!(report.passed_count(), 1);
        assert_eq!(report.failed_count(), 1);
    }

    #[test]
    fn test_report_format() {
        let mut report = TestReport::new("test_format");
        report.executed = true;
        report.duration_ms = 123;
        report.add_result("expectation 1", ValidationResult::pass());
        report.add_result("expectation 2", ValidationResult::fail("failed"));

        let formatted = report.format();
        assert!(formatted.contains("test_format"));
        assert!(formatted.contains("1 passed, 1 failed"));
        assert!(formatted.contains("123ms"));
    }
}
