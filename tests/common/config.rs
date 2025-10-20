// Test configuration parser for TOML-based test specifications

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Global test configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestConfig {
    /// Default timeout for tests in seconds
    pub default_timeout: u64,
    /// Maximum number of parallel tests (0 = use all cores)
    pub max_parallel: usize,
    /// Path to rholang-cli binary
    pub rholang_cli: String,
    /// Output verbosity level
    pub verbosity: VerbosityLevel,
}

impl Default for TestConfig {
    fn default() -> Self {
        TestConfig {
            default_timeout: 30,
            max_parallel: 0,
            rholang_cli: "../f1r3node/target/release/rholang-cli".to_string(),
            verbosity: VerbosityLevel::Normal,
        }
    }
}

/// Output verbosity level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VerbosityLevel {
    Quiet,
    Normal,
    Verbose,
}

/// Individual test specification
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestSpec {
    /// Test name (identifier)
    pub name: String,
    /// Path to test file relative to repo root
    pub file: String,
    /// Test categories
    pub categories: Vec<String>,
    /// Timeout in seconds
    pub timeout: u64,
    /// Whether test is enabled
    pub enabled: bool,
    /// Test description
    pub description: String,
    /// Optional tags for filtering
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Category definition
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CategorySpec {
    /// Category description
    pub description: String,
    /// Priority (lower = higher priority)
    pub priority: u32,
}

/// Test suite definition
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestSuiteSpec {
    /// Suite description
    pub description: String,
    /// Specific tests to include (by name)
    #[serde(default)]
    pub tests: Vec<String>,
    /// Categories to include
    #[serde(default)]
    pub categories: Vec<String>,
}

/// Complete test manifest
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestManifest {
    /// Global configuration
    pub config: TestConfig,
    /// All test specifications
    #[serde(rename = "test")]
    pub tests: Vec<TestSpec>,
    /// Category definitions
    #[serde(default)]
    pub categories: HashMap<String, CategorySpec>,
    /// Test suite definitions
    #[serde(default)]
    pub suites: HashMap<String, TestSuiteSpec>,
}

impl TestManifest {
    /// Load test manifest from TOML file
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let content = std::fs::read_to_string(path.as_ref())
            .map_err(|e| format!("Failed to read manifest file: {}", e))?;

        toml::from_str(&content)
            .map_err(|e| format!("Failed to parse TOML: {}", e))
    }

    /// Load test manifest from default location (tests/integration_tests.toml)
    pub fn load_default() -> Result<Self, String> {
        let manifest_path = PathBuf::from("tests/integration_tests.toml");
        Self::from_file(manifest_path)
    }

    /// Get all enabled tests
    pub fn enabled_tests(&self) -> Vec<&TestSpec> {
        self.tests.iter().filter(|t| t.enabled).collect()
    }

    /// Filter tests by category
    pub fn tests_by_category(&self, category: &str) -> Vec<&TestSpec> {
        self.enabled_tests()
            .into_iter()
            .filter(|t| t.categories.contains(&category.to_string()))
            .collect()
    }

    /// Filter tests by tag
    pub fn tests_by_tag(&self, tag: &str) -> Vec<&TestSpec> {
        self.enabled_tests()
            .into_iter()
            .filter(|t| t.tags.contains(&tag.to_string()))
            .collect()
    }

    /// Filter tests by name pattern (glob-like)
    pub fn tests_by_name(&self, pattern: &str) -> Vec<&TestSpec> {
        self.enabled_tests()
            .into_iter()
            .filter(|t| t.name.contains(pattern))
            .collect()
    }

    /// Get tests in a suite
    pub fn tests_in_suite(&self, suite_name: &str) -> Vec<&TestSpec> {
        let Some(suite) = self.suites.get(suite_name) else {
            return Vec::new();
        };

        let mut tests = Vec::new();

        // Add tests by name
        for test_name in &suite.tests {
            if let Some(test) = self.tests.iter().find(|t| &t.name == test_name && t.enabled) {
                tests.push(test);
            }
        }

        // Add tests by category
        for category in &suite.categories {
            for test in self.tests_by_category(category) {
                if !tests.iter().any(|t| t.name == test.name) {
                    tests.push(test);
                }
            }
        }

        tests
    }

    /// Get test by name
    pub fn get_test(&self, name: &str) -> Option<&TestSpec> {
        self.tests.iter().find(|t| t.name == name)
    }

    /// Get all categories sorted by priority
    pub fn categories_by_priority(&self) -> Vec<(String, &CategorySpec)> {
        let mut categories: Vec<_> = self.categories.iter()
            .map(|(name, spec)| (name.clone(), spec))
            .collect();
        categories.sort_by_key(|(_, spec)| spec.priority);
        categories
    }
}

/// Test filter for selecting which tests to run
#[derive(Debug, Clone, Default)]
pub struct TestFilter {
    /// Include tests matching these names
    pub names: Vec<String>,
    /// Include tests in these categories
    pub categories: Vec<String>,
    /// Include tests with these tags
    pub tags: Vec<String>,
    /// Include tests in these suites
    pub suites: Vec<String>,
    /// Exclude tests matching these patterns
    pub exclude: Vec<String>,
}

impl TestFilter {
    /// Create a new empty filter
    pub fn new() -> Self {
        Self::default()
    }

    /// Add name filter
    pub fn with_name(mut self, name: String) -> Self {
        self.names.push(name);
        self
    }

    /// Add category filter
    pub fn with_category(mut self, category: String) -> Self {
        self.categories.push(category);
        self
    }

    /// Add tag filter
    pub fn with_tag(mut self, tag: String) -> Self {
        self.tags.push(tag);
        self
    }

    /// Add suite filter
    pub fn with_suite(mut self, suite: String) -> Self {
        self.suites.push(suite);
        self
    }

    /// Add exclusion pattern
    pub fn exclude(mut self, pattern: String) -> Self {
        self.exclude.push(pattern);
        self
    }

    /// Check if filter is empty (no criteria)
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
            && self.categories.is_empty()
            && self.tags.is_empty()
            && self.suites.is_empty()
    }

    /// Apply filter to manifest and return matching tests
    pub fn apply<'a>(&self, manifest: &'a TestManifest) -> Vec<&'a TestSpec> {
        // If filter is empty, return all enabled tests
        if self.is_empty() && self.exclude.is_empty() {
            return manifest.enabled_tests();
        }

        let mut tests = Vec::new();

        // Collect tests by name
        for name in &self.names {
            if let Some(test) = manifest.tests.iter().find(|t| &t.name == name && t.enabled) {
                if !tests.iter().any(|t: &&TestSpec| t.name == test.name) {
                    tests.push(test);
                }
            }
        }

        // Collect tests by category
        for category in &self.categories {
            for test in manifest.tests_by_category(category) {
                if !tests.iter().any(|t| t.name == test.name) {
                    tests.push(test);
                }
            }
        }

        // Collect tests by tag
        for tag in &self.tags {
            for test in manifest.tests_by_tag(tag) {
                if !tests.iter().any(|t| t.name == test.name) {
                    tests.push(test);
                }
            }
        }

        // Collect tests by suite
        for suite in &self.suites {
            for test in manifest.tests_in_suite(suite) {
                if !tests.iter().any(|t| t.name == test.name) {
                    tests.push(test);
                }
            }
        }

        // Apply exclusions
        if !self.exclude.is_empty() {
            tests.retain(|test| {
                !self.exclude.iter().any(|pattern| test.name.contains(pattern))
            });
        }

        tests
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_manifest() {
        let manifest = TestManifest::load_default();
        assert!(manifest.is_ok(), "Failed to load manifest: {:?}", manifest.err());

        let manifest = manifest.unwrap();
        assert!(!manifest.tests.is_empty(), "No tests found in manifest");
        assert!(manifest.config.default_timeout > 0);
    }

    #[test]
    fn test_filter_by_category() {
        let manifest = TestManifest::load_default().unwrap();
        let basic_tests = manifest.tests_by_category("basic");
        assert!(!basic_tests.is_empty(), "No basic tests found");
    }

    #[test]
    fn test_filter_by_suite() {
        let manifest = TestManifest::load_default().unwrap();
        let core_tests = manifest.tests_in_suite("core");
        assert!(!core_tests.is_empty(), "No tests in 'core' suite");
    }

    #[test]
    fn test_test_filter() {
        let manifest = TestManifest::load_default().unwrap();

        let filter = TestFilter::new()
            .with_category("basic".to_string());

        let tests = filter.apply(&manifest);
        assert!(!tests.is_empty(), "Filter returned no tests");
    }

    #[test]
    fn test_categories_by_priority() {
        let manifest = TestManifest::load_default().unwrap();
        let categories = manifest.categories_by_priority();

        assert!(!categories.is_empty(), "No categories found");

        // Check that priorities are sorted
        for i in 1..categories.len() {
            assert!(
                categories[i - 1].1.priority <= categories[i].1.priority,
                "Categories not sorted by priority"
            );
        }
    }
}
