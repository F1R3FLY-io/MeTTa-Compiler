use std::sync::Arc;

use crate::backend::compile::compile;
use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, Rule};
use crate::backend::modules::{hash_content, resolve_module_path};

use super::eval;

// ============================================================
// Module Operations (include)
// ============================================================

/// include: Load and evaluate a MeTTa file
/// Usage: (include "path/to/file.metta") or (include path:to:module)
///
/// Features:
/// - Caching: Modules are only loaded once per unique content
/// - Relative paths: Uses current_module_path for `self:` notation
/// - Two-pass loading: Indexes rules before evaluation (handles cyclic deps)
///
/// Returns the result of the last expression
pub(super) fn eval_include(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("include", items, 1, env, "(include path)");

    let path_arg = &items[1];

    // Get the path string
    let path_str = match path_arg {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "include: expected string or symbol path, got {}",
                    super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
            );
            return (vec![err], env);
        }
    };

    // Resolve path using module path notation (self:, top:, bare names)
    let resolved_path = resolve_module_path(&path_str, env.current_module_dir());

    // Check if already cached by path
    if let Some(_mod_id) = env.get_module_by_path(&resolved_path) {
        // Module already loaded - just return Unit
        // (The rules are already in the environment from the first load)
        return (vec![MettaValue::Unit], env);
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
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], env);
        }
    };

    // Check content hash for deduplication
    let content_hash = hash_content(&contents);

    // Check if already loading (cycle detection)
    if env.is_module_loading(content_hash) {
        // Cycle detected - but this is OK because we use two-pass loading
        // The rules are already indexed (Pass 1), so forward references work
        // Just return Unit without re-evaluating
        return (vec![MettaValue::Unit], env);
    }

    // Check if same content already loaded at different path
    if let Some(mod_id) = env.get_module_by_content(content_hash) {
        // Content already loaded - add path alias and return
        env.add_module_path_alias(&resolved_path, mod_id);
        return (vec![MettaValue::Unit], env);
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
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], env);
        }
    };
    let expressions = state.source;

    // === PASS 1: Index rules (extract and register without evaluating RHS) ===
    let mut rules_to_add = Vec::new();
    let mut expressions_to_eval = Vec::new();

    for expr in expressions {
        // Check if it's a rule definition (= pattern body)
        if let MettaValue::SExpr(ref sexpr_items) = expr {
            if sexpr_items.len() == 3 {
                if let MettaValue::Atom(ref op) = sexpr_items[0] {
                    if op == "=" {
                        // Collect the rule for bulk addition
                        let rule = Rule {
                            lhs: sexpr_items[1].clone(),
                            rhs: sexpr_items[2].clone(),
                        };
                        rules_to_add.push(rule);
                        continue;
                    }
                    if op == ":" {
                        // Type declarations are stored but don't produce output
                        // For now, just continue (could index types here too)
                        continue;
                    }
                }
            }
        }

        // Collect expressions to evaluate in Pass 2
        expressions_to_eval.push(expr);
    }

    // Add all rules at once (PASS 1 completion)
    let mut current_env = env;
    if !rules_to_add.is_empty() {
        if let Err(e) = current_env.add_rules_bulk(rules_to_add) {
            current_env.unmark_module_loading(content_hash);
            let err = MettaValue::Error(
                format!("include: failed to add rules: {}", e),
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], current_env);
        }
    }

    // Register the module in the registry (after Pass 1, before Pass 2)
    let mod_path = path_str.replace('/', ":").replace(".metta", "");
    let resource_dir = resolved_path.parent().map(|p| p.to_path_buf());
    let _mod_id =
        current_env.register_module(mod_path, &resolved_path, content_hash, resource_dir.clone());

    // Update current module path for nested includes
    let prev_module_path = current_env.current_module_dir().map(|p| p.to_path_buf());
    current_env.set_current_module_path(resource_dir);

    // === PASS 2: Evaluate expressions ===
    let mut last_results = vec![MettaValue::Unit];
    for expr in expressions_to_eval {
        let (results, new_env) = eval(expr, current_env);
        current_env = new_env;
        if !results.is_empty() {
            last_results = results;
        }
    }

    // Restore previous module path
    current_env.set_current_module_path(prev_module_path);

    // Unmark as loading and mark as loaded
    current_env.unmark_module_loading(content_hash);

    // Mark module as fully loaded (if we had access to the module)
    // For now, the registry tracks this via the unmark_loading

    (last_results, current_env)
}

/// import!: Import a module with optional aliasing and selective imports
/// Usage:
///   (import! &self module-path)                    - Import all into current space
///   (import! alias module-path)                    - Import with alias (namespaced access)
///   (import! &self module-path item)               - Import specific item from module
///   (import! &self module-path item as new-name)   - Import specific item with alias
///   (import! &self module-path :no-transitive)    - Import without transitive deps
///
/// Selective imports work by:
/// 1. Loading the module (like include)
/// 2. Looking up the specified item in the module's rules
/// 3. Optionally renaming the item in the current space
///
/// Returns Unit on success
pub(super) fn eval_import(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // (import! dest module [item [as alias]] [options...])
    if items.len() < 3 {
        let err = MettaValue::Error(
            "import!: expected at least 2 arguments. Usage: (import! dest module [item [as alias]])".to_string(),
            Arc::new(MettaValue::Nil),
        );
        return (vec![err], env);
    }

    let dest = &items[1];
    let module_arg = &items[2];

    // Check for selective import: (import! &self module item [as alias])
    // item is at index 3, "as" at index 4, alias at index 5
    let selective_import = if items.len() >= 4 {
        let potential_item = &items[3];
        // Check if it's an option (starts with :) or an item to import
        match potential_item {
            MettaValue::Atom(name) if !name.starts_with(':') => {
                // Check for "as alias" syntax
                let alias = if items.len() >= 6 {
                    match (&items[4], &items[5]) {
                        (MettaValue::Atom(as_kw), MettaValue::Atom(alias_name))
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

    // Get module path string
    let _module_path_str = match module_arg {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "import!: expected string or symbol for module path, got {}",
                    super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
            );
            return (vec![err], env);
        }
    };

    // Check destination
    match dest {
        MettaValue::Atom(name) if name == "&self" => {
            // Import into current space
            // First, load the module
            let include_items = vec![MettaValue::Atom("include".to_string()), module_arg.clone()];
            let (results, new_env) = eval_include(include_items, env);

            // Check for errors
            if results.iter().any(|r| matches!(r, MettaValue::Error(_, _))) {
                return (results, new_env);
            }

            // Handle selective import if specified
            if let Some((item_name, alias)) = selective_import {
                // For selective imports, we've loaded the module and its rules are now
                // in the environment. If an alias is provided, we bind the item to
                // the new name in the tokenizer.
                if let Some(alias_name) = alias {
                    // Look up the item in the environment and bind with alias
                    // Since include adds all rules, we can lookup and re-bind with alias
                    let item_atom = MettaValue::Atom(item_name.clone());
                    let (lookup_results, mut final_env) = eval(item_atom, new_env);

                    if lookup_results.is_empty()
                        || lookup_results
                            .iter()
                            .any(|r| matches!(r, MettaValue::Error(_, _)))
                    {
                        // Item not found or error - return what we got
                        if lookup_results.is_empty() {
                            let err = MettaValue::Error(
                                format!("import!: item '{}' not found in module", item_name),
                                Arc::new(MettaValue::Atom(item_name)),
                            );
                            return (vec![err], final_env);
                        }
                        return (lookup_results, final_env);
                    }

                    // Bind the result to the alias
                    final_env.register_token(&alias_name, lookup_results[0].clone());
                    (vec![MettaValue::Unit], final_env)
                } else {
                    // No alias - selective import without renaming
                    // The item is already available from include, just return success
                    (vec![MettaValue::Unit], new_env)
                }
            } else {
                // Full import (no selective)
                (vec![MettaValue::Unit], new_env)
            }
        }
        MettaValue::Atom(_alias) => {
            // Import with alias - load module and bind alias to module reference
            let include_items = vec![MettaValue::Atom("include".to_string()), module_arg.clone()];
            let (results, new_env) = eval_include(include_items, env);

            // If successful, we could bind the module reference here
            if !results.iter().any(|r| matches!(r, MettaValue::Error(_, _))) {
                // Successfully loaded - in future, would bind alias to module's space
                (vec![MettaValue::Unit], new_env)
            } else {
                (results, new_env)
            }
        }
        other => {
            let err = MettaValue::Error(
                format!(
                    "import!: destination must be &self or a symbol alias, got {}",
                    super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env)
        }
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
    let module_path_str = match module_arg {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "mod-space!: expected string or symbol for module path, got {}",
                    super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
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
                Arc::new(MettaValue::Atom(module_path_str)),
            );
            (vec![err], env)
        }
    } else {
        // Module not loaded - try to load it first
        let include_items = vec![MettaValue::Atom("include".to_string()), module_arg.clone()];
        let (_, new_env) = eval_include(include_items, env);

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
                    Arc::new(MettaValue::Atom(module_path_str)),
                );
                (vec![err], new_env)
            }
        } else {
            let err = MettaValue::Error(
                format!("mod-space!: failed to load module '{}'", module_path_str),
                Arc::new(MettaValue::Atom(module_path_str)),
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
            Arc::new(MettaValue::Nil),
        );
        return (vec![err], env);
    }

    let count = env.module_count();
    println!("Loaded modules: {}", count);

    (vec![MettaValue::Unit], env)
}

// ============================================================
// Token Binding Operations (bind!)
// ============================================================

/// bind!: Register a token in the current module's tokenizer (HE-compatible)
/// Usage: (bind! token atom)
///
/// Registers a token which is replaced with an atom during evaluation.
/// This is an evaluation-time token substitution, similar to MeTTa HE's bind!.
///
/// When the token is subsequently encountered during evaluation, it will be
/// replaced with the bound atom value.
///
/// Example:
///   (bind! &kb (new-space))   ; Create a space and bind it to &kb
///   (add-atom &kb (foo bar))  ; &kb resolves to the space
///
/// Returns Unit on success
pub(super) fn eval_bind(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("bind!", items, 2, env, "(bind! token atom)");

    let token = match &items[1] {
        MettaValue::Atom(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "bind!: expected symbol for token, got {}",
                    super::friendly_type_name(other)
                ),
                Arc::new(other.clone()),
            );
            return (vec![err], env);
        }
    };

    // Evaluate the atom expression (like HE does)
    let (results, mut new_env) = eval(items[2].clone(), env);
    if results.is_empty() {
        return (
            vec![MettaValue::Error(
                "bind!: atom evaluated to empty".to_string(),
                Arc::new(items[2].clone()),
            )],
            new_env,
        );
    }

    // Check for errors in evaluation
    if let MettaValue::Error(_, _) = &results[0] {
        return (results, new_env);
    }

    let atom = results[0].clone();

    // Register in the tokenizer for subsequent atom resolution
    new_env.register_token(&token, atom);

    (vec![MettaValue::Unit], new_env)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_include_nonexistent_file() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::String("/nonexistent/path/file.metta".to_string()),
        ];

        let (results, _) = eval_include(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_include_with_module_notation() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::Atom("nonexistent:module".to_string()),
        ];

        let (results, _) = eval_include(items, env);

        // Should fail with file not found (the path is resolved but file doesn't exist)
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        assert_eq!(results[0], MettaValue::Unit);
        assert_eq!(env.module_count(), 0);
    }

    // ============================================================
    // bind! tests
    // ============================================================

    #[test]
    fn test_bind_simple_value() {
        let env = Environment::new();
        let items = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&my-value".to_string()),
            MettaValue::Long(42),
        ];

        let (results, new_env) = eval_bind(items, env);

        // bind! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);

        // Token should be registered
        assert!(new_env.has_token("&my-value"));
        assert_eq!(
            new_env.lookup_token("&my-value"),
            Some(MettaValue::Long(42))
        );
    }

    #[test]
    fn test_bind_atom_resolution() {
        use crate::backend::eval::eval;

        let env = Environment::new();

        // First, bind a value
        let bind_items = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&answer".to_string()),
            MettaValue::Long(42),
        ];
        let (_, env_with_binding) = eval_bind(bind_items, env);

        // Now, evaluate the bound atom - it should resolve to 42
        let atom = MettaValue::Atom("&answer".to_string());
        let (results, _) = eval(atom, env_with_binding);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_bind_with_expression() {
        use crate::backend::eval::eval;

        let env = Environment::new();

        // bind! with an expression that gets evaluated: (bind! &sum (+ 2 3))
        let bind_items = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&sum".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ];
        let (results, new_env) = eval_bind(bind_items, env);

        // bind! returns Unit
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Unit);

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
        let items = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Long(42), // Not a symbol!
            MettaValue::Long(100),
        ];

        let (results, _) = eval_bind(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("expected symbol for token"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_bind_shadowing() {
        let env = Environment::new();

        // Bind &x to 1
        let bind1 = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&x".to_string()),
            MettaValue::Long(1),
        ];
        let (_, env1) = eval_bind(bind1, env);

        // Bind &x to 2 (shadows previous)
        let bind2 = vec![
            MettaValue::Atom("bind!".to_string()),
            MettaValue::Atom("&x".to_string()),
            MettaValue::Long(2),
        ];
        let (_, env2) = eval_bind(bind2, env1);

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
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("expected at least 2 arguments"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_invalid_destination() {
        let env = Environment::new();

        // Invalid destination type
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Long(42), // Not a valid destination
            MettaValue::String("module.metta".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("destination must be &self or a symbol alias"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_invalid_module_path() {
        let env = Environment::new();

        // Invalid module path type (Long instead of String/Atom)
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::Long(42), // Not a valid path
        ];

        let (results, _) = eval_import(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("expected string or symbol for module path"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_nonexistent_module() {
        let env = Environment::new();

        // Try to import a module that doesn't exist
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("failed to read file"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_import_with_alias_destination() {
        let env = Environment::new();

        // Import with alias - should fail since module doesn't exist
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("my-module".to_string()), // Alias destination
            MettaValue::String("/nonexistent/module.metta".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        // Should fail with file not found
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
            MettaValue::Atom("some-function".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        // Should fail with file not found (can't test item lookup without real file)
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        let items = vec![
            MettaValue::Atom("import!".to_string()),
            MettaValue::Atom("&self".to_string()),
            MettaValue::String("/nonexistent/module.metta".to_string()),
            MettaValue::Atom("original-name".to_string()),
            MettaValue::Atom("as".to_string()),
            MettaValue::Atom("new-name".to_string()),
        ];

        let (results, _) = eval_import(items, env);

        // Should fail with file not found
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
        assert_eq!(results[0], MettaValue::Unit);
    }

    // ============================================================
    // include tests (additional)
    // ============================================================

    #[test]
    fn test_include_missing_args() {
        let env = Environment::new();

        let items = vec![MettaValue::Atom("include".to_string())];

        let (results, _) = eval_include(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("requires exactly 1 argument"));
            }
            _ => panic!("Expected error"),
        }
    }

    #[test]
    fn test_include_invalid_path_type() {
        let env = Environment::new();

        let items = vec![
            MettaValue::Atom("include".to_string()),
            MettaValue::Long(42), // Not a valid path
        ];

        let (results, _) = eval_include(items, env);

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
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
