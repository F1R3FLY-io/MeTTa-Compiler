//! Integration tests for the MeTTaTron module system.
//!
//! These tests verify:
//! - Module inclusion with `include`
//! - Module imports with `import!`
//! - Token binding with `bind!`
//! - Export control with `export!`
//! - Package manifest loading
//! - Strict mode behavior

use mettatron::{compile, eval, Environment, MettaValue};
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
    let mut env = Environment::new();
    let fixture_path = fixtures_dir().join("test_module.metta");

    // Set up the environment to know about our test directory
    env.set_current_module_path(Some(fixtures_dir()));

    let code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&code).expect("compilation should succeed");

    let (results, _) = eval(state.source[0].clone(), env);

    // Include should return Unit on success
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);
}

#[test]
fn test_include_defines_rules() {
    let mut env = Environment::new();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Include the module
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source[0].clone(), env);

    // Now test that the defined functions work
    let test_code = "!(test-add 2 3)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source[0].clone(), env);

    // Should evaluate to 5
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(5));
}

#[test]
fn test_include_caches_module() {
    let mut env = Environment::new();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Include the same module twice
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source[0].clone(), env);

    // Module count should be 1
    assert_eq!(env.module_count(), 1);

    // Include again
    let (_, env) = eval(state.source[0].clone(), env);

    // Module count should still be 1 (cached)
    assert_eq!(env.module_count(), 1);
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
    let mut env = Environment::new();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // Use include (which import! internally calls with &self)
    let code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&code).expect("compilation should succeed");
    let (results, env) = eval(state.source[0].clone(), env);

    // Include should return Unit on success
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);

    // Module should be loaded
    assert!(env.module_count() >= 1);
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
    let env = Environment::new();

    // bind! is a special form that evaluates directly
    // Using token without & prefix due to parser limitation with &-prefixed tokens
    let code = r#"(bind! my-value 42)"#;
    let state = compile(code).expect("compilation should succeed");
    let (results, env) = eval(state.source[0].clone(), env);

    // bind! returns Unit
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);

    // Token should be registered
    assert!(env.has_token("my-value"));
    assert_eq!(env.lookup_token("my-value"), Some(MettaValue::Long(42)));
}

#[test]
fn test_bind_with_expression() {
    let env = Environment::new();

    // Bind to a computed value
    let code = r#"(bind! sum-value (+ 10 20))"#;
    let state = compile(code).expect("compilation should succeed");
    let (results, env) = eval(state.source[0].clone(), env);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);

    // Should bind to the computed value
    assert_eq!(env.lookup_token("sum-value"), Some(MettaValue::Long(30)));
}

#[test]
fn test_bind_token_resolution() {
    let env = Environment::new();

    // First bind a value
    let bind_code = r#"(bind! x-val 100)"#;
    let state = compile(bind_code).expect("compilation should succeed");
    let (_, env) = eval(state.source[0].clone(), env);

    // Then use the bound token in an expression
    // Use ! to force evaluation and get the result
    let use_code = r#"!(+ x-val 5)"#;
    let state = compile(use_code).expect("compilation should succeed");
    let (results, _) = eval(state.source[0].clone(), env);

    // Should resolve x-val to 100, then add 5
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(105));
}

// ============================================================
// Export tests
// ============================================================

#[test]
fn test_export_marks_symbol() {
    let env = Environment::new();

    // export! is a special form (no ! prefix needed)
    let code = r#"(export! my-function)"#;
    let state = compile(code).expect("compilation should succeed");
    let (results, env) = eval(state.source[0].clone(), env);

    // export! returns Unit
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Unit);

    // Symbol should be marked as exported
    assert!(env.is_exported("my-function"));
}

#[test]
fn test_export_multiple_symbols() {
    let env = Environment::new();

    // export! is a special form (no ! prefix needed)
    let code = r#"
        (export! func1)
        (export! func2)
        (export! func3)
    "#;
    let state = compile(code).expect("compilation should succeed");

    let mut current_env = env;
    for expr in state.source {
        let (_, new_env) = eval(expr, current_env);
        current_env = new_env;
    }

    assert!(current_env.is_exported("func1"));
    assert!(current_env.is_exported("func2"));
    assert!(current_env.is_exported("func3"));
    assert!(!current_env.is_exported("func4"));
}

// ============================================================
// Strict mode tests
// ============================================================

#[test]
fn test_strict_mode_default_disabled() {
    let env = Environment::new();
    assert!(!env.is_strict_mode());
}

#[test]
fn test_strict_mode_can_be_enabled() {
    let mut env = Environment::new();
    env.set_strict_mode(true);
    assert!(env.is_strict_mode());
}

// ============================================================
// Package manifest tests
// ============================================================

#[test]
fn test_load_package_manifest() {
    use mettatron::backend::modules::PackageInfo;

    let manifest_path = fixtures_dir().join("metta.toml");
    let pkg = PackageInfo::load_from_path(&manifest_path).expect("should load manifest");

    assert_eq!(pkg.name(), "test-fixtures");
    assert_eq!(pkg.version_str(), "1.0.0");
    assert!(pkg.is_exported("test-add"));
    assert!(pkg.is_exported("public-fn"));
}

// ============================================================
// Integration scenarios
// ============================================================

#[test]
fn test_full_module_workflow() {
    let mut env = Environment::new();
    let fixture_path = fixtures_dir().join("test_module.metta");
    env.set_current_module_path(Some(fixtures_dir()));

    // 1. Include the module
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source[0].clone(), env);

    // 2. Test that functions work
    let test_code = "!(test-nested 8)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source[0].clone(), env);

    // test-nested(8) = test-add(8, test-value()) = test-add(8, 42) = 50
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(50));
}

#[test]
fn test_transitive_imports() {
    let mut env = Environment::new();
    env.set_current_module_path(Some(fixtures_dir()));

    // Include module B, which includes module A
    let fixture_path = fixtures_dir().join("test_import_b.metta");
    let include_code = format!(r#"(include "{}")"#, fixture_path.display());
    let state = compile(&include_code).expect("compilation should succeed");
    let (_, env) = eval(state.source[0].clone(), env);

    // Both modules should be loaded
    assert!(env.module_count() >= 2);

    // Test function from module B
    let test_code = "!(add-from-b 5)";
    let state = compile(test_code).expect("compilation should succeed");
    let (results, _) = eval(state.source[0].clone(), env);

    // add-from-b(5) = add-from-a(5) + 5 = (5 + 10) + 5 = 20
    assert_eq!(results.len(), 1);
    assert_eq!(results[0], MettaValue::Long(20));
}
