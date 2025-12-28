//! Pattern matching helper functions for the bytecode VM.
//!
//! This module contains helper functions for pattern matching and unification
//! used by the VM opcodes.

use crate::backend::models::MettaValue;

/// Check if a value is a variable (atom starting with $)
#[inline]
pub fn is_variable(value: &MettaValue) -> bool {
    matches!(value, MettaValue::Atom(s) if s.starts_with('$'))
}

/// Get the variable name from a variable atom (strips the $ prefix)
#[inline]
pub fn get_variable_name(value: &MettaValue) -> Option<&str> {
    match value {
        MettaValue::Atom(s) if s.starts_with('$') => Some(&s[1..]),
        _ => None,
    }
}

/// Check if pattern matches value (without binding)
pub fn pattern_matches(pattern: &MettaValue, value: &MettaValue) -> bool {
    match (pattern, value) {
        // Variable matches anything (Atom starting with $)
        (MettaValue::Atom(s), _) if s.starts_with('$') => true,
        // Wildcard matches anything
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len() && ps.iter().zip(vs.iter()).all(|(p, v)| pattern_matches(p, v))
        }
        _ => false,
    }
}

/// Pattern match with variable binding
pub fn pattern_match_bind(
    pattern: &MettaValue,
    value: &MettaValue,
) -> Option<Vec<(String, MettaValue)>> {
    let mut bindings = Vec::new();
    if pattern_match_bind_impl(pattern, value, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn pattern_match_bind_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
    match (pattern, value) {
        // Variable binds to value (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Wildcard matches without binding
        (MettaValue::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
        (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
        (MettaValue::String(a), MettaValue::String(b)) => a == b,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // S-expression matching
        (MettaValue::SExpr(ps), MettaValue::SExpr(vs)) => {
            ps.len() == vs.len()
                && ps
                    .iter()
                    .zip(vs.iter())
                    .all(|(p, v)| pattern_match_bind_impl(p, v, bindings))
        }
        _ => false,
    }
}

/// Unification with variable binding
pub fn unify(a: &MettaValue, b: &MettaValue) -> Option<Vec<(String, MettaValue)>> {
    let mut bindings = Vec::new();
    if unify_impl(a, b, &mut bindings) {
        Some(bindings)
    } else {
        None
    }
}

fn unify_impl(a: &MettaValue, b: &MettaValue, bindings: &mut Vec<(String, MettaValue)>) -> bool {
    match (a, b) {
        // Variables unify with anything (Atom starting with $)
        (MettaValue::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        (val, MettaValue::Atom(name)) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Same structure
        (MettaValue::Atom(x), MettaValue::Atom(y)) => x == y,
        (MettaValue::Long(x), MettaValue::Long(y)) => x == y,
        (MettaValue::Bool(x), MettaValue::Bool(y)) => x == y,
        (MettaValue::String(x), MettaValue::String(y)) => x == y,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        (MettaValue::SExpr(xs), MettaValue::SExpr(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| unify_impl(x, y, bindings))
        }
        _ => false,
    }
}
