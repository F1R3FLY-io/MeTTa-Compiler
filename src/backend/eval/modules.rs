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
//! - `eval_include_generic`: Load and evaluate a MeTTa file (force-evals `!` expressions)
//! - `eval_import_generic`: Import a module into scope (force-evals `!` expressions)
//! - `eval_mod_space_generic`: Module space operations
//! - `eval_print_mods_generic`: Print loaded modules

use std::hash::{Hash, Hasher};

use crate::backend::compile::compile_generic;
use crate::backend::eval::frame_chain::{maybe_push_frame, FrameLabel};
use crate::backend::eval::trampoline::eval_loop::eval_trampoline;
use crate::backend::eval::trampoline::{EvalContext, MettaEnvironment};
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait};
use crate::backend::modules::path::{resolve_library_form_with_importer, resolve_module_path};

// ============================================================================
// Generic Module Operations
// ============================================================================

/// Generic eval_include: Load and evaluate a MeTTa file
///
/// Zero-conversion implementation that works with any value type.
/// Evaluates `!`-prefixed expressions (force-eval) encountered in the file,
/// enabling transitive imports and runtime operations in included modules.
pub fn eval_include_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    mut env: MettaEnvironment,
    ctx: &C,
) -> (Vec<MettaValue>, MettaEnvironment)
where
    MettaValue: Clone,
{
    let factory = ctx.factory();

    if items.len() < 2 {
        let err = factory.error(
            factory.sexpr(items),
            factory.atom("IncorrectNumberOfArguments"),
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
            path_arg.clone(),
            factory.string("include: expected string or symbol path"),
        );
        return (vec![err], env);
    };

    // Resolve path using module path notation
    let resolved_path = resolve_module_path(&path_str, env.current_module_dir());

    // Cycle detection: hash the resolved path
    let content_hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        resolved_path.hash(&mut hasher);
        hasher.finish()
    };

    if env.is_module_loading(content_hash) {
        // Cycle detected — return unit silently (HE-compatible: no error)
        return (vec![factory.unit()], env);
    }
    env.mark_module_loading(content_hash);

    // Read the file contents
    let contents = match std::fs::read_to_string(&resolved_path) {
        Ok(c) => c,
        Err(e) => {
            env.unmark_module_loading(content_hash);
            let err = factory.error(
                factory.atom(&path_str),
                factory.string(&format!(
                    "include: failed to read file '{}': {}",
                    resolved_path.display(),
                    e
                )),
            );
            return (vec![err], env);
        }
    };

    // Compile the file contents using generic compile
    let expressions: Vec<MettaValue> = match compile_generic(&contents, factory) {
        Ok(exprs) => exprs,
        Err(e) => {
            env.unmark_module_loading(content_hash);
            let err = factory.error(
                factory.atom(&path_str),
                factory.string(&format!(
                    "include: failed to parse file '{}': {}",
                    resolved_path.display(),
                    e
                )),
            );
            return (vec![err], env);
        }
    };

    // Save current module dir and set to the included file's directory
    let prev_module_dir = env.current_module_dir().map(|p| p.to_path_buf());
    if let Some(parent) = resolved_path.parent() {
        env.set_current_module_path(Some(parent.to_path_buf()));
    }

    // Push a frame guard protecting compiled expressions from GC during nested eval.
    // This ensures that when a nested trampoline (e.g., !(import! ...)) fires a GC
    // safepoint, the remaining expressions in this Vec are visible as roots.
    // SAFETY: `expressions` outlives `_frame_guard` (both are locals in this scope).
    let _frame_guard = unsafe { maybe_push_frame::<C>(FrameLabel::Include, &expressions) };

    // Process expressions: extract rules, evaluate force-eval expressions.
    // Iterate by reference so the Vec stays alive for the frame guard.
    let mut last_result = factory.unit();

    for expr in expressions.iter() {
        if let Some(sexpr_items) = expr.as_sexpr() {
            // Force-eval: (! inner) → evaluate inner via trampoline
            if sexpr_items.len() == 2 {
                if let Some("!") = sexpr_items[0].as_atom() {
                    let inner = sexpr_items[1].clone();
                    let (results, new_env) = eval_trampoline(inner, env, ctx);
                    env = (*new_env).clone();
                    if let Some((r, _b)) = results.into_iter().last() {
                        last_result = r;
                    }
                    continue;
                }
            }

            // Rule definition: (= pattern body)
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
        env.add_to_space(expr);
        last_result = expr.clone();
    }

    // Drop frame guard before restoring state (explicit for clarity; also drops at scope end)
    drop(_frame_guard);

    // Restore previous module dir and unmark loading
    env.set_current_module_path(prev_module_dir);
    env.unmark_module_loading(content_hash);

    (vec![last_result], env)
}

/// Generic eval_import: Import a module into scope
///
/// MeTTa HE syntax: `(import! space-ref module-path)` or `(import! module-path)`
///
/// - 3 items: `[import!, &self, PLN]` → space ref ignored, module-path = items[2]
/// - 2 items: `[import!, PLN]` → module-path = items[1]
///
/// Loads the module file, adds all rules/types/atoms to the current environment,
/// evaluates `!`-prefixed expressions (enabling transitive imports),
/// and returns `()` (unit).
pub fn eval_import_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    mut env: MettaEnvironment,
    ctx: &C,
) -> (Vec<MettaValue>, MettaEnvironment)
where
    MettaValue: Clone,
{
    let factory = ctx.factory();

    if items.len() < 2 {
        let err = factory.error(
            factory.sexpr(items),
            factory.atom("IncorrectNumberOfArguments"),
        );
        return (vec![err], env);
    }

    // Parse arguments: (import! &self PLN) or (import! PLN)
    let path_arg = if items.len() >= 3 {
        // 3-arg form: items[1] is space ref (ignored), items[2] is module path
        &items[2]
    } else {
        // 2-arg form: items[1] is module path
        &items[1]
    };

    // Resolve the module path. Three accepted forms:
    //
    //   1. String literal: `(import! &self "path/to/file.metta")`
    //   2. Bare atom: `(import! &self PLN)` — resolved via METTA_MODULE_PATH
    //   3. PeTTa-compatible `(library X)` / `(library X Y)` S-expression — resolved
    //      against the LIBRARY_PATHS registry (seeded from METTA_LIBRARY_PATH and
    //      `<MeTTaTron>/stdlib`, plus runtime additions from `git-import!`).
    //
    // All failure paths return a graceful error MettaValue (never panic).
    let (resolved_path, path_display): (std::path::PathBuf, String) = if let Some(s) =
        path_arg.as_string()
    {
        let p = resolve_module_path(s, env.current_module_dir());
        let d = s.to_string();
        (p, d)
    } else if let Some(s) = path_arg.as_atom() {
        let p = resolve_module_path(s, env.current_module_dir());
        let d = s.to_string();
        (p, d)
    } else if let Some(items_ref) = path_arg.as_sexpr() {
        // PeTTa-compatible (library X) / (library X Y) form.
        if items_ref.first().and_then(|h| h.as_atom()) == Some("library") {
            match resolve_library_form_with_importer(items_ref, env.current_module_dir()) {
                Some(p) => {
                    let d = format!("{:?}", path_arg);
                    (p, d)
                }
                None => {
                    let err = factory.error(
                        path_arg.clone(),
                        factory.string(
                            "import!: (library ...) form did not resolve to an existing file. \
                             Check METTA_LIBRARY_PATH and that any required `git-import!` has been called.",
                        ),
                    );
                    return (vec![err], env);
                }
            }
        } else {
            let err = factory.error(
                path_arg.clone(),
                factory.string(
                    "import!: expected string, symbol, or (library ...) S-expression for module path",
                ),
            );
            return (vec![err], env);
        }
    } else {
        let err = factory.error(
            path_arg.clone(),
            factory.string(
                "import!: expected string, symbol, or (library ...) S-expression for module path",
            ),
        );
        return (vec![err], env);
    };
    let path_str = path_display;

    // Cycle detection: hash the resolved path
    let content_hash = {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        resolved_path.hash(&mut hasher);
        hasher.finish()
    };

    if env.is_module_loading(content_hash) {
        // Cycle detected — return unit silently (HE-compatible: no error)
        return (vec![factory.unit()], env);
    }
    env.mark_module_loading(content_hash);

    // Read the file contents
    let contents = match std::fs::read_to_string(&resolved_path) {
        Ok(c) => c,
        Err(e) => {
            env.unmark_module_loading(content_hash);
            let err = factory.error(
                factory.atom(&path_str),
                factory.string(&format!(
                    "import!: failed to read file '{}': {}",
                    resolved_path.display(),
                    e
                )),
            );
            return (vec![err], env);
        }
    };

    // Save current module dir and set it to the imported file's directory
    let prev_module_dir = env.current_module_dir().map(|p| p.to_path_buf());
    if let Some(parent) = resolved_path.parent() {
        env.set_current_module_path(Some(parent.to_path_buf()));
    }

    // Compile the file contents
    let expressions: Vec<MettaValue> = match compile_generic(&contents, factory) {
        Ok(exprs) => exprs,
        Err(e) => {
            // Restore module dir and unmark before returning
            env.set_current_module_path(prev_module_dir);
            env.unmark_module_loading(content_hash);
            let err = factory.error(
                factory.atom(&path_str),
                factory.string(&format!(
                    "import!: failed to parse file '{}': {}",
                    resolved_path.display(),
                    e
                )),
            );
            return (vec![err], env);
        }
    };

    // Push a frame guard protecting compiled expressions from GC during nested eval.
    // SAFETY: `expressions` outlives `_frame_guard` (both are locals in this scope).
    let _frame_guard = unsafe { maybe_push_frame::<C>(FrameLabel::Import, &expressions) };

    // Process expressions: extract rules, type declarations, evaluate force-eval.
    // Iterate by reference so the Vec stays alive for the frame guard.
    for expr in expressions.iter() {
        if let Some(sexpr_items) = expr.as_sexpr() {
            // Force-eval: (! inner) → evaluate inner via trampoline
            if sexpr_items.len() == 2 {
                if let Some("!") = sexpr_items[0].as_atom() {
                    let inner = sexpr_items[1].clone();
                    let (_results, new_env) = eval_trampoline(inner, env, ctx);
                    env = (*new_env).clone();
                    continue;
                }
            }

            // Rule/type extraction
            if sexpr_items.len() == 3 {
                if let Some(op) = sexpr_items[0].as_atom() {
                    if op == "=" {
                        env.add_rule(sexpr_items[1].clone(), sexpr_items[2].clone());
                        continue;
                    }
                    if op == ":" {
                        if let Some(name) = sexpr_items[1].as_atom() {
                            env.add_type_generic(name, sexpr_items[2].clone());
                        }
                        continue;
                    }
                }
            }
        }

        // For other expressions, add to space as facts (queryable via `match &self`)
        env.add_to_space(expr);
    }

    // Drop frame guard before restoring state
    drop(_frame_guard);

    // Restore previous module dir and unmark loading
    env.set_current_module_path(prev_module_dir);
    env.unmark_module_loading(content_hash);

    (vec![factory.unit()], env)
}

/// Generic eval_mod_space: Module space operations
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_mod_space_generic<V, F>(
    items: Vec<V>,
    env: ContextEnv2<V, F>,
    factory: &F,
) -> (Vec<V>, ContextEnv2<V, F>)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    if items.len() < 2 {
        let err = factory.error(
            factory.sexpr(items),
            factory.atom("IncorrectNumberOfArguments"),
        );
        return (vec![err], env);
    }

    let module_arg = &items[1];
    let module_name = if let Some(s) = module_arg.as_atom() {
        s.to_string()
    } else {
        let err = factory.error(
            module_arg.clone(),
            factory.string("mod-space!: expected symbol for module name"),
        );
        return (vec![err], env);
    };

    // Return a space handle representation
    let result = factory.sexpr(vec![factory.atom("space"), factory.atom(&module_name)]);

    (vec![result], env)
}

/// Generic eval_print_mods: Print loaded modules
///
/// Zero-conversion implementation that works with any value type.
pub fn eval_print_mods_generic<V, F>(
    _items: Vec<V>,
    env: ContextEnv2<V, F>,
    factory: &F,
) -> (Vec<V>, ContextEnv2<V, F>)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // Return a placeholder - in a full implementation this would list modules
    let result = factory.sexpr(vec![factory.atom("modules"), factory.atom("none")]);

    (vec![result], env)
}

/// Generic eval_get_modules: return loaded module names as a tuple.
///
/// (Workstream X.5g — MTT-FN-GETMODULES). Walks `env.shared.module_registry`
/// via `ModuleRegistry::iter()` (`modules/loader.rs:268`), collecting each
/// module's name (`MettaMod::name()`) into an SExpr tuple. Class is
/// `mettatron-environmental-extension` — the exact module list is host-
/// dependent (depends on `--module-dir` and prior `import!` invocations).
pub fn eval_get_modules_generic<V, F>(
    _items: Vec<V>,
    env: ContextEnv2<V, F>,
    factory: &F,
) -> (Vec<V>, ContextEnv2<V, F>)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let registry = env.shared.module_registry.read();
    let mut names: Vec<V> = Vec::with_capacity(registry.module_count());
    for m in registry.iter() {
        names.push(factory.atom(m.name()));
    }
    let result = factory.sexpr(names);
    drop(registry);
    (vec![result], env)
}

/// Type alias for non-context module operations (mod-space!, print-mods!)
/// that don't need force-eval and still use the old V, F generic parameters.
type ContextEnv2<V, F> = crate::backend::environment::GenericEnvironment<V, F>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::eval::trampoline::StaticEvalContext;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_eval_include_generic_missing_args() {
        let env = MettaEnvironment::new(GcFactory::default());
        let ctx = StaticEvalContext::get();

        let items = vec![MettaValue::Atom("include".to_string())];
        let (results, _) = eval_include_generic(items, env, &ctx);

        assert_eq!(results.len(), 1);
        assert!(results[0].as_error().is_some());
    }

    #[test]
    fn test_eval_import_generic_missing_args() {
        let env = MettaEnvironment::new(GcFactory::default());
        let ctx = StaticEvalContext::get();

        let items = vec![MettaValue::Atom("import!".to_string())];
        let (results, _) = eval_import_generic(items, env, &ctx);

        assert_eq!(results.len(), 1);
        assert!(results[0].as_error().is_some());
    }
}
