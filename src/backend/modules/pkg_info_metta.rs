//! MeTTa Package Info Parser
//!
//! Parses `_pkg-info.metta` files using the HE-compatible Atom-Serde format.
//! This format uses `#`-prefixed symbols as keys in S-expressions.
//!
//! ## Format
//!
//! ```metta
//! (#package
//!     (#name "my-module")
//!     (#version "1.0.0")
//!     (#description "My awesome module")
//!     (#authors ("Alice" "Bob"))
//!     (#license "MIT")
//! )
//!
//! (#dependencies
//!     (#std "^1.0")
//!     (#my-lib (#path "../my-lib"))
//!     (#external (#git "https://github.com/user/repo" #tag "v1.0"))
//! )
//!
//! (#exports
//!     (#public (func1 func2))
//!     (#all True)
//! )
//! ```

use std::collections::HashMap;
use std::path::Path;

use crate::ir::SExpr;
use crate::tree_sitter_parser::TreeSitterMettaParser;

use super::package::{Dependency, DependencyDetail, ExportConfig, PackageInfo, PackageMeta};

/// Error type for `_pkg-info.metta` parsing.
#[derive(Debug, Clone)]
pub struct PkgInfoParseError {
    pub message: String,
}

impl std::fmt::Display for PkgInfoParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Package info parse error: {}", self.message)
    }
}

impl std::error::Error for PkgInfoParseError {}

impl From<String> for PkgInfoParseError {
    fn from(message: String) -> Self {
        Self { message }
    }
}

impl From<&str> for PkgInfoParseError {
    fn from(message: &str) -> Self {
        Self {
            message: message.to_string(),
        }
    }
}

/// Parse a `_pkg-info.metta` file from a directory.
///
/// Returns `None` if the file doesn't exist.
/// Returns `Err` if the file exists but contains invalid syntax.
pub fn load_pkg_info_metta(module_dir: &Path) -> Result<Option<PackageInfo>, PkgInfoParseError> {
    let pkg_info_path = module_dir.join("_pkg-info.metta");

    if !pkg_info_path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&pkg_info_path)
        .map_err(|e| PkgInfoParseError::from(format!("Failed to read file: {}", e)))?;

    parse_pkg_info_metta(&content).map(Some)
}

/// Parse package info from MeTTa content.
pub fn parse_pkg_info_metta(content: &str) -> Result<PackageInfo, PkgInfoParseError> {
    // Handle empty content
    let content = content.trim();
    if content.is_empty() {
        return Err(PkgInfoParseError::from(
            "Empty package info file: #package section is required",
        ));
    }

    // Parse the MeTTa content
    let mut parser = TreeSitterMettaParser::new()
        .map_err(|e| PkgInfoParseError::from(format!("Parser initialization failed: {}", e)))?;

    let exprs = parser
        .parse(content)
        .map_err(|e| PkgInfoParseError::from(format!("Syntax error: {}", e)))?;

    // Extract sections from top-level expressions
    let mut package_meta: Option<PackageMeta> = None;
    let mut dependencies: HashMap<String, Dependency> = HashMap::new();
    let mut exports = ExportConfig::default();

    for expr in exprs {
        if let Some((key, children)) = extract_hash_section(&expr) {
            match key.as_str() {
                "#package" => {
                    package_meta = Some(parse_package_section(&children)?);
                }
                "#dependencies" => {
                    dependencies = parse_dependencies_section(&children)?;
                }
                "#exports" => {
                    exports = parse_exports_section(&children)?;
                }
                _ => {
                    // Ignore unknown sections for forward compatibility
                }
            }
        }
    }

    // Validate required fields
    let package = package_meta.ok_or_else(|| {
        PkgInfoParseError::from("Missing required #package section")
    })?;

    Ok(PackageInfo {
        package,
        dependencies,
        exports,
    })
}

/// Extract a `#`-prefixed section from an S-expression.
///
/// Returns `Some((key, children))` if the expression is a list starting with a `#`-prefixed atom.
fn extract_hash_section(expr: &SExpr) -> Option<(String, Vec<SExpr>)> {
    if let SExpr::List(items, _) = expr {
        if let Some(SExpr::Atom(key, _)) = items.first() {
            if key.starts_with('#') {
                return Some((key.clone(), items[1..].to_vec()));
            }
        }
    }
    None
}

/// Extract the value from a `#`-prefixed key-value pair.
///
/// For `(#key value)`, returns `Some(("key", value))`.
/// For `(#key (nested ...))`, returns `Some(("key", (nested ...)))`.
fn extract_hash_key_value(expr: &SExpr) -> Option<(String, SExpr)> {
    if let SExpr::List(items, _) = expr {
        if items.len() >= 2 {
            if let SExpr::Atom(key, _) = &items[0] {
                if key.starts_with('#') {
                    // Strip the '#' prefix
                    let key_name = key[1..].to_string();
                    // If there's exactly one value, return it; otherwise wrap in a list
                    if items.len() == 2 {
                        return Some((key_name, items[1].clone()));
                    } else {
                        return Some((key_name, SExpr::list(items[1..].to_vec())));
                    }
                }
            }
        }
    }
    None
}

/// Extract a string value from an S-expression.
fn extract_string(expr: &SExpr) -> Option<String> {
    match expr {
        SExpr::String(s, _) => Some(s.clone()),
        SExpr::Atom(s, _) => Some(s.clone()),
        _ => None,
    }
}

/// Extract a boolean value from an S-expression.
fn extract_bool(expr: &SExpr) -> Option<bool> {
    match expr {
        SExpr::Atom(s, _) => match s.as_str() {
            "True" | "true" => Some(true),
            "False" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Extract a list of strings from an S-expression.
fn extract_string_list(expr: &SExpr) -> Vec<String> {
    match expr {
        SExpr::List(items, _) => items.iter().filter_map(extract_string).collect(),
        SExpr::String(s, _) => vec![s.clone()],
        SExpr::Atom(s, _) => vec![s.clone()],
        _ => vec![],
    }
}

/// Parse the `#package` section.
fn parse_package_section(children: &[SExpr]) -> Result<PackageMeta, PkgInfoParseError> {
    let mut name: Option<String> = None;
    let mut version: Option<String> = None;
    let mut description: Option<String> = None;
    let mut authors: Vec<String> = vec![];
    let mut license: Option<String> = None;
    let mut repository: Option<String> = None;
    let mut documentation: Option<String> = None;
    let mut homepage: Option<String> = None;
    let mut keywords: Vec<String> = vec![];
    let mut categories: Vec<String> = vec![];

    for child in children {
        if let Some((key, value)) = extract_hash_key_value(child) {
            match key.as_str() {
                "name" => name = extract_string(&value),
                "version" => version = extract_string(&value),
                "description" => description = extract_string(&value),
                "authors" => authors = extract_string_list(&value),
                "license" => license = extract_string(&value),
                "repository" => repository = extract_string(&value),
                "documentation" => documentation = extract_string(&value),
                "homepage" => homepage = extract_string(&value),
                "keywords" => keywords = extract_string_list(&value),
                "categories" => categories = extract_string_list(&value),
                _ => {
                    // Ignore unknown fields for forward compatibility
                }
            }
        }
    }

    // Validate required fields
    let name = name.ok_or_else(|| PkgInfoParseError::from("Missing required #name in #package"))?;
    let version =
        version.ok_or_else(|| PkgInfoParseError::from("Missing required #version in #package"))?;

    Ok(PackageMeta {
        name,
        version,
        description,
        authors,
        license,
        repository,
        documentation,
        homepage,
        keywords,
        categories,
    })
}

/// Parse the `#dependencies` section.
fn parse_dependencies_section(
    children: &[SExpr],
) -> Result<HashMap<String, Dependency>, PkgInfoParseError> {
    let mut deps = HashMap::new();

    for child in children {
        if let Some((dep_name, value)) = extract_hash_key_value(child) {
            let dependency = parse_dependency(&value)?;
            deps.insert(dep_name, dependency);
        }
    }

    Ok(deps)
}

/// Parse a single dependency specification.
fn parse_dependency(expr: &SExpr) -> Result<Dependency, PkgInfoParseError> {
    match expr {
        // Simple version constraint: "^1.0"
        SExpr::String(version, _) => Ok(Dependency::Version(version.clone())),
        SExpr::Atom(version, _) => Ok(Dependency::Version(version.clone())),

        // Detailed dependency: (#path "../lib") or (#git "url" #tag "v1.0")
        SExpr::List(items, _) => {
            let mut detail = DependencyDetail::default();

            // Parse key-value pairs within the dependency
            let mut i = 0;
            while i < items.len() {
                if let SExpr::Atom(key, _) = &items[i] {
                    if key.starts_with('#') {
                        let key_name = &key[1..];
                        // Get the next item as the value
                        if i + 1 < items.len() {
                            let value = &items[i + 1];
                            match key_name {
                                "version" => detail.version = extract_string(value),
                                "path" => detail.path = extract_string(value),
                                "git" => detail.git = extract_string(value),
                                "tag" => detail.tag = extract_string(value),
                                "branch" => detail.branch = extract_string(value),
                                "rev" => detail.rev = extract_string(value),
                                "features" => detail.features = extract_string_list(value),
                                "optional" => detail.optional = extract_bool(value).unwrap_or(false),
                                _ => {}
                            }
                            i += 2;
                            continue;
                        }
                    }
                }
                i += 1;
            }

            Ok(Dependency::Detailed(detail))
        }

        _ => Err(PkgInfoParseError::from(format!(
            "Invalid dependency specification: {:?}",
            expr
        ))),
    }
}

/// Parse the `#exports` section.
fn parse_exports_section(children: &[SExpr]) -> Result<ExportConfig, PkgInfoParseError> {
    let mut exports = ExportConfig::default();

    for child in children {
        if let Some((key, value)) = extract_hash_key_value(child) {
            match key.as_str() {
                "public" => {
                    exports.public = extract_string_list(&value);
                }
                "all" => {
                    exports.all = extract_bool(&value).unwrap_or(false);
                }
                _ => {
                    // Ignore unknown fields
                }
            }
        }
    }

    Ok(exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // Basic Parsing Tests
    // ============================================================

    #[test]
    fn test_parse_minimal_pkg_info() {
        let content = r#"
            (#package
                (#name "test-pkg")
                (#version "1.0.0")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "test-pkg");
        assert_eq!(pkg.version_str(), "1.0.0");
        assert!(pkg.description().is_none());
    }

    #[test]
    fn test_parse_full_package_section() {
        let content = r#"
            (#package
                (#name "my-module")
                (#version "2.1.0")
                (#description "A test module")
                (#authors ("Alice" "Bob"))
                (#license "MIT")
                (#repository "https://github.com/user/repo")
                (#documentation "https://docs.example.com")
                (#homepage "https://example.com")
                (#keywords ("metta" "test"))
                (#categories ("utilities"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "my-module");
        assert_eq!(pkg.version_str(), "2.1.0");
        assert_eq!(pkg.description(), Some("A test module"));
        assert_eq!(pkg.package.authors, vec!["Alice", "Bob"]);
        assert_eq!(pkg.package.license, Some("MIT".to_string()));
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

    // ============================================================
    // Dependency Parsing Tests
    // ============================================================

    #[test]
    fn test_parse_version_dependency() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#std "^1.0")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert!(pkg.has_dependency("std"));

        let std_dep = pkg.get_dependency("std").unwrap();
        assert_eq!(std_dep.version_constraint(), Some("^1.0"));
        assert!(std_dep.is_registry());
    }

    #[test]
    fn test_parse_path_dependency() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#my-lib (#path "../my-lib"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        let lib_dep = pkg.get_dependency("my-lib").unwrap();
        assert_eq!(lib_dep.path(), Some("../my-lib"));
        assert!(lib_dep.is_path());
    }

    #[test]
    fn test_parse_git_dependency_with_tag() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#external (#git "https://github.com/user/repo" #tag "v1.0"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        let ext_dep = pkg.get_dependency("external").unwrap();
        assert_eq!(ext_dep.git(), Some("https://github.com/user/repo"));
        assert!(ext_dep.is_git());

        if let Dependency::Detailed(d) = ext_dep {
            assert_eq!(d.tag, Some("v1.0".to_string()));
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_parse_git_dependency_with_branch() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#dev-lib (#git "https://github.com/user/repo" #branch "develop"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        if let Dependency::Detailed(d) = pkg.get_dependency("dev-lib").unwrap() {
            assert_eq!(d.git, Some("https://github.com/user/repo".to_string()));
            assert_eq!(d.branch, Some("develop".to_string()));
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_parse_git_dependency_with_rev() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#pinned (#git "https://github.com/user/repo" #rev "abc123"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        if let Dependency::Detailed(d) = pkg.get_dependency("pinned").unwrap() {
            assert_eq!(d.rev, Some("abc123".to_string()));
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_parse_optional_dependency() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#optional-dep (#version "1.0" #optional True))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        if let Dependency::Detailed(d) = pkg.get_dependency("optional-dep").unwrap() {
            assert!(d.optional);
        } else {
            panic!("Expected detailed dependency");
        }
    }

    #[test]
    fn test_parse_dependency_with_features() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#feature-dep (#version "1.0" #features ("foo" "bar")))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        if let Dependency::Detailed(d) = pkg.get_dependency("feature-dep").unwrap() {
            assert_eq!(d.features, vec!["foo", "bar"]);
        } else {
            panic!("Expected detailed dependency");
        }
    }

    // ============================================================
    // Exports Parsing Tests
    // ============================================================

    #[test]
    fn test_parse_public_exports() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#exports
                (#public (foo bar baz))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert!(pkg.is_exported("foo"));
        assert!(pkg.is_exported("bar"));
        assert!(pkg.is_exported("baz"));
        assert!(!pkg.is_exported("private"));
    }

    #[test]
    fn test_parse_export_all() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#exports
                (#all True)
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert!(pkg.is_exported("anything"));
        assert!(pkg.is_exported("everything"));
    }

    // ============================================================
    // Error Handling Tests
    // ============================================================

    #[test]
    fn test_parse_empty_content() {
        let result = parse_pkg_info_metta("");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("Empty package info file"));
    }

    #[test]
    fn test_parse_missing_package_section() {
        let content = r#"
            (#dependencies
                (#std "^1.0")
            )
        "#;

        let result = parse_pkg_info_metta(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("Missing required #package"));
    }

    #[test]
    fn test_parse_missing_name() {
        let content = r#"
            (#package
                (#version "1.0.0")
            )
        "#;

        let result = parse_pkg_info_metta(content);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Missing required #name"));
    }

    #[test]
    fn test_parse_missing_version() {
        let content = r#"
            (#package
                (#name "test")
            )
        "#;

        let result = parse_pkg_info_metta(content);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .message
            .contains("Missing required #version"));
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn test_parse_with_comments() {
        let content = r#"
            ; This is a comment
            (#package
                (#name "test")  ; inline comment
                (#version "1.0.0")
            )
            ; Another comment
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "test");
    }

    #[test]
    fn test_parse_unknown_sections_ignored() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#unknown-section
                (#foo "bar")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "test");
    }

    #[test]
    fn test_parse_unknown_fields_ignored() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
                (#unknown-field "value")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "test");
    }

    #[test]
    fn test_parse_empty_dependencies() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies)
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert!(pkg.dependencies().is_empty());
    }

    #[test]
    fn test_parse_empty_exports() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#exports)
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert!(pkg.exported_symbols().is_empty());
        assert!(!pkg.exports.all);
    }

    #[test]
    fn test_parse_special_characters_in_names() {
        let content = r#"
            (#package
                (#name "my-awesome_module.v2")
                (#version "1.0.0")
            )
            (#dependencies
                (#dep-with-dashes "^1.0")
                (#dep_with_underscores "^2.0")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.name(), "my-awesome_module.v2");
        assert!(pkg.has_dependency("dep-with-dashes"));
        assert!(pkg.has_dependency("dep_with_underscores"));
    }

    #[test]
    fn test_parse_unicode_in_strings() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
                (#description "MeTTa 模块 🚀")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.description(), Some("MeTTa 模块 🚀"));
    }

    #[test]
    fn test_parse_multiple_dependencies() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
            )
            (#dependencies
                (#std "^1.0")
                (#core "~2.0")
                (#local (#path "../local"))
                (#remote (#git "https://example.com/repo" #tag "v1.0"))
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.dependencies().len(), 4);
        assert!(pkg.has_dependency("std"));
        assert!(pkg.has_dependency("core"));
        assert!(pkg.has_dependency("local"));
        assert!(pkg.has_dependency("remote"));
    }

    #[test]
    fn test_parse_single_author() {
        let content = r#"
            (#package
                (#name "test")
                (#version "1.0.0")
                (#authors "Single Author")
            )
        "#;

        let pkg = parse_pkg_info_metta(content).expect("valid package info");
        assert_eq!(pkg.package.authors, vec!["Single Author"]);
    }
}
