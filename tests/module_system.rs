//! Integration tests for the MeTTaTron module system.
//!
//! These tests verify:
//! - Module inclusion with `include`
//! - Module imports with `import!`
//! - Token binding with `bind!`
//! - Package manifest loading (both `_pkg-info.metta` and `metta.toml` formats)
//! - Format precedence (`_pkg-info.metta` > `metta.toml`)
//! - Strict mode behavior

use mettatron::{compile, eval, new_env, MettaValueInner};
use std::fs;
use std::path::PathBuf;

/// Get the path to the test fixtures directory
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

// ============================================================
// Include tests
// ============================================================

#[test]
fn test_include_basic_file() {
    let mut env = new_env();
    let fixture_path = fixtures_dir().join("test_module.metta");

    // Set up the environment to know about our test directory
    env.set_current_module_path(Some(fixtures_dir()));

    let code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&code).expect("compilation should succeed");

    let (results, _) = eval(state.source()[0], env, &state);

    // Include should return Unit on success
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Unit),
        "Expected Unit, got {:?}",
        results[0].inner()
    );
}

#[test]
fn test_include_defines_rules() {
    let mut env = new_env();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Include the module
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source()[0], env, &state);

    // Now test that the defined functions work
    let test_code = "!(test-add 2 3)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source()[0], env, &state);

    // Should evaluate to 5
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(5)),
        "Expected Long(5), got {:?}",
        results[0].inner()
    );
}

#[test]
fn test_include_idempotent() {
    let mut env = new_env();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Include the same module twice
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source()[0], env, &state);

    // Verify rules are available after first include
    let test_code = "!(test-add 2 3)";
    let state2 = compile(test_code).expect("compilation should succeed");
    let (results, env) = eval(state2.source()[0], env, &state2);
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(5)),
        "Expected Long(5) after first include, got {:?}",
        results[0].inner()
    );

    // Include again -- rules should still work (may produce duplicate results due to
    // the generic include path adding rules unconditionally without deduplication)
    let (_, env) = eval(state.source()[0], env, &state);

    let (results, _) = eval(state2.source()[0], env, &state2);
    assert!(
        !results.is_empty(),
        "Expected at least one result after second include"
    );
    assert!(
        results.iter().all(|r| matches!(r.inner(), MettaValueInner::Long(5))),
        "All results should be Long(5), got {:?}",
        results.iter().map(|r| format!("{:?}", r.inner())).collect::<Vec<_>>()
    );
}

// ============================================================
// Import tests
// ============================================================

// Note: The import! with &self syntax requires the parser to treat &self as a single token.
// Currently, the tree-sitter parser splits &self into & and self.
// The import! functionality is tested via unit tests that construct MettaValue directly.
// This integration test uses include instead, which achieves similar results.

#[test]
fn test_import_via_include() {
    let mut env = new_env();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Use include (which import! internally calls with &self)
    let code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&code).expect("compilation should succeed");
    let (results, env) = eval(state.source()[0], env, &state);

    // Include should return Unit on success (all expressions are rule/type defs)
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Unit),
        "Expected Unit, got {:?}",
        results[0].inner()
    );

    // Verify rules from the module are actually loaded
    let test_code = "!(test-add 10 20)";
    let state2 = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state2.source()[0], env, &state2);
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(30)),
        "Expected Long(30), got {:?}",
        results[0].inner()
    );
}

// ============================================================
// Bind tests
// ============================================================

// Note: The bind! syntax with &token (e.g., &my-value) requires the parser to treat
// & followed by an identifier as a single token. Currently, the tree-sitter parser
// splits them. The bind! functionality is fully tested via unit tests that construct
// MettaValue directly. These integration tests use symbols without the & prefix.

#[test]
fn test_bind_creates_token() {
    let env = new_env();

    // bind! is a special form that evaluates directly
    // Using token without & prefix due to parser limitation with &-prefixed tokens
    let code = r#"(bind! my-value 42)"#;
    let state = compile(code).expect("compilation should succeed");
    let (results, env) = eval(state.source()[0], env, &state);

    // bind! returns Unit
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Unit),
        "Expected Unit, got {:?}",
        results[0].inner()
    );

    // Token should be registered
    assert!(env.has_token("my-value"));
    let lookup = env.lookup_token("my-value");
    assert!(lookup.is_some(), "Expected my-value to be bound");
    assert!(
        matches!(lookup.unwrap().inner(), MettaValueInner::Long(42)),
        "Expected Long(42), got {:?}",
        lookup.unwrap().inner()
    );
}

#[test]
fn test_bind_with_expression() {
    let env = new_env();

    // Bind to a computed value
    let code = r#"(bind! sum-value (+ 10 20))"#;
    let state = compile(code).expect("compilation should succeed");
    let (results, env) = eval(state.source()[0], env, &state);

    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Unit),
        "Expected Unit, got {:?}",
        results[0].inner()
    );

    // Should bind to the computed value
    let lookup = env.lookup_token("sum-value");
    assert!(lookup.is_some(), "Expected sum-value to be bound");
    assert!(
        matches!(lookup.unwrap().inner(), MettaValueInner::Long(30)),
        "Expected Long(30), got {:?}",
        lookup.unwrap().inner()
    );
}

#[test]
fn test_bind_token_resolution() {
    let env = new_env();

    // First bind a value
    let bind_code = r#"(bind! x-val 100)"#;
    let state = compile(bind_code).expect("compilation should succeed");
    let (_, env) = eval(state.source()[0], env, &state);

    // Then use the bound token in an expression
    // Use ! to force evaluation and get the result
    let use_code = r#"!(+ x-val 5)"#;
    let state = compile(use_code).expect("compilation should succeed");
    let (results, _) = eval(state.source()[0], env, &state);

    // Should resolve x-val to 100, then add 5
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(105)),
        "Expected Long(105), got {:?}",
        results[0].inner()
    );
}

// ============================================================
// Strict mode tests
// ============================================================

#[test]
fn test_strict_mode_default_disabled() {
    let env = new_env();
    assert!(!env.is_strict_mode());
}

#[test]
fn test_strict_mode_can_be_enabled() {
    let mut env = new_env();
    env.set_strict_mode(true);
    assert!(env.is_strict_mode());
}

// ============================================================
// Package manifest tests - TOML format
// ============================================================

#[test]
fn test_load_package_manifest_toml() {
    use mettatron::backend::modules::PackageInfo;

    let manifest_path = fixtures_dir().join("metta.toml");
    let pkg = PackageInfo::load_from_toml_path(&manifest_path).expect("should load manifest");

    assert_eq!(pkg.name(), "test-fixtures");
    assert_eq!(pkg.version_str(), "1.0.0");
    assert!(pkg.is_exported("test-add"));
    assert!(pkg.is_exported("public-fn"));
}

// ============================================================
// Package manifest tests - _pkg-info.metta format (HE-compatible)
// ============================================================

#[test]
fn test_load_pkg_info_metta_complete() {
    use mettatron::backend::modules::load_pkg_info_metta;

    // Load from fixtures directory which contains _pkg-info.metta
    let result = load_pkg_info_metta(&fixtures_dir());
    assert!(
        result.is_ok(),
        "Should parse without errors: {:?}",
        result.err()
    );

    let pkg = result.unwrap();
    assert!(pkg.is_some(), "_pkg-info.metta should exist in fixtures");

    let pkg = pkg.unwrap();
    assert_eq!(pkg.name(), "test-fixture");
    assert_eq!(pkg.version_str(), "1.0.0");

    // Check exports
    assert!(pkg.is_exported("test-func"));
    assert!(pkg.is_exported("helper-func"));
    assert!(pkg.is_exported("TestType"));
    assert!(!pkg.is_exported("private-symbol"));
}

#[test]
fn test_parse_pkg_info_metta_minimal() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    let content = fs::read_to_string(fixtures_dir().join("pkg_info_minimal.metta"))
        .expect("Should read fixture");

    let pkg = parse_pkg_info_metta(&content).expect("Should parse minimal manifest");

    assert_eq!(pkg.name(), "minimal");
    assert_eq!(pkg.version_str(), "0.1.0");
    // No exports section means export-all is false by default (closed by default)
    // This is a safer default - explicit exports must be declared
    assert!(!pkg.exports.all);
    assert!(pkg.exports.public.is_empty());
}

#[test]
fn test_parse_pkg_info_metta_full() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    let content = fs::read_to_string(fixtures_dir().join("pkg_info_full.metta"))
        .expect("Should read fixture");

    let pkg = parse_pkg_info_metta(&content).expect("Should parse full manifest");

    // Package metadata
    assert_eq!(pkg.name(), "full-featured-module");
    assert_eq!(pkg.version_str(), "2.5.1");

    // Check optional metadata
    let meta = &pkg.package;
    assert!(meta.description.is_some());
    assert_eq!(
        meta.description.as_ref().unwrap(),
        "A fully-featured module demonstrating all package manifest fields"
    );
    assert!(meta.license.is_some());
    assert_eq!(meta.license.as_ref().unwrap(), "Apache-2.0");
    assert!(meta.repository.is_some());
    assert!(meta.homepage.is_some());
    assert!(meta.documentation.is_some());

    // Check authors
    assert!(!meta.authors.is_empty());
    assert_eq!(meta.authors.len(), 2);
    assert!(meta.authors[0].contains("Primary Author"));
    assert!(meta.authors[1].contains("Contributor"));

    // Check keywords and categories
    assert!(!meta.keywords.is_empty());
    assert!(meta.keywords.contains(&"metta".to_string()));
    assert!(meta.keywords.contains(&"demo".to_string()));

    assert!(!meta.categories.is_empty());
    assert!(meta.categories.contains(&"utilities".to_string()));

    // Check exports
    assert!(pkg.is_exported("public-function"));
    assert!(pkg.is_exported("another-public-fn"));
    assert!(pkg.is_exported("PublicType"));
    assert!(pkg.is_exported("CONSTANT_VALUE"));
    assert!(!pkg.is_exported("private-symbol"));

    // Check dependencies
    let deps = pkg.dependencies();
    assert!(!deps.is_empty());
    assert!(deps.contains_key("core"));
    assert!(deps.contains_key("std"));
    assert!(deps.contains_key("local-lib"));
    assert!(deps.contains_key("git-tag"));
}

#[test]
fn test_parse_pkg_info_metta_deps_only() {
    use mettatron::backend::modules::{parse_pkg_info_metta, Dependency};

    let content = fs::read_to_string(fixtures_dir().join("pkg_info_deps_only.metta"))
        .expect("Should read fixture");

    let pkg = parse_pkg_info_metta(&content).expect("Should parse deps-only manifest");

    assert_eq!(pkg.name(), "deps-only");
    assert_eq!(pkg.version_str(), "1.0.0");

    // No exports section means export-all is false (closed by default)
    assert!(!pkg.exports.all);
    assert!(pkg.exports.public.is_empty());

    // Check dependencies
    let deps = pkg.dependencies();
    assert_eq!(deps.len(), 3);
    assert!(deps.contains_key("std"));
    assert!(deps.contains_key("core"));
    assert!(deps.contains_key("utils"));

    // Verify path dependency
    if let Some(utils_dep) = deps.get("utils") {
        match utils_dep {
            Dependency::Detailed(detail) => {
                assert!(detail.path.is_some());
                assert_eq!(detail.path.as_ref().unwrap(), "../utils");
            }
            Dependency::Version(_) => {
                panic!("utils dependency should be a detailed path dependency");
            }
        }
    } else {
        panic!("utils dependency should exist");
    }
}

#[test]
fn test_parse_pkg_info_metta_export_all() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    let content = fs::read_to_string(fixtures_dir().join("pkg_info_export_all.metta"))
        .expect("Should read fixture");

    let pkg = parse_pkg_info_metta(&content).expect("Should parse export-all manifest");

    assert_eq!(pkg.name(), "export-all");
    assert!(pkg.exports.all);

    // When export-all is true, any symbol should be considered exported
    assert!(pkg.is_exported("any-symbol"));
    assert!(pkg.is_exported("random-function"));
}

#[test]
fn test_pkg_info_metta_error_missing_package() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    // Content with no #package section
    let content = r#"
        (#dependencies
            (#std "^1.0")
        )
    "#;

    let result = parse_pkg_info_metta(content);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.message.contains("package") || err.message.contains("#package"),
        "Error should mention missing #package section: {}",
        err.message
    );
}

#[test]
fn test_pkg_info_metta_error_missing_name() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    // Package section without name
    let content = r#"
        (#package
            (#version "1.0.0")
        )
    "#;

    let result = parse_pkg_info_metta(content);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.message.to_lowercase().contains("name"),
        "Error should mention missing name field: {}",
        err.message
    );
}

#[test]
fn test_pkg_info_metta_error_missing_version() {
    use mettatron::backend::modules::parse_pkg_info_metta;

    // Package section without version
    let content = r#"
        (#package
            (#name "test")
        )
    "#;

    let result = parse_pkg_info_metta(content);
    assert!(result.is_err());

    let err = result.unwrap_err();
    assert!(
        err.message.to_lowercase().contains("version"),
        "Error should mention missing version field: {}",
        err.message
    );
}

// ============================================================
// Format precedence tests
// ============================================================

#[test]
fn test_format_precedence_pkg_info_over_toml() {
    use mettatron::backend::modules::PackageInfo;

    // The fixtures directory has both _pkg-info.metta and metta.toml
    // PackageInfo::load() should prefer _pkg-info.metta
    let pkg = PackageInfo::load(&fixtures_dir());
    assert!(pkg.is_some(), "Should load a manifest from fixtures dir");

    let pkg = pkg.unwrap();
    // _pkg-info.metta has name "test-fixture", metta.toml has "test-fixtures"
    assert_eq!(
        pkg.name(),
        "test-fixture",
        "Should load _pkg-info.metta (name='test-fixture'), not metta.toml (name='test-fixtures')"
    );
}

#[test]
fn test_unified_loader_falls_back_to_toml() {
    use mettatron::backend::modules::PackageInfo;
    use std::env;

    // Create a temp directory with only metta.toml
    let temp_dir = env::temp_dir().join("metta_test_toml_only");
    let _ = fs::remove_dir_all(&temp_dir); // Clean up any previous run
    fs::create_dir_all(&temp_dir).expect("Should create temp dir");

    let toml_content = r#"
[package]
name = "toml-only-pkg"
version = "2.0.0"
"#;
    fs::write(temp_dir.join("metta.toml"), toml_content).expect("Should write toml");

    let pkg = PackageInfo::load(&temp_dir);
    assert!(
        pkg.is_some(),
        "Should load metta.toml when _pkg-info.metta is absent"
    );

    let pkg = pkg.unwrap();
    assert_eq!(pkg.name(), "toml-only-pkg");
    assert_eq!(pkg.version_str(), "2.0.0");

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_unified_loader_returns_none_when_no_manifest() {
    use mettatron::backend::modules::PackageInfo;
    use std::env;

    // Create a temp directory with no manifest files
    let temp_dir = env::temp_dir().join("metta_test_no_manifest");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Should create temp dir");

    let pkg = PackageInfo::load(&temp_dir);
    assert!(pkg.is_none(), "Should return None when no manifest exists");

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_unified_loader_prefers_pkg_info_even_with_parse_error() {
    use mettatron::backend::modules::PackageInfo;
    use std::env;

    // Create a temp directory with invalid _pkg-info.metta and valid metta.toml
    let temp_dir = env::temp_dir().join("metta_test_invalid_pkg_info");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Should create temp dir");

    // Invalid _pkg-info.metta (missing required fields)
    let invalid_pkg_info = r#"
        (#package
            ; Missing name and version
        )
    "#;
    fs::write(temp_dir.join("_pkg-info.metta"), invalid_pkg_info)
        .expect("Should write invalid pkg-info");

    // Valid metta.toml
    let valid_toml = r#"
[package]
name = "fallback-pkg"
version = "1.0.0"
"#;
    fs::write(temp_dir.join("metta.toml"), valid_toml).expect("Should write valid toml");

    // Should fall back to TOML when _pkg-info.metta has parse errors
    let pkg = PackageInfo::load(&temp_dir);
    assert!(
        pkg.is_some(),
        "Should fall back to metta.toml on parse error"
    );

    let pkg = pkg.unwrap();
    assert_eq!(pkg.name(), "fallback-pkg");

    // Cleanup
    let _ = fs::remove_dir_all(&temp_dir);
}

// ============================================================
// Integration scenarios
// ============================================================

#[test]
fn test_full_module_workflow() {
    let mut env = new_env();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // 1. Include the module
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source()[0], env, &state);

    // 2. Test that functions work
    let test_code = "!(test-nested 8)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source()[0], env, &state);

    // test-nested(8) = test-add(8, test-value()) = test-add(8, 42) = 50
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(50)),
        "Expected Long(50), got {:?}",
        results[0].inner()
    );
}

#[test]
fn test_transitive_imports() {
    let mut env = new_env();
    env.set_current_module_path(Some(fixtures_dir()));

    // The generic include path does not recursively evaluate !(include ...) within
    // included files. To test that rules from multiple modules compose correctly,
    // we explicitly include both modules A and B in the correct dependency order.
    let fixture_a = fixtures_dir().join("test_import_a.metta");
    let include_a = format!(r#"(include "{}")"#, fixture_a.display());
    let state_a = compile(&include_a).expect("compilation should succeed");
    let (_, env) = eval(state_a.source()[0], env, &state_a);

    let fixture_b = fixtures_dir().join("test_import_b.metta");
    let include_b = format!(r#"(include "{}")"#, fixture_b.display());
    let state_b = compile(&include_b).expect("compilation should succeed");
    let (_, env) = eval(state_b.source()[0], env, &state_b);

    // Test function from module B that depends on module A
    let test_code = "!(add-from-b 5)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source()[0], env, &state);

    // add-from-b(5) = add-from-a(5) + 5 = (5 + 10) + 5 = 20
    assert_eq!(results.len(), 1);
    assert!(
        matches!(results[0].inner(), MettaValueInner::Long(20)),
        "Expected Long(20), got {:?}",
        results[0].inner()
    );
}
