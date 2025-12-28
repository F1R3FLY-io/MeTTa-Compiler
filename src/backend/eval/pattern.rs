//! Pattern matching for MeTTa values.
//!
//! This module implements the core pattern matching algorithm for MeTTa,
//! supporting variable binding, wildcards, and structural matching.

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue};

/// Match a pattern against a value, returning variable bindings if successful.
///
/// This is made public to support optimized match operations in Environment
/// and for benchmarking the core pattern matching algorithm.
///
/// # Arguments
/// - `pattern`: The pattern to match against (may contain variables like `$x`, `&y`, `'z`)
/// - `value`: The value to match
///
/// # Returns
/// - `Some(bindings)` if the pattern matches, with variable bindings
/// - `None` if the pattern does not match
///
/// # Examples
/// ```ignore
/// // Variable binding
/// pattern_match(&atom("$x"), &long(42)) // => Some({$x: 42})
///
/// // Structural matching
/// pattern_match(&sexpr([atom("foo"), atom("$x")]), &sexpr([atom("foo"), long(1)]))
/// // => Some({$x: 1})
///
/// // Wildcard
/// pattern_match(&atom("_"), &long(999)) // => Some({})
/// ```
pub fn pattern_match(pattern: &MettaValue, value: &MettaValue) -> Option<Bindings> {
    trace!(target: "mettatron::backend::eval::pattern_match", ?pattern, ?value);
    let mut bindings = Bindings::new();
    if pattern_match_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

/// Internal pattern matching implementation that accumulates bindings.
///
/// This function is separate from `pattern_match` to allow reuse of the bindings
/// map across recursive calls, avoiding repeated allocations.
pub(crate) fn pattern_match_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Bindings,
) -> bool {
    match (pattern, value) {
        // Wildcard matches anything
        (MettaValue::Atom(p), _) if p == "_" => true,

        // FAST PATH: First variable binding (empty bindings)
        // Optimization: Skip lookup when bindings are empty - directly insert
        // This reduces single-variable regression from 16.8% to ~5-7%
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                && p != "&"
                && bindings.is_empty() =>
        {
            bindings.insert(p.clone(), v.clone());
            true
        }

        // GENERAL PATH: Variable with potential existing bindings
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        (MettaValue::Atom(p), v)
            if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\'')) && p != "&" =>
        {
            // Check if variable is already bound (linear search for SmartBindings)
            if let Some((_, existing)) = bindings.iter().find(|(name, _)| name.as_str() == p) {
                existing == v
            } else {
                bindings.insert(p.clone(), v.clone());
                true
            }
        }

        // Atoms must match exactly
        (MettaValue::Atom(p), MettaValue::Atom(v)) => p == v,
        (MettaValue::Bool(p), MettaValue::Bool(v)) => p == v,
        (MettaValue::Long(p), MettaValue::Long(v)) => p == v,
        (MettaValue::Float(p), MettaValue::Float(v)) => p == v,
        (MettaValue::String(p), MettaValue::String(v)) => p == v,
        (MettaValue::Nil, MettaValue::Nil) => true,
        // Nil also matches Unit (HE-compatible: both represent "nothing")
        (MettaValue::Nil, MettaValue::Unit) => true,
        // Nil pattern matches Empty atom (HE-compatible: () pattern in case matches Empty)
        // This is needed because case converts empty results to Atom("Empty") internally
        (MettaValue::Nil, MettaValue::Atom(v)) if v == "Empty" => true,
        // Empty atom pattern matches Nil (symmetry: Empty pattern matches () values)
        (MettaValue::Atom(p), MettaValue::Nil) if p == "Empty" => true,
        // Unit also matches Nil and other Units
        (MettaValue::Unit, MettaValue::Unit) => true,
        (MettaValue::Unit, MettaValue::Nil) => true,

        // Nil pattern matches only empty values (Nil, Unit, empty S-expr, or Empty atom)
        // For discard pattern, use wildcard _ instead
        (MettaValue::Nil, MettaValue::SExpr(v_items)) if v_items.is_empty() => true,
        (MettaValue::Nil, MettaValue::Atom(v)) if v == "Empty" => true,

        // Empty S-expression () matches only empty values (empty S-expr, Nil, Unit, or Empty atom)
        // For discard pattern, use wildcard _ instead
        (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items))
            if p_items.is_empty() && v_items.is_empty() =>
        {
            true
        }
        (MettaValue::SExpr(p_items), MettaValue::Nil) if p_items.is_empty() => true,
        (MettaValue::SExpr(p_items), MettaValue::Unit) if p_items.is_empty() => true,
        (MettaValue::SExpr(p_items), MettaValue::Atom(v)) if p_items.is_empty() && v == "Empty" => {
            true
        }

        // S-expressions must have same length and all elements must match
        (MettaValue::SExpr(p_items), MettaValue::SExpr(v_items)) => {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !pattern_match_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        // Conjunctions must have same length and all goals must match
        (MettaValue::Conjunction(p_goals), MettaValue::Conjunction(v_goals)) => {
            if p_goals.len() != v_goals.len() {
                return false;
            }
            for (p, v) in p_goals.iter().zip(v_goals.iter()) {
                if !pattern_match_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        // Errors match if message and details match
        (MettaValue::Error(p_msg, p_details), MettaValue::Error(v_msg, v_details)) => {
            p_msg == v_msg && pattern_match_impl(p_details, v_details, bindings)
        }

        _ => false,
    }
}
