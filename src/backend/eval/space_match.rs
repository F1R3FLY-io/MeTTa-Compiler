//! Bidirectional Space Matching for AtomSpace Operations
//!
//! Provides bidirectional pattern matching where BOTH the query pattern and the
//! stored atom can contain variables. This is needed for MeTTa HE-compatible
//! `match` on spaces that store atoms with variables.
//!
//! ## Difference from `pattern_match_generic`
//!
//! `pattern_match_generic` (in `bindings.rs`) is **unidirectional**: only
//! pattern-side variables bind; stored atom variables are treated as literal symbols.
//! That function remains untouched — it's the fast path for rule application on the
//! hot evaluation path.
//!
//! `space_match_bidirectional_generic` handles the space-query case where stored atoms
//! may contain variables (freshened before matching to prevent capture). It produces
//! bindings for both sides, then narrows to pattern-side variables only.
//!
//! ## Matching Rules
//!
//! | Pattern       | Stored (freshened) | Action                            |
//! |---------------|-------------------|-----------------------------------|
//! | var `$a`      | var `$x_fr`       | try_bind($a, $x_fr)               |
//! | var `$a`      | concrete          | try_bind($a, concrete)            |
//! | concrete      | var `$x_fr`       | try_bind($x_fr, concrete) — NEW   |
//! | `_`           | anything          | skip (wildcard)                   |
//! | anything      | `_`               | skip (wildcard on stored side too) |
//! | literal       | literal           | check equality                    |
//! | S-expr        | S-expr            | check arity, push children        |
//!
//! ## Binding Chain Resolution
//!
//! After matching, binding chains (var→var→...→concrete) are resolved to their
//! final values. Then bindings are narrowed to pattern-side variables only.

use std::collections::{HashMap, HashSet};

use crate::backend::models::{GenericBindings, MettaValueTrait};

/// Check if an atom name is a variable (starts with `$`, `&`, or `'`),
/// excluding space references (`&self`, `&kb`, `&stack`) and the literal `&` operator.
#[inline]
fn is_variable_name(name: &str) -> bool {
    (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
        && name != "&"
        && name != "&self"
        && name != "&kb"
        && name != "&stack"
}

/// Attempt to bind a variable to a value in the bindings map.
///
/// Handles binding chain resolution:
/// - If the variable is already bound to the same value → OK (idempotent)
/// - If the variable is already bound to another variable → chain resolution
/// - If the variable is already bound to a different concrete value → conflict
///
/// Returns `true` if binding succeeds, `false` on conflict.
fn try_bind<V: MettaValueTrait + Clone + PartialEq>(
    bindings: &mut HashMap<String, V>,
    var: &str,
    val: &V,
) -> bool {
    if let Some(existing) = bindings.get(var).cloned() {
        // Already bound — check compatibility
        if &existing == val {
            return true; // Identical value, OK
        }
        // Check if existing binding is a variable → chain resolution
        if let Some(existing_name) = existing.as_atom() {
            if is_variable_name(existing_name) {
                // Chain: var→existing_var. Resolve: bind existing_var→val
                return try_bind(bindings, existing_name, val);
            }
        }
        // Check if val is a variable → reverse chain
        if let Some(val_name) = val.as_atom() {
            if is_variable_name(val_name) {
                // Reverse chain: bind val_var→existing
                return try_bind(bindings, val_name, &existing);
            }
        }
        // Both concrete, different → conflict
        false
    } else {
        bindings.insert(var.to_string(), val.clone());
        true
    }
}

/// Resolve binding chains to their final (concrete) values.
///
/// Follows var→var→...→concrete chains until a fixed point. Variables that
/// form cycles or don't resolve to concrete values remain as-is.
fn resolve_chains<V: MettaValueTrait + Clone>(bindings: &mut HashMap<String, V>) {
    // Collect keys to iterate (avoids borrow issues)
    let keys: Vec<String> = bindings.keys().cloned().collect();

    for key in &keys {
        let mut current_name = key.clone();
        let mut visited: HashSet<String> = HashSet::with_capacity(4);
        visited.insert(current_name.clone());

        loop {
            let val = match bindings.get(&current_name) {
                Some(v) => v.clone(),
                None => break,
            };
            if let Some(name) = val.as_atom() {
                if is_variable_name(name) && !visited.contains(name) {
                    visited.insert(name.to_string());
                    current_name = name.to_string();
                    continue;
                }
            }
            // Reached a concrete value (or cycle) — update the original binding
            if &current_name != key {
                bindings.insert(key.clone(), val);
            }
            break;
        }
    }
}

/// Perform bidirectional pattern matching between a query pattern and a stored atom.
///
/// Both `pattern` and `stored` may contain variables. The `stored` atom should
/// already be freshened (variables renamed to unique names) before calling this
/// function — use `freshen_variables_generic()` from `freshening.rs`.
///
/// ## Arguments
///
/// * `pattern` — The query pattern (may have variables like `$a`, `$b`)
/// * `stored` — The stored atom (freshened variables like `$__fr_42_x`)
/// * `pattern_vars` — Pre-collected set of pattern variable names (for narrowing)
///
/// ## Returns
///
/// `Some(GenericBindings)` with pattern-side variable bindings on success,
/// `None` if matching fails.
///
/// ## Fast Path
///
/// When `stored` has no variables (ground atom), this degenerates to unidirectional
/// matching — equivalent to `pattern_match_generic` but producing `GenericBindings`.
pub fn space_match_bidirectional_generic<V>(
    pattern: &V,
    stored: &V,
    pattern_vars: &HashSet<String>,
) -> Option<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + PartialEq,
{
    let mut bindings: HashMap<String, V> = HashMap::with_capacity(pattern_vars.len() + 4);

    // Work stack: (pattern_node, stored_node) pairs to match
    let mut work_stack: Vec<(&V, &V)> = Vec::with_capacity(16);
    work_stack.push((pattern, stored));

    while let Some((pat, sto)) = work_stack.pop() {
        // --- Atom vs Atom ---
        let pat_atom = pat.as_atom();
        let sto_atom = sto.as_atom();

        match (pat_atom, sto_atom) {
            (Some(p_name), Some(s_name)) => {
                // Both atoms
                let p_is_wild = p_name == "_";
                let s_is_wild = s_name == "_";
                if p_is_wild || s_is_wild {
                    continue; // Wildcard on either side matches anything
                }

                let p_is_var = is_variable_name(p_name);
                let s_is_var = is_variable_name(s_name);

                match (p_is_var, s_is_var) {
                    (true, true) => {
                        // Both variables: bind pattern var → stored var
                        if !try_bind(&mut bindings, p_name, sto) {
                            return None;
                        }
                    }
                    (true, false) => {
                        // Pattern variable, stored concrete
                        if !try_bind(&mut bindings, p_name, sto) {
                            return None;
                        }
                    }
                    (false, true) => {
                        // Pattern concrete, stored variable → bind stored var → pattern concrete
                        if !try_bind(&mut bindings, s_name, pat) {
                            return None;
                        }
                    }
                    (false, false) => {
                        // Both literal atoms — must match exactly
                        if p_name != s_name {
                            return None;
                        }
                    }
                }
                continue;
            }
            (Some(p_name), None) => {
                // Pattern is atom, stored is compound/ground
                if p_name == "_" {
                    continue;
                }
                if is_variable_name(p_name) {
                    if !try_bind(&mut bindings, p_name, sto) {
                        return None;
                    }
                    continue;
                }
                // Literal atom vs compound/ground — no match
                // Exception: "Empty" matches empty sentinel
                if p_name == "Empty" && sto.is_empty() {
                    continue;
                }
                return None;
            }
            (None, Some(s_name)) => {
                // Pattern is compound/ground, stored is atom
                if s_name == "_" {
                    continue;
                }
                if is_variable_name(s_name) {
                    if !try_bind(&mut bindings, s_name, pat) {
                        return None;
                    }
                    continue;
                }
                // Stored literal atom vs pattern compound/ground — no match
                if s_name == "Empty" && pat.is_empty() {
                    continue;
                }
                return None;
            }
            (None, None) => {
                // Neither is an atom — fall through to structural matching
            }
        }

        // --- Ground types ---
        if let Some(p_bool) = pat.as_bool() {
            if let Some(s_bool) = sto.as_bool() {
                if p_bool == s_bool {
                    continue;
                }
            }
            return None;
        }

        if let Some(p_long) = pat.as_long() {
            if let Some(s_long) = sto.as_long() {
                if p_long == s_long {
                    continue;
                }
            }
            return None;
        }

        if let Some(p_float) = pat.as_float() {
            if let Some(s_float) = sto.as_float() {
                if (p_float - s_float).abs() < f64::EPSILON {
                    continue;
                }
            }
            return None;
        }

        if let Some(p_str) = pat.as_string() {
            if let Some(s_str) = sto.as_string() {
                if p_str == s_str {
                    continue;
                }
            }
            return None;
        }

        // --- Unit ---
        if pat.is_unit() {
            if sto.is_unit() {
                continue;
            }
            return None;
        }

        // --- S-expressions ---
        if let Some(p_items) = pat.as_sexpr() {
            if p_items.is_empty() {
                if sto.is_unit() {
                    continue;
                }
                if let Some(s_items) = sto.as_sexpr() {
                    if s_items.is_empty() {
                        continue;
                    }
                }
                return None;
            }
            if let Some(s_items) = sto.as_sexpr() {
                if p_items.len() != s_items.len() {
                    return None;
                }
                // Push children in reverse order (LIFO)
                for (p, s) in p_items.iter().zip(s_items.iter()).rev() {
                    work_stack.push((p, s));
                }
                continue;
            }
            return None;
        }

        // --- Conjunctions ---
        if let Some(p_goals) = pat.as_conjunction() {
            if let Some(s_goals) = sto.as_conjunction() {
                if p_goals.len() != s_goals.len() {
                    return None;
                }
                for (p, s) in p_goals.iter().zip(s_goals.iter()).rev() {
                    work_stack.push((p, s));
                }
                continue;
            }
            return None;
        }

        // --- Errors ---
        if let Some((p_msg, p_details)) = pat.as_error() {
            if let Some((s_msg, s_details)) = sto.as_error() {
                if p_msg != s_msg {
                    return None;
                }
                work_stack.push((p_details, s_details));
                continue;
            }
            return None;
        }

        // --- Space handles ---
        if let Some(p_handle) = pat.as_space() {
            if let Some(s_handle) = sto.as_space() {
                if p_handle.id == s_handle.id {
                    continue;
                }
            }
            return None;
        }

        // --- State handles ---
        if let Some(p_id) = pat.as_state() {
            if let Some(s_id) = sto.as_state() {
                if p_id == s_id {
                    continue;
                }
            }
            return None;
        }

        // --- Type wrappers ---
        if let Some(p_inner) = pat.as_type() {
            if let Some(s_inner) = sto.as_type() {
                work_stack.push((p_inner, s_inner));
                continue;
            }
            return None;
        }

        // --- Empty sentinel ---
        if pat.is_empty() && sto.is_empty() {
            continue;
        }

        // Default: no match
        return None;
    }

    // Post-match: resolve binding chains
    resolve_chains(&mut bindings);

    // Narrow to pattern-side variables only
    let mut result = GenericBindings::new();
    let alloc = crate::backend::models::gc_allocator::global_allocator();
    for (name, value) in &bindings {
        if pattern_vars.contains(name) {
            let interned: &'static str = alloc.alloc_str(name);
            result.insert(interned, value.clone());
        }
    }

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::bindings::collect_variables_generic;
    use crate::backend::eval::freshening::freshen_variables_generic;
    use crate::backend::models::{GcFactory, MettaValueFactory};

    fn factory() -> GcFactory {
        GcFactory::default()
    }

    /// Helper: match pattern against stored (with automatic freshening),
    /// returning narrowed pattern-side bindings.
    fn do_match(
        pattern: &crate::backend::models::MettaValue,
        stored: &crate::backend::models::MettaValue,
    ) -> Option<GenericBindings<crate::backend::models::MettaValue>> {
        let f = factory();
        let pattern_vars = collect_variables_generic(pattern);
        let freshened = freshen_variables_generic(stored, &f);
        space_match_bidirectional_generic(pattern, &freshened, &pattern_vars)
    }

    // -----------------------------------------------------------------------
    // Ground-only matching (degenerates to unidirectional)
    // -----------------------------------------------------------------------

    #[test]
    fn test_ground_vs_ground_match() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let stored = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let result = do_match(&pattern, &stored);
        assert!(result.is_some(), "Identical ground atoms should match");
        assert!(result.expect("should match").is_empty(), "No bindings for ground match");
    }

    #[test]
    fn test_ground_vs_ground_mismatch() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let stored = f.sexpr(vec![f.atom("foo"), f.long(99)]);
        assert!(do_match(&pattern, &stored).is_none(), "Different values should not match");
    }

    #[test]
    fn test_pattern_var_vs_ground() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$x")]);
        let stored = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 1);
        let bound = result.get("$x").expect("$x should be bound");
        assert_eq!(bound.as_long(), Some(42));
    }

    // -----------------------------------------------------------------------
    // Bidirectional: concrete pattern vs stored variable
    // -----------------------------------------------------------------------

    #[test]
    fn test_concrete_vs_stored_variable() {
        // Pattern: (foo 42), Stored: (foo $x)
        // Should succeed: $x_fr binds to 42, narrowed to empty (no pattern vars)
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let stored = f.sexpr(vec![f.atom("foo"), f.atom("$x")]);
        let result = do_match(&pattern, &stored);
        assert!(result.is_some(), "Concrete vs stored variable should match");
        // No pattern-side variables → empty bindings
        let bindings = result.expect("should match");
        assert!(bindings.is_empty(), "No pattern vars to bind");
    }

    #[test]
    fn test_pattern_var_vs_stored_variable() {
        // Pattern: (foo $a), Stored: (foo $x)
        // Should succeed: $a binds to freshened $x_fr
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a")]);
        let stored = f.sexpr(vec![f.atom("foo"), f.atom("$x")]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 1);
        let bound = result.get("$a").expect("$a should be bound");
        // Should be bound to the freshened variable name
        let bound_name = bound.as_atom().expect("should be an atom");
        assert!(bound_name.starts_with("$__fr_"), "Should be freshened: {}", bound_name);
    }

    // -----------------------------------------------------------------------
    // Constraint propagation: same-variable patterns
    // -----------------------------------------------------------------------

    #[test]
    fn test_same_var_pattern_vs_same_var_stored() {
        // Pattern: (pair $a $a), Stored: (pair $x $x)
        // After freshening stored → (pair $__fr_N_x $__fr_N_x)
        // $a binds to $__fr_N_x (both positions match)
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("pair"), f.atom("$a"), f.atom("$a")]);
        let stored = f.sexpr(vec![f.atom("pair"), f.atom("$x"), f.atom("$x")]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 1);
        let bound = result.get("$a").expect("$a should be bound");
        let bound_name = bound.as_atom().expect("should be atom");
        assert!(bound_name.starts_with("$__fr_"), "Should be freshened: {}", bound_name);
    }

    #[test]
    fn test_same_var_pattern_vs_different_var_stored() {
        // Pattern: (pair $a $a), Stored: (pair $x $y)
        // After freshening → (pair $__fr_N_x $__fr_N_y)
        // First: $a binds to $__fr_N_x
        // Second: $a already bound to $__fr_N_x, but stored is $__fr_N_y
        // Chain resolution: $__fr_N_x ≠ $__fr_N_y → try_bind chains
        // $a→$__fr_N_x, then try_bind($a, $__fr_N_y) → existing=$__fr_N_x, val=$__fr_N_y
        // $__fr_N_x is variable → try_bind($__fr_N_x, $__fr_N_y) → chain
        // This SHOULD succeed: $a→$__fr_N_x→$__fr_N_y (both are variables, they equate)
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("pair"), f.atom("$a"), f.atom("$a")]);
        let stored = f.sexpr(vec![f.atom("pair"), f.atom("$x"), f.atom("$y")]);
        let result = do_match(&pattern, &stored);
        // This should succeed with chain resolution: $a → $__fr_N_y (resolved through chain)
        assert!(result.is_some(), "(pair $a $a) should match (pair $x $y) via equating");
    }

    #[test]
    fn test_different_var_pattern_vs_same_var_stored() {
        // Pattern: (pair $a $b), Stored: (pair $x $x)
        // After freshening → (pair $__fr_N_x $__fr_N_x)
        // $a binds to $__fr_N_x, $b binds to $__fr_N_x
        // After chain resolution: $a=$__fr_N_x, $b=$__fr_N_x
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("pair"), f.atom("$a"), f.atom("$b")]);
        let stored = f.sexpr(vec![f.atom("pair"), f.atom("$x"), f.atom("$x")]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 2);
        let bound_a = result.get("$a").expect("$a should be bound");
        let bound_b = result.get("$b").expect("$b should be bound");
        // Both should be bound to the same freshened variable
        assert_eq!(bound_a, bound_b, "$a and $b should be equal (same stored var)");
    }

    #[test]
    fn test_chain_resolution_concrete() {
        // Pattern: (chain $a 42), Stored: (chain $x $x)
        // After freshening → (chain $__fr_N_x $__fr_N_x)
        // Position 1: $a binds to $__fr_N_x
        // Position 2: $__fr_N_x binds to 42
        // Chain resolution: $a→$__fr_N_x→42 ⟹ $a=42
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("chain"), f.atom("$a"), f.long(42)]);
        let stored = f.sexpr(vec![f.atom("chain"), f.atom("$x"), f.atom("$x")]);
        let result = do_match(&pattern, &stored).expect("should match");
        let bound_a = result.get("$a").expect("$a should be bound");
        assert_eq!(bound_a.as_long(), Some(42), "$a should resolve to 42 via chain");
    }

    // -----------------------------------------------------------------------
    // Wildcards on both sides
    // -----------------------------------------------------------------------

    #[test]
    fn test_wildcard_pattern() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("_")]);
        let stored = f.sexpr(vec![f.atom("foo"), f.long(99)]);
        assert!(do_match(&pattern, &stored).is_some());
    }

    #[test]
    fn test_wildcard_stored() {
        // Stored atoms can technically have _ too
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.long(42)]);
        let stored = f.sexpr(vec![f.atom("foo"), f.atom("_")]);
        assert!(do_match(&pattern, &stored).is_some());
    }

    // -----------------------------------------------------------------------
    // Arity mismatch
    // -----------------------------------------------------------------------

    #[test]
    fn test_arity_mismatch() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$x")]);
        let stored = f.sexpr(vec![f.atom("foo"), f.long(1), f.long(2)]);
        assert!(do_match(&pattern, &stored).is_none(), "Arity mismatch should fail");
    }

    // -----------------------------------------------------------------------
    // Nested S-expressions
    // -----------------------------------------------------------------------

    #[test]
    fn test_nested_bidirectional() {
        // Pattern: (outer (inner $a)), Stored: (outer (inner $x))
        let f = factory();
        let pattern = f.sexpr(vec![
            f.atom("outer"),
            f.sexpr(vec![f.atom("inner"), f.atom("$a")]),
        ]);
        let stored = f.sexpr(vec![
            f.atom("outer"),
            f.sexpr(vec![f.atom("inner"), f.atom("$x")]),
        ]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 1);
        let bound = result.get("$a").expect("$a should be bound");
        assert!(bound.as_atom().expect("should be atom").starts_with("$__fr_"));
    }

    // -----------------------------------------------------------------------
    // Space references are NOT variables
    // -----------------------------------------------------------------------

    #[test]
    fn test_space_ref_not_variable() {
        let f = factory();
        let pattern = f.sexpr(vec![f.atom("match"), f.atom("&self"), f.atom("$x")]);
        let stored = f.sexpr(vec![f.atom("match"), f.atom("&self"), f.long(42)]);
        let result = do_match(&pattern, &stored).expect("should match");
        assert_eq!(result.len(), 1);
        let bound = result.get("$x").expect("$x should be bound");
        assert_eq!(bound.as_long(), Some(42));
        // &self should NOT appear as a binding
        assert!(result.get("&self").is_none());
    }

    // -----------------------------------------------------------------------
    // Unit and empty
    // -----------------------------------------------------------------------

    #[test]
    fn test_unit_match() {
        let f = factory();
        let pattern = f.unit();
        let stored = f.unit();
        assert!(do_match(&pattern, &stored).is_some());
    }

    // -----------------------------------------------------------------------
    // Ground type matching
    // -----------------------------------------------------------------------

    #[test]
    fn test_bool_match() {
        let f = factory();
        assert!(do_match(&f.bool(true), &f.bool(true)).is_some());
        assert!(do_match(&f.bool(true), &f.bool(false)).is_none());
    }

    #[test]
    fn test_string_match() {
        let f = factory();
        assert!(do_match(&f.string("hello"), &f.string("hello")).is_some());
        assert!(do_match(&f.string("hello"), &f.string("world")).is_none());
    }
}
