// Advanced test runner with parallel execution and filtering

use super::config::{TestConfig, TestManifest, TestFilter, TestSpec};
use super::test_specs::{TestReport, ValidationResult};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Test execution result
#[derive(Debug, Clone)]
pub struct TestResult {
    /// Test name
    pub name: String,
    /// Test file path
    pub file: String,
    /// Whether test executed successfully
    pub success: bool,
    /// Standard output
    pub stdout: String,
    /// Standard error
    pub stderr: String,
    /// Exit code
    pub exit_code: i32,
    /// Execution duration
    pub duration: Duration,
    /// Test report (if generated)
    pub report: Option<TestReport>,
}

/// Test runner with parallel execution support
pub struct TestRunner {
    /// Test manifest
    manifest: TestManifest,
    /// Path to rholang-cli
    rholang_cli: String,
    /// Number of parallel workers
    workers: usize,
}

impl TestRunner {
    /// Create a new test runner from manifest
    pub fn new(manifest: TestManifest) -> Self {
        let rholang_cli = std::env::var("RHOLANG_CLI_PATH")
            .unwrap_or_else(|_| manifest.config.rholang_cli.clone());

        let workers = if manifest.config.max_parallel == 0 {
            num_cpus::get()
        } else {
            manifest.config.max_parallel
        };

        TestRunner {
            manifest,
            rholang_cli,
            workers,
        }
    }

    /// Create test runner from default manifest
    pub fn from_default() -> Result<Self, String> {
        let manifest = TestManifest::load_default()?;
        Ok(Self::new(manifest))
    }

    /// Get the test manifest
    pub fn manifest(&self) -> &TestManifest {
        &self.manifest
    }

    /// Run a single test
    pub fn run_test(&self, test: &TestSpec) -> TestResult {
        let start = Instant::now();

        // Resolve test file path
        let test_path = Path::new(&test.file);

        // Execute rholang-cli
        let output = Command::new(&self.rholang_cli)
            .arg(test_path)
            .output();

        let duration = start.elapsed();

        match output {
            Ok(output) => {
                let success = output.status.success();
                let exit_code = output.status.code().unwrap_or(-1);
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success,
                    stdout,
                    stderr,
                    exit_code,
                    duration,
                    report: None,
                }
            }
            Err(e) => {
                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to execute rholang-cli: {}", e),
                    exit_code: -1,
                    duration,
                    report: None,
                }
            }
        }
    }

    /// Run tests with filter
    pub fn run_filtered(&self, filter: &TestFilter) -> Vec<TestResult> {
        let tests = filter.apply(&self.manifest);

        if tests.is_empty() {
            println!("No tests matched filter");
            return Vec::new();
        }

        println!("Running {} test(s)...\n", tests.len());

        // For now, run sequentially (parallel execution would require rayon or similar)
        let mut results = Vec::new();
        for test in tests {
            println!("Running: {} ...", test.name);
            let result = self.run_test(test);
            let status = if result.success { "ok" } else { "FAILED" };
            println!("  {} ({}ms)", status, result.duration.as_millis());
            results.push(result);
        }

        results
    }

    /// Run all enabled tests
    pub fn run_all(&self) -> Vec<TestResult> {
        let filter = TestFilter::new();
        self.run_filtered(&filter)
    }

    /// Run tests in a suite
    pub fn run_suite(&self, suite_name: &str) -> Vec<TestResult> {
        let filter = TestFilter::new().with_suite(suite_name.to_string());
        self.run_filtered(&filter)
    }

    /// Run tests in a category
    pub fn run_category(&self, category: &str) -> Vec<TestResult> {
        let filter = TestFilter::new().with_category(category.to_string());
        self.run_filtered(&filter)
    }

    /// Generate summary report
    pub fn summary(&self, results: &[TestResult]) -> String {
        let total = results.len();
        let passed = results.iter().filter(|r| r.success).count();
        let failed = total - passed;

        let total_duration: Duration = results.iter().map(|r| r.duration).sum();

        format!(
            "\n=== Test Summary ===\n\
             Total:  {}\n\
             Passed: {} ✓\n\
             Failed: {} ✗\n\
             Time:   {}ms\n",
            total,
            passed,
            failed,
            total_duration.as_millis()
        )
    }

    /// Print detailed results
    pub fn print_results(&self, results: &[TestResult], verbose: bool) {
        println!("{}", self.summary(results));

        let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();

        if !failed.is_empty() {
            println!("\nFailed tests:");
            for result in failed {
                println!("  - {} (exit code: {})", result.name, result.exit_code);
                if verbose {
                    println!("    stderr: {}", result.stderr.trim());
                }
            }
        }
    }
}

/// Helper function to get num_cpus (placeholder - would use num_cpus crate)
mod num_cpus {
    pub fn get() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_runner() {
        let result = TestRunner::from_default();
        assert!(result.is_ok(), "Failed to create test runner: {:?}", result.err());

        let runner = result.unwrap();
        assert!(!runner.manifest().tests.is_empty());
    }

    #[test]
    fn test_filter_tests() {
        let runner = TestRunner::from_default().unwrap();

        let filter = TestFilter::new()
            .with_category("basic".to_string());

        let tests = filter.apply(runner.manifest());
        assert!(!tests.is_empty(), "No basic tests found");
    }

    #[test]
    fn test_suite_selection() {
        let runner = TestRunner::from_default().unwrap();
        let tests = runner.manifest().tests_in_suite("quick");

        assert!(!tests.is_empty(), "No tests in 'quick' suite");
    }
}
