//! Pattern matching helper functions for the bytecode VM.
//!
//! This module contains helper functions for pattern matching and unification
//! used by the VM opcodes.

use crate::backend::models::{MettaValue, MettaValueInner};

/// Check if a value is a variable (atom starting with $)
#[inline]
pub fn is_variable(value: &MettaValue) -> bool {
    matches!(value.inner(), MettaValueInner::Atom(s) if s.starts_with('$'))
}

/// Get the variable name from a variable atom (strips the $ prefix)
#[inline]
pub fn get_variable_name(value: &MettaValue) -> Option<&str> {
    match value.inner() {
        MettaValueInner::Atom(s) if s.starts_with('$') => Some(&s[1..]),
        _ => None,
    }
}

/// Check if pattern matches value (without binding)
pub fn pattern_matches(pattern: &MettaValue, value: &MettaValue) -> bool {
    match (pattern.inner(), value.inner()) {
        // Variable matches anything (Atom starting with $)
        (MettaValueInner::Atom(s), _) if s.starts_with('$') => true,
        // Wildcard matches anything
        (MettaValueInner::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,
        // Exact match for literals
        (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
        (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
        (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
        (MettaValueInner::Nil, MettaValueInner::Nil) => true,
        (MettaValueInner::Unit, MettaValueInner::Unit) => true,
        // S-expression matching
        (MettaValueInner::SExpr(ps), MettaValueInner::SExpr(vs)) => {
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
    match (pattern.inner(), value) {
        // Variable binds to value (Atom starting with $)
        (MettaValueInner::Atom(name), val) if name.starts_with('$') => {
            bindings.push((name.clone(), val.clone()));
            true
        }
        // Wildcard matches without binding
        (MettaValueInner::Atom(s), _) if s == "_" => true,
        // Exact match for atoms
        (MettaValueInner::Atom(a), val) => {
            matches!(val.inner(), MettaValueInner::Atom(b) if a == b)
        }
        // Exact match for literals
        (MettaValueInner::Long(a), val) => {
            matches!(val.inner(), MettaValueInner::Long(b) if a == b)
        }
        (MettaValueInner::Bool(a), val) => {
            matches!(val.inner(), MettaValueInner::Bool(b) if a == b)
        }
        (MettaValueInner::String(a), val) => {
            matches!(val.inner(), MettaValueInner::String(b) if a == b)
        }
        (MettaValueInner::Nil, val) => matches!(val.inner(), MettaValueInner::Nil),
        (MettaValueInner::Unit, val) => matches!(val.inner(), MettaValueInner::Unit),
        // S-expression matching
        (MettaValueInner::SExpr(ps), val) => {
            if let MettaValueInner::SExpr(vs) = val.inner() {
                ps.len() == vs.len()
                    && ps
                        .iter()
                        .zip(vs.iter())
                        .all(|(p, v)| pattern_match_bind_impl(p, v, bindings))
            } else {
                false
            }
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
    match (a.inner(), b.inner()) {
        // Variables unify with anything (Atom starting with $)
        (MettaValueInner::Atom(name), _) if name.starts_with('$') => {
            bindings.push((name.clone(), b.clone()));
            true
        }
        (_, MettaValueInner::Atom(name)) if name.starts_with('$') => {
            bindings.push((name.clone(), a.clone()));
            true
        }
        // Same structure
        (MettaValueInner::Atom(x), MettaValueInner::Atom(y)) => x == y,
        (MettaValueInner::Long(x), MettaValueInner::Long(y)) => x == y,
        (MettaValueInner::Bool(x), MettaValueInner::Bool(y)) => x == y,
        (MettaValueInner::String(x), MettaValueInner::String(y)) => x == y,
        (MettaValueInner::Nil, MettaValueInner::Nil) => true,
        (MettaValueInner::Unit, MettaValueInner::Unit) => true,
        (MettaValueInner::SExpr(xs), MettaValueInner::SExpr(ys)) => {
            xs.len() == ys.len()
                && xs
                    .iter()
                    .zip(ys.iter())
                    .all(|(x, y)| unify_impl(x, y, bindings))
        }
        _ => false,
    }
}
