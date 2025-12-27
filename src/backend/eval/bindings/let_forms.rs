//! Let binding forms for MeTTa evaluation.
//!
//! This module implements let binding operations:
//! - let: Basic variable binding with pattern matching
//! - let*: Sequential bindings
//! - let_step: TCO-enabled let for trampoline integration

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MettaValue};
use std::sync::Arc;
use tracing::trace;

use super::super::{apply_bindings, eval, pattern_match, EvalStep};

/// let*: Sequential bindings - (let* (($x 1) ($y (+ $x 1))) body)
/// Transforms to nested let: (let $x 1 (let $y (+ $x 1) body))
/// Each binding can use variables from previous bindings
pub(crate) fn eval_let_star(items: Vec<MettaValue>, env: Environment) -> EvalResult {
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
                    super::super::friendly_value_repr(bindings_expr)
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
                        super::super::friendly_value_repr(binding)
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

/// Generate helpful message for pattern mismatch in let bindings
pub(crate) fn pattern_mismatch_suggestion(pattern: &MettaValue, value: &MettaValue) -> String {
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
pub(crate) fn eval_let(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    let args = &items[1..];
    trace!(target: "mettatron::eval::eval_let", ?args, ?items);

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
                    super::super::friendly_value_repr(pattern),
                    super::super::friendly_value_repr(&value),
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
pub(crate) fn eval_let_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
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
