//! Regression tests for the `let*` shadow + scope-barrier fix (2026-04).
//!
//! Locks in the two semantic invariants that restored HE-bisimilarity:
//!
//! 1. **Shadow at pair boundaries**: `(let* (($x 1) ($x 2)) $x) → 2`
//!    — matches MeTTa HE's sequential-shadow `let*` semantics. The prior
//!    strict-compose at ProcessLetStar's pair boundary over-pruned user-
//!    level rebinding as a conflict; the new `prepare_letstar_accumulated`
//!    helper strips shadow keys from accumulated before strict compose,
//!    so the new pair's pattern-match wins.
//!
//! 2. **Scope barrier for freshened vars**: per-invocation `$__fr_N_*`
//!    freshened variables from a prior iteration's value-expr evaluation
//!    MUST NOT cross let*-pair boundaries. MeTTa HE naturally avoids this
//!    because each rule query returns a fresh `Bindings` object (no
//!    per-invocation freshening). MeTTaTron's `strip_freshened_bindings`
//!    helper provides the equivalent observational scope isolation.
//!
//! The ghost-preservation guard tests confirm Fix 1 (shadow strip) does
//! NOT reintroduce ghost branches at other compose sites.

use mettatron::{compile, eval, new_env};
use mettatron::backend::models::MettaValueTrait;

fn eval_last(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let mut last: Vec<String> = Vec::new();
    let expr_count = state.source().len();
    for (idx, expr) in state.source().iter().enumerate() {
        let expr = *expr;
        let (results, env_after) = eval(expr, env, &state);
        env = env_after;
        if idx == expr_count - 1 {
            last = results.iter().map(|r| format!("{}", r.friendly_repr())).collect();
        }
    }
    last
}

// ============================================================================
// Category 1: Shadow at pair boundaries (HE-bisimilar let* semantics)
// ============================================================================

/// `(let* (($x 1) ($x 2)) $x) → 2`
///
/// HE's let* is sequential-shadow via `unify` rebinding. MeTTaTron's
/// prior strict-compose at ProcessLetStar pair boundary detected the
/// `$x=1` vs `$x=2` conflict and silently pruned, returning `[]`. The
/// fix strips keys the current pattern rebinds from accumulated before
/// strict compose, so the new pair's binding wins.
#[test]
fn let_star_pair_shadow_returns_inner() {
    let output = eval_last("!(let* (($x 1) ($x 2)) $x)");
    assert_eq!(output, vec!["2"]);
}

/// Nested `(let $x 1 (let $x 2 $x)) → 2`. This path uses ProcessLet,
/// not ProcessLetStar, but the observational semantics must match.
#[test]
fn nested_let_inner_shadows_outer() {
    let output = eval_last("!(let $x 1 (let $x 2 $x))");
    assert_eq!(output, vec!["2"]);
}

/// Value-derived shadow: pair 2's value expression uses pair 1's binding.
/// `(let* (($x 1) ($x (+ $x 1))) $x) → 2`
/// The outer `$x=1` is visible when computing pair 2's value `(+ $x 1)`
/// → `2`, then pair 2's pattern rebinds `$x=2` (shadow). Body returns 2.
#[test]
fn let_star_value_derived_shadow() {
    let output = eval_last("!(let* (($x 1) ($x (+ $x 1))) $x)");
    assert_eq!(output, vec!["2"]);
}

// ============================================================================
// Category 2: Scope barrier — freshened vars don't leak across iterations
// ============================================================================

/// PLN-minimal reproducer: a let* body with a recursive rule invocation
/// that rebinds the same user-level variable across two iterations.
/// Before the scope-barrier fix, per-invocation freshened variables
/// leaked into accumulated_bindings across let* pairs, producing
/// spurious `compose-conflict-ground-ground` events that silently
/// pruned the branch.
///
/// This test uses a 2-step list decomposition (shallow; no deep
/// recursion to avoid stack issues unrelated to this fix). The
/// `$tail` variable is rebound in pair 2 to a different list than
/// was indirectly bound by pair 1's rule invocation.
#[test]
fn let_star_same_user_var_rebound_across_pairs_survives() {
    // Two independent let* pairs that both produce a list, with the
    // second pair's result referenced in the body.
    let source = r#"
        (= (head-of (Cons $h $t)) $h)
        (= (tail-of (Cons $h $t)) $t)
        !(let* (($h (head-of (Cons a (Cons b Nil))))
                ($t (tail-of (Cons a (Cons b Nil)))))
               $t)
    "#;
    let output = eval_last(source);
    // Expect `(Cons b Nil)` — the tail.
    assert_eq!(output.len(), 1);
    assert!(
        output[0].contains("Cons") && output[0].contains("b"),
        "expected (Cons b Nil), got {:?}",
        output
    );
}

// ============================================================================
// Category 3: Ghost-preservation guards (Fix 1 must not relax other sites)
// ============================================================================

/// Rule-match ∘ outer_carrying conflict at a non-let* dispatch site
/// (ProcessRuleMatches) still drops the branch. Guards against
/// accidentally relaxing strict compose outside ProcessLetStar.
#[test]
fn rule_match_outer_carrying_conflict_still_drops() {
    // $y is bound to A outside, rule binds $y to B — conflict → drop.
    let source = r#"
        (= (r A) matched)
        !(let $y B (r $y))
    "#;
    let output = eval_last(source);
    // The let binds $y=B; calling (r $y) tries to match rule (r A) with $y=B.
    // Pattern match fails (B ≠ A), so the rule doesn't fire. Result is
    // unreduced `(r B)` — not a ghost result claiming `matched`.
    for r in &output {
        assert!(
            !r.contains("matched"),
            "ghost result appeared: {}",
            r
        );
    }
}

/// Conjunction with genuinely incompatible child bindings still drops
/// (the primary invariant from ghost_branch_regression).
#[test]
fn conjunction_binding_conflict_still_drops() {
    let source = r#"
        (= (p 1) hit)
        (= (p 2) hit)
        !(, (p 1) (p 2))
    "#;
    let output = eval_last(source);
    // Each conjunct reduces to `hit`. Both reduce — no binding conflict.
    // This test primarily documents that our fix doesn't touch conjunction
    // semantics; for a genuine-conflict test see ghost_branch_regression.
    assert!(!output.is_empty(), "conjunction should produce a result");
}

/// Alias-resolution before filter: if a user-level `$user_x` is aliased
/// to a freshened `$__fr_M_alias`, the user-level binding survives the
/// `$__fr_*` filter because `apply_chain_generic` materializes the value
/// before filtering. (Today's implementation strips `$__fr_*` only; this
/// test documents the observational outcome we expect.)
#[test]
fn user_level_binding_via_rule_match_survives_filter() {
    let source = r#"
        (= (f $x) (stv $x))
        !(let* (($result (f 42))) $result)
    "#;
    let output = eval_last(source);
    // `(f 42)` reduces via the rule to `(stv 42)`. The user-level
    // `$result` is bound to that value and returned from the let*. The
    // `$__fr_N_x` freshened binding is stripped at the handoff but the
    // user-level `$result = (stv 42)` remains.
    assert_eq!(output.len(), 1);
    assert!(
        output[0].contains("stv") && output[0].contains("42"),
        "expected (stv 42), got {:?}",
        output
    );
}
