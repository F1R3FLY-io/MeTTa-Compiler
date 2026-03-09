//! Helper functions for fuzzy matching heuristics.

use std::collections::HashMap;

use crate::backend::builtin_signatures::TypeExpr;
use crate::backend::models::{MettaValue, ValueView};
use crate::backend::MettaEnvironment;

use super::types::SuggestionConfidence;

/// Check if a term looks like an intentional data constructor
///
/// Data constructors in MeTTa typically follow these patterns:
/// - PascalCase: `MyType`, `DataConstructor`, `True`, `False`
/// - Contains hyphens: `is-valid`, `get-value`, `my-function`
/// - All uppercase: `NIL`, `VOID`, `ERROR`
/// - Contains underscores: `my_value`, `data_type`
pub fn is_likely_data_constructor(term: &str) -> bool {
    // Skip empty terms
    if term.is_empty() {
        return false;
    }

    let first_char = term.chars().next().unwrap();

    // Skip if starts with special prefix (these are handled elsewhere)
    if matches!(first_char, '$' | '&' | '\'' | '%') {
        return false;
    }

    // PascalCase: starts with uppercase letter followed by lowercase
    if first_char.is_uppercase() {
        let has_lowercase = term.chars().skip(1).any(|c| c.is_lowercase());
        if has_lowercase {
            return true;
        }
    }

    // All uppercase (constants): `NIL`, `VOID`
    let all_upper = term.chars().all(|c| c.is_uppercase() || c == '_');
    if term.len() >= 2 && all_upper {
        return true;
    }

    // Contains hyphen (compound names): `is-valid`, `my-func`
    if term.contains('-') {
        return true;
    }

    // Contains underscore (snake_case): `my_value`
    if term.contains('_') {
        return true;
    }

    // Contains digits (likely intentional): `value1`, `test2`
    if term.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }

    false
}

/// Check if two terms have compatible prefix types
///
/// Different prefixes have different semantics:
/// - `$x` - pattern variable
/// - `&x` - space reference
/// - `'x` - quoted symbol
///
/// Suggesting across these boundaries would be unhelpful.
pub fn are_prefixes_compatible(query: &str, suggestion: &str) -> bool {
    let query_prefix = query.chars().next();
    let suggestion_prefix = suggestion.chars().next();

    match (query_prefix, suggestion_prefix) {
        (Some('$'), Some('$')) => true,
        (Some('&'), Some('&')) => true,
        (Some('\''), Some('\'')) => true,
        (Some('%'), Some('%')) => true,
        // Both are regular identifiers (no special prefix)
        (Some(q), Some(s))
            if !matches!(q, '$' | '&' | '\'' | '%') && !matches!(s, '$' | '&' | '\'' | '%') =>
        {
            true
        }
        _ => false,
    }
}

/// Check if a MettaValue matches an expected TypeExpr.
///
/// This performs structural type matching for fuzzy suggestion filtering.
/// It uses simple heuristics to determine compatibility without full type inference.
pub fn type_matches(actual: &MettaValue, expected: &TypeExpr, _env: &MettaEnvironment) -> bool {
    match expected {
        // Universal types - accept anything
        TypeExpr::Any | TypeExpr::Pattern | TypeExpr::Bindings | TypeExpr::Expr => true,

        // %Undefined% matches anything (HE parity)
        TypeExpr::Undefined => true,

        // Type variables accept anything (instantiate on first use)
        TypeExpr::Var(_) => true,

        // Concrete types - check structural compatibility
        TypeExpr::Number => matches!(
            actual.view(),
            ValueView::Long(_) | ValueView::Float(_)
        ),

        TypeExpr::Bool => {
            matches!(actual.view(), ValueView::Bool(_))
                || matches!(actual.view(), ValueView::Atom(s) if s == "True" || s == "False")
        }

        TypeExpr::String => matches!(actual.view(), ValueView::String(_)),

        // Atom meta-type: matches any atom (symbol)
        TypeExpr::Atom => matches!(actual.view(), ValueView::Atom(_)),

        // Expression meta-type: matches any S-expression
        TypeExpr::Expression => matches!(actual.view(), ValueView::SExpr(_) | ValueView::Unit),

        // Variable meta-type: matches variable atoms ($x, $var, etc.)
        TypeExpr::Variable => matches!(actual.view(), ValueView::Atom(s) if s.starts_with('$')),

        TypeExpr::Space => {
            matches!(actual.view(), ValueView::Space(_))
                || matches!(actual.view(), ValueView::Atom(s) if s.starts_with('&'))
        }

        TypeExpr::State | TypeExpr::StateMonad(_) => matches!(actual.view(), ValueView::State(_)),

        TypeExpr::Unit => matches!(actual.view(), ValueView::Unit),

        TypeExpr::Error => matches!(actual.view(), ValueView::Error(_, _)),

        TypeExpr::Type => {
            matches!(actual.view(), ValueView::Type(_))
                || matches!(actual.view(), ValueView::Atom(s) if is_type_name(s))
        }

        // Grounded type: matches grounded atoms
        TypeExpr::Grounded => matches!(actual.view(), ValueView::Atom(_)),

        // List type - check if it's an s-expression (Unit is normalized SExpr([]))
        TypeExpr::List(_) => matches!(actual.view(), ValueView::SExpr(_) | ValueView::Unit),

        // Arrow type - callable things (atoms/s-expressions)
        TypeExpr::Arrow(_, _) => matches!(
            actual.view(),
            ValueView::Atom(_) | ValueView::SExpr(_)
        ),
    }
}

/// Check if a string looks like a type name
pub fn is_type_name(s: &str) -> bool {
    matches!(
        s,
        "Number"
            | "Bool"
            | "String"
            | "Atom"
            | "Symbol"
            | "Expression"
            | "Type"
            | "Space"
            | "State"
            | "Unit"
            | "Nil"
            | "Error"
            | "List"
    )
}

/// Validate type variable consistency across arguments.
///
/// For polymorphic types like `(if Bool $a $a) -> $a`, this ensures
/// that arguments bound to the same type variable have compatible types.
pub fn validate_type_vars(
    args: &[MettaValue],
    expected_types: &[TypeExpr],
    _env: &MettaEnvironment,
) -> bool {
    let mut var_bindings: HashMap<&str, &MettaValue> = HashMap::new();

    for (arg, expected) in args.iter().zip(expected_types.iter()) {
        if let TypeExpr::Var(name) = expected {
            if let Some(bound_value) = var_bindings.get(name) {
                // Check consistency with previous binding
                if !values_compatible(bound_value, arg) {
                    return false;
                }
            } else {
                var_bindings.insert(name, arg);
            }
        }
    }
    true
}

/// Check if two MettaValues are type-compatible.
///
/// Used for type variable unification - ensures values bound to the same
/// type variable have compatible types.
pub fn values_compatible(a: &MettaValue, b: &MettaValue) -> bool {
    match (a.view(), b.view()) {
        // Same ground types are compatible
        (ValueView::Long(_), ValueView::Long(_))
        | (ValueView::Long(_), ValueView::Float(_))
        | (ValueView::Float(_), ValueView::Long(_))
        | (ValueView::Float(_), ValueView::Float(_)) => true,
        (ValueView::Bool(_), ValueView::Bool(_)) => true,
        (ValueView::String(_), ValueView::String(_)) => true,
        (ValueView::Unit, ValueView::Unit) => true,

        // Atoms - could be same type
        (ValueView::Atom(_), ValueView::Atom(_)) => true,

        // S-expressions could have compatible types
        (ValueView::SExpr(_), ValueView::SExpr(_)) => true,

        // Space and State
        (ValueView::Space(_), ValueView::Space(_)) => true,
        (ValueView::State(_), ValueView::State(_)) => true,

        // Errors
        (ValueView::Error(_, _), ValueView::Error(_, _)) => true,

        // Type values
        (ValueView::Type(_), ValueView::Type(_)) => true,

        // Different structural types are not compatible
        _ => false,
    }
}

/// Compute suggestion confidence based on distance and length heuristics
pub fn compute_suggestion_confidence(
    _query: &str,
    suggested: &str,
    distance: usize,
    query_len: usize,
) -> SuggestionConfidence {
    let suggested_len = suggested.chars().count();
    let min_len = query_len.min(suggested_len);

    // Relative distance threshold: distance/min_len must be <= 1/3 (~0.333)
    // This allows single-character typos (like lett→let) while rejecting
    // higher ratios. Context-aware arity checks handle structural mismatches
    // (e.g., (lit p) with arity 1 won't suggest let which needs arity 3).
    let relative_distance = distance as f64 / min_len as f64;
    if relative_distance > 0.34 {
        return SuggestionConfidence::None;
    }

    // For distance 1, require minimum length of 4
    if distance == 1 && query_len < 4 {
        return SuggestionConfidence::None;
    }

    // For distance 2, require minimum length of 6
    if distance == 2 && query_len < 6 {
        return SuggestionConfidence::Low;
    }

    // High confidence for longer words with small relative distance
    if relative_distance < 0.20 {
        SuggestionConfidence::High
    } else {
        SuggestionConfidence::Low
    }
}
