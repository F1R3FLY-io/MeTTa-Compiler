//! Generic Module Operations - Zero-Conversion Implementation
//!
//! This module provides generic versions of module operations that work with any
//! value type implementing `MettaValueTrait`. This eliminates MettaValue <-> MettaValue
//! conversions when using arena allocation.
//!
//! ## Zero-Conversion Path
//!
//! - `compile_generic` produces values directly in the target type
//! - Pattern matching uses `MettaValueTrait` methods
//! - No serialization/deserialization at module boundaries
//!
//! ## Operations
//!
//! - `eval_include_generic`: Load and evaluate a MeTTa file
//! - `eval_import_generic`: Import a module into scope
//! - `eval_mod_space_generic`: Module space operations
//! - `eval_print_mods_generic`: Print loaded modules

use crate::backend::compile::compile_generic;
use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};
use crate::backend::modules::resolve_module_path;

/// Generic result type for module operations
pub type GenericModuleResult<V, F> = (Vec<V>, GenericEnvironment<V, F>);

// ============================================================================
// Generic Module Operations
// ============================================================================

/// Generic eval_include: Load and evaluate a MeTTa file
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_include_generic<V, F>(
    items: Vec<V>,
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericModuleResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if items.len() < 2 {
        let err = factory.error(
            "include requires 1 argument: (include path)",
            factory.sexpr(items),
        );
        return (vec![err], env);
    }

    let path_arg = &items[1];

    // Get the path string using trait methods
    let path_str = if let Some(s) = path_arg.as_string() {
        s.to_string()
    } else if let Some(s) = path_arg.as_atom() {
        s.to_string()
    } else {
        let err = factory.error(
            "include: expected string or symbol path",
            path_arg.clone(),
        );
        return (vec![err], env);
    };

    // Resolve path using module path notation
    let resolved_path = resolve_module_path(&path_str, env.current_module_dir());

    // Read the file contents
    let contents = match std::fs::read_to_string(&resolved_path) {
        Ok(c) => c,
        Err(e) => {
            let err = factory.error(
                &format!(
                    "include: failed to read file '{}': {}",
                    resolved_path.display(),
                    e
                ),
                factory.atom(&path_str),
            );
            return (vec![err], env);
        }
    };

    // Compile the file contents using generic compile
    let expressions: Vec<V> = match compile_generic(&contents, factory) {
        Ok(exprs) => exprs,
        Err(e) => {
            let err = factory.error(
                &format!(
                    "include: failed to parse file '{}': {}",
                    resolved_path.display(),
                    e
                ),
                factory.atom(&path_str),
            );
            return (vec![err], env);
        }
    };

    // Process expressions: extract rules and evaluate
    let mut last_result = factory.unit();

    for expr in expressions {
        // Check if it's a rule definition (= pattern body) using trait methods
        if let Some(sexpr_items) = expr.as_sexpr() {
            if sexpr_items.len() == 3 {
                if let Some(op) = sexpr_items[0].as_atom() {
                    if op == "=" {
                        // Add rule directly for zero-conversion
                        env.add_rule(sexpr_items[1].clone(), sexpr_items[2].clone());
                        continue;
                    }
                    if op == ":" {
                        // Type declaration - add using direct value
                        if let Some(name) = sexpr_items[1].as_atom() {
                            env.add_type_generic(name, sexpr_items[2].clone());
                        }
                        continue;
                    }
                }
            }
        }

        // For other expressions, add to space as facts
        env.add_to_space(&expr);
        last_result = expr;
    }

    (vec![last_result], env)
}

/// Generic eval_import: Import a module into scope
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_import_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericModuleResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() < 2 {
        let err = factory.error(
            "import! requires 1 argument: (import! module-path)",
            factory.sexpr(items),
        );
        return (vec![err], env);
    }

    // For now, import! is similar to include
    // In a full implementation, it would handle module namespacing
    let path_arg = &items[1];
    let path_str = if let Some(s) = path_arg.as_string() {
        s.to_string()
    } else if let Some(s) = path_arg.as_atom() {
        s.to_string()
    } else {
        let err = factory.error(
            "import!: expected string or symbol path",
            path_arg.clone(),
        );
        return (vec![err], env);
    };

    // Return a placeholder indicating the import
    let result = factory.sexpr(vec![
        factory.atom("imported"),
        factory.atom(&path_str),
    ]);

    (vec![result], env)
}

/// Generic eval_mod_space: Module space operations
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_mod_space_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericModuleResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() < 2 {
        let err = factory.error(
            "mod-space! requires 1 argument: (mod-space! module-name)",
            factory.sexpr(items),
        );
        return (vec![err], env);
    }

    let module_arg = &items[1];
    let module_name = if let Some(s) = module_arg.as_atom() {
        s.to_string()
    } else {
        let err = factory.error(
            "mod-space!: expected symbol for module name",
            module_arg.clone(),
        );
        return (vec![err], env);
    };

    // Return a space handle representation
    let result = factory.sexpr(vec![
        factory.atom("space"),
        factory.atom(&module_name),
    ]);

    (vec![result], env)
}

/// Generic eval_print_mods: Print loaded modules
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_print_mods_generic<V, F>(
    _items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericModuleResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // Return a placeholder - in a full implementation this would list modules
    let result = factory.sexpr(vec![
        factory.atom("modules"),
        factory.atom("none"),
    ]);

    (vec![result], env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_eval_include_generic_missing_args() {
        let env = MettaEnvironment::new(GcFactory::default());
        let factory = GcFactory::default();

        let items = vec![MettaValue::Atom("include".to_string())];
        let (results, _) = eval_include_generic(items, env, &factory);

        assert_eq!(results.len(), 1);
        assert!(results[0].as_error().is_some());
    }

    #[test]
    fn test_eval_import_generic_missing_args() {
        let env = MettaEnvironment::new(GcFactory::default());
        let factory = GcFactory::default();

        let items = vec![MettaValue::Atom("import!".to_string())];
        let (results, _) = eval_import_generic(items, env, &factory);

        assert_eq!(results.len(), 1);
        assert!(results[0].as_error().is_some());
    }
}
