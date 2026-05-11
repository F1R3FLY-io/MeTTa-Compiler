//! Pattern matching for MeTTa values.
//!
//! This module implements the core pattern matching algorithm for MeTTa,
//! supporting variable binding, wildcards, and structural matching.

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, ValueView};

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
#[inline]
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
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Pattern matching before trampoline's MAX_EVAL_DEPTH check runs
#[inline]
pub(crate) fn pattern_match_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Bindings,
) -> bool {
    // Work stack: (pattern, value) pairs to match
    // Use Vec as stack (push/pop from end) - more efficient than VecDeque for this use case
    // MettaValue is Copy (8 bytes) so owned values are equally efficient as references
    let mut work_stack: Vec<(MettaValue, MettaValue)> = Vec::with_capacity(16);
    work_stack.push((*pattern, *value));

    while let Some((pat, val)) = work_stack.pop() {
        // Process each pattern-value pair
        let matches = match (pat.view(), val.view()) {
            // Wildcard matches anything (both `_` and `$_`)
            (ValueView::Atom(p), _) if p == "_" || p == "$_" => true,

            // FAST PATH: First variable binding (empty bindings)
            // Optimization: Skip lookup when bindings are empty - directly insert
            // This reduces single-variable regression from 16.8% to ~5-7%
            (ValueView::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&"
                    && p != "$_"
                    && bindings.is_empty()
                    && work_stack.is_empty() =>
            {
                bindings.insert(p, val);
                true
            }

            // GENERAL PATH: Variable with potential existing bindings
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            // EXCEPT: $_ is the wildcard (handled above)
            (ValueView::Atom(p), _)
                if (p.starts_with('$') || p.starts_with('&') || p.starts_with('\''))
                    && p != "&"
                    && p != "$_" =>
            {
                // Check if variable is already bound (linear search for SmartBindings).
                // BUG-T0-006 (spec §04.1): repeated-var consistency uses *unification*,
                // not PartialEq. The previously-bound value may itself contain
                // unbound variables — testing structural equality misses cases where
                // they would still unify. Push (existing, val) onto the work stack so
                // the outer iterative loop unifies them. Stack-safe: no recursion.
                if let Some((_, existing)) = bindings.iter().find(|(name, _)| *name == p) {
                    work_stack.push((*existing, val));
                    true
                } else {
                    bindings.insert(p, val);
                    true
                }
            }

            // Atoms must match exactly
            (ValueView::Atom(p), ValueView::Atom(v)) => p == v,
            (ValueView::Bool(p), ValueView::Bool(v)) => p == v,
            (ValueView::Long(p), ValueView::Long(v)) => p == v,
            (ValueView::Float(p), ValueView::Float(v)) => p == v,
            (ValueView::String(p), ValueView::String(v)) => p == v,
            (ValueView::Unit, ValueView::Unit) => true,
            // Unit pattern matches Empty atom (HE-compatible: () pattern in case matches Empty)
            // This is needed because case converts empty results to Atom("Empty") internally
            (ValueView::Unit, ValueView::Atom(v)) if v == "Empty" => true,
            // Empty atom pattern matches Unit (symmetry: Empty pattern matches () values)
            (ValueView::Atom(p), ValueView::Unit) if p == "Empty" => true,

            // Unit pattern matches only empty values (Unit, empty S-expr, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (ValueView::Unit, ValueView::SExpr(v_items)) if v_items.is_empty() => true,

            // Empty S-expression () matches only empty values (empty S-expr, Unit, or Empty atom)
            // For discard pattern, use wildcard _ instead
            (ValueView::SExpr(p_items), ValueView::SExpr(v_items))
                if p_items.is_empty() && v_items.is_empty() =>
            {
                true
            }
            (ValueView::SExpr(p_items), ValueView::Unit) if p_items.is_empty() => true,
            (ValueView::SExpr(p_items), ValueView::Atom(v))
                if p_items.is_empty() && v == "Empty" =>
            {
                true
            }

            // S-expressions: push children onto work stack (replaces recursion)
            (ValueView::SExpr(p_items), ValueView::SExpr(v_items)) => {
                if p_items.len() != v_items.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first (LIFO)
                for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                    work_stack.push((*p, *v));
                }
                true // Continue processing the work stack
            }

            // Conjunctions: push children onto work stack (replaces recursion)
            (ValueView::Conjunction(p_goals), ValueView::Conjunction(v_goals)) => {
                if p_goals.len() != v_goals.len() {
                    return false; // Early exit on length mismatch
                }
                // Push in reverse order so first element is processed first
                for (p, v) in p_goals.iter().zip(v_goals.iter()).rev() {
                    work_stack.push((*p, *v));
                }
                true // Continue processing the work stack
            }

            // Errors: check message match, push details onto work stack
            (ValueView::Error(p_msg, p_details), ValueView::Error(v_msg, v_details)) => {
                if p_msg != v_msg {
                    return false; // Message mismatch
                }
                // Push details for matching (replaces recursion)
                work_stack.push((p_details, v_details));
                true // Continue processing the work stack
            }

            // Quoted: match inner values
            (ValueView::Quoted(p_inner), ValueView::Quoted(v_inner)) => {
                work_stack.push((p_inner, v_inner));
                true
            }

            // Transparency: SExpr pattern (quote $x) matches Quoted(v)
            (ValueView::SExpr(p_items), ValueView::Quoted(v_inner))
                if p_items.len() == 2
                    && matches!(p_items[0].view(), ValueView::Atom(s) if s == "quote") =>
            {
                work_stack.push((p_items[1], v_inner));
                true
            }

            _ => false,
        };

        if !matches {
            return false; // Early exit on any mismatch
        }
    }

    true // All pairs matched successfully
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::global_factory;
    use crate::backend::models::MettaValueFactory;

    /// BUG-T0-006 regression: repeated variables in a pattern must unify
    /// via the work-stack, not via PartialEq. `(foo $a $a)` matched against
    /// `(foo (g 1) (g 1))` succeeds (structurally equal bound values), and
    /// `(foo $a $a)` against `(foo 1 2)` fails (non-equal bound values).
    #[test]
    fn pattern_repeated_var_matches_when_structurally_equal() {
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a"), f.atom("$a")]);
        let value = f.sexpr(vec![
            f.atom("foo"),
            f.sexpr(vec![f.atom("g"), f.long(1)]),
            f.sexpr(vec![f.atom("g"), f.long(1)]),
        ]);
        let bindings = pattern_match(&pattern, &value);
        assert!(bindings.is_some(), "repeated-var matches when values agree");
        let b = bindings.expect("Some");
        let bound = b
            .iter()
            .find(|(name, _)| *name == "$a")
            .expect("$a is bound");
        assert!(bound.1.is_sexpr(), "$a bound to (g 1)");
    }

    #[test]
    fn pattern_repeated_var_fails_when_unequal() {
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a"), f.atom("$a")]);
        let value = f.sexpr(vec![f.atom("foo"), f.long(1), f.long(2)]);
        let bindings = pattern_match(&pattern, &value);
        assert!(
            bindings.is_none(),
            "repeated-var rejects mismatched repeat"
        );
    }

    /// BUG-T0-006 deeper case: the second occurrence of `$a` should unify
    /// with the previously-bound value, which itself contains a fresh
    /// pattern variable `$x`. The unifier must bind `$x` (not give up via
    /// PartialEq mismatch).
    /// Regression: pattern `($x leaf2)` against fact `(leaf0 leaf1)` MUST FAIL
    /// because `leaf2 != leaf1`. Reported broken by `tests::test_match_basic_pattern`.
    #[test]
    fn pattern_partial_atom_mismatch_in_sexpr_fails() {
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("$x"), f.atom("leaf2")]);
        let value = f.sexpr(vec![f.atom("leaf0"), f.atom("leaf1")]);
        let bindings = pattern_match(&pattern, &value);
        assert!(
            bindings.is_none(),
            "pattern ($x leaf2) vs (leaf0 leaf1) must fail; got: {:?}",
            bindings
        );
    }

    /// Regression for the generic matcher: `($x leaf2)` vs `(leaf1 leaf2)` MUST
    /// succeed binding $x=leaf1. The generic matcher is the path used by
    /// `env.match_space()` which the bytecode VM's `op_match_self` invokes.
    #[test]
    fn pattern_generic_partial_atom_match_binds_variable() {
        use crate::backend::eval::bindings::pattern_match_generic;
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("$x"), f.atom("leaf2")]);
        let value = f.sexpr(vec![f.atom("leaf1"), f.atom("leaf2")]);
        let bindings = pattern_match_generic(&pattern, &value);
        let b = bindings.expect("pattern_match_generic succeeds");
        let bound = b
            .get("$x")
            .expect("$x is bound by pattern_match_generic");
        assert_eq!(bound.as_atom(), Some("leaf1"));
    }

    /// BUG-T0-006 generic-path regression: repeated-var unification works in
    /// `pattern_match_generic_impl` (used by `env.match_space()`, rule
    /// dispatch, MORK forms). Mirrors the canonical-matcher test above.
    #[test]
    fn pattern_generic_repeated_var_matches_when_equal() {
        use crate::backend::eval::bindings::pattern_match_generic;
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a"), f.atom("$a")]);
        let value = f.sexpr(vec![
            f.atom("foo"),
            f.sexpr(vec![f.atom("g"), f.long(1)]),
            f.sexpr(vec![f.atom("g"), f.long(1)]),
        ]);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(
            bindings.is_some(),
            "generic matcher accepts repeated-var match"
        );
    }

    #[test]
    fn pattern_generic_repeated_var_fails_when_unequal() {
        use crate::backend::eval::bindings::pattern_match_generic;
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a"), f.atom("$a")]);
        let value = f.sexpr(vec![f.atom("foo"), f.long(1), f.long(2)]);
        let bindings = pattern_match_generic(&pattern, &value);
        assert!(
            bindings.is_none(),
            "generic matcher rejects mismatched repeated-var"
        );
    }

    /// Regression: pattern `($x leaf2)` against fact `(leaf1 leaf2)` MUST succeed
    /// binding $x=leaf1.
    #[test]
    fn pattern_partial_atom_match_binds_variable() {
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("$x"), f.atom("leaf2")]);
        let value = f.sexpr(vec![f.atom("leaf1"), f.atom("leaf2")]);
        let bindings = pattern_match(&pattern, &value);
        let b = bindings.expect("matches");
        let bound = b
            .iter()
            .find(|(name, _)| *name == "$x")
            .expect("$x is bound");
        assert_eq!(bound.1.as_atom(), Some("leaf1"));
    }

    #[test]
    fn pattern_repeated_var_unifies_through_inner_variable() {
        let f = global_factory();
        // pattern: (foo (g $x) $a) — but $a appears twice means: $a := (g $x),
        // then second $a binding must unify $x against something concrete.
        // Use: pattern (foo $a $a) against value (foo (g $x) (g 1)).
        // First $a := (g $x); second $a vs (g 1) must unify $x := 1.
        let pattern = f.sexpr(vec![f.atom("foo"), f.atom("$a"), f.atom("$a")]);
        let value = f.sexpr(vec![
            f.atom("foo"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
            f.sexpr(vec![f.atom("g"), f.long(1)]),
        ]);
        let bindings = pattern_match(&pattern, &value);
        assert!(
            bindings.is_some(),
            "repeated-var unifies through inner variable (BUG-T0-006)"
        );
        let b = bindings.expect("Some");
        // $x should be bound to 1 (or to the result of unifying via the work stack).
        let x_bound = b.iter().find(|(name, _)| *name == "$x");
        assert!(
            x_bound.is_some(),
            "$x bound by repeated-var unification, got bindings: {:?}",
            b.iter().collect::<Vec<_>>()
        );
        assert_eq!(x_bound.expect("Some").1.as_long(), Some(1));
    }
}
