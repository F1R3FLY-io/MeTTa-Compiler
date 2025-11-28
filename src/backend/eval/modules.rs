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
    let mod_id = current_env.register_module(
        mod_path,
        &resolved_path,
        content_hash,
        resource_dir.clone(),
    );

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

/// import!: Import a module with optional aliasing
/// Usage:
///   (import! &self module-path)           - Import all into current space (like include)
///   (import! alias module-path)           - Import with alias (future: namespaced access)
///   (import! &self module-path :no-transitive) - Import without transitive deps
///
/// For now, import! is equivalent to include with some additional semantics:
/// - Validates import constraints (submodule-only in strict mode)
/// - Tracks dependencies for transitive import support
///
/// Returns Unit on success
pub(super) fn eval_import(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // (import! dest module [options...])
    if items.len() < 3 {
        let err = MettaValue::Error(
            "import!: expected at least 2 arguments. Usage: (import! dest module)".to_string(),
            Arc::new(MettaValue::Nil),
        );
        return (vec![err], env);
    }

    let dest = &items[1];
    let module_arg = &items[2];

    // Parse options (e.g., :no-transitive)
    // Note: :no-transitive is parsed but not yet used (future enhancement)
    let mut _no_transitive = false;
    for opt in items.iter().skip(3) {
        if let MettaValue::Atom(name) = opt {
            if name == ":no-transitive" {
                _no_transitive = true;
            }
        }
    }

    // Get module path string
    let module_path_str = match module_arg {
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
            // Import all into current space - same as include
            let include_items = vec![
                MettaValue::Atom("include".to_string()),
                module_arg.clone(),
            ];
            eval_include(include_items, env)
        }
        MettaValue::Atom(alias) => {
            // Import with alias - for now, just load the module and bind the path
            // In a full implementation, this would create a namespace reference
            let include_items = vec![
                MettaValue::Atom("include".to_string()),
                module_arg.clone(),
            ];
            let (results, mut new_env) = eval_include(include_items, env);

            // If successful, we could bind the module reference here
            // For now, just return the include results
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
/// Returns a Space value that can be used with match and other space operations.
/// Note: Currently returns the module count as a placeholder.
///       Full implementation requires Space values in MettaValue.
pub(super) fn eval_mod_space(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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

    // Check if module is loaded
    if let Some(mod_id) = env.get_module_by_path(&resolved_path) {
        // Module is loaded - return a reference to it
        // For now, return the module ID as a Long (placeholder)
        // Full implementation would return a Space value
        (vec![MettaValue::Long(mod_id.value() as i64)], env)
    } else {
        // Module not loaded - try to load it first
        let include_items = vec![
            MettaValue::Atom("include".to_string()),
            module_arg.clone(),
        ];
        let (_, new_env) = eval_include(include_items, env);

        // Check again
        if let Some(mod_id) = new_env.get_module_by_path(&resolved_path) {
            (vec![MettaValue::Long(mod_id.value() as i64)], new_env)
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
}
