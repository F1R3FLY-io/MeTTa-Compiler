//! Pattern matching for MeTTa values.
//!
//! This module implements the core pattern matching algorithm for MeTTa,
//! supporting variable binding, wildcards, and structural matching.

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, MettaValueFactory, ValueView};

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
        // PT-canonical Lazy semantics (2026-05-21): a Lazy-wrapped VALUE
        // means "this is data, do not dispatch rules on it". Rule matching
        // probes structural equality against literal rule LHSs — so a Lazy
        // value should NOT match any rule (rule dispatch is inhibited).
        //
        // We do NOT strip Lazy here. Instead, a Lazy-wrapped value fails
        // to match anything except wildcards (handled below). The trampoline
        // `Eval` arm's Lazy short-circuit returns the inner value before
        // rule lookup would even reach pattern_match.
        //
        // The Lazy on the PATTERN side is unusual (rules are user-authored
        // and wouldn't contain Lazy markers); leave that case as-is — bare
        // structural equality against a Lazy pattern fails for any non-Lazy
        // value, which is the conservative correct behavior.

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
                //
                // Y.1 (2026-05-12): fast-path for the structurally-identical case
                // (e.g. stored fact `$x` queried against pattern `$x`). Without it
                // the unify work-stack push (existing, val) where existing == val
                // re-enters this arm with same name, same value → infinite loop
                // (mork_removal_demo.metta hang). PartialEq on MettaValue is cheap
                // (Copy struct holding a 'static pointer).
                if let Some((_, existing)) = bindings.iter().find(|(name, _)| *name == p) {
                    if *existing == val {
                        true
                    } else {
                        work_stack.push((*existing, val));
                        true
                    }
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

            // S-expressions: push children onto work stack (replaces recursion).
            //
            // Phase 2.x PT/PLN cons-pattern (restored 2026-05-22):
            // `(cons HEAD TAIL)` pattern matches any SExpr where the value's
            // first element matches HEAD and the value's remaining elements
            // (as an SExpr) match TAIL. This is needed by PLN's `=>` macro:
            //   `(= (=> (cons , $args) $C $stvImp) ...)` expects $args to
            //   bind to the tail of a `(, A B ...)` value.
            //
            // Dotted-pair pattern support (2026-05-11): `($x . $rest)` patterns
            // bind $x to the first value and $rest to an SExpr of the rest.
            // The pattern is recognized by `.` at position n-2 in p_items.
            (ValueView::SExpr(p_items), ValueView::SExpr(v_items)) => {
                if p_items.len() == 3
                    && p_items[0].as_atom() == Some("cons")
                    && !v_items.is_empty()
                {
                    // (cons HEAD TAIL) destructure: HEAD matches v_items[0],
                    // TAIL matches the SExpr of v_items[1..].
                    let factory = crate::backend::models::gc_allocator::global_factory();
                    let tail_sexpr = factory.sexpr(v_items[1..].to_vec());
                    work_stack.push((p_items[2], tail_sexpr));
                    work_stack.push((p_items[1], v_items[0]));
                    continue;
                }
                if p_items.len() >= 2
                    && p_items[p_items.len() - 2].as_atom() == Some(".")
                {
                    let head_len = p_items.len() - 2;
                    if v_items.len() < head_len {
                        return false;
                    }
                    // Push head matches in reverse for LIFO order.
                    for i in (0..head_len).rev() {
                        work_stack.push((p_items[i], v_items[i]));
                    }
                    // Build the rest SExpr from v_items[head_len..] and push
                    // a match against the rest-var pattern.
                    let rest_pattern = p_items[p_items.len() - 1];
                    let rest_value = if v_items.len() == head_len {
                        // Zero remaining → empty SExpr.
                        crate::backend::models::global_factory().sexpr(Vec::new())
                    } else {
                        crate::backend::models::global_factory()
                            .sexpr(v_items[head_len..].to_vec())
                    };
                    work_stack.push((rest_pattern, rest_value));
                    return true;
                }
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

            // S3 (ERROR-MATCH cross-shape): HE represents errors as the 3-element
            // S-expression `(Error <offending> <detail>)`, but MeTTaTron stores
            // them as a dedicated `Error(offending, detail)` variant. User patterns
            // are parsed as SExpr-shaped `(Error $a $c)`, so we project the variant
            // into the pseudo-SExpr shape and push the two child pairs onto the
            // work stack. Matches HE's `error_atom() = Atom::expr([ERROR_SYMBOL, atom, err])`
            // (hyperon-experimental/lib/src/metta/mod.rs:54-74; metta-specification/spec/C-errors.md:7-14).
            (ValueView::SExpr(p_items), ValueView::Error(v_off, v_detail))
                if p_items.len() == 3
                    && matches!(p_items[0].view(), ValueView::Atom(s) if s == "Error") =>
            {
                // Push in reverse so detail child is processed after offending (LIFO).
                work_stack.push((p_items[2], v_detail));
                work_stack.push((p_items[1], v_off));
                true
            }

            // S3 (symmetric): Error-variant pattern vs SExpr-shaped value
            // (`(Error $a $c)` value reaching `(Error _ _)` variant pattern). This
            // is less common but ensures bidirectional cross-shape unification so
            // that data-borne errors flow through pattern_match identically to
            // variant-borne errors.
            (ValueView::Error(p_off, p_detail), ValueView::SExpr(v_items))
                if v_items.len() == 3
                    && matches!(v_items[0].view(), ValueView::Atom(s) if s == "Error") =>
            {
                work_stack.push((p_detail, v_items[2]));
                work_stack.push((p_off, v_items[1]));
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

    /// S3 (ERROR-MATCH cross-shape): `(Error $a $c)` SExpr-pattern must match
    /// against the dedicated `Error(offending, detail)` variant.
    #[test]
    fn pattern_sexpr_error_matches_error_variant() {
        let f = global_factory();
        let pattern = f.sexpr(vec![f.atom("Error"), f.atom("$a"), f.atom("$c")]);
        let value = f.error(f.atom("foo"), f.string("boom"));
        let bindings = pattern_match(&pattern, &value);
        assert!(
            bindings.is_some(),
            "(Error $a $c) must match Error(offending, detail), got {:?}",
            bindings
        );
        let b = bindings.expect("Some");
        let a = b
            .iter()
            .find(|(name, _)| *name == "$a")
            .expect("$a is bound");
        assert_eq!(a.1.as_atom(), Some("foo"));
        let c = b
            .iter()
            .find(|(name, _)| *name == "$c")
            .expect("$c is bound");
        assert_eq!(c.1.as_string(), Some("boom"));
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
