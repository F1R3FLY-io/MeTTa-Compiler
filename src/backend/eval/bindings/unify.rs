//! Unification and variable sealing operations for MeTTa evaluation.
//!
//! This module implements:
//! - unify: Pattern unification with success/failure branches
//! - sealed: Create locally scoped variables by replacing free variables
//! - atom-subst: Variable substitution through pattern matching

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue, MettaValueInner};
use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::{apply_bindings, pattern_match, EvalStep};

/// Global counter for generating unique variable IDs in `sealed`
static SEALED_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Step version of eval_unify that defers evaluation to trampoline.
/// This prevents stack overflow for deeply nested unify operations.
pub(crate) fn eval_unify_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    let args = &items[1..];

    if args.len() < 4 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "unify requires 4 arguments, got {}. Usage: (unify pattern1 pattern2 success failure)",
                got
            ),
            MettaValue::SExpr(args.to_vec()),
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
            MettaValue::SExpr(items),
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
        match val.inner() {
            MettaValueInner::Atom(name) if name.starts_with('$') => {
                vars.insert(name.clone());
            }
            MettaValueInner::SExpr(items) => {
                // Push all children onto work stack
                for item in items.iter().rev() {
                    work_stack.push(item);
                }
            }
            MettaValueInner::Conjunction(goals) => {
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
    match expr.inner() {
        MettaValueInner::Atom(name) if name.starts_with('$') && !ignore.contains(name) => {
            return MettaValue::Atom(format!("{}_{}", name, unique_id));
        }
        MettaValueInner::Atom(_)
        | MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::Bool(_)
        | MettaValueInner::String(_)
        | MettaValueInner::Nil
        | MettaValueInner::Unit
        | MettaValueInner::Space(_)
        | MettaValueInner::State(_)
        | MettaValueInner::Type(_)
        | MettaValueInner::Memo(_)
        | MettaValueInner::Empty
        | MettaValueInner::Error(_, _) => return expr.clone(),
        // Compound types need iterative processing
        MettaValueInner::SExpr(_) | MettaValueInner::Conjunction(_) => {}
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
                match val.inner() {
                    // Variable replacement (if not ignored)
                    MettaValueInner::Atom(name)
                        if name.starts_with('$') && !ignore.contains(name) =>
                    {
                        result_stack.push(MettaValue::Atom(format!("{}_{}", name, unique_id)));
                    }
                    // S-expression: push build marker, then push children in reverse order
                    MettaValueInner::SExpr(items) => {
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
                    MettaValueInner::Conjunction(goals) => {
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
    result_stack
        .pop()
        .expect("Result stack should not be empty")
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
            MettaValue::SExpr(items),
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
