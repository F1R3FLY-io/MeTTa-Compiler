//! Bidirectional Unification (Martelli-Montanari)
//!
//! Implements bidirectional structural unification using the Martelli-Montanari
//! algorithm, operating directly on MettaValue trees without intermediate
//! heap flattening.
//!
//! ## Algorithm
//!
//! Maintains a work stack of (lhs, rhs) equation pairs. For each pair:
//! 1. Dereference both sides through existing bindings (transitive)
//! 2. Variable vs anything: occurs check, then bind (or verify consistency)
//! 3. S-expression decomposition: arity check, then push child pairs
//! 4. Ground term comparison: structural equality
//!
//! Based on: Martelli & Montanari (1982), "An Efficient Unification Algorithm"
//! Reference implementation with Rocq proofs: mettail-rust/prattail/src/unification.rs
//!
//! ## Complexity
//!
//! O(n * k) where n = total term size and k = number of variables. For typical
//! MeTTa patterns with 3-10 variables, this is effectively linear.
//!
//! ## History
//!
//! Previously used WAM-style union-find with heap flattening (O(n·α(n)) amortized).
//! Replaced with Martelli-Montanari because:
//! - Eliminates MettaValue → Cell heap → MettaValue round-trip conversion
//! - Path compression didn't amortize (fresh heap per call)
//! - Trail was unused (no choice-point backtracking within single unification)
//! - Formally verified reference available in mettail-rust

use super::engine::Bindings;
use crate::backend::eval::bindings::{
    bidirectional_unify_generic, bidirectional_unify_generic_with_mode,
};
use crate::backend::models::{BindingsWithClasses, MettaValue, UnifyMode};

// ============================================================================
// Public API
// ============================================================================

/// Bidirectional structural unification using Martelli-Montanari.
///
/// Returns `Some(bindings)` if `a` and `b` unify, `None` on failure.
/// Handles variables on both sides simultaneously, performs occurs check,
/// and checks variable consistency.
///
/// # Examples
///
/// ```text
/// bidirectional_unify((a $x), ($y b)) → Some({$x → b, $y → a})
/// bidirectional_unify(($x $x), (a b)) → None (conflict: $x can't be both a and b)
/// bidirectional_unify($x, (f $x))     → None (occurs check)
/// ```
pub fn bidirectional_unify(a: &MettaValue, b: &MettaValue) -> Option<Bindings> {
    bidirectional_unify_generic(a, b)
}

/// S0d.1: Bidirectional unification with explicit [`UnifyMode`].
///
/// - [`UnifyMode::Match`]: behaves identically to [`bidirectional_unify`] —
///   var-var-distinct creates an ordinary chain-terminus binding. Returned
///   [`BindingsWithClasses`] has no class table.
/// - [`UnifyMode::Unify`]: for the user-facing `(unify ...)` form.
///   Var-var-distinct creates an equivalence class via
///   [`BindingsWithClasses::insert_equivalence`], preserving HE's
///   M-VAR-VAR-DISTINCT semantics (spec §4.3.1).
pub fn bidirectional_unify_with_mode(
    a: &MettaValue,
    b: &MettaValue,
    mode: UnifyMode,
) -> Option<BindingsWithClasses<MettaValue>> {
    bidirectional_unify_generic_with_mode(a, b, mode)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{
        global_factory, init_global_allocator, MettaValue, MettaValueFactory,
    };

    fn setup() {
        let _ = init_global_allocator();
    }

    fn atom(s: &str) -> MettaValue {
        global_factory().atom(s)
    }

    fn sexpr(items: Vec<MettaValue>) -> MettaValue {
        global_factory().sexpr(items)
    }

    fn long(n: i64) -> MettaValue {
        global_factory().long(n)
    }

    #[test]
    fn test_identical_atoms() {
        setup();
        let result = bidirectional_unify(&atom("a"), &atom("a"));
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_different_atoms_fail() {
        setup();
        assert!(bidirectional_unify(&atom("a"), &atom("b")).is_none());
    }

    #[test]
    fn test_variable_left() {
        setup();
        let result = bidirectional_unify(&atom("$x"), &atom("a")).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_variable_right() {
        setup();
        let result = bidirectional_unify(&atom("a"), &atom("$x")).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_both_sides_variables() {
        setup();
        // (a $x) unify ($y b) → {$x → b, $y → a}
        let lhs = sexpr(vec![atom("a"), atom("$x")]);
        let rhs = sexpr(vec![atom("$y"), atom("b")]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "b");
        assert_eq!(result.get("$y").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_conflict_same_variable() {
        setup();
        // ($x $x) unify (a b) → fail (conflict: $x can't be both a and b)
        let lhs = sexpr(vec![atom("$x"), atom("$x")]);
        let rhs = sexpr(vec![atom("a"), atom("b")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_same_variable_consistent() {
        setup();
        // ($x $x) unify (a a) → {$x → a}
        let lhs = sexpr(vec![atom("$x"), atom("$x")]);
        let rhs = sexpr(vec![atom("a"), atom("a")]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_occurs_check() {
        setup();
        // $x unify (f $x) → fail (infinite term)
        let lhs = atom("$x");
        let rhs = sexpr(vec![atom("f"), atom("$x")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_nested_sexpr() {
        setup();
        // (f (g $x)) unify (f (g a)) → {$x → a}
        let lhs = sexpr(vec![atom("f"), sexpr(vec![atom("g"), atom("$x")])]);
        let rhs = sexpr(vec![atom("f"), sexpr(vec![atom("g"), atom("a")])]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_arity_mismatch() {
        setup();
        let lhs = sexpr(vec![atom("a"), atom("b")]);
        let rhs = sexpr(vec![atom("a"), atom("b"), atom("c")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_two_variables() {
        setup();
        // $x unify $y → binds one to the other (both unbound)
        let result = bidirectional_unify(&atom("$x"), &atom("$y"));
        assert!(result.is_some());
    }

    #[test]
    fn test_wildcard() {
        setup();
        let result = bidirectional_unify(&atom("_"), &atom("a"));
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_integers() {
        setup();
        assert!(bidirectional_unify(&long(42), &long(42)).is_some());
        assert!(bidirectional_unify(&long(42), &long(43)).is_none());
    }

    #[test]
    fn test_integer_in_sexpr() {
        setup();
        // (2 $list) unify (2 (Cons a b)) → {$list → (Cons a b)}
        let lhs = sexpr(vec![long(2), atom("$list")]);
        let rhs = sexpr(vec![
            long(2),
            sexpr(vec![atom("Cons"), atom("a"), atom("b")]),
        ]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        let list = result.get("$list").unwrap();
        assert!(list.as_sexpr().is_some());
    }

    #[test]
    fn test_ampersand_variable() {
        setup();
        let result = bidirectional_unify(&atom("&x"), &atom("hello")).unwrap();
        assert_eq!(result.get("&x").unwrap().as_atom().unwrap(), "hello");
    }

    #[test]
    fn test_quote_variable() {
        setup();
        let result = bidirectional_unify(&atom("'x"), &atom("hello")).unwrap();
        assert_eq!(result.get("'x").unwrap().as_atom().unwrap(), "hello");
    }
}
