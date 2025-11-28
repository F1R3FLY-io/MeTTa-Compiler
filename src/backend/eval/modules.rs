use std::path::Path;
use std::sync::Arc;

use crate::backend::compile::compile;
use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, Rule};

use super::eval;

// ============================================================
// Module Operations (include)
// ============================================================

/// include: Load and evaluate a MeTTa file
/// Usage: (include "path/to/file.metta") or (include path:to:module)
/// Loads the file, parses all expressions, evaluates them, and adds rules to the environment
/// Returns the result of the last expression
pub(super) fn eval_include(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("include", items, 1, env, "(include path)");

    let path_arg = &items[1];

    // Get the path string
    let path_str = match path_arg {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => {
            // Convert module path notation (path:to:module) to file path
            s.replace(':', "/") + ".metta"
        }
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

    // Try to resolve the path relative to the current working directory
    let path = Path::new(&path_str);

    // Read the file contents
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            let err = MettaValue::Error(
                format!("include: failed to read file '{}': {}", path_str, e),
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], env);
        }
    };

    // Compile the file contents to MettaValue expressions
    let state = match compile(&contents) {
        Ok(s) => s,
        Err(e) => {
            let err = MettaValue::Error(
                format!("include: failed to parse file '{}': {}", path_str, e),
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], env);
        }
    };
    let expressions = state.source;

    // Collect rules from the file
    let mut rules_to_add = Vec::new();
    let mut expressions_to_eval = Vec::new();

    for expr in expressions {
        // Check if it's a rule definition (= pattern body)
        if let MettaValue::SExpr(ref items) = expr {
            if items.len() == 3 {
                if let MettaValue::Atom(ref op) = items[0] {
                    if op == "=" {
                        // Collect the rule for bulk addition
                        let rule = Rule {
                            lhs: items[1].clone(),
                            rhs: items[2].clone(),
                        };
                        rules_to_add.push(rule);
                        continue;
                    }
                    if op == ":" {
                        // Type declarations are stored but don't produce output
                        // For now, just continue
                        continue;
                    }
                }
            }
        }

        // Collect expressions to evaluate
        expressions_to_eval.push(expr);
    }

    // Add all rules at once
    let mut current_env = env;
    if !rules_to_add.is_empty() {
        if let Err(e) = current_env.add_rules_bulk(rules_to_add) {
            let err = MettaValue::Error(
                format!("include: failed to add rules: {}", e),
                Arc::new(MettaValue::Atom(path_str)),
            );
            return (vec![err], current_env);
        }
    }

    // Evaluate remaining expressions
    let mut last_results = vec![MettaValue::Unit];
    for expr in expressions_to_eval {
        let (results, new_env) = eval(expr, current_env);
        current_env = new_env;
        if !results.is_empty() {
            last_results = results;
        }
    }

    (last_results, current_env)
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
}
