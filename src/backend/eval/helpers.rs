//! Helper functions for MeTTa evaluation.
//!
//! This module contains utility functions used throughout the evaluation process,
//! including pattern specificity calculation, head symbol extraction, grounded
//! operation detection, and token resolution.
//!
//! Note: Functions that depend on `eval()` (like eval_conjunction, evaluate_grounded_args)
//! remain in mod.rs to avoid circular dependencies.

use std::borrow::Cow;
use std::sync::Arc;
use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::fuzzy_match::{FuzzyMatcher, SmartSuggestion, SuggestionContext};
use crate::backend::models::{Bindings, MettaValue};

use super::builtin;

/// MeTTa special forms for "did you mean" suggestions during evaluation
pub const SPECIAL_FORMS: &[&str] = &[
    "=",
    "!",
    "quote",
    "if",
    "error",
    "is-error",
    "catch",
    "eval",
    "function",
    "return",
    "chain",
    "match",
    "case",
    "switch",
    "let",
    ":",
    "get-type",
    "check-type",
    "map-atom",
    "filter-atom",
    "foldl-atom",
    "car-atom",
    "cdr-atom",
    "cons-atom",
    "decons-atom",
    "size-atom",
    "max-atom",
    "let*",
    "unify",
    "new-space",
    "add-atom",
    "remove-atom",
    "collapse",
    "superpose",
    "amb",
    "guard",
    "commit",
    "backtrack",
    "get-atoms",
    "new-state",
    "get-state",
    "change-state!",
    "new-memo",
    "memo",
    "memo-first",
    "clear-memo!",
    "memo-stats",
    "bind!",
    "println!",
    "trace!",
    "nop",
    "repr",
    "format-args",
    "empty",
    "get-metatype",
    "include",
];

/// Grounded operations that should be evaluated eagerly (before pattern matching)
const GROUNDED_OPS: &[&str] = &[
    // Arithmetic operations
    "+", "-", "*", "/", "%", "pow", "abs", "floor", "ceil", "round", "sqrt",
    // Comparison operations
    "<", "<=", ">", ">=", "==", "!=", // Boolean operations
    "not", "and", "or", // Type operations that return concrete values
    "get-type",
];

/// Convert MettaValue to a friendly type name for error messages
/// This provides user-friendly type names instead of debug format like "Long(5)"
pub fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value {
        MettaValue::Long(_) => "Number (integer)",
        MettaValue::Float(_) => "Number (float)",
        MettaValue::Bool(_) => "Bool",
        MettaValue::String(_) => "String",
        MettaValue::Atom(_) => "Atom",
        MettaValue::Nil => "Nil",
        MettaValue::SExpr(_) => "S-expression",
        MettaValue::Error(_, _) => "Error",
        MettaValue::Type(_) => "Type",
        MettaValue::Conjunction(_) => "Conjunction",
        MettaValue::Space(_) => "Space",
        MettaValue::State(_) => "State",
        MettaValue::Unit => "Unit",
        MettaValue::Memo(_) => "Memo",
        MettaValue::Empty => "Empty",
    }
}

/// Convert MettaValue to a user-friendly representation for error messages
/// Unlike debug format, this shows values in MeTTa syntax
pub fn friendly_value_repr(value: &MettaValue) -> String {
    match value {
        MettaValue::Long(n) => n.to_string(),
        MettaValue::Float(f) => f.to_string(),
        MettaValue::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValue::String(s) => format!("\"{}\"", s),
        MettaValue::Atom(a) => a.clone(),
        MettaValue::Nil => "Nil".to_string(),
        MettaValue::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(friendly_value_repr).collect();
            format!("({})", inner.join(" "))
        }
        MettaValue::Error(msg, _) => format!("(error \"{}\")", msg),
        MettaValue::Type(t) => format!("(: {})", friendly_value_repr(t)),
        MettaValue::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(friendly_value_repr).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValue::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValue::State(id) => format!("(State {})", id),
        MettaValue::Unit => "()".to_string(),
        MettaValue::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValue::Empty => "Empty".to_string(),
    }
}

/// Check if an operator is close to a known special form using context-aware heuristics
///
/// Returns a SmartSuggestion with confidence level to determine how to present
/// the suggestion (as a warning/note vs. error vs. not at all).
///
/// Uses the three-pillar context-aware approach to avoid false positives:
/// - **Arity compatibility**: Expression arity must match candidate's min/max arity
/// - **Type compatibility**: Argument types must match expected types from signatures
/// - **Prefix compatibility**: $vars vs &spaces vs plain atoms
///
/// # Arguments
/// - `op`: The operator/head of the expression (potentially misspelled)
/// - `expr`: The full expression (for arity/type checking)
/// - `env`: The environment (for type inference)
pub fn suggest_special_form_with_context(
    op: &str,
    expr: &[MettaValue],
    env: &Environment,
) -> Option<SmartSuggestion> {
    use std::sync::OnceLock;

    static MATCHER: OnceLock<FuzzyMatcher> = OnceLock::new();
    let matcher = MATCHER.get_or_init(|| FuzzyMatcher::from_terms(SPECIAL_FORMS.iter().copied()));

    // Build context for position 0 (head position)
    let ctx = SuggestionContext::for_head(expr, env);

    // Use context-aware suggestion with max distance 2
    // The three-pillar validation filters out structurally incompatible suggestions
    matcher.smart_suggest_with_context(op, 2, &ctx)
}

/// Check if an atom name is a grounded operation that should be eagerly evaluated.
pub fn is_grounded_op(name: &str) -> bool {
    GROUNDED_OPS.contains(&name)
}

/// Resolve registered tokens (like &stack → Space) at the top level only.
/// This is a "shallow resolution" for lazy evaluation:
/// - Atoms that are registered tokens are replaced with their values
/// - Variables ($x) are kept as-is (they're for pattern matching)
/// - S-expressions are kept unevaluated (lazy evaluation)
/// - Special tokens like &self are NOT resolved here (handled in eval_step)
pub fn resolve_tokens_shallow(items: &[MettaValue], env: &Environment) -> Vec<MettaValue> {
    items
        .iter()
        .map(|item| {
            match item {
                MettaValue::Atom(name) => {
                    // Skip variables - they're for pattern matching
                    if name.starts_with('$') {
                        return item.clone();
                    }
                    // Skip special atoms like &self, &kb that might be handled elsewhere
                    // or are truly space references
                    if name == "&self" {
                        // Let &self be resolved later in eval_step
                        return item.clone();
                    }
                    // Try to resolve registered tokens (e.g., &stack → Space)
                    if let Some(bound_value) = env.lookup_token(name) {
                        bound_value
                    } else {
                        item.clone()
                    }
                }
                // Keep everything else unchanged (S-expressions, literals, etc.)
                _ => item.clone(),
            }
        })
        .collect()
}

/// Preprocess S-expression items to combine `& name` into `&name`.
/// The Tree-Sitter parser treats `&foo` as two tokens (`&` and `foo`), but we need
/// them combined for HE-compatible space reference semantics (e.g., `&self`, `&kb`, `&stack`).
/// Also recursively processes nested S-expressions.
pub fn preprocess_space_refs(items: Vec<MettaValue>) -> Vec<MettaValue> {
    let mut result = Vec::with_capacity(items.len());
    let mut i = 0;
    while i < items.len() {
        // Check for `& name` pattern - combine any `&` followed by an atom
        if i + 1 < items.len() {
            if let (MettaValue::Atom(ref a), MettaValue::Atom(ref b)) = (&items[i], &items[i + 1]) {
                if a == "&" {
                    // Combine `& name` into `&name`
                    result.push(MettaValue::Atom(format!("&{}", b)));
                    i += 2;
                    continue;
                }
            }
        }
        // Recursively process nested S-expressions
        let item = match &items[i] {
            MettaValue::SExpr(nested) => MettaValue::SExpr(preprocess_space_refs(nested.clone())),
            other => other.clone(),
        };
        result.push(item);
        i += 1;
    }
    result
}

/// Extract the head symbol from a pattern for indexing
/// Returns None if the pattern doesn't have a clear head symbol
pub fn get_head_symbol(pattern: &MettaValue) -> Option<&str> {
    let hs = match pattern {
        // For s-expressions like (double $x), extract "double"
        // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
        MettaValue::SExpr(items) if !items.is_empty() => match &items[0] {
            MettaValue::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || head == "&")
                    && !head.starts_with('\'')
                    && head != "_" =>
            {
                Some(head.as_str())
            }
            _ => None,
        },
        // For bare atoms like foo, use the atom itself
        // EXCEPT: standalone "&" is allowed (used in match)
        MettaValue::Atom(head)
            if !head.starts_with('$')
                && (!head.starts_with('&') || head == "&")
                && !head.starts_with('\'')
                && head != "_" =>
        {
            Some(head.as_str())
        }
        _ => None,
    };

    trace!(target: "mettatron::backend::eval::get_head_symbol", ?hs);
    hs
}

/// Compute the specificity of a pattern (lower is more specific)
/// More specific patterns have fewer variables
pub fn pattern_specificity(pattern: &MettaValue) -> usize {
    match pattern {
        // Variables are least specific
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') || s == "_")
                && s != "&" =>
        {
            1000 // Variables are least specific
        }
        MettaValue::Atom(_)
        | MettaValue::Bool(_)
        | MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Space(_)
        | MettaValue::State(_)
        | MettaValue::Unit
        | MettaValue::Memo(_)
        | MettaValue::Empty => {
            0 // Literals are most specific (including standalone "&")
        }
        MettaValue::SExpr(items) => {
            // Sum specificity of all items
            items.iter().map(pattern_specificity).sum()
        }
        // Conjunctions: sum specificity of all goals
        MettaValue::Conjunction(goals) => goals.iter().map(pattern_specificity).sum(),
        // Errors: use specificity of details
        MettaValue::Error(_, details) => pattern_specificity(details),
        // Types: use specificity of inner type
        MettaValue::Type(t) => pattern_specificity(t),
    }
}

/// Apply variable bindings to a value
///
/// This is made public to support optimized match operations in Environment
///
/// Uses Cow<'a, MettaValue> to avoid cloning when no substitution is needed.
/// Returns Cow::Borrowed(value) when the expression contains no variables bound in `bindings`.
/// Returns Cow::Owned(new_value) only when actual substitution occurred.
pub fn apply_bindings<'a>(value: &'a MettaValue, bindings: &Bindings) -> Cow<'a, MettaValue> {
    trace!(target: "mettatron::backend::eval::apply_bindings", ?value, ?bindings);

    // Fast path: empty bindings means no substitutions possible
    if bindings.is_empty() {
        return Cow::Borrowed(value);
    }
    match value {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValue::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&" =>
        {
            match bindings.iter().find(|(name, _)| name.as_str() == s) {
                Some((_name, val)) => Cow::Owned(val.clone()),
                None => Cow::Borrowed(value),
            }
        }
        MettaValue::SExpr(items) => {
            // Check if any substitution will occur before allocating
            let mut needs_copy = false;
            let mut results: Vec<Cow<'_, MettaValue>> = Vec::with_capacity(items.len());

            for item in items {
                let result = apply_bindings(item, bindings);
                if matches!(result, Cow::Owned(_)) {
                    needs_copy = true;
                }
                results.push(result);
            }

            if needs_copy {
                Cow::Owned(MettaValue::SExpr(
                    results.into_iter().map(|cow| cow.into_owned()).collect(),
                ))
            } else {
                Cow::Borrowed(value)
            }
        }
        MettaValue::Conjunction(goals) => {
            // Check if any substitution will occur before allocating
            let mut needs_copy = false;
            let mut results: Vec<Cow<'_, MettaValue>> = Vec::with_capacity(goals.len());

            for goal in goals {
                let result = apply_bindings(goal, bindings);
                if matches!(result, Cow::Owned(_)) {
                    needs_copy = true;
                }
                results.push(result);
            }

            if needs_copy {
                Cow::Owned(MettaValue::Conjunction(
                    results.into_iter().map(|cow| cow.into_owned()).collect(),
                ))
            } else {
                Cow::Borrowed(value)
            }
        }
        MettaValue::Error(msg, details) => {
            let new_details = apply_bindings(details, bindings);
            if matches!(new_details, Cow::Owned(_)) {
                Cow::Owned(MettaValue::Error(
                    msg.clone(),
                    Arc::new(new_details.into_owned()),
                ))
            } else {
                Cow::Borrowed(value)
            }
        }
        // Literals don't need substitution - return borrowed reference
        _ => Cow::Borrowed(value),
    }
}

/// Delegate to builtin module for built-in operations
pub fn try_eval_builtin(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    builtin::try_eval_builtin(op, args)
}

/// Check structural equality between two MettaValues
/// HE-compatible: Nil and empty SExpr are considered equal
pub fn values_equal(a: &MettaValue, b: &MettaValue) -> bool {
    match (a, b) {
        // Same-type comparisons
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Float(a), MettaValue::Float(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,

        // HE-compatible: Nil equals empty SExpr
        (MettaValue::Nil, MettaValue::SExpr(items))
        | (MettaValue::SExpr(items), MettaValue::Nil) => items.is_empty(),

        // HE-compatible: Nil equals Unit
        (MettaValue::Nil, MettaValue::Unit) | (MettaValue::Unit, MettaValue::Nil) => true,

        // HE-compatible: Nil (value) equals Nil (atom symbol)
        (MettaValue::Nil, MettaValue::Atom(s)) | (MettaValue::Atom(s), MettaValue::Nil) => {
            s == "Nil"
        }

        // S-expression structural equality
        (MettaValue::SExpr(a_items), MettaValue::SExpr(b_items)) => {
            if a_items.len() != b_items.len() {
                return false;
            }
            a_items
                .iter()
                .zip(b_items.iter())
                .all(|(a, b)| values_equal(a, b))
        }

        // Conjunction structural equality
        (MettaValue::Conjunction(a_goals), MettaValue::Conjunction(b_goals)) => {
            if a_goals.len() != b_goals.len() {
                return false;
            }
            a_goals
                .iter()
                .zip(b_goals.iter())
                .all(|(a, b)| values_equal(a, b))
        }

        // Error equality (message and details must match)
        (MettaValue::Error(a_msg, a_details), MettaValue::Error(b_msg, b_details)) => {
            a_msg == b_msg && values_equal(a_details, b_details)
        }

        // Space and State equality by identity
        (MettaValue::Space(a), MettaValue::Space(b)) => a.id == b.id,
        (MettaValue::State(a), MettaValue::State(b)) => a == b,

        // Type equality
        (MettaValue::Type(a), MettaValue::Type(b)) => a == b,

        // Different types are not equal
        _ => false,
    }
}
