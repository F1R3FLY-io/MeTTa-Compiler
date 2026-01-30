use crate::backend::compile::compile;
use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner, Rule};
use crate::backend::modules::{hash_content, resolve_module_path};

use super::eval;
use super::EvalStep;

// ============================================================
// Module Operations (include)
// ============================================================

/// Step version of eval_include that defers expression evaluation to trampoline.
/// This prevents stack overflow for deeply nested code in included files.
pub(crate) fn eval_include_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() != 2 {
        let err = MettaValue::Error(
            format!(
                "include requires exactly 1 argument, got {}. Usage: (include path)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let path_arg = &items[1];

    // Get the path string
    let path_str = match path_arg.inner() {
        MettaValueInner::String(s) => s.clone(),
        MettaValueInner::Atom(s) => s.clone(),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "include: expected string or symbol path, got {}",
                    super::friendly_type_name(path_arg)
                ),
                path_arg.clone(),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Resolve path using module path notation (self:, top:, bare names)
    let resolved_path = resolve_module_path(&path_str, env.current_module_dir());

    // Check if already cached by path
    if let Some(_mod_id) = env.get_module_by_path(&resolved_path) {
        // Module already loaded - just return Unit
        return EvalStep::Done((vec![MettaValue::Unit()], env));
    }

    // Read the file contents
    let contents = match std::fs::read_to_string(&resolved_path) {
        Ok(c) => c,
        Err(e) => {
            let err = MettaValue::Error(
                format!(
                    "include: failed to read file '{}': {}",
                    resolved_path.display(),
                    e
                ),
                MettaValue::Atom(path_str),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Check content hash for deduplication
    let content_hash = hash_content(&contents);

    // Check if already loading (cycle detection)
    if env.is_module_loading(content_hash) {
        // Cycle detected - rules already indexed, return Unit
        return EvalStep::Done((vec![MettaValue::Unit()], env));
    }

    // Check if same content already loaded at different path
    if let Some(mod_id) = env.get_module_by_content(content_hash) {
        env.add_module_path_alias(&resolved_path, mod_id);
        return EvalStep::Done((vec![MettaValue::Unit()], env));
    }

    // Mark as loading (for cycle detection)
    env.mark_module_loading(content_hash);

    // Compile the file contents to MettaValue expressions
    let state = match compile(&contents) {
        Ok(s) => s,
        Err(e) => {
            env.unmark_module_loading(content_hash);
            let err = MettaValue::Error(
                format!(
                    "include: failed to parse file '{}': {}",
                    resolved_path.display(),
                    e
                ),
                MettaValue::Atom(path_str),
            );
            return EvalStep::Done((vec![err], env));
        }
    };
    let expressions = state.source;

    // === PASS 1: Index rules (extract and register without evaluating RHS) ===
    let mut rules_to_add = Vec::new();
    let mut expressions_to_eval = Vec::new();

    for expr in expressions {
        if let MettaValueInner::SExpr(ref sexpr_items) = expr.inner() {
            if sexpr_items.len() == 3 {
                if let MettaValueInner::Atom(ref op) = sexpr_items[0].inner() {
                    if op == "=" {
                        let rule = Rule::new(sexpr_items[1].clone(), sexpr_items[2].clone());
                        rules_to_add.push(rule);
                        continue;
                    }
                    if op == ":" {
                        // Type declarations don't produce output
                        continue;
                    }
                }
            }
        }
        expressions_to_eval.push(expr);
    }

    // Add all rules at once (PASS 1 completion)
    let mut current_env = env;
    if !rules_to_add.is_empty() {
        if let Err(e) = current_env.add_rules_bulk(rules_to_add) {
            current_env.unmark_module_loading(content_hash);
            let err = MettaValue::Error(
                format!("include: failed to add rules: {}", e),
                MettaValue::Atom(path_str),
            );
            return EvalStep::Done((vec![err], current_env));
        }
    }

    // Register the module in the registry
    let mod_path = path_str.replace('/', ":").replace(".metta", "");
    let resource_dir = resolved_path.parent().map(|p| p.to_path_buf());
    let _mod_id =
        current_env.register_module(mod_path, &resolved_path, content_hash, resource_dir.clone());

    // Update current module path for nested includes
    let prev_module_path = current_env.current_module_dir().map(|p| p.to_path_buf());
    current_env.set_current_module_path(resource_dir);

    // If no expressions to evaluate, return Unit immediately
    if expressions_to_eval.is_empty() {
        current_env.set_current_module_path(prev_module_path);
        current_env.unmark_module_loading(content_hash);
        return EvalStep::Done((vec![MettaValue::Unit()], current_env));
    }

    // === PASS 2: Return StartInclude to evaluate expressions via trampoline ===
    EvalStep::StartInclude {
        expressions: expressions_to_eval,
        prev_module_path,
        content_hash,
        env: current_env,
        depth,
    }
}

/// Step version of import! that defers evaluation to trampoline.
/// This prevents stack overflow when importing modules with deeply nested code.
///
/// Usage:
///   (import! &self module-path)                    - Import all into current space
///   (import! alias module-path)                    - Import with alias (namespaced access)
///   (import! &self module-path item)               - Import specific item from module
///   (import! &self module-path item as new-name)   - Import specific item with alias
pub(crate) fn eval_import_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    // (import! dest module [item [as alias]] [options...])
    if items.len() < 3 {
        let err = MettaValue::Error(
            "import!: expected at least 2 arguments. Usage: (import! dest module [item [as alias]])".to_string(),
            MettaValue::Nil(),
        );
        return EvalStep::Done((vec![err], env));
    }

    let dest = items[1].clone();
    let module_arg = items[2].clone();

    // Check for selective import: (import! &self module item [as alias])
    // item is at index 3, "as" at index 4, alias at index 5
    let selective_import: Option<(String, Option<String>)> = if items.len() >= 4 {
        let potential_item = &items[3];
        // Check if it's an option (starts with :) or an item to import
        match potential_item.inner() {
            MettaValueInner::Atom(name) if !name.starts_with(':') => {
                // Check for "as alias" syntax
                let alias: Option<String> = if items.len() >= 6 {
                    match (items[4].inner(), items[5].inner()) {
                        (MettaValueInner::Atom(as_kw), MettaValueInner::Atom(alias_name))
                            if as_kw == "as" =>
                        {
                            Some(alias_name.clone())
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                Some((name.clone(), alias))
            }
            _ => None,
        }
    } else {
        None
    };

    // Get module path string (validate format)
    match module_arg.inner() {
        MettaValueInner::String(_) | MettaValueInner::Atom(_) => {}
        _ => {
            let err = MettaValue::Error(
                format!(
                    "import!: expected string or symbol for module path, got {}",
                    super::friendly_type_name(&module_arg)
                ),
                module_arg.clone(),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    // Check destination format
    match dest.inner() {
        MettaValueInner::Atom(_) => {}
        _ => {
            let err = MettaValue::Error(
                format!(
                    "import!: destination must be &self or a symbol alias, got {}",
                    super::friendly_type_name(&dest)
                ),
                dest.clone(),
            );
            return EvalStep::Done((vec![err], env));
        }
    }

    // Return StartImport to defer include and selective import to trampoline
    EvalStep::StartImport {
        module_arg,
        dest,
        selective_import,
        env,
        depth,
    }
}

/// mod-space!: Get a module's space (for direct querying)
/// Usage: (mod-space! module-path)
///
/// Returns a Space value that can be used with match, get-atoms, add-atom, etc.
/// The returned space provides a live reference - mutations are immediately visible.
///
/// Example:
/// ```metta
/// !(let $s (mod-space! "mymodule.metta")
///     (match $s (person $name) $name))
/// ```
pub(super) fn eval_mod_space(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    use crate::backend::models::SpaceHandle;

    require_args_with_usage!("mod-space!", items, 1, env, "(mod-space! module-path)");

    let module_arg = &items[1];

    // Get module path string
    let module_path_str = match module_arg.inner() {
        MettaValueInner::String(s) => s.clone(),
        MettaValueInner::Atom(s) => s.clone(),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "mod-space!: expected string or symbol for module path, got {}",
                    super::friendly_type_name(module_arg)
                ),
                module_arg.clone(),
            );
            return (vec![err], env);
        }
    };

    // Resolve the path
    let resolved_path = resolve_module_path(&module_path_str, env.current_module_dir());

    // Helper to create Space from module
    let create_space = |mod_id, module_path: &str, env: &Environment| -> Option<MettaValue> {
        env.get_module_space(mod_id).map(|space| {
            let handle = SpaceHandle::for_module(mod_id, module_path.to_string(), space);
            MettaValue::Space(handle)
        })
    };

    // Check if module is loaded
    if let Some(mod_id) = env.get_module_by_path(&resolved_path) {
        // Module is loaded - return a Space reference
        if let Some(space_value) = create_space(mod_id, &module_path_str, &env) {
            (vec![space_value], env)
        } else {
            let err = MettaValue::Error(
                format!(
                    "mod-space!: module '{}' exists but space not accessible",
                    module_path_str
                ),
                MettaValue::Atom(module_path_str),
            );
            (vec![err], env)
        }
    } else {
        // Module not loaded - try to load it first
        let include_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("include".to_string()),
            module_arg.clone(),
        ]);
        let (_, new_env) = eval(include_expr, env);

        // Check again
        if let Some(mod_id) = new_env.get_module_by_path(&resolved_path) {
            if let Some(space_value) = create_space(mod_id, &module_path_str, &new_env) {
                (vec![space_value], new_env)
            } else {
                let err = MettaValue::Error(
                    format!(
                        "mod-space!: module '{}' loaded but space not accessible",
                        module_path_str
                    ),
                    MettaValue::Atom(module_path_str),
                );
                (vec![err], new_env)
            }
        } else {
            let err = MettaValue::Error(
                format!("mod-space!: failed to load module '{}'", module_path_str),
                MettaValue::Atom(module_path_str),
            );
            (vec![err], new_env)
        }
    }
}

/// print-mods!: Print all loaded modules (debug utility)
/// Usage: (print-mods!)
/// Returns Unit
pub(super) fn eval_print_mods(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // No arguments required
    if items.len() > 1 {
        let err = MettaValue::Error(
            "print-mods!: takes no arguments".to_string(),
            MettaValue::Nil(),
        );
        return (vec![err], env);
    }

    let count = env.module_count();
    println!("Loaded modules: {}", count);

    (vec![MettaValue::Unit()], env)
}

// ============================================================
// Token Binding Operations (bind!)
// ============================================================

/// Step version of bind! - defers evaluation to trampoline.
/// Usage: (bind! token atom)
pub(crate) fn eval_bind_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            format!(
                "bind! requires exactly 2 arguments, got {}. Usage: (bind! token atom)",
                items.len() - 1
            ),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let token = match items[1].inner() {
        MettaValueInner::Atom(s) => s.clone(),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "bind!: expected symbol for token, got {}",
                    super::friendly_type_name(&items[1])
                ),
                items[1].clone(),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    let atom_expr = items[2].clone();

    EvalStep::StartBind {
        token,
        atom_expr,
        env,
        depth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // All tests now use eval() which routes through the trampoline.
    // Tests for eval_include, eval_import, and eval_bind have been updated
    // to use eval(MettaValue::SExpr(items), env) to test the trampoline path.

    #[test]
    fn test_include_nonexistent_file() {
        let env = Environment::new();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::String("/nonexistent/path/file.metta".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_include_with_module_notation() {
        let env = Environment::new();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::Atom("nonexistent:module".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        // Should fail with file not found (the path is resolved but file doesn't exist)
        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_print_mods_no_modules() {
        let env = Environment::new();
        let items = vec![MettaValue::Atom("print-mods!".to_string())];

        let (results, env) = eval_print_mods(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
        assert_eq!(env.module_count(), 0);
    }

    // ============================================================
    // bind! tests
    // ============================================================

    #[test]
    fn test_bind_simple_value() {
        let env = Environment::new();
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&my-value".to_string()),
            MettaValue::Long(42),
        ]);

        let (results, new_env) = eval(expr, env);

        // bind! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());

        // Token should be registered
        assert!(new_env.has_token("&my-value"));
        assert_eq!(
            new_env.lookup_token("&my-value"),
            Some(MettaValue::Long(42))
        );
    }

    #[test]
    fn test_bind_atom_resolution() {
        let env = Environment::new();

        // First, bind a value
        let bind_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&answer".to_string()),
            MettaValue::Long(42),
        ]);
        let (_, env_with_binding) = eval(bind_expr, env);

        // Now, evaluate the bound atom - it should resolve to 42
        let atom = MettaValue::Atom("&answer".to_string());
        let (results, _) = eval(atom, env_with_binding);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_bind_with_expression() {
        let env = Environment::new();

        // bind! with an expression that gets evaluated: (bind! &sum (+ 2 3))
        let bind_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&sum".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        let (results, new_env) = eval(bind_expr, env);

        // bind! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());

        // Token should resolve to the evaluated result (5)
        assert_eq!(new_env.lookup_token("&sum"), Some(MettaValue::Long(5)));

        // Evaluating the atom should also return 5
        let atom = MettaValue::Atom("&sum".to_string());
        let (eval_results, _) = eval(atom, new_env);
        assert_eq!(eval_results.len(), 1);
        assert_eq!(eval_results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_bind_error_non_symbol() {
        let env = Environment::new();

        // Try to bind with a non-symbol token
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Long(42), // Not a symbol!
            MettaValue::Long(100),
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("expected symbol for token"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_bind_shadowing() {
        let env = Environment::new();

        // Bind &x to 1
        let bind1 = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&x".to_string()),
            MettaValue::Long(1),
        ]);
        let (_, env1) = eval(bind1, env);

        // Bind &x to 2 (shadows previous)
        let bind2 = MettaValue::SExpr(vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&x".to_string()),
            MettaValue::Long(2),
        ]);
        let (_, env2) = eval(bind2, env1);

        // Should resolve to the most recent binding
        assert_eq!(env2.lookup_token("&x"), Some(MettaValue::Long(2)));
    }

    // ============================================================
    // import! tests
    // ============================================================

    #[test]
    fn test_import_missing_args() {
        let env = Environment::new();

        // Only one argument - missing module path
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("expected at least 2 arguments"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_invalid_destination() {
        let env = Environment::new();

        // Invalid destination type
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Long(42), // Not a valid destination
            MettaValue::String("module.metta".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("destination must be &self or a symbol alias"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_invalid_module_path() {
        let env = Environment::new();

        // Invalid module path type (Long instead of String/Atom)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::Long(42), // Not a valid path
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("expected string or symbol for module path"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_nonexistent_module() {
        let env = Environment::new();

        // Try to import a module that doesn't exist
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_with_alias_destination() {
        let env = Environment::new();

        // Import with alias - should fail since module doesn't exist
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("my-module".to_string()), // Alias destination
            MettaValue::String("/nonexistent/module.metta".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        // Should fail with file not found
        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error for nonexistent file"),
        }
    }

    #[test]
    fn test_import_selective_item_not_found() {
        // This tests the selective import path - trying to import a specific item
        // Since we can't create real files in unit tests, we test the error handling
        let env = Environment::new();

        // Try selective import (import! &self module item)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
            MettaValue::Atom("some-function".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        // Should fail with file not found (can't test item lookup without real file)
        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_selective_with_as_alias() {
        // Test selective import with "as" syntax
        let env = Environment::new();

        // (import! &self module item as new-name)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
            MettaValue::Atom("original-name".to_string()),
            MettaValue::Atom("as".to_string()),
            MettaValue::Atom("new-name".to_string()),
        ]);

        let (results, _) = eval(expr, env);

        // Should fail with file not found
        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    // ============================================================
    // mod-space! tests
    // ============================================================

    #[test]
    fn test_mod_space_missing_args() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("mod-space!".to_string())];

        let (results, _) = eval_mod_space(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_mod_space_invalid_path_type() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("mod-space!".to_string()),
            MettaValue::Long(42), // Not a valid path
        ];

        let (results, _) = eval_mod_space(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("expected string or symbol for module path"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_mod_space_nonexistent_module() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("mod-space!".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
        ];

        let (results, _) = eval_mod_space(items, env);

        // Should fail because module doesn't exist
        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("failed to load module") || msg.contains("failed to read"));
            }
            _ => panic!("Expected error"),
        }
    }

    // ============================================================
    // print-mods! tests
    // ============================================================

    #[test]
    fn test_print_mods_with_extra_args() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("print-mods!".to_string()),
            MettaValue::Long(42), // Extra unwanted argument
        ];

        let (results, _) = eval_print_mods(items, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("takes no arguments"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_print_mods_returns_unit() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("print-mods!".to_string())];

        let (results, _) = eval_print_mods(items, env);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit());
    }

    // ============================================================
    // include tests (additional)
    // ============================================================

    #[test]
    fn test_include_missing_args() {
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![MettaValue::Atom("include".to_string())]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_include_invalid_path_type() {
        let env = Environment::new();

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::Long(42), // Not a valid path
        ]);

        let (results, _) = eval(expr, env);

        assert_eq!(results.len(), 1);
        match results[0].inner() {
            MettaValueInner::Error(msg, _) => {
                assert!(msg.contains("expected string or symbol path"));
            }
            _ => panic!("Expected error"),
        }
    }

    // ============================================================
    // Strict mode tests
    // ============================================================

    #[test]
    fn test_strict_mode_default_is_permissive() {
        let env = Environment::new();

        // Default should be permissive (not strict)
        assert!(!env.is_strict_mode());
    }

    #[test]
    fn test_strict_mode_can_be_enabled() {
        let mut env = Environment::new();
        env.set_strict_mode(true);

        assert!(env.is_strict_mode());
    }

    #[test]
    fn test_strict_mode_toggle() {
        let mut env = Environment::new();

        // Default is false
        assert!(!env.is_strict_mode());

        // Enable
        env.set_strict_mode(true);
        assert!(env.is_strict_mode());

        // Disable
        env.set_strict_mode(false);
        assert!(!env.is_strict_mode());
    }
}
