// Advanced test runner with async parallel execution and filtering

use super::config::{TestConfig, TestManifest, TestFilter, TestSpec, VerbosityLevel};
use super::test_specs::{TestReport, ValidationResult};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::sync::Mutex;
use std::sync::Arc;

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
    /// Whether test timed out
    pub timed_out: bool,
}

/// Test runner with parallel execution support
pub struct TestRunner {
    /// Test manifest
    manifest: TestManifest,
    /// Path to rholang-cli
    rholang_cli: String,
    /// Number of parallel workers
    workers: usize,
    /// Verbosity level
    verbosity: VerbosityLevel,
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
            verbosity: manifest.config.verbosity,
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

    /// Set verbosity level
    pub fn with_verbosity(mut self, level: VerbosityLevel) -> Self {
        self.verbosity = level;
        self
    }

    /// Run a single test with timeout support
    pub async fn run_test(&self, test: &TestSpec) -> TestResult {
        let start = Instant::now();

        // Resolve test file path
        let test_path = Path::new(&test.file);

        // Build command
        let mut cmd = Command::new(&self.rholang_cli);
        cmd.arg(test_path);

        // Add --quiet flag for quiet/normal verbosity
        if self.verbosity != VerbosityLevel::Verbose {
            cmd.arg("--quiet");
        }

        // Execute with timeout
        let output_result = if test.timeout > 0 {
            tokio::time::timeout(
                Duration::from_secs(test.timeout),
                cmd.output()
            ).await
        } else {
            Ok(cmd.output().await)
        };

        let duration = start.elapsed();

        match output_result {
            Ok(Ok(output)) => {
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
                    timed_out: false,
                }
            }
            Ok(Err(e)) => {
                // Execution error
                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to execute test: {}", e),
                    exit_code: -1,
                    duration,
                    report: None,
                    timed_out: false,
                }
            }
            Err(_) => {
                // Timeout
                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Test timed out after {}s", test.timeout),
                    exit_code: -1,
                    duration,
                    report: None,
                    timed_out: true,
                }
            }
        }
    }

    /// Run tests with filter (parallel execution)
    pub async fn run_filtered(&self, filter: &TestFilter) -> Vec<TestResult> {
        self.run_filtered_with_config(filter, true).await
    }

    /// Run tests with filter, optionally parallel
    pub async fn run_filtered_with_config(&self, filter: &TestFilter, parallel: bool) -> Vec<TestResult> {
        let tests = filter.apply(&self.manifest);

        if tests.is_empty() {
            if self.verbosity != VerbosityLevel::Quiet {
                println!("No tests matched filter");
            }
            return Vec::new();
        }

        if self.verbosity != VerbosityLevel::Quiet {
            println!("Running {} test(s) with {} worker(s)...\n",
                     tests.len(),
                     if parallel { self.workers } else { 1 });
        }

        let progress = Arc::new(Mutex::new(0usize));
        let total = tests.len();

        let results = if parallel && self.workers > 1 {
            // Parallel execution with tokio
            let mut handles = Vec::new();

            for test in tests {
                let test_owned = test.clone();
                let rholang_cli = self.rholang_cli.clone();
                let verbosity = self.verbosity;
                let progress = Arc::clone(&progress);

                let handle = tokio::spawn(async move {
                    Self::run_test_standalone(&test_owned, &rholang_cli, verbosity, &progress, total).await
                });

                handles.push(handle);
            }

            // Await all tasks
            let mut results = Vec::new();
            for handle in handles {
                if let Ok(result) = handle.await {
                    results.push(result);
                }
            }
            results
        } else {
            // Sequential execution
            let mut results = Vec::new();
            for test in tests {
                let result = self.run_test_with_progress(test, &progress, total).await;
                results.push(result);
            }
            results
        };

        if self.verbosity != VerbosityLevel::Quiet {
            println!(); // Newline after progress
        }

        results
    }

    /// Run a single test with progress reporting (async)
    async fn run_test_with_progress(
        &self,
        test: &TestSpec,
        progress: &Arc<Mutex<usize>>,
        total: usize,
    ) -> TestResult {
        if self.verbosity == VerbosityLevel::Verbose {
            println!("Running: {} ...", test.name);
        }

        let result = self.run_test(test).await;

        // Update progress
        let current = {
            let mut p = progress.lock().await;
            *p += 1;
            *p
        };

        // Print status based on verbosity
        match self.verbosity {
            VerbosityLevel::Quiet => {
                // No output
            }
            VerbosityLevel::Normal => {
                let status = if result.success { "." } else { "F" };
                print!("{}", status);
                use std::io::{self, Write};
                io::stdout().flush().unwrap();
            }
            VerbosityLevel::Verbose => {
                let status = if result.success { "ok" } else { "FAILED" };
                println!("  {} ({}ms) [{}/{}]",
                         status,
                         result.duration.as_millis(),
                         current,
                         total);
            }
        }

        result
    }

    /// Standalone test runner for spawned tasks
    async fn run_test_standalone(
        test: &TestSpec,
        rholang_cli: &str,
        verbosity: VerbosityLevel,
        progress: &Arc<Mutex<usize>>,
        total: usize,
    ) -> TestResult {
        let start = Instant::now();

        // Build command
        let mut cmd = Command::new(rholang_cli);
        cmd.arg(&test.file);

        // Add --quiet flag for quiet/normal verbosity
        if verbosity != VerbosityLevel::Verbose {
            cmd.arg("--quiet");
        }

        // Execute with timeout
        let output_result = if test.timeout > 0 {
            tokio::time::timeout(
                Duration::from_secs(test.timeout),
                cmd.output()
            ).await
        } else {
            Ok(cmd.output().await)
        };

        let duration = start.elapsed();

        let result = match output_result {
            Ok(Ok(output)) => {
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
                    timed_out: false,
                }
            }
            Ok(Err(e)) => {
                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Failed to execute test: {}", e),
                    exit_code: -1,
                    duration,
                    report: None,
                    timed_out: false,
                }
            }
            Err(_) => {
                TestResult {
                    name: test.name.clone(),
                    file: test.file.clone(),
                    success: false,
                    stdout: String::new(),
                    stderr: format!("Test timed out after {}s", test.timeout),
                    exit_code: -1,
                    duration,
                    report: None,
                    timed_out: true,
                }
            }
        };

        // Update progress
        let current = {
            let mut p = progress.lock().await;
            *p += 1;
            *p
        };

        // Print status based on verbosity
        match verbosity {
            VerbosityLevel::Quiet => {
                // No output
            }
            VerbosityLevel::Normal => {
                let status = if result.success { "." } else { "F" };
                print!("{}", status);
                use std::io::{self, Write};
                io::stdout().flush().unwrap();
            }
            VerbosityLevel::Verbose => {
                let status = if result.success { "ok" } else { "FAILED" };
                println!("  {} ({}ms) [{}/{}]",
                         status,
                         result.duration.as_millis(),
                         current,
                         total);
            }
        }

        result
    }

    /// Run all enabled tests
    pub async fn run_all(&self) -> Vec<TestResult> {
        let filter = TestFilter::new();
        self.run_filtered(&filter).await
    }

    /// Run tests in a suite
    pub async fn run_suite(&self, suite_name: &str) -> Vec<TestResult> {
        let filter = TestFilter::new().with_suite(suite_name.to_string());
        self.run_filtered(&filter).await
    }

    /// Run tests in a category
    pub async fn run_category(&self, category: &str) -> Vec<TestResult> {
        let filter = TestFilter::new().with_category(category.to_string());
        self.run_filtered(&filter).await
    }

    /// Run tests sequentially (disable parallelism)
    pub async fn run_sequential(&self, filter: &TestFilter) -> Vec<TestResult> {
        self.run_filtered_with_config(filter, false).await
    }

    /// Generate summary report
    pub fn summary(&self, results: &[TestResult]) -> String {
        let total = results.len();
        let passed = results.iter().filter(|r| r.success).count();
        let failed = total - passed;
        let timed_out = results.iter().filter(|r| r.timed_out).count();

        let total_duration: Duration = results.iter().map(|r| r.duration).sum();

        let mut summary = format!(
            "\n=== Test Summary ===\n\
             Total:    {}\n\
             Passed:   {} ✓\n\
             Failed:   {} ✗\n",
            total,
            passed,
            failed
        );

        if timed_out > 0 {
            summary.push_str(&format!("Timed out: {}\n", timed_out));
        }

        summary.push_str(&format!(
            "Time:     {}ms\n\
             Parallel: {} workers\n",
            total_duration.as_millis(),
            self.workers
        ));

        summary
    }

    /// Print detailed results
    pub fn print_results(&self, results: &[TestResult], verbose: bool) {
        if self.verbosity == VerbosityLevel::Quiet && !verbose {
            return;
        }

        println!("{}", self.summary(results));

        let failed: Vec<_> = results.iter().filter(|r| !r.success).collect();

        if !failed.is_empty() {
            println!("\nFailed tests:");
            for result in failed {
                println!("  - {} (exit code: {})", result.name, result.exit_code);
                if verbose || self.verbosity == VerbosityLevel::Verbose {
                    if !result.stderr.is_empty() {
                        println!("    stderr: {}", result.stderr.trim());
                    }
                    if result.timed_out {
                        println!("    (TIMEOUT)");
                    }
                }
            }
        }

        // Show timing breakdown in verbose mode
        if verbose || self.verbosity == VerbosityLevel::Verbose {
            println!("\nTiming breakdown:");
            let mut sorted = results.to_vec();
            sorted.sort_by_key(|r| std::cmp::Reverse(r.duration));

            for result in sorted.iter().take(5) {
                println!("  {} - {}ms", result.name, result.duration.as_millis());
            }
        }
    }
}

/// Helper function to get num_cpus
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
        assert!(runner.workers > 0);
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

    #[test]
    fn test_verbosity_levels() {
        let runner = TestRunner::from_default().unwrap();

        assert_eq!(runner.verbosity, VerbosityLevel::Normal);

        let verbose_runner = runner.with_verbosity(VerbosityLevel::Verbose);
        assert_eq!(verbose_runner.verbosity, VerbosityLevel::Verbose);
    }

    #[test]
    fn test_worker_count() {
        let runner = TestRunner::from_default().unwrap();

        // Should auto-detect CPU count
        let cpu_count = num_cpus::get();
        assert_eq!(runner.workers, cpu_count);
    }
}
