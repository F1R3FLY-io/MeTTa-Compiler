use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::{apply_bindings, eval, pattern_match, EvalStep};

/// Global counter for generating unique variable IDs in `sealed`
static SEALED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// let*: Sequential bindings - (let* (($x 1) ($y (+ $x 1))) body)
/// Transforms to nested let: (let $x 1 (let $y (+ $x 1) body))
/// Each binding can use variables from previous bindings
pub(super) fn eval_let_star(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    if args.len() < 2 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "let* requires at least 2 arguments (bindings and body), got {}. Usage: (let* ((pattern value) ...) body)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let bindings_expr = &args[0];
    let body = &args[1];

    // Extract bindings list
    let bindings = match bindings_expr {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => {
            // Empty bindings - just evaluate body
            return eval(body.clone(), env);
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "let* bindings must be a list, got {}. Usage: (let* ((pattern value) ...) body)",
                    super::friendly_value_repr(bindings_expr)
                ),
                Arc::new(bindings_expr.clone()),
            );
            return (vec![err], env);
        }
    };

    if bindings.is_empty() {
        // No bindings - just evaluate body
        return eval(body.clone(), env);
    }

    // Transform to nested let
    // (let* ((a 1) (b 2) (c 3)) body) -> (let a 1 (let b 2 (let c 3 body)))
    let mut result_body = body.clone();

    // Process bindings in reverse order to build nested structure
    for binding in bindings.iter().rev() {
        match binding {
            MettaValue::SExpr(pair) if pair.len() == 2 => {
                let pattern = &pair[0];
                let value = &pair[1];

                result_body = MettaValue::SExpr(vec![
                    MettaValue::Atom("let".to_string()),
                    pattern.clone(),
                    value.clone(),
                    result_body,
                ]);
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "let* binding must be (pattern value) pair, got {}. Usage: (let* ((pattern value) ...) body)",
                        super::friendly_value_repr(binding)
                    ),
                    Arc::new(binding.clone()),
                );
                return (vec![err], env);
            }
        }
    }

    // Evaluate the nested let structure
    eval(result_body, env)
}

/// unify: Pattern unification with success/failure branches
/// (unify pattern1 pattern2 success-body failure-body)
///
/// If pattern1 and pattern2 can be unified, evaluates success-body with bindings
/// Otherwise evaluates failure-body
///
/// HE-compatible space-aware behavior:
/// (unify space pattern success-body failure-body)
/// When the first argument is a space (like &kb), searches all atoms in the space
/// for ones matching the pattern, and evaluates success-body for each match.
pub(super) fn eval_unify(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    if args.len() < 4 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "unify requires 4 arguments, got {}. Usage: (unify pattern1 pattern2 success failure)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern1 = &args[0];
    let pattern2 = &args[1];
    let success_body = &args[2];
    let failure_body = &args[3];

    // First evaluate the first argument to see if it's a space
    let (results1, env1) = eval(pattern1.clone(), env);
    if results1.is_empty() {
        return eval(failure_body.clone(), env1);
    }

    // DEBUG: Log what results1 contains
    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
        eprintln!(
            "[DEBUG unify] results1 count={}, values={:?}",
            results1.len(),
            results1.iter().map(|v| match v {
                MettaValue::Space(h) => format!("Space({})", h.name),
                other => format!("{:?}", other)
            }).collect::<Vec<_>>()
        );
    }

    let mut all_results = Vec::new();
    let mut final_env = env1.clone();

    // OPTIMIZATION: Detect boolean existence check pattern: (unify &space pattern True False)
    // This avoids iterating all atoms when we only need to check if ANY match exists
    let is_boolean_check = match (success_body, failure_body) {
        (MettaValue::Bool(true), MettaValue::Bool(false)) => true,
        (MettaValue::Atom(s), MettaValue::Atom(f)) if s == "True" && f == "False" => true,
        _ => false,
    };

    for val1 in results1 {
        // HE-compatible: If val1 is a Space, search atoms in the space
        if let MettaValue::Space(ref handle) = val1 {
            // Pattern2 is treated as a pattern to match against space atoms
            // It should NOT be evaluated - it's a pattern template
            let pattern = pattern2.clone();

            // DEBUG: Log pattern and space info
            if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                eprintln!(
                    "[DEBUG unify] Space query: pattern={:?}, is_module={}, is_boolean_check={}",
                    pattern, handle.is_module_space(), is_boolean_check
                );
            }

            // OPTIMIZATION: For module-backed spaces, use Environment's optimized match functions
            // which have bloom filter pre-filtering and early-exit capabilities
            if handle.is_module_space() || handle.name == "self" {
                // FAST PATH: Boolean existence check uses match_space_exists (early exit)
                if is_boolean_check {
                    let exists = final_env.match_space_exists(&pattern);
                    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                        eprintln!("[DEBUG unify] Boolean check result: exists={}", exists);
                    }
                    let result = if exists {
                        MettaValue::Bool(true)
                    } else {
                        MettaValue::Bool(false)
                    };
                    all_results.push(result);
                    continue;
                }

                // For non-boolean patterns on module spaces, use match_space for all matches
                // This path benefits from bloom filter + head filtering but returns all matches
                let matches = final_env.match_space(&pattern, &pattern);
                if matches.is_empty() {
                    // No matches - evaluate failure body
                    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                        eprintln!("[DEBUG unify] NO MATCH FOUND (module space path)");
                    }
                    let (failure_results, failure_env) = eval(failure_body.clone(), final_env.clone());
                    final_env = failure_env;
                    all_results.extend(failure_results);
                } else {
                    // Apply bindings and evaluate success body for each match
                    for matched_atom in matches {
                        if let Some(bindings) = pattern_match(&pattern, &matched_atom) {
                            if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                                eprintln!("[DEBUG unify] MATCH (module): atom={:?}, bindings={:?}", matched_atom, bindings);
                            }
                            let instantiated = apply_bindings(success_body, &bindings).into_owned();
                            let (body_results, body_env) = eval(instantiated, final_env.clone());
                            final_env = body_env;
                            all_results.extend(body_results);
                        } else if let Some(bindings) = pattern_match(&matched_atom, &pattern) {
                            // Try reverse direction
                            let instantiated = apply_bindings(success_body, &bindings).into_owned();
                            let (body_results, body_env) = eval(instantiated, final_env.clone());
                            final_env = body_env;
                            all_results.extend(body_results);
                        }
                    }
                }
                continue;
            }

            // SLOW PATH: Owned spaces (from new-space) - use collapse() and iterate
            let space_atoms = handle.collapse();
            if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                eprintln!("[DEBUG unify] Owned space, {} atoms", space_atoms.len());
            }

            let mut found_match = false;
            for atom in &space_atoms {
                // Try to match the pattern against this atom
                if let Some(bindings) = pattern_match(&pattern, atom) {
                    found_match = true;
                    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                        eprintln!("[DEBUG unify] MATCH: atom={:?}, bindings={:?}", atom, bindings);
                    }
                    // Apply bindings and evaluate success body
                    let instantiated = apply_bindings(success_body, &bindings).into_owned();
                    let (body_results, body_env) = eval(instantiated, final_env.clone());
                    final_env = body_env;
                    all_results.extend(body_results);
                } else if let Some(bindings) = pattern_match(atom, &pattern) {
                    // Try reverse direction too
                    found_match = true;
                    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                        eprintln!("[DEBUG unify] MATCH (reverse): atom={:?}, bindings={:?}", atom, bindings);
                    }
                    let instantiated = apply_bindings(success_body, &bindings).into_owned();
                    let (body_results, body_env) = eval(instantiated, final_env.clone());
                    final_env = body_env;
                    all_results.extend(body_results);
                }
            }

            // If no matches found, evaluate failure body once
            if !found_match {
                if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                    eprintln!(
                        "[DEBUG unify] NO MATCH FOUND - evaluating failure. pattern={:?}, atoms={:?}",
                        pattern, space_atoms
                    );
                }
                let (failure_results, failure_env) = eval(failure_body.clone(), final_env.clone());
                final_env = failure_env;
                all_results.extend(failure_results);
            }
        } else {
            // Normal unification (not space-aware)
            if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                eprintln!(
                    "[DEBUG unify] NON-SPACE branch! val1={:?}",
                    val1
                );
            }
            let (results2, env2) = eval(pattern2.clone(), final_env.clone());
            final_env = env2.clone();

            for val2 in results2 {
                // Try to unify val1 and val2
                // First try pattern_match in one direction
                if let Some(bindings) = pattern_match(&val1, &val2) {
                    // Apply bindings and evaluate success body
                    let instantiated = apply_bindings(success_body, &bindings).into_owned();
                    let (body_results, body_env) = eval(instantiated, env2.clone());
                    final_env = body_env;
                    all_results.extend(body_results);
                } else if let Some(bindings) = pattern_match(&val2, &val1) {
                    // Try the other direction
                    let instantiated = apply_bindings(success_body, &bindings).into_owned();
                    let (body_results, body_env) = eval(instantiated, env2.clone());
                    final_env = body_env;
                    all_results.extend(body_results);
                } else {
                    // Unification failed - evaluate failure body
                    let (failure_results, failure_env) = eval(failure_body.clone(), env2.clone());
                    final_env = failure_env;
                    all_results.extend(failure_results);
                }
            }
        }
    }

    if all_results.is_empty() {
        eval(failure_body.clone(), final_env)
    } else {
        (all_results, final_env)
    }
}

/// Generate helpful message for pattern mismatch in let bindings
fn pattern_mismatch_suggestion(pattern: &MettaValue, value: &MettaValue) -> String {
    let pattern_arity = match pattern {
        MettaValue::SExpr(items) => items.len(),
        _ => 1,
    };
    let value_arity = match value {
        MettaValue::SExpr(items) => items.len(),
        _ => 1,
    };

    // Check for arity mismatch
    if pattern_arity != value_arity {
        return format!(
            "Hint: pattern has {} element(s) but value has {}. Adjust pattern to match value structure.",
            pattern_arity, value_arity
        );
    }

    // Check for structure mismatch (different head atoms)
    if let (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) = (pattern, value) {
        if let (Some(MettaValue::Atom(p_head)), Some(MettaValue::Atom(v_head))) =
            (p_items.first(), v_items.first())
        {
            if p_head != v_head {
                return format!(
                    "Hint: pattern head '{}' doesn't match value head '{}'.",
                    p_head, v_head
                );
            }
        }
    }

    // Check for literal mismatch inside structures
    if let (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) = (pattern, value) {
        for (i, (p, v)) in p_items.iter().zip(v_items.iter()).enumerate() {
            // Skip if pattern is a variable (starts with $, &, or ')
            if let MettaValue::Atom(name) = p {
                if name.starts_with('$')
                    || name.starts_with('&')
                    || name.starts_with('\'')
                    || name == "_"
                {
                    continue;
                }
            }
            // Check for literal mismatch
            if p != v && !matches!(p, MettaValue::SExpr(_)) {
                return format!(
                    "Hint: element at position {} doesn't match - pattern has {:?} but value has {:?}.",
                    i, p, v
                );
            }
        }
    }

    // Default hint
    "Hint: pattern structure doesn't match value. Check that variable names align with value positions.".to_string()
}

/// Evaluate let binding: (let pattern value body)
/// Evaluates value, binds it to pattern, and evaluates body with those bindings
/// Supports both simple variable binding and pattern matching:
///   - (let $x 42 body) - simple binding
///   - (let ($a $b) (tuple 1 2) body) - destructuring pattern
///
/// IMPORTANT: This function propagates environment changes (including state mutations)
/// through each iteration to ensure side effects like change-state! are visible.
pub(super) fn eval_let(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];

    if args.len() < 3 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let value_expr = &args[1];
    let body = &args[2];

    // Evaluate the value expression first
    let (value_results, mut current_env) = eval(value_expr.clone(), env);

    // Handle nondeterminism: if value evaluates to multiple results, try each one
    let mut all_results = Vec::new();

    for value in value_results {
        // Try to match the pattern against the value
        if let Some(bindings) = pattern_match(pattern, &value) {
            // Apply bindings to the body and evaluate it
            // Propagate environment through iterations to preserve state changes
            let instantiated_body = apply_bindings(body, &bindings).into_owned();
            let (body_results, body_env) = eval(instantiated_body, current_env);
            current_env = body_env;
            all_results.extend(body_results);
        } else {
            // Pattern match failure - return Empty (HE-compatible)
            // In strict mode, print a warning with helpful diagnostics to stderr
            if current_env.is_strict_mode() {
                let suggestion = pattern_mismatch_suggestion(pattern, &value);
                eprintln!(
                    "Warning: let pattern {} does not match value {}. {}",
                    super::friendly_value_repr(pattern),
                    super::friendly_value_repr(&value),
                    suggestion
                );
            }
            // Return Empty (no results) - allows nondeterministic alternatives to be tried
            // In HE, let is defined as: (= (let $pattern $atom $template) (unify $atom $pattern $template Empty))
        }
    }

    (all_results, current_env)
}

/// Evaluate let binding with trampoline integration (TCO-enabled)
/// Returns EvalStep::StartLetBinding to defer evaluation to the trampoline,
/// enabling the let body to participate in tail call optimization.
///
/// This is the TCO-enabled version of eval_let(). Instead of calling eval()
/// directly for the value and body, it returns an EvalStep that the trampoline
/// will process, preventing nested trampolines.
pub(super) fn eval_let_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    let args = &items[1..];

    // Validate arity - same as eval_let
    if args.len() < 3 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                got
            ),
            Arc::new(MettaValue::SExpr(args.to_vec())),
        );
        return EvalStep::Done((vec![err], env));
    }

    let pattern = args[0].clone();
    let value_expr = args[1].clone();
    let body = args[2].clone();

    // Return EvalStep to start let binding evaluation via trampoline
    // The trampoline will:
    // 1. Create a ProcessLet continuation
    // 2. Push value_expr evaluation
    // 3. When value eval completes, match pattern and evaluate body
    EvalStep::StartLetBinding {
        pattern,
        value_expr,
        body,
        env,
        depth,
    }
}

/// sealed: Create locally scoped variables by replacing free variables with unique ones
/// Usage: (sealed ignore-vars expr)
///
/// HE-compatible behavior:
/// - Takes a list of variables to preserve (ignore-vars)
/// - Replaces all other variables in expr with unique versions
/// - Critical for preventing variable capture in higher-order functions
///
/// Example:
/// ```metta
/// !(sealed ($x) (foo $x $y $z))
/// ; → (foo $x $y_123 $z_123)  ; $x preserved, $y and $z made unique
/// ```
pub(super) fn eval_sealed(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    if items.len() < 3 {
        let got = items.len() - 1;
        let err = MettaValue::Error(
            format!(
                "sealed requires 2 arguments, got {}. Usage: (sealed ignore-vars expr)",
                got
            ),
            Arc::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let ignore_vars = &items[1]; // Variables to preserve
    let expr = &items[2]; // Expression to seal

    // 1. Collect variables to ignore (from first arg, typically ($x $y))
    let ignore_set = collect_variables(ignore_vars);

    // 2. Generate unique variable ID using atomic counter
    let unique_id = SEALED_COUNTER.fetch_add(1, Ordering::SeqCst);

    // 3. Recursively replace free variables NOT in ignore_set
    let sealed_expr = seal_variables(expr, &ignore_set, unique_id);

    (vec![sealed_expr], env)
}

/// Collect all variable names from an expression (variables start with $)
fn collect_variables(expr: &MettaValue) -> HashSet<String> {
    let mut vars = HashSet::new();
    collect_variables_impl(expr, &mut vars);
    vars
}

fn collect_variables_impl(expr: &MettaValue, vars: &mut HashSet<String>) {
    match expr {
        MettaValue::Atom(name) if name.starts_with('$') => {
            vars.insert(name.clone());
        }
        MettaValue::SExpr(items) => {
            for item in items {
                collect_variables_impl(item, vars);
            }
        }
        _ => {}
    }
}

/// Recursively replace variables in expr with unique versions, except those in ignore set
fn seal_variables(expr: &MettaValue, ignore: &HashSet<String>, unique_id: u64) -> MettaValue {
    match expr {
        MettaValue::Atom(name) if name.starts_with('$') && !ignore.contains(name) => {
            // Replace $var with $var_unique_id
            MettaValue::Atom(format!("{}_{}", name, unique_id))
        }
        MettaValue::SExpr(items) => MettaValue::SExpr(
            items
                .iter()
                .map(|item| seal_variables(item, ignore, unique_id))
                .collect(),
        ),
        MettaValue::Conjunction(goals) => MettaValue::Conjunction(
            goals
                .iter()
                .map(|goal| seal_variables(goal, ignore, unique_id))
                .collect(),
        ),
        // All other values pass through unchanged
        _ => expr.clone(),
    }
}

/// atom-subst: Variable substitution through pattern matching
/// Usage: (atom-subst value $var template)
///
/// HE-compatible behavior:
/// - Substitutes value for $var in template via pattern matching
/// - Uses the same binding mechanism as let/unify
///
/// Example:
/// ```metta
/// !(atom-subst 42 $x (+ $x 1))
/// ; → (+ 42 1)
/// ```
pub(super) fn eval_atom_subst(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    if items.len() < 4 {
        let got = items.len() - 1;
        let err = MettaValue::Error(
            format!(
                "atom-subst requires 3 arguments, got {}. Usage: (atom-subst value $var template)",
                got
            ),
            Arc::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let value = &items[1];
    let var = &items[2];
    let template = &items[3];

    // Use pattern matching semantics: bind value to var, apply to template
    if let Some(bindings) = pattern_match(var, value) {
        let instantiated = apply_bindings(template, &bindings).into_owned();
        (vec![instantiated], env)
    } else {
        // Pattern didn't match - return empty (nondeterministic failure)
        // This shouldn't happen with a simple variable pattern like $x
        (vec![], env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_let_simple_binding() {
        let env = Environment::new();

        // (let $x 42 $x)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_let_with_expression() {
        let env = Environment::new();

        // (let $y (+ 10 5) (* $y 2))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$y".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(10),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$y".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30));
    }

    #[test]
    fn test_let_with_pattern_matching() {
        let env = Environment::new();

        // (let (tuple $a $b) (tuple 1 2) (+ $a $b))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_let_nested() {
        let env = Environment::new();

        // (let $z 3 (let $w 4 (+ $z $w)))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$z".to_string()),
            MettaValue::Long(3),
            MettaValue::SExpr(vec![
                MettaValue::Atom("let".to_string()),
                MettaValue::Atom("$w".to_string()),
                MettaValue::Long(4),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$z".to_string()),
                    MettaValue::Atom("$w".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(7));
    }

    #[test]
    fn test_let_with_if() {
        let env = Environment::new();

        // (let $base 10 (if (> $base 5) (* $base 2) $base))
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$base".to_string()),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(5),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(2),
                ]),
                MettaValue::Atom("$base".to_string()),
            ]),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(20));
    }

    #[test]
    fn test_let_pattern_mismatch() {
        let env = Environment::new();

        // (let (foo $x) (bar 42) $x) - pattern mismatch returns Empty (HE-compatible)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        // HE-compatible: pattern mismatch returns Empty (no results)
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_let_with_wildcard_pattern() {
        let env = Environment::new();

        // (let _ 42 "ignored")
        // Wildcard should match anything but not bind
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("_".to_string()),
            MettaValue::Long(42),
            MettaValue::String("ignored".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("ignored".to_string()));
    }

    #[test]
    fn test_let_with_complex_pattern_structures() {
        let env = Environment::new();

        // (let (nested (inner $x $y) $z) (nested (inner 1 2) 3) (+ $x (+ $y $z)))
        let complex_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nested".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("inner".to_string()),
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Atom("$y".to_string()),
                ]),
                MettaValue::Atom("$z".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nested".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("inner".to_string()),
                    MettaValue::Long(1),
                    MettaValue::Long(2),
                ]),
                MettaValue::Long(3),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("$y".to_string()),
                    MettaValue::Atom("$z".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(complex_pattern, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6)); // 1 + (2 + 3)
    }

    #[test]
    fn test_let_with_variable_consistency() {
        let env = Environment::new();

        // Test that same variable in pattern must match same value
        // (let (same $x $x) (same 5 5) (* $x 2))
        let consistent_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(consistent_vars, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10)); // 5 * 2

        // Test inconsistent variables - returns Empty (HE-compatible)
        // (let (same $x $x) (same 5 7) (* $x 2))
        let inconsistent_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("same".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(7), // Different value - pattern doesn't match
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);

        let (results, _) = eval(inconsistent_vars, env);
        // HE-compatible: pattern mismatch returns Empty (no results)
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_let_with_different_variable_types() {
        let env = Environment::new();

        // Test different variable prefixes: $, &, '
        let mixed_vars = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("triple".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("&y".to_string()),
                MettaValue::Atom("'z".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("triple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    MettaValue::Atom("&y".to_string()),
                    MettaValue::Atom("'z".to_string()),
                ]),
            ]),
        ]);

        let (results, _) = eval(mixed_vars, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(6)); // 1 + (2 + 3)
    }

    #[test]
    fn test_let_missing_arguments() {
        let env = Environment::new();

        // Test let with only 2 arguments
        let let_two_args = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(42),
        ]);
        let (results, _) = eval(let_two_args, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 2"), "Expected 'got 2' in: {}", msg);
                assert!(msg.contains("Usage:"), "Expected 'Usage:' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }

        // Test let with only 1 argument
        let let_one_arg = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let (results, _) = eval(let_one_arg, env.clone());
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 1"), "Expected 'got 1' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }

        // Test let with no arguments
        let let_no_args = MettaValue::SExpr(vec![MettaValue::Atom("let".to_string())]);
        let (results, _) = eval(let_no_args, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert!(msg.contains("let"), "Expected 'let' in: {}", msg);
                assert!(
                    msg.contains("3 arguments"),
                    "Expected '3 arguments' in: {}",
                    msg
                );
                assert!(msg.contains("got 0"), "Expected 'got 0' in: {}", msg);
            }
            _ => panic!("Expected error for missing arguments"),
        }
    }

    #[test]
    fn test_let_with_evaluated_value_expression() {
        let env = Environment::new();

        // Test let where value needs evaluation
        // (let $result (+ (* 3 4) 5) (if (> $result 10) "big" "small"))
        let eval_value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$result".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("*".to_string()),
                    MettaValue::Long(3),
                    MettaValue::Long(4),
                ]),
                MettaValue::Long(5),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$result".to_string()),
                    MettaValue::Long(10),
                ]),
                MettaValue::String("big".to_string()),
                MettaValue::String("small".to_string()),
            ]),
        ]);

        let (results, _) = eval(eval_value, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("big".to_string())); // 17 > 10
    }

    #[test]
    fn test_let_with_error_in_value() {
        let env = Environment::new();

        // Test let where value expression produces error
        // (let $x (error "value-error" nil) $x)
        let error_value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$x".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("error".to_string()),
                MettaValue::String("value-error".to_string()),
                MettaValue::Nil,
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(error_value, env);
        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::Error(msg, _) => {
                assert_eq!(msg, "value-error");
            }
            _ => panic!("Expected error to be bound and returned"),
        }
    }

    // === Tests for pattern mismatch scenarios (HE-compatible Empty semantics) ===
    // Note: In strict mode, these would log warnings. Here we just verify Empty is returned.

    #[test]
    fn test_pattern_mismatch_arity_hint() {
        let env = Environment::new();

        // (let ($a $b) (tuple 1 2 3) ...) - pattern has 2 elements, value has 4
        // Pattern mismatch returns Empty (HE-compatible)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("$a".to_string()),
                MettaValue::Atom("$b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("tuple".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Atom("$a".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
    }

    #[test]
    fn test_pattern_mismatch_head_hint() {
        let env = Environment::new();

        // (let (foo $x) (bar 42) $x) - head atoms don't match
        // Pattern mismatch returns Empty (HE-compatible)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("foo".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("bar".to_string()),
                MettaValue::Long(42),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
    }

    #[test]
    fn test_pattern_mismatch_literal_hint() {
        let env = Environment::new();

        // (let (pair 42 $x) (pair 99 hello) $x) - literal 42 doesn't match 99
        // Pattern mismatch returns Empty (HE-compatible)
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("pair".to_string()),
                MettaValue::Long(99),
                MettaValue::Atom("hello".to_string()),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(value, env);
        assert_eq!(results.len(), 0); // HE-compatible: Empty on pattern mismatch
    }

    #[test]
    fn test_let_with_mixed_pattern_elements() {
        let env = Environment::new();

        // Pattern with mix of literals and variables
        // (let (mixed 42 $x "literal" $y) (mixed 42 100 "literal" 200) (+ $x $y))
        let mixed_pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
                MettaValue::String("literal".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Long(100),
                MettaValue::String("literal".to_string()),
                MettaValue::Long(200),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);

        let (results, _) = eval(mixed_pattern, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(300)); // 100 + 200

        // Test failure case where literal doesn't match - returns Empty (HE-compatible)
        // (let (mixed 42 $x "literal" $y) (mixed 43 100 "literal" 200) (+ $x $y))
        let mixed_fail = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(42),
                MettaValue::Atom("$x".to_string()),
                MettaValue::String("literal".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mixed".to_string()),
                MettaValue::Long(43), // Different literal - pattern doesn't match
                MettaValue::Long(100),
                MettaValue::String("literal".to_string()),
                MettaValue::Long(200),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$y".to_string()),
            ]),
        ]);

        let (results, _) = eval(mixed_fail, env);
        // HE-compatible: pattern mismatch returns Empty (no results)
        assert_eq!(results.len(), 0);
    }

    #[test]
    fn test_let_with_complex_body_expressions() {
        let env = Environment::new();

        // Test let with complex body containing multiple operations
        // (let $base 5
        //   (if (> $base 0)
        //     (let $squared (* $base $base)
        //       (+ $squared $base))
        //     0))
        let complex_body = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("$base".to_string()),
            MettaValue::Long(5),
            MettaValue::SExpr(vec![
                MettaValue::Atom("if".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::Atom(">".to_string()),
                    MettaValue::Atom("$base".to_string()),
                    MettaValue::Long(0),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::Atom("let".to_string()),
                    MettaValue::Atom("$squared".to_string()),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("*".to_string()),
                        MettaValue::Atom("$base".to_string()),
                        MettaValue::Atom("$base".to_string()),
                    ]),
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("+".to_string()),
                        MettaValue::Atom("$squared".to_string()),
                        MettaValue::Atom("$base".to_string()),
                    ]),
                ]),
                MettaValue::Long(0),
            ]),
        ]);

        let (results, _) = eval(complex_body, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(30)); // (5 * 5) + 5 = 30
    }

    #[test]
    fn test_let_star_with_discard_pattern() {
        // Test that wildcard _ works as a discard pattern in let*
        // This is the proper way to discard values
        let env = Environment::new();

        // (let* ((_ 42)) "success") should succeed, discarding 42
        let discard_binding = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
                MettaValue::Long(42),
            ])]),
            MettaValue::String("success".to_string()),
        ]);

        let (results, _) = eval(discard_binding, env.clone());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("success".to_string()));
    }

    #[test]
    fn test_let_star_with_discard_and_binding() {
        // (let* ((_ (+ 1 2)) ($x 5)) $x) should return 5
        let env = Environment::new();

        let mixed_bindings = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![
                // First binding: discard the result of (+ 1 2)
                MettaValue::SExpr(vec![
                    MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
                    MettaValue::SExpr(vec![
                        MettaValue::Atom("+".to_string()),
                        MettaValue::Long(1),
                        MettaValue::Long(2),
                    ]),
                ]),
                // Second binding: $x = 5
                MettaValue::SExpr(vec![
                    MettaValue::Atom("$x".to_string()),
                    MettaValue::Long(5),
                ]),
            ]),
            MettaValue::Atom("$x".to_string()),
        ]);

        let (results, _) = eval(mixed_bindings, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(5));
    }

    #[test]
    fn test_let_with_discard_pattern() {
        // (let _ "any-value" "ok") should succeed
        let env = Environment::new();

        let discard_let = MettaValue::SExpr(vec![
            MettaValue::Atom("let".to_string()),
            MettaValue::Atom("_".to_string()), // _ - wildcard discard pattern
            MettaValue::String("any-value".to_string()),
            MettaValue::String("ok".to_string()),
        ]);

        let (results, _) = eval(discard_let, env);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::String("ok".to_string()));
    }

    #[test]
    fn test_let_star_wildcard_matches_any_type() {
        // Test that _ wildcard matches different types in let*
        let env = Environment::new();

        // Discard a string
        let discard_string = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("_".to_string()), // _ - wildcard
                MettaValue::String("ignored".to_string()),
            ])]),
            MettaValue::Long(1),
        ]);
        let (results, _) = eval(discard_string, env.clone());
        assert_eq!(results[0], MettaValue::Long(1));

        // Discard a boolean
        let discard_bool = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("_".to_string()), // _ - wildcard
                MettaValue::Bool(true),
            ])]),
            MettaValue::Long(2),
        ]);
        let (results, _) = eval(discard_bool, env.clone());
        assert_eq!(results[0], MettaValue::Long(2));

        // Discard an S-expression
        let discard_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("let*".to_string()),
            MettaValue::SExpr(vec![MettaValue::SExpr(vec![
                MettaValue::Atom("_".to_string()), // _ - wildcard
                MettaValue::SExpr(vec![
                    MettaValue::Atom("some".to_string()),
                    MettaValue::Atom("expression".to_string()),
                ]),
            ])]),
            MettaValue::Long(3),
        ]);
        let (results, _) = eval(discard_sexpr, env);
        assert_eq!(results[0], MettaValue::Long(3));
    }
}
