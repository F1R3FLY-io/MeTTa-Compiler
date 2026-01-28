//! Unification and variable sealing operations for MeTTa evaluation.
//!
//! This module implements:
//! - unify: Pattern unification with success/failure branches
//! - sealed: Create locally scoped variables by replacing free variables
//! - atom-subst: Variable substitution through pattern matching

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::super::{apply_bindings, eval, pattern_match, EvalStep};

/// Global counter for generating unique variable IDs in `sealed`
static SEALED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Step version of eval_unify that defers evaluation to trampoline.
/// This prevents stack overflow for deeply nested unify operations.
pub(crate) fn eval_unify_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
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
        return EvalStep::Done((vec![err], env));
    }

    let pattern1 = args[0].clone();
    let pattern2 = args[1].clone();
    let success_body = args[2].clone();
    let failure_body = args[3].clone();

    EvalStep::StartUnify {
        pattern1,
        pattern2,
        success_body,
        failure_body,
        env,
        depth,
    }
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
///
/// DEPRECATED: Use eval_unify_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_unify(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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
            results1
                .iter()
                .map(|v| match v {
                    MettaValue::Space(h) => format!("Space({})", h.name),
                    other => format!("{:?}", other),
                })
                .collect::<Vec<_>>()
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
                    pattern,
                    handle.is_module_space(),
                    is_boolean_check
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

                // For non-boolean patterns on module spaces, use match_space with deferred expansion
                // This benefits from bloom filter + head filtering with compressed multiplicity storage
                let lazy_matches = final_env.match_space(&pattern, &pattern);
                if lazy_matches.is_empty() {
                    // No matches - evaluate failure body
                    if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                        eprintln!("[DEBUG unify] NO MATCH FOUND (module space path)");
                    }
                    let (failure_results, failure_env) =
                        eval(failure_body.clone(), final_env.clone());
                    final_env = failure_env;
                    all_results.extend(failure_results);
                } else {
                    // Apply bindings and evaluate success body for each match (expand multiplicity on demand)
                    for matched_atom in lazy_matches.into_iter().flat_map(|m| m.expand()) {
                        if let Some(bindings) = pattern_match(&pattern, &matched_atom) {
                            if std::env::var("METTA_DEBUG_UNIFY").is_ok() {
                                eprintln!(
                                    "[DEBUG unify] MATCH (module): atom={:?}, bindings={:?}",
                                    matched_atom, bindings
                                );
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
                        eprintln!(
                            "[DEBUG unify] MATCH: atom={:?}, bindings={:?}",
                            atom, bindings
                        );
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
                        eprintln!(
                            "[DEBUG unify] MATCH (reverse): atom={:?}, bindings={:?}",
                            atom, bindings
                        );
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
                eprintln!("[DEBUG unify] NON-SPACE branch! val1={:?}", val1);
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
pub(crate) fn eval_sealed(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
fn collect_variables(expr: &MettaValue) -> HashSet<String> {
    let mut vars = HashSet::new();

    // Work stack: expressions to process
    let mut work_stack: Vec<&MettaValue> = Vec::with_capacity(16);
    work_stack.push(expr);

    while let Some(val) = work_stack.pop() {
        match val {
            MettaValue::Atom(name) if name.starts_with('$') => {
                vars.insert(name.clone());
            }
            MettaValue::SExpr(items) => {
                // Push all children onto work stack
                for item in items.iter().rev() {
                    work_stack.push(item);
                }
            }
            MettaValue::Conjunction(goals) => {
                for goal in goals.iter().rev() {
                    work_stack.push(goal);
                }
            }
            _ => {}
        }
    }

    vars
}

/// Replace variables in expr with unique versions, except those in ignore set
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
fn seal_variables(expr: &MettaValue, ignore: &HashSet<String>, unique_id: u64) -> MettaValue {
    // Fast path for simple cases
    match expr {
        MettaValue::Atom(name) if name.starts_with('$') && !ignore.contains(name) => {
            return MettaValue::Atom(format!("{}_{}", name, unique_id));
        }
        MettaValue::Atom(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::Bool(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Unit
        | MettaValue::Space(_)
        | MettaValue::State(_)
        | MettaValue::Type(_)
        | MettaValue::Memo(_)
        | MettaValue::Empty
        | MettaValue::Error(_, _) => return expr.clone(),
        // Compound types need iterative processing
        MettaValue::SExpr(_) | MettaValue::Conjunction(_) => {}
    }

    // Iterative implementation using explicit work stack
    seal_variables_iterative(expr, ignore, unique_id)
}

/// Work item for iterative seal_variables
enum SealWork<'a> {
    /// Process a value - may push more work
    Process(&'a MettaValue),
    /// Build an SExpr from the last N results
    BuildSExpr(usize),
    /// Build a Conjunction from the last N results
    BuildConjunction(usize),
}

/// Iterative implementation of seal_variables using explicit work stack.
fn seal_variables_iterative(
    expr: &MettaValue,
    ignore: &HashSet<String>,
    unique_id: u64,
) -> MettaValue {
    // Work stack: items to process
    let mut work_stack: Vec<SealWork> = Vec::with_capacity(32);
    // Result stack: processed results
    let mut result_stack: Vec<MettaValue> = Vec::with_capacity(32);

    work_stack.push(SealWork::Process(expr));

    while let Some(work) = work_stack.pop() {
        match work {
            SealWork::Process(val) => {
                match val {
                    // Variable replacement (if not ignored)
                    MettaValue::Atom(name) if name.starts_with('$') && !ignore.contains(name) => {
                        result_stack.push(MettaValue::Atom(format!("{}_{}", name, unique_id)));
                    }
                    // S-expression: push build marker, then push children in reverse order
                    MettaValue::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push(val.clone());
                        } else {
                            work_stack.push(SealWork::BuildSExpr(items.len()));
                            for item in items.iter().rev() {
                                work_stack.push(SealWork::Process(item));
                            }
                        }
                    }
                    // Conjunction: similar to SExpr
                    MettaValue::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push(val.clone());
                        } else {
                            work_stack.push(SealWork::BuildConjunction(goals.len()));
                            for goal in goals.iter().rev() {
                                work_stack.push(SealWork::Process(goal));
                            }
                        }
                    }
                    // All other types: pass through unchanged
                    _ => {
                        result_stack.push(val.clone());
                    }
                }
            }
            SealWork::BuildSExpr(count) => {
                let start = result_stack.len() - count;
                let children: Vec<MettaValue> = result_stack.drain(start..).collect();
                result_stack.push(MettaValue::SExpr(children));
            }
            SealWork::BuildConjunction(count) => {
                let start = result_stack.len() - count;
                let children: Vec<MettaValue> = result_stack.drain(start..).collect();
                result_stack.push(MettaValue::Conjunction(children));
            }
        }
    }

    // Final result should be on the stack
    debug_assert_eq!(
        result_stack.len(),
        1,
        "seal_variables should produce exactly one result"
    );
    result_stack.pop().expect("Result stack should not be empty")
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
pub(crate) fn eval_atom_subst(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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
