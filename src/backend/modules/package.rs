//! Package Management for MeTTa Modules
//!
//! This module provides package manifest parsing and version constraint
//! validation for MeTTa modules. It supports two manifest formats:
//!
//! 1. **`_pkg-info.metta`** (HE-compatible, preferred)
//! 2. **`metta.toml`** (MeTTaTron-native)
//!
//! ## TOML Manifest Format (`metta.toml`)
//!
//! ```toml
//! [package]
//! name = "my-module"
//! version = "1.0.0"
//! description = "My awesome MeTTa module"
//!
//! [dependencies]
//! std = "^1.0"
//! my-lib = { path = "../my-lib" }
//! external = { git = "https://github.com/user/repo", tag = "v1.0" }
//!
//! [exports]
//! public = ["my-function", "my-type"]
//! ```
//!
//! ## MeTTa Manifest Format (`_pkg-info.metta`)
//!
//! Uses HE-compatible Atom-Serde format with `#`-prefixed keys:
//!
//! ```metta
//! (#package
//!     (#name "my-module")
//!     (#version "1.0.0")
//!     (#description "My awesome MeTTa module")
//! )
//!
//! (#dependencies
//!     (#std "^1.0")
//!     (#my-lib (#path "../my-lib"))
//!     (#external (#git "https://github.com/user/repo" #tag "v1.0"))
//! )
//!
//! (#exports
//!     (#public (my-function my-type))
//! )
//! ```

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use super::pkg_info_metta::load_pkg_info_metta;

/// Package metadata from `metta.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct PackageInfo {
    /// Package metadata section.
    pub package: PackageMeta,

    /// Optional dependencies section.
    #[serde(default)]
    pub dependencies: HashMap<String, Dependency>,

    /// Optional exports section.
    #[serde(default)]
    pub exports: ExportConfig,
}

/// Package metadata (the `[package]` section).
#[derive(Debug, Clone, Deserialize)]
pub struct PackageMeta {
    /// Package name (required).
    pub name: String,

    /// Package version (required, semver format).
    pub version: String,

    /// Optional package description.
    #[serde(default)]
    pub description: Option<String>,

    /// Optional author list.
    #[serde(default)]
    pub authors: Vec<String>,

    /// Optional license identifier (e.g., "MIT", "Apache-2.0").
    #[serde(default)]
    pub license: Option<String>,

    /// Optional repository URL.
    #[serde(default)]
    pub repository: Option<String>,

    /// Optional documentation URL.
    #[serde(default)]
    pub documentation: Option<String>,

    /// Optional homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,

    /// Optional keywords for discovery.
    #[serde(default)]
    pub keywords: Vec<String>,

    /// Optional categories for classification.
    #[serde(default)]
    pub categories: Vec<String>,
}

/// Dependency specification.
///
/// Dependencies can be specified in several ways:
/// - Simple version: `"^1.0"`
/// - Local path: `{ path = "../my-lib" }`
/// - Git repository: `{ git = "...", tag = "v1.0" }`
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// Simple version constraint (e.g., `"^1.0"`, `">=1.0.0"`)
    Version(String),

    /// Detailed dependency specification.
    Detailed(DependencyDetail),
}

/// Detailed dependency specification.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DependencyDetail {
    /// Version constraint (e.g., `"^1.0"`).
    #[serde(default)]
    pub version: Option<String>,

    /// Local filesystem path.
    #[serde(default)]
    pub path: Option<String>,

    /// Git repository URL.
    #[serde(default)]
    pub git: Option<String>,

    /// Git tag for version pinning.
    #[serde(default)]
    pub tag: Option<String>,

    /// Git branch name.
    #[serde(default)]
    pub branch: Option<String>,

    /// Git commit SHA.
    #[serde(default)]
    pub rev: Option<String>,

    /// Optional features to enable.
    #[serde(default)]
    pub features: Vec<String>,

    /// Whether this is an optional dependency.
    #[serde(default)]
    pub optional: bool,
}

/// Export configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExportConfig {
    /// List of public symbols (can be imported by other modules).
    #[serde(default)]
    pub public: Vec<String>,

    /// Whether to export all symbols by default.
    #[serde(default)]
    pub all: bool,
}

impl PackageInfo {
    /// Load a package manifest from a directory.
    ///
    /// Tries the following formats in order:
    /// 1. `_pkg-info.metta` (HE-compatible Atom-Serde format, preferred)
    /// 2. `metta.toml` (TOML format)
    ///
    /// Returns `None` if neither file exists or can be parsed.
    pub fn load(module_dir: &Path) -> Option<Self> {
        // Try _pkg-info.metta first (HE-compatible format)
        match load_pkg_info_metta(module_dir) {
            Ok(Some(pkg_info)) => return Some(pkg_info),
            Ok(None) => {
                // File doesn't exist, try TOML
            }
            Err(e) => {
                // Log error but continue to try TOML
                eprintln!(
                    "Warning: Failed to parse _pkg-info.metta in {}: {}",
                    module_dir.display(),
                    e
                );
            }
        }

        // Fall back to metta.toml
        let toml_path = module_dir.join("metta.toml");
        Self::load_from_toml_path(&toml_path)
    }

    /// Load a package manifest from a TOML file path.
    pub fn load_from_toml_path(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(path).ok()?;
        Self::parse_toml(&content).ok()
    }

    /// Load a package manifest from a specific file path.
    ///
    /// Deprecated: Use `load()` for unified loading or `load_from_toml_path()` for TOML only.
    #[deprecated(since = "0.2.0", note = "Use load() or load_from_toml_path() instead")]
    pub fn load_from_path(path: &Path) -> Option<Self> {
        Self::load_from_toml_path(path)
    }

    /// Parse a package manifest from TOML content.
    pub fn parse_toml(content: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(content)
    }

    /// Parse a package manifest from TOML content.
    ///
    /// Deprecated: Use `parse_toml()` for explicit TOML parsing.
    #[deprecated(since = "0.2.0", note = "Use parse_toml() instead")]
    pub fn parse(content: &str) -> Result<Self, toml::de::Error> {
        Self::parse_toml(content)
    }

    /// Get the package name.
    pub fn name(&self) -> &str {
        &self.package.name
    }

    /// Get the package version as a string.
    pub fn version_str(&self) -> &str {
        &self.package.version
    }

    /// Parse the package version as semver.
    pub fn version(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.package.version).ok()
    }

    /// Get the package description.
    pub fn description(&self) -> Option<&str> {
        self.package.description.as_deref()
    }

    /// Check if a symbol is marked as public in the exports config.
    pub fn is_exported(&self, symbol: &str) -> bool {
        self.exports.all || self.exports.public.contains(&symbol.to_string())
    }

    /// Get all exported symbols.
    pub fn exported_symbols(&self) -> &[String] {
        &self.exports.public
    }

    /// Get all dependencies.
    pub fn dependencies(&self) -> &HashMap<String, Dependency> {
        &self.dependencies
    }

    /// Check if a specific dependency exists.
    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains_key(name)
    }

    /// Get a specific dependency.
    pub fn get_dependency(&self, name: &str) -> Option<&Dependency> {
        self.dependencies.get(name)
    }
}

impl Dependency {
    /// Get the version constraint if specified.
    pub fn version_constraint(&self) -> Option<&str> {
        match self {
            Self::Version(v) => Some(v),
            Self::Detailed(d) => d.version.as_deref(),
        }
    }

    /// Get the local path if specified.
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Version(_) => None,
            Self::Detailed(d) => d.path.as_deref(),
        }
    }

    /// Get the git URL if specified.
    pub fn git(&self) -> Option<&str> {
        match self {
            Self::Version(_) => None,
            Self::Detailed(d) => d.git.as_deref(),
        }
    }

    /// Check if this is a path dependency.
    pub fn is_path(&self) -> bool {
        self.path().is_some()
    }

    /// Check if this is a git dependency.
    pub fn is_git(&self) -> bool {
        self.git().is_some()
    }

    /// Check if this is a registry dependency (version-only).
    pub fn is_registry(&self) -> bool {
        match self {
            Self::Version(_) => true,
            Self::Detailed(d) => d.path.is_none() && d.git.is_none() && d.version.is_some(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let content = r#"
            [package]
            name = "test-pkg"
            version = "1.0.0"
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        assert_eq!(pkg.name(), "test-pkg");
        assert_eq!(pkg.version_str(), "1.0.0");
        assert!(pkg.description().is_none());
    }

    #[test]
    fn test_parse_full_manifest() {
        let content = r#"
            [package]
            name = "my-module"
            version = "2.1.0"
            description = "A test module"
            authors = ["Alice", "Bob"]
            license = "MIT"

            [dependencies]
            std = "^1.0"
            my-lib = { path = "../my-lib" }
            external = { git = "https://github.com/user/repo", tag = "v1.0" }

            [exports]
            public = ["foo", "bar"]
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        assert_eq!(pkg.name(), "my-module");
        assert_eq!(pkg.version_str(), "2.1.0");
        assert_eq!(pkg.description(), Some("A test module"));
        assert_eq!(pkg.package.authors, vec!["Alice", "Bob"]);
        assert_eq!(pkg.package.license, Some("MIT".to_string()));

        // Check dependencies
        assert!(pkg.has_dependency("std"));
        assert!(pkg.has_dependency("my-lib"));
        assert!(pkg.has_dependency("external"));

        let std_dep = pkg.get_dependency("std").unwrap();
        assert_eq!(std_dep.version_constraint(), Some("^1.0"));
        assert!(std_dep.is_registry());

        let lib_dep = pkg.get_dependency("my-lib").unwrap();
        assert_eq!(lib_dep.path(), Some("../my-lib"));
        assert!(lib_dep.is_path());

        let ext_dep = pkg.get_dependency("external").unwrap();
        assert_eq!(ext_dep.git(), Some("https://github.com/user/repo"));
        assert!(ext_dep.is_git());

        // Check exports
        assert!(pkg.is_exported("foo"));
        assert!(pkg.is_exported("bar"));
        assert!(!pkg.is_exported("baz"));
    }

    #[test]
    fn test_parse_export_all() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [exports]
            all = true
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        assert!(pkg.is_exported("anything"));
        assert!(pkg.is_exported("everything"));
    }

    #[test]
    fn test_version_parsing() {
        let content = r#"
            [package]
            name = "test"
            version = "1.2.3"
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        let ver = pkg.version().expect("valid semver");
        assert_eq!(ver.major, 1);
        assert_eq!(ver.minor, 2);
        assert_eq!(ver.patch, 3);
    }

    #[test]
    fn test_empty_dependencies() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        assert!(pkg.dependencies().is_empty());
        assert!(!pkg.has_dependency("anything"));
    }

    #[test]
    fn test_optional_dependency() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [dependencies]
            optional-dep = { version = "1.0", optional = true }
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        if let Dependency::Detailed(d) = pkg.get_dependency("optional-dep").unwrap() {
            assert!(d.optional);
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_dependency_with_features() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [dependencies]
            feature-dep = { version = "1.0", features = ["foo", "bar"] }
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        if let Dependency::Detailed(d) = pkg.get_dependency("feature-dep").unwrap() {
            assert_eq!(d.features, vec!["foo", "bar"]);
        } else {
            panic!("Expected detailed dependency");
        }
    }

    // ============================================================
    // Additional edge case tests
    // ============================================================

    #[test]
    fn test_parse_invalid_toml() {
        let content = "this is not valid toml";
        assert!(PackageInfo::parse_toml(content).is_err());
    }

    #[test]
    fn test_parse_missing_required_fields() {
        // Missing version
        let content = r#"
            [package]
            name = "test"
        "#;
        assert!(PackageInfo::parse_toml(content).is_err());

        // Missing name
        let content = r#"
            [package]
            version = "1.0.0"
        "#;
        assert!(PackageInfo::parse_toml(content).is_err());
    }

    #[test]
    fn test_load_nonexistent_path() {
        let result = PackageInfo::load(Path::new("/nonexistent/path"));
        assert!(result.is_none());
    }

    #[test]
    fn test_load_from_nonexistent_file() {
        let result = PackageInfo::load_from_toml_path(Path::new("/nonexistent/metta.toml"));
        assert!(result.is_none());
    }

    #[test]
    fn test_exported_symbols() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [exports]
            public = ["sym1", "sym2", "sym3"]
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        let symbols = pkg.exported_symbols();
        assert_eq!(symbols.len(), 3);
        assert!(symbols.contains(&"sym1".to_string()));
        assert!(symbols.contains(&"sym2".to_string()));
        assert!(symbols.contains(&"sym3".to_string()));
    }

    #[test]
    fn test_git_dependency_with_branch() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [dependencies]
            dev-lib = { git = "https://github.com/user/repo", branch = "develop" }
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        if let Dependency::Detailed(d) = pkg.get_dependency("dev-lib").unwrap() {
            assert_eq!(d.git, Some("https://github.com/user/repo".to_string()));
            assert_eq!(d.branch, Some("develop".to_string()));
            assert!(d.tag.is_none());
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_git_dependency_with_rev() {
        let content = r#"
            [package]
            name = "test"
            version = "1.0.0"

            [dependencies]
            pinned-lib = { git = "https://github.com/user/repo", rev = "abc123" }
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        if let Dependency::Detailed(d) = pkg.get_dependency("pinned-lib").unwrap() {
            assert_eq!(d.git, Some("https://github.com/user/repo".to_string()));
            assert_eq!(d.rev, Some("abc123".to_string()));
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_package_metadata() {
        let content = r#"
            [package]
            name = "full-meta"
            version = "1.0.0"
            description = "Test package"
            authors = ["Author One", "Author Two"]
            license = "Apache-2.0"
            repository = "https://github.com/user/repo"
            documentation = "https://docs.example.com"
            homepage = "https://example.com"
            keywords = ["metta", "test"]
            categories = ["utilities"]
        "#;

        let pkg = PackageInfo::parse_toml(content).expect("valid manifest");
        assert_eq!(
            pkg.package.repository,
            Some("https://github.com/user/repo".to_string())
        );
        assert_eq!(
            pkg.package.documentation,
            Some("https://docs.example.com".to_string())
        );
        assert_eq!(
            pkg.package.homepage,
            Some("https://example.com".to_string())
        );
        assert_eq!(pkg.package.keywords, vec!["metta", "test"]);
        assert_eq!(pkg.package.categories, vec!["utilities"]);
    }

    #[test]
    fn test_dependency_type_detection() {
        let simple = Dependency::Version("^1.0".to_string());
        assert!(simple.is_registry());
        assert!(!simple.is_path());
        assert!(!simple.is_git());

        let path_dep = Dependency::Detailed(DependencyDetail {
            path: Some("../lib".to_string()),
            ..Default::default()
        });
        assert!(path_dep.is_path());
        assert!(!path_dep.is_registry());
        assert!(!path_dep.is_git());

        let git_dep = Dependency::Detailed(DependencyDetail {
            git: Some("https://github.com/user/repo".to_string()),
            ..Default::default()
        });
        assert!(git_dep.is_git());
        assert!(!git_dep.is_registry());
        assert!(!git_dep.is_path());
    }
}
