//! Let binding forms for MeTTa evaluation.
//!
//! This module implements let binding operations:
//! - let: Basic variable binding with pattern matching
//! - let*: Sequential bindings
//! - let_step: TCO-enabled let for trampoline integration

use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::EvalStep;

/// let* step version: Sequential bindings - (let* (($x 1) ($y (+ $x 1))) body)
/// Transforms to nested let: (let $x 1 (let $y (+ $x 1) body))
/// Each binding can use variables from previous bindings
///
/// This version returns EvalStep to enable tail call optimization via the trampoline.
pub(crate) fn eval_let_star_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    let args = &items[1..];

    if args.len() < 2 {
        let got = args.len();
        let err = MettaValue::Error(
            format!(
                "let* requires at least 2 arguments (bindings and body), got {}. Usage: (let* ((pattern value) ...) body)",
                got
            ),
            MettaValue::SExpr(args.to_vec()),
        );
        return EvalStep::Done((vec![err], env));
    }

    let bindings_expr = &args[0];
    let body = &args[1];

    // Extract bindings list
    let bindings = match bindings_expr.inner() {
        MettaValueInner::SExpr(items) => items,
        MettaValueInner::Nil => {
            // Empty bindings - evaluate body via trampoline (tail call)
            return EvalStep::EvalIfBranch {
                branch: body.clone(),
                env,
                depth,
            };
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "let* bindings must be a list, got {}. Usage: (let* ((pattern value) ...) body)",
                    super::super::friendly_value_repr(bindings_expr)
                ),
                bindings_expr.clone(),
            );
            return EvalStep::Done((vec![err], env));
        }
    };

    if bindings.is_empty() {
        // No bindings - evaluate body via trampoline (tail call)
        return EvalStep::EvalIfBranch {
            branch: body.clone(),
            env,
            depth,
        };
    }

    // Transform to nested let
    // (let* ((a 1) (b 2) (c 3)) body) -> (let a 1 (let b 2 (let c 3 body)))
    let mut result_body = body.clone();

    // Process bindings in reverse order to build nested structure
    for binding in bindings.iter().rev() {
        match binding.inner() {
            MettaValueInner::SExpr(pair) if pair.len() == 2 => {
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
                    binding.clone(),
                );
                return EvalStep::Done((vec![err], env));
            }
        }
    }

    // Evaluate the nested let structure via trampoline (tail call)
    EvalStep::EvalIfBranch {
        branch: result_body,
        env,
        depth,
    }
}

/// Generate helpful message for pattern mismatch in let bindings
pub(crate) fn pattern_mismatch_suggestion(pattern: &MettaValue, value: &MettaValue) -> String {
    let pattern_arity = match pattern.inner() {
        MettaValueInner::SExpr(items) => items.len(),
        _ => 1,
    };
    let value_arity = match value.inner() {
        MettaValueInner::SExpr(items) => items.len(),
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
    if let (MettaValueInner::SExpr(p_items), MettaValueInner::SExpr(v_items)) =
        (pattern.inner(), value.inner())
    {
        if let (Some(p_first), Some(v_first)) = (p_items.first(), v_items.first()) {
            if let (MettaValueInner::Atom(p_head), MettaValueInner::Atom(v_head)) =
                (p_first.inner(), v_first.inner())
            {
                if p_head != v_head {
                    return format!(
                        "Hint: pattern head '{}' doesn't match value head '{}'.",
                        p_head, v_head
                    );
                }
            }
        }
    }

    // Check for literal mismatch inside structures
    if let (MettaValueInner::SExpr(p_items), MettaValueInner::SExpr(v_items)) =
        (pattern.inner(), value.inner())
    {
        for (i, (p, v)) in p_items.iter().zip(v_items.iter()).enumerate() {
            // Skip if pattern is a variable (starts with $, &, or ')
            if let MettaValueInner::Atom(name) = p.inner() {
                if name.starts_with('$')
                    || name.starts_with('&')
                    || name.starts_with('\'')
                    || name == "_"
                {
                    continue;
                }
            }
            // Check for literal mismatch
            if p != v && !matches!(p.inner(), MettaValueInner::SExpr(_)) {
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
            MettaValue::SExpr(args.to_vec()),
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
