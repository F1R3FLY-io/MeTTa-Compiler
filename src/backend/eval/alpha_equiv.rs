//! Alpha Equivalence for MeTTa Values
//!
//! Two expressions are alpha-equivalent if they are identical up to consistent
//! variable renaming. This module provides the shared infrastructure used by
//! `if-equal`, `=alpha`, testing assertions, and `unique-atom`.
//!
//! ## Algorithm
//!
//! Uses bidirectional variable mapping (left→right and right→left) to ensure
//! consistent renaming. Variables are atoms starting with `$`.
//!
//! ## Fast Path
//!
//! For slab-allocated `MettaValue`, pointer equality provides an O(1) fast path
//! before falling through to the O(n) structural comparison.
//!
//! ## Reference
//!
//! Matches MeTTa HE's `atoms_are_equivalent()` semantics from
//! `hyperon-experimental/lib/src/atom/matcher.rs`.

use std::collections::HashMap;

use crate::backend::models::MettaValueTrait;

/// Check if two values are alpha-equivalent (identical up to variable renaming).
///
/// Returns `true` if the two values have the same structure with a consistent
/// bijective mapping between their variables.
///
/// # Fast Path
///
/// Uses `MettaValueTrait::inner_ptr()` for O(1) pointer equality on slab-allocated
/// values. Falls through to full structural comparison only when pointers differ.
///
/// # Examples
///
/// ```text
/// atoms_are_alpha_equivalent(($x $y), ($a $b))  → true  (consistent rename)
/// atoms_are_alpha_equivalent(($x $x), ($a $b))  → false (inconsistent: $x→$a and $x→$b)
/// atoms_are_alpha_equivalent((foo $x), (foo $y)) → true
/// atoms_are_alpha_equivalent((foo $x), (bar $x)) → false (different head symbol)
/// ```
pub fn atoms_are_alpha_equivalent<V: MettaValueTrait>(left: &V, right: &V) -> bool {
    // Fast path: pointer equality for slab-allocated values (same allocation = same value)
    if left.inner_ptr() == right.inner_ptr() {
        return true;
    }
    // Full bidirectional mapping check
    let mut l2r: HashMap<&str, &str> = HashMap::new();
    let mut r2l: HashMap<&str, &str> = HashMap::new();
    alpha_equiv_inner(left, right, &mut l2r, &mut r2l)
}

/// Check if an atom string represents a variable (starts with `$`).
///
/// Matches MeTTa HE's `VariableAtom` semantics where variables are atoms
/// whose name starts with `$`.
#[inline]
fn is_variable(s: &str) -> bool {
    s.starts_with('$')
}

/// Recursive alpha-equivalence check with bidirectional variable mappings.
///
/// Maintains two maps:
/// - `l2r`: maps left-side variable names to their right-side counterparts
/// - `r2l`: maps right-side variable names to their left-side counterparts
///
/// Both must be consistent for the check to pass (bijective mapping).
fn alpha_equiv_inner<'a, V: MettaValueTrait>(
    left: &'a V,
    right: &'a V,
    l2r: &mut HashMap<&'a str, &'a str>,
    r2l: &mut HashMap<&'a str, &'a str>,
) -> bool {
    // Both atoms?
    if let (Some(la), Some(ra)) = (left.as_atom(), right.as_atom()) {
        if is_variable(la) && is_variable(ra) {
            // Both variables: check bidirectional mapping consistency
            return check_bidirectional_mapping(l2r, r2l, la, ra);
        }
        if is_variable(la) || is_variable(ra) {
            // One variable, one non-variable: never alpha-equivalent
            return false;
        }
        // Both non-variable atoms: must be identical
        return la == ra;
    }

    // Both booleans?
    if let (Some(lb), Some(rb)) = (left.as_bool(), right.as_bool()) {
        return lb == rb;
    }

    // Both longs?
    if let (Some(ln), Some(rn)) = (left.as_long(), right.as_long()) {
        return ln == rn;
    }

    // Both floats?
    if let (Some(lf), Some(rf)) = (left.as_float(), right.as_float()) {
        return lf == rf;
    }

    // Both strings?
    if let (Some(ls), Some(rs)) = (left.as_string(), right.as_string()) {
        return ls == rs;
    }

    // Both unit?
    if left.is_unit() && right.is_unit() {
        return true;
    }

    // Both s-expressions?
    if let (Some(l_items), Some(r_items)) = (left.as_sexpr(), right.as_sexpr()) {
        if l_items.len() != r_items.len() {
            return false;
        }
        return l_items
            .iter()
            .zip(r_items.iter())
            .all(|(l, r)| alpha_equiv_inner(l, r, l2r, r2l));
    }

    // Both errors?
    if let (Some((lm, ld)), Some((rm, rd))) = (left.as_error(), right.as_error()) {
        return lm == rm && alpha_equiv_inner(ld, rd, l2r, r2l);
    }

    // Both types?
    if let (Some(lt), Some(rt)) = (left.as_type(), right.as_type()) {
        return alpha_equiv_inner(lt, rt, l2r, r2l);
    }

    // Both conjunctions?
    if let (Some(lg), Some(rg)) = (left.as_conjunction(), right.as_conjunction()) {
        if lg.len() != rg.len() {
            return false;
        }
        return lg
            .iter()
            .zip(rg.iter())
            .all(|(l, r)| alpha_equiv_inner(l, r, l2r, r2l));
    }

    // Both empty?
    if left.is_empty() && right.is_empty() {
        return true;
    }

    // Both spaces?
    if let (Some(ls), Some(rs)) = (left.as_space(), right.as_space()) {
        return ls.id == rs.id;
    }

    // Both states?
    if let (Some(ls), Some(rs)) = (left.as_state(), right.as_state()) {
        return ls == rs;
    }

    // Different variant types: never alpha-equivalent
    false
}

/// Check bidirectional variable mapping consistency.
///
/// For alpha-equivalence, when we encounter variables `$x` (left) and `$a` (right),
/// we must ensure:
/// 1. If `$x` was previously mapped, it maps to `$a` (not something else)
/// 2. If `$a` was previously mapped, it maps back to `$x` (not something else)
/// 3. If neither was mapped, establish the mapping in both directions
#[inline]
fn check_bidirectional_mapping<'a>(
    l2r: &mut HashMap<&'a str, &'a str>,
    r2l: &mut HashMap<&'a str, &'a str>,
    left_var: &'a str,
    right_var: &'a str,
) -> bool {
    // Check left→right consistency
    match l2r.get(left_var) {
        Some(&existing) => {
            if existing != right_var {
                return false;
            }
        }
        None => {
            l2r.insert(left_var, right_var);
        }
    }
    // Check right→left consistency
    match r2l.get(right_var) {
        Some(&existing) => {
            if existing != left_var {
                return false;
            }
        }
        None => {
            r2l.insert(right_var, left_var);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_identical_atoms() {
        let f = global_factory();
        let a = f.atom("foo");
        let b = f.atom("foo");
        assert!(atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_different_atoms() {
        let f = global_factory();
        let a = f.atom("foo");
        let b = f.atom("bar");
        assert!(!atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_variable_renaming() {
        let f = global_factory();
        let left = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let right = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        assert!(atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_inconsistent_variable_mapping() {
        let f = global_factory();
        // ($x $x) vs ($a $b) — $x maps to both $a and $b
        let left = f.sexpr(vec![f.atom("$x"), f.atom("$x")]);
        let right = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        assert!(!atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_inconsistent_reverse_mapping() {
        let f = global_factory();
        // ($x $y) vs ($a $a) — $a maps back to both $x and $y
        let left = f.sexpr(vec![f.atom("$x"), f.atom("$y")]);
        let right = f.sexpr(vec![f.atom("$a"), f.atom("$a")]);
        assert!(!atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_nested_expressions_with_renaming() {
        let f = global_factory();
        let left = f.sexpr(vec![
            f.atom("foo"),
            f.sexpr(vec![f.atom("$x"), f.long(42)]),
            f.atom("$y"),
        ]);
        let right = f.sexpr(vec![
            f.atom("foo"),
            f.sexpr(vec![f.atom("$a"), f.long(42)]),
            f.atom("$b"),
        ]);
        assert!(atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_mixed_variables_and_symbols() {
        let f = global_factory();
        let left = f.sexpr(vec![f.atom("foo"), f.atom("$x"), f.atom("bar")]);
        let right = f.sexpr(vec![f.atom("foo"), f.atom("$y"), f.atom("bar")]);
        assert!(atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_mixed_variables_and_symbols_mismatch() {
        let f = global_factory();
        let left = f.sexpr(vec![f.atom("foo"), f.atom("$x"), f.atom("bar")]);
        let right = f.sexpr(vec![f.atom("foo"), f.atom("$y"), f.atom("baz")]);
        assert!(!atoms_are_alpha_equivalent(&left, &right));
    }

    #[test]
    fn test_pointer_equal_values_fast_path() {
        let f = global_factory();
        let v = f.sexpr(vec![f.atom("$x"), f.long(1)]);
        let v_copy = v; // Copy — same pointer
        assert!(atoms_are_alpha_equivalent(&v, &v_copy));
    }

    #[test]
    fn test_different_variant_types() {
        let f = global_factory();
        let a = f.long(42);
        let b = f.atom("42");
        assert!(!atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_primitives() {
        let f = global_factory();
        assert!(atoms_are_alpha_equivalent(&f.long(42), &f.long(42)));
        assert!(!atoms_are_alpha_equivalent(&f.long(42), &f.long(43)));
        assert!(atoms_are_alpha_equivalent(&f.bool(true), &f.bool(true)));
        assert!(!atoms_are_alpha_equivalent(&f.bool(true), &f.bool(false)));
        assert!(atoms_are_alpha_equivalent(
            &f.float(3.14),
            &f.float(3.14)
        ));
        assert!(atoms_are_alpha_equivalent(
            &f.string("hi"),
            &f.string("hi")
        ));
        assert!(!atoms_are_alpha_equivalent(
            &f.string("hi"),
            &f.string("bye")
        ));
    }

    #[test]
    fn test_unit() {
        let f = global_factory();
        assert!(atoms_are_alpha_equivalent(&f.unit(), &f.unit()));
    }

    #[test]
    fn test_empty() {
        let f = global_factory();
        assert!(atoms_are_alpha_equivalent(&f.empty(), &f.empty()));
        assert!(!atoms_are_alpha_equivalent(&f.unit(), &f.empty()));
    }

    #[test]
    fn test_different_length_sexprs() {
        let f = global_factory();
        let a = f.sexpr(vec![f.atom("$x")]);
        let b = f.sexpr(vec![f.atom("$a"), f.atom("$b")]);
        assert!(!atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_variable_vs_nonvariable() {
        let f = global_factory();
        let a = f.atom("$x");
        let b = f.atom("foo");
        assert!(!atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_conjunctions() {
        let f = global_factory();
        let a = f.conjunction(vec![f.atom("$x"), f.atom("$y")]);
        let b = f.conjunction(vec![f.atom("$a"), f.atom("$b")]);
        assert!(atoms_are_alpha_equivalent(&a, &b));
    }

    #[test]
    fn test_type_values() {
        let f = global_factory();
        let a = f.type_value(f.atom("Number"));
        let b = f.type_value(f.atom("Number"));
        assert!(atoms_are_alpha_equivalent(&a, &b));
        let c = f.type_value(f.atom("String"));
        assert!(!atoms_are_alpha_equivalent(&a, &c));
    }

    #[test]
    fn test_errors() {
        let f = global_factory();
        let a = f.error("msg", f.atom("$x"));
        let b = f.error("msg", f.atom("$y"));
        assert!(atoms_are_alpha_equivalent(&a, &b));
        let c = f.error("other", f.atom("$y"));
        assert!(!atoms_are_alpha_equivalent(&a, &c));
    }
}
