use crate::backend::environment::Environment;
use crate::backend::fuzzy_match::FuzzyMatcher;
use crate::backend::models::{EvalResult, MettaValue, Rule, SpaceHandle};
use std::sync::{Arc, OnceLock};

use super::eval;

/// Valid space names for "Did you mean?" suggestions
const VALID_SPACE_NAMES: &[&str] = &["self"];

/// Get fuzzy matcher for space names (lazily initialized)
fn space_name_matcher() -> &'static FuzzyMatcher {
    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    MATCHER.get_or_init(|| FuzzyMatcher::from_terms(VALID_SPACE_NAMES.iter().copied()))
}

/// Suggest a valid space name if the given name is close to one
fn suggest_space_name(name: &str) -> Option<String> {
    // First check for common case errors
    let lower = name.to_lowercase();
    if lower == "self" && name != "self" {
        return Some("Did you mean: self? (space names are case-sensitive)".to_string());
    }

    // Use fuzzy matcher for other typos (e.g., "slef" -> "self")
    space_name_matcher().did_you_mean(name, 2, 1)
}

/// Rule definition: (= lhs rhs) - add to MORK Space and rule cache
pub(super) fn eval_add(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("=", items, 2, env, "(= pattern body)");

    let lhs = items[1].clone();
    let rhs = items[2].clone();
    let mut new_env = env.clone();

    // Add rule using add_rule (stores in both rule_cache and MORK Space)
    new_env.add_rule(Rule::new(lhs, rhs));

    // Return empty list (rule definitions don't produce output)
    (vec![], new_env)
}

/// Evaluate match: (match <space> <pattern> <template>)
/// Searches the space for all atoms matching the pattern and returns instantiated templates
///
/// Supports two syntaxes:
/// - New: (match space pattern template) - where space is a Space value (e.g., from mod-space! or &self)
/// - Legacy: (match & self pattern template) - backward compatible syntax
///
/// Optimized to use Environment::match_space which performs pattern matching
/// directly on MORK expressions without unnecessary intermediate allocations
pub(super) fn eval_match(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    // Debug: Show what eval_match receives
    let debug = std::env::var("METTA_DEBUG_MATCH").is_ok();
    if debug {
        eprintln!("[DEBUG eval_match] items={:?}", items);
        eprintln!("[DEBUG eval_match] args.len()={}", args.len());
    }

    // Support both: (match space pattern template) and (match & self pattern template)
    if args.len() == 3 {
        // New-style syntax: (match space pattern template)
        let space_arg = &args[0];
        let pattern = &args[1];
        let template = &args[2];

        if debug {
            eprintln!("[DEBUG eval_match] space_arg={:?}", space_arg);
            eprintln!("[DEBUG eval_match] pattern={:?}", pattern);
            eprintln!("[DEBUG eval_match] template={:?}", template);
        }

        // Evaluate the space argument
        let (space_results, env1) = eval(space_arg.clone(), env);
        if space_results.is_empty() {
            let err = MettaValue::Error(
                "match: space evaluated to empty".to_string(),
                Arc::new(space_arg.clone()),
            );
            return (vec![err], env1);
        }

        match &space_results[0] {
            MettaValue::Space(handle) => {
                let results = match_with_space_handle(handle, pattern, template, &env1);
                (results, env1)
            }
            other => {
                let err = MettaValue::Error(
                    format!(
                        "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                        super::friendly_value_repr(other)
                    ),
                    Arc::new(other.clone()),
                );
                (vec![err], env1)
            }
        }
    } else if args.len() == 4 {
        // Legacy syntax: (match & self pattern template)
        let space_ref = &args[0];
        let space_name = &args[1];
        let pattern = &args[2];
        let template = &args[3];

        // Check that first arg is & (space reference operator)
        match space_ref {
            MettaValue::Atom(s) if s == "&" => {
                // Check space name (for now, only support "self")
                match space_name {
                    MettaValue::Atom(name) if name == "self" => {
                        // Use optimized match_space method that works directly with MORK
                        let results = env.match_space(pattern, template);
                        (results, env)
                    }
                    _ => {
                        // Try to suggest a valid space name
                        let name_str = match space_name {
                            MettaValue::Atom(s) => s.as_str(),
                            _ => "",
                        };

                        let suggestion = suggest_space_name(name_str);
                        let msg = match suggestion {
                            Some(s) => format!(
                                "match only supports 'self' as space name, got: {:?}. {}",
                                space_name, s
                            ),
                            None => format!(
                                "match only supports 'self' as space name, got: {:?}",
                                space_name
                            ),
                        };

                        let err = MettaValue::Error(msg, Arc::new(MettaValue::SExpr(args.to_vec())));
                        (vec![err], env)
                    }
                }
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "match requires & as first argument (legacy syntax), got: {}",
                        super::friendly_value_repr(space_ref)
                    ),
                    Arc::new(MettaValue::SExpr(args.to_vec())),
                );
                (vec![err], env)
            }
        }
    } else {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        (vec![err], env)
    }
}

/// Pattern match against a SpaceHandle and return evaluated instantiated templates.
/// HE-compatible: After pattern matching, the template is evaluated with bindings applied.
///
/// IMPORTANT: Each match result is evaluated in a forked environment to provide
/// nondeterministic branch isolation. This prevents one branch's mutations from
/// affecting other branches.
fn match_with_space_handle(
    handle: &SpaceHandle,
    pattern: &MettaValue,
    template: &MettaValue,
    env: &Environment,
) -> Vec<MettaValue> {
    use super::{apply_bindings, eval, pattern_match};

    // Debug logging
    let debug = std::env::var("METTA_DEBUG_MATCH").is_ok();
    if debug {
        eprintln!("[DEBUG match] handle.name={}, is_module_space={}", handle.name, handle.is_module_space());
        eprintln!("[DEBUG match] pattern={:?}", pattern);
        eprintln!("[DEBUG match] template={:?}", template);
    }

    // For module-backed spaces or the global "self" space, use Environment's MORK-based matching
    // For other owned spaces (from new-space), match against the atoms in the space
    if handle.is_module_space() || handle.name == "self" {
        // Module space or global "self" space - use Environment's match_space for MORK integration
        // The Environment has the rules from (= ...) definitions
        if debug {
            eprintln!("[DEBUG match] Using env.match_space (module/self path)");
        }
        env.match_space(pattern, template)
    } else {
        // Owned space (from new-space) - match against atoms stored in SpaceHandle
        let atoms = handle.collapse();
        if debug {
            eprintln!("[DEBUG match] Using owned space path, {} atoms", atoms.len());
        }

        // Collect all matching atoms first (don't evaluate yet)
        let mut matching_bindings: Vec<_> = Vec::new();
        for atom in &atoms {
            if debug {
                eprintln!("[DEBUG match] Trying to match atom={:?}", atom);
            }
            if let Some(bindings) = pattern_match(pattern, atom) {
                if debug {
                    eprintln!("[DEBUG match] MATCH! bindings={:?}", bindings);
                }
                matching_bindings.push(bindings);
            }
        }

        // If only one match, no need for forking (optimization)
        if matching_bindings.len() == 1 {
            let bindings = &matching_bindings[0];
            let instantiated = apply_bindings(template, bindings).into_owned();
            if debug {
                eprintln!("[DEBUG match] Single match, instantiated={:?}", instantiated);
            }
            let (eval_results, _) = eval(instantiated, env.clone());
            if debug {
                eprintln!("[DEBUG match] eval_results={:?}", eval_results);
            }
            return eval_results;
        }

        // Multiple matches - fork environment for each to isolate mutations
        let mut results = Vec::new();
        for bindings in &matching_bindings {
            let instantiated = apply_bindings(template, bindings).into_owned();
            if debug {
                eprintln!("[DEBUG match] Multi-match, instantiated={:?}", instantiated);
            }

            // Fork environment for this branch to isolate space mutations
            let forked_env = env.fork_for_nondeterminism();

            // HE-compatible: Evaluate the instantiated template in forked environment
            let (eval_results, _) = eval(instantiated, forked_env);
            if debug {
                eprintln!("[DEBUG match] eval_results={:?}", eval_results);
            }
            results.extend(eval_results);
        }

        results
    }
}

/// new-space: Create a new named space
/// Returns a Space reference that can be used with add-atom, remove-atom, collapse
/// Usage: (new-space) or (new-space "name")
pub(super) fn eval_new_space(items: Vec<MettaValue>, mut env: Environment) -> EvalResult {
    let args = &items[1..];

    // Get optional name, default to "space-N"
    let name = if !args.is_empty() {
        match &args[0] {
            MettaValue::String(s) => s.clone(),
            MettaValue::Atom(s) => s.clone(),
            other => {
                let err = MettaValue::Error(
                    format!(
                        "new-space: optional name must be a string, got {}. Usage: (new-space) or (new-space \"name\")",
                        super::friendly_value_repr(other)
                    ),
                    Arc::new(other.clone()),
                );
                return (vec![err], env);
            }
        }
    } else {
        "unnamed".to_string()
    };

    let space_id = env.create_named_space(&name);
    let handle = SpaceHandle::new(space_id, name);
    (vec![MettaValue::Space(handle)], env)
}

/// add-atom: Add an atom to a space
/// Usage: (add-atom space-ref atom)
pub(super) fn eval_add_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("add-atom", items, 2, env, "(add-atom space atom)");

    let space_ref = &items[1];
    let atom = &items[2];

    // Evaluate both arguments
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "add-atom: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let (atom_results, mut env2) = eval(atom.clone(), env1);
    if atom_results.is_empty() {
        let err = MettaValue::Error(
            "add-atom: atom evaluated to empty".to_string(),
            Arc::new(atom.clone()),
        );
        return (vec![err], env2);
    }

    // Get the space ID
    let space_value = &space_results[0];
    let atom_value = &atom_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Use SpaceHandle's add_atom method directly (it has its own backing store)
            handle.add_atom(atom_value.clone());
            (vec![MettaValue::Unit], env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                    super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env2)
        }
    }
}

/// remove-atom: Remove an atom from a space
/// Usage: (remove-atom space-ref atom)
pub(super) fn eval_remove_atom(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("remove-atom", items, 2, env, "(remove-atom space atom)");

    let space_ref = &items[1];
    let atom = &items[2];

    // Evaluate both arguments
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "remove-atom: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let (atom_results, mut env2) = eval(atom.clone(), env1);
    if atom_results.is_empty() {
        let err = MettaValue::Error(
            "remove-atom: atom evaluated to empty".to_string(),
            Arc::new(atom.clone()),
        );
        return (vec![err], env2);
    }

    // Get the space ID
    let space_value = &space_results[0];
    let atom_value = &atom_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Use SpaceHandle's remove_atom method directly (it has its own backing store)
            handle.remove_atom(atom_value);
            (vec![MettaValue::Unit], env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                    super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env2)
        }
    }
}

/// collapse: Gather all nondeterministic results into a list
/// Usage: (collapse expr)
///
/// HE-compatible behavior:
/// - If expr evaluates to multiple results (superposition), gathers them into a list
/// - If expr is a space, returns all atoms in the space as a list
/// - If expr is empty, returns Nil
///
/// Example:
/// ```metta
/// !(collapse (get-atoms &self))  ; Wraps atoms in a list
/// !(collapse &myspace)           ; Gets atoms from space as list
/// ```
pub(super) fn eval_collapse(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("collapse", items, 1, env, "(collapse expr)");

    let expr = &items[1];

    // Evaluate the expression - this may return multiple results (superposition)
    let (results, env1) = eval(expr.clone(), env);

    if results.is_empty() {
        // Empty superposition returns empty list ()
        return (vec![MettaValue::SExpr(vec![])], env1);
    }

    // Filter out Nil values from results (they represent "no result" in nondeterministic evaluation)
    let filtered: Vec<MettaValue> = results
        .into_iter()
        .filter(|v| !matches!(v, MettaValue::Nil))
        .collect();

    if filtered.is_empty() {
        // All results were Nil → return empty list ()
        return (vec![MettaValue::SExpr(vec![])], env1);
    }

    // Check if the single result is a space (direct space collapse)
    if filtered.len() == 1 {
        if let MettaValue::Space(handle) = &filtered[0] {
            // Use SpaceHandle's collapse method directly
            let atoms = handle.collapse();
            if atoms.is_empty() {
                return (vec![MettaValue::SExpr(vec![])], env1);
            } else {
                return (vec![MettaValue::SExpr(atoms)], env1);
            }
        }
    }

    // For any other expression, gather all results into a list
    (vec![MettaValue::SExpr(filtered)], env1)
}

/// get-atoms: Get all atoms from a space as a superposition
/// Usage: (get-atoms space)
///
/// Unlike `collapse` which returns atoms wrapped in a list, `get-atoms` returns
/// atoms as a superposition (multiple values). This is HE-compatible behavior.
///
/// Example:
/// ```metta
/// !(get-atoms &self)  ; Returns each atom as a separate result
/// ```
pub(super) fn eval_get_atoms(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-atoms", items, 1, env, "(get-atoms space)");

    let space_ref = &items[1];

    // Evaluate the space reference
    let (space_results, env1) = eval(space_ref.clone(), env);
    if space_results.is_empty() {
        let err = MettaValue::Error(
            "get-atoms: space evaluated to empty".to_string(),
            Arc::new(space_ref.clone()),
        );
        return (vec![err], env1);
    }

    let space_value = &space_results[0];

    match space_value {
        MettaValue::Space(handle) => {
            // Return atoms as superposition (multiple results), not wrapped in list
            let atoms = handle.collapse();
            if atoms.is_empty() {
                // Empty space returns empty results
                (vec![], env1)
            } else {
                // Return all atoms as separate results (superposition semantics)
                (atoms, env1)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "get-atoms: argument must be a space, got {}. Usage: (get-atoms space)",
                    super::friendly_value_repr(space_value)
                ),
                Arc::new(space_value.clone()),
            );
            (vec![err], env1)
        }
    }
}

// ============================================================
// State Operations (new-state, get-state, change-state!)
// ============================================================

/// new-state: Create a new mutable state cell with an initial value
/// Usage: (new-state initial-value)
pub(super) fn eval_new_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("new-state", items, 1, env, "(new-state initial-value)");

    let initial_value = &items[1];

    // Evaluate the initial value
    let (value_results, mut env1) = eval(initial_value.clone(), env);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "new-state: initial value evaluated to empty".to_string(),
            Arc::new(initial_value.clone()),
        );
        return (vec![err], env1);
    }

    let value = value_results[0].clone();
    let state_id = env1.create_state(value);
    (vec![MettaValue::State(state_id)], env1)
}

/// get-state: Get the current value from a state cell
/// Usage: (get-state state-ref)
pub(super) fn eval_get_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("get-state", items, 1, env, "(get-state state)");

    let state_ref = &items[1];

    // Evaluate the state reference
    let (state_results, env1) = eval(state_ref.clone(), env);
    if state_results.is_empty() {
        let err = MettaValue::Error(
            "get-state: state evaluated to empty".to_string(),
            Arc::new(state_ref.clone()),
        );
        return (vec![err], env1);
    }

    let state_value = &state_results[0];

    match state_value {
        MettaValue::State(state_id) => {
            if let Some(value) = env1.get_state(*state_id) {
                (vec![value], env1)
            } else {
                let err = MettaValue::Error(
                    format!("get-state: state {} not found", state_id),
                    Arc::new(state_value.clone()),
                );
                (vec![err], env1)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "get-state: argument must be a state reference, got {}. Usage: (get-state state)",
                    super::friendly_value_repr(state_value)
                ),
                Arc::new(state_value.clone()),
            );
            (vec![err], env1)
        }
    }
}

/// change-state!: Change the value in a state cell
/// Usage: (change-state! state-ref new-value)
/// Returns the state reference for chaining
pub(super) fn eval_change_state(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!(
        "change-state!",
        items,
        2,
        env,
        "(change-state! state new-value)"
    );

    let state_ref = &items[1];
    let new_value = &items[2];

    // Evaluate the state reference
    let (state_results, env1) = eval(state_ref.clone(), env);
    if state_results.is_empty() {
        let err = MettaValue::Error(
            "change-state!: state evaluated to empty".to_string(),
            Arc::new(state_ref.clone()),
        );
        return (vec![err], env1);
    }

    // Evaluate the new value
    let (value_results, mut env2) = eval(new_value.clone(), env1);
    if value_results.is_empty() {
        let err = MettaValue::Error(
            "change-state!: new value evaluated to empty".to_string(),
            Arc::new(new_value.clone()),
        );
        return (vec![err], env2);
    }

    let state_value = &state_results[0];
    let value = value_results[0].clone();

    match state_value {
        MettaValue::State(state_id) => {
            if env2.change_state(*state_id, value) {
                // Return the state reference for chaining
                (vec![state_value.clone()], env2)
            } else {
                let err = MettaValue::Error(
                    format!("change-state!: state {} not found", state_id),
                    Arc::new(state_value.clone()),
                );
                (vec![err], env2)
            }
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "change-state!: first argument must be a state reference, got {}. Usage: (change-state! state new-value)",
                    super::friendly_value_repr(state_value)
                ),
                Arc::new(state_value.clone()),
            );
            (vec![err], env2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval;

    #[test]
    fn test_add_missing_arguments() {
        let env = Environment::new();

        // (=) - missing both arguments
        let value = MettaValue::SExpr(vec![MettaValue::Atom("=".to_string())]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("="));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed (note plural)
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_add_missing_one_argument() {
        let env = Environment::new();

        // (= lhs) - missing rhs
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("lhs".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("="));
                assert!(msg.contains("requires exactly 2 arguments")); // Changed
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn test_rule_definition() {
        let env = Environment::new();

        // (= (f) 42)
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("f".to_string())]),
            MettaValue::Long(42),
        ]);

        let (result, _new_env) = eval(rule_def, env);

        // Rule definition should return empty list
        assert!(result.is_empty());
    }

    #[test]
    fn test_rule_definition_with_function_patterns() {
        let env = Environment::new();

        // Test function rule: (= (double $x) (* $x 2))
        let function_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (result, new_env) = eval(function_rule, env);
        assert!(result.is_empty());

        // Test the function: (double 5) should return 10
        let test_function = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);
        let (results, _) = eval(test_function, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_rule_definition_with_variable_consistency() {
        let env = Environment::new();

        // Test rule with repeated variables: (= (same $x $x) (duplicate $x))
        let consistency_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("duplicate".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(consistency_rule, env);
        assert!(result.is_empty());

        // Test matching with same values: (same 5 5)
        let test_same = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(5),
        ]);
        let (results, new_env2) = eval(test_same, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("duplicate".to_string()));
                assert_eq!(items[1], MettaValue::Long(5));
            }
            _ => panic!("Expected S-expression"),
        }

        // Test non-matching with different values: (same 5 7)
        let test_different = MettaValue::SExpr(vec![
            MettaValue::Atom("same".to_string()),
            MettaValue::Long(5),
            MettaValue::Long(7),
        ]);
        let (results, _) = eval(test_different, new_env2);
        assert_eq!(results.len(), 1);
        // Should return the original expression as it doesn't match any rule
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("same".to_string()));
                assert_eq!(items[1], MettaValue::Long(5));
                assert_eq!(items[2], MettaValue::Long(7));
            }
            _ => panic!("Expected S-expression"),
        }
    }

    #[test]
    fn test_multiple_rules_same_function() {
        let mut env = Environment::new();

        // Define multiple rules for the same function (factorial example)
        // (= (fact 0) 1)
        let fact_base = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Long(0),
            ]),
            MettaValue::Long(1),
        ]);
        let (_, env1) = eval(fact_base, env);
        env = env1;

        // (= (fact $n) (* $n (fact (- $n 1))))
        let fact_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("fact".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$n".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("fact".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                ]),
            ]),
        ]);
        let (_, env2) = eval(fact_recursive, env);

        // Test base case: (fact 0) should return 1
        let test_base = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, env3) = eval(test_base, env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));

        // Test recursive case: (fact 3) should return 6
        let test_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("fact".to_string()),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(test_recursive, env3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6));
    }

    #[test]
    fn test_rule_with_wildcard_patterns() {
        let env = Environment::new();

        // Test rule with wildcard: (= (ignore _ $x) $x)
        let wildcard_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("ignore".to_string()),
                MettaValue::Atom("_".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (result, new_env) = eval(wildcard_rule, env);
        assert!(result.is_empty());

        // Test with any first argument: (ignore "anything" 42)
        let test_wildcard = MettaValue::SExpr(vec![
            MettaValue::Atom("ignore".to_string()),
            MettaValue::String("anything".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(test_wildcard, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_match_basic_functionality() {
        let mut env = Environment::new();

        // Add some facts to the space
        let fact1 = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("alice".to_string()),
            MettaValue::Long(25),
        ]);
        env.add_to_space(&fact1);

        let fact2 = MettaValue::SExpr(vec![
            MettaValue::Atom("person".to_string()),
            MettaValue::Atom("bob".to_string()),
            MettaValue::Long(30),
        ]);
        env.add_to_space(&fact2);

        // Test basic match: (match & self (person $name $age) $name)
        let match_query = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("person".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("$age".to_string()),
            ]),
            MettaValue::Atom("$name".to_string()),
        ]);

        let (results, _) = eval(match_query, env);
        assert!(results.len() >= 2); // Should return both alice and bob
        assert!(results.contains(&MettaValue::Atom("alice".to_string())));
        assert!(results.contains(&MettaValue::Atom("bob".to_string())));
    }

    #[test]
    fn test_match_with_specific_patterns() {
        let mut env = Environment::new();

        // Add some facts
        let facts = vec![
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("coffee".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("bob".to_string()),
                MettaValue::Atom("tea".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("books".to_string()),
            ]),
        ];

        for fact in facts {
            env.add_to_space(&fact);
        }

        // Test specific match: (match & self (likes alice $what) $what)
        let specific_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("likes".to_string()),
                MettaValue::Atom("alice".to_string()),
                MettaValue::Atom("$what".to_string()),
            ]),
            MettaValue::Atom("$what".to_string()),
        ]);

        let (results, _) = eval(specific_match, env);
        assert!(results.len() >= 2); // Should return coffee and books
        assert!(results.contains(&MettaValue::Atom("coffee".to_string())));
        assert!(results.contains(&MettaValue::Atom("books".to_string())));
        assert!(!results.contains(&MettaValue::Atom("tea".to_string()))); // bob's preference
    }

    #[test]
    fn test_match_with_complex_templates() {
        let mut env = Environment::new();

        // Add facts
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("student".to_string()),
            MettaValue::Atom("john".to_string()),
            MettaValue::Atom("math".to_string()),
            MettaValue::Long(85),
        ]);
        env.add_to_space(&fact);

        // Test complex template: (match & self (student $name $subject $grade) (result $name scored $grade in $subject))
        let complex_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("student".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("$subject".to_string()),
                MettaValue::Atom("$grade".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Atom("$name".to_string()),
                MettaValue::Atom("scored".to_string()),
                MettaValue::Atom("$grade".to_string()),
                MettaValue::Atom("in".to_string()),
                MettaValue::Atom("$subject".to_string()),
            ]),
        ]);

        let (results, _) = eval(complex_match, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 6);
                assert_eq!(items[0], MettaValue::Atom("result".to_string()));
                assert_eq!(items[1], MettaValue::Atom("john".to_string()));
                assert_eq!(items[2], MettaValue::Atom("scored".to_string()));
                assert_eq!(items[3], MettaValue::Long(85));
                assert_eq!(items[4], MettaValue::Atom("in".to_string()));
                assert_eq!(items[5], MettaValue::Atom("math".to_string()));
            }
            _ => panic!("Expected complex template result"),
        }
    }

    #[test]
    fn test_match_error_cases() {
        let env = Environment::new();

        // Test match with insufficient arguments
        // Note: `& self` is preprocessed into `&self`, so (match & self) becomes (match &self)
        // which has only 1 argument after "match"
        let match_insufficient = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
        ]);
        let (results, _) = eval(match_insufficient, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("match"), "Expected 'match' in: {}", msg);
                assert!(
                    msg.contains("3 or 4 arguments"),
                    "Expected '3 or 4 arguments' in: {}",
                    msg
                );
                // After preprocessing `& self` -> `&self`, we have 1 arg
                assert!(msg.contains("got 1"), "Expected 'got 1' in: {}", msg);
                assert!(msg.contains("Usage:"), "Expected 'Usage:' in: {}", msg);
            }
            _ => panic!("Expected error for insufficient arguments"),
        }

        // Test match with wrong space reference
        let match_wrong_ref = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("wrong".to_string()), // Should be &
            MettaValue::Atom("self".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);
        let (results, _) = eval(match_wrong_ref, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("match requires & as first argument"));
            }
            _ => panic!("Expected error for wrong space reference"),
        }

        // Test match with unsupported space name (new-style syntax)
        // Note: With the new space_ref token, `& other` is preprocessed to `&other`
        // which triggers 3-arg new-style syntax, producing a "must be a space" error
        let match_wrong_space = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&other".to_string()), // Unrecognized space reference
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);
        let (results, _) = eval(match_wrong_space, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for unsupported space name"),
        }
    }

    #[test]
    fn test_rule_definition_with_errors_in_rhs() {
        let env = Environment::new();

        // Test rule with error in RHS: (= (error-func $x) (error "always fails" $x))
        let error_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error-func".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("always fails".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(error_rule, env);
        assert!(result.is_empty());

        // Test the error-producing rule
        let test_error = MettaValue::SExpr(vec![
            MettaValue::Atom("error-func".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(test_error, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, details) => {
                assert_eq!(msg, "always fails");
                assert_eq!(**details, MettaValue::Long(42));
            }
            _ => panic!("Expected error from rule"),
        }
    }

    #[test]
    fn test_rule_precedence_and_specificity() {
        let mut env = Environment::new();

        // Define general rule first: (= (test $x) (general $x))
        let general_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("general".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);
        let (_, env1) = eval(general_rule, env);
        env = env1;

        // Define specific rule: (= (test 42) specific-case)
        let specific_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("specific-case".to_string()),
        ]);
        let (_, env2) = eval(specific_rule, env);

        // Test that specific rule takes precedence: (test 42)
        let test_specific = MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, env3) = eval(test_specific, env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("specific-case".to_string()));

        // Test that general rule still works for other values: (test 100)
        let test_general = MettaValue::SExpr(vec![
            MettaValue::Atom("test".to_string()),
            MettaValue::Long(100),
        ]);
        let (results, _) = eval(test_general, env3);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items[0], MettaValue::Atom("general".to_string()));
                assert_eq!(items[1], MettaValue::Long(100));
            }
            _ => panic!("Expected general rule result"),
        }
    }

    #[test]
    fn test_recursive_rules() {
        let env = Environment::new();

        // Define recursive rule: (= (countdown $n) (if (> $n 0) (countdown (- $n 1)) done))
        let recursive_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("countdown".to_string()),
                MettaValue::Atom("$n".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$n".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("countdown".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("-".to_string()),
                        MettaValue::Atom("$n".to_string()),
                        MettaValue::Long(1),
                    ]),
                ]),
                MettaValue::Atom("done".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(recursive_rule, env);
        assert!(result.is_empty());

        // Test recursive execution: (countdown 0) should return "done"
        let test_base = MettaValue::SExpr(vec![
            MettaValue::Atom("countdown".to_string()),
            MettaValue::Long(0),
        ]);
        let (results, new_env2) = eval(test_base, new_env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("done".to_string()));

        // Test recursive call: (countdown 1) should eventually return "done"
        let test_recursive = MettaValue::SExpr(vec![
            MettaValue::Atom("countdown".to_string()),
            MettaValue::Long(1),
        ]);
        let (results, _) = eval(test_recursive, new_env2);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Atom("done".to_string()));
    }

    #[test]
    fn test_match_with_no_results() {
        let env = Environment::new();

        // Test match with pattern that doesn't match anything
        let no_match = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("self".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nonexistent".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(no_match, env);
        assert!(results.is_empty()); // No matches should return empty
    }

    #[test]
    fn test_rule_with_different_variable_types() {
        let env = Environment::new();

        // Test rule with different variable prefixes: (= (mixed $a &b 'c) (result $a &b 'c))
        let mixed_vars_rule = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("&b".to_string()),
                MettaValue::Atom("'c".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("result".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("&b".to_string()),
                MettaValue::Atom("'c".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(mixed_vars_rule, env);
        assert!(result.is_empty());

        // Test the mixed variables rule
        let test_mixed = MettaValue::SExpr(vec![
            MettaValue::Atom("mixed".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(test_mixed, new_env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 4);
                assert_eq!(items[0], MettaValue::Atom("result".to_string()));
                assert_eq!(items[1], MettaValue::Long(1));
                assert_eq!(items[2], MettaValue::Long(2));
                assert_eq!(items[3], MettaValue::Long(3));
            }
            _ => panic!("Expected result with mixed variables"),
        }
    }

    #[test]
    fn test_rule_definition_in_fact_database() {
        let env = Environment::new();

        // Define a rule and verify it's added to the fact database
        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("test-rule".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("processed".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        ]);

        let (result, new_env) = eval(rule_def.clone(), env);
        assert!(result.is_empty());

        // Verify the rule definition is in the fact database
        assert!(new_env.has_sexpr_fact(&rule_def));
    }

    // Tests for "Did You Mean" space name suggestions

    #[test]
    fn test_space_name_case_sensitivity_suggestion() {
        let env = Environment::new();

        // Test "&Self" (capital S) -> should error (unrecognized space reference)
        // Note: With the new space_ref token, &Self is a single atom, triggering new-style syntax
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&Self".to_string()), // Wrong case - combined as single token
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // New-style syntax produces different error (no suggestion)
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for unrecognized space reference"),
        }
    }

    #[test]
    fn test_space_name_typo_suggestion() {
        let env = Environment::new();

        // Test "&slef" (typo) -> should error (unrecognized space reference)
        // Note: With the new space_ref token, &slef is a single atom, triggering new-style syntax
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&slef".to_string()), // Typo - combined as single token
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // New-style syntax produces different error (no suggestion)
                assert!(
                    msg.contains("must be a space"),
                    "Expected 'must be a space' in: {}",
                    msg
                );
            }
            _ => panic!("Expected error for typo space reference"),
        }
    }

    #[test]
    fn test_space_name_no_suggestion_for_unrelated() {
        let env = Environment::new();

        // Test "foobar" -> no suggestion (too different from "self")
        let match_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("match".to_string()),
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("foobar".to_string()), // Completely different
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("template".to_string()),
        ]);

        let (results, _) = eval(match_expr, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                // Should NOT contain "Did you mean" for completely unrelated names
                assert!(
                    !msg.contains("Did you mean"),
                    "Should not have suggestion for unrelated name: {}",
                    msg
                );
            }
            _ => panic!("Expected error without suggestion"),
        }
    }
}
