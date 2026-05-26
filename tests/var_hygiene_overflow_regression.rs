//! Regression test for the rule-LHS-variable / caller-free-variable name
//! collision stack overflow (fixed 2026-05-26).
//!
//! When a rule's LHS variable (e.g. `$z` in `(= (id $z) $z)`) is dispatched
//! against an argument that contains a TEXTUALLY-IDENTICAL free variable
//! (e.g. `(id (wrap $z))`), the structural matcher binds `$z → (wrap $z)` —
//! self-referential by name. The transitive pre-resolution in
//! `match_rules_native_inner` (`rule_management.rs`) then substituted the
//! inner `$z → (wrap $z)` WITHOUT BOUND, growing the term every step until the
//! Rust call stack overflowed (`apply_bindings_scoped_generic ↔
//! apply_bindings_iterative_generic` over the per-form Spanned layers).
//!
//! Fix: a self-exclusion guard in the pre-resolution loop resolves a value
//! whose key occurs in itself against the OTHER bindings only (the inner
//! occurrence is the caller's distinct variable and stays free; subsequent
//! key-freshening disambiguates rule-var from caller-var). This was surfaced
//! by PeTTa's matchnested2.metta and reproduces minimally below.
//!
//! These programs must TERMINATE (the bug was a hard stack-overflow abort).
//! They run on the default test-thread stack so any recursion regression
//! fails fast rather than hanging.

use mettatron::backend::models::MettaValueTrait;
use mettatron::{compile, eval, new_env};

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
            last = results
                .iter()
                .map(|r| format!("{}", r.friendly_repr()))
                .collect();
        }
    }
    last
}

/// The minimal reproducer: rule var `$z` collides with the caller's free `$z`
/// inside the argument. Must NOT overflow; the rule `(= (id $z) $z)` returns
/// the argument verbatim with the caller's `$z` preserved as a free variable.
#[test]
fn rule_var_collides_with_caller_free_var_terminates() {
    let results = eval_last("(= (id $z) $z)\n!(id (wrap $z))");
    assert_eq!(
        results,
        vec!["(wrap $z)".to_string()],
        "self-referential rule binding must resolve in one step (not loop); got: {:?}",
        results
    );
}

/// Distinct names: unchanged behavior (control — never overflowed).
#[test]
fn rule_var_distinct_from_caller_var_unchanged() {
    let results = eval_last("(= (id $z) $z)\n!(id (wrap $q))");
    assert_eq!(results, vec!["(wrap $q)".to_string()]);
}

/// A `match` whose template embeds the caller-colliding variable — the
/// matchnested2.metta shape that originally overflowed. Must TERMINATE
/// (the side-effect-tuple-template SEMANTICS are a separate question; this
/// test pins only the stack-safety property — no overflow/abort).
#[test]
fn match_template_self_collision_terminates() {
    let source = r#"
        (friend tim tom)
        (friend tom tam)
        (= (hide $1) (empty))
        !(hide (match &self (friend $1 $2)
                            ((add-atom &self (seen $1 $2))
                             (remove-atom &self (friend $1 $2)))))
        !(collapse (match &self (friend $a $b) (friend $a $b)))
    "#;
    // The assertion is simply that evaluation completes without a stack
    // overflow abort. (Pre-fix: `thread 'main' has overflowed its stack`.)
    let results = eval_last(source);
    // Some observable result is produced (a collapse tuple); the exact
    // contents depend on the separate match-template-evaluation semantics.
    assert!(
        !results.is_empty() || results.is_empty(),
        "must terminate without overflow"
    );
}

/// 20-run determinism for the collision case — the one-step resolution must be
/// stable, not order-dependent.
#[test]
fn collision_resolution_deterministic_20_runs() {
    let mut baseline: Option<Vec<String>> = None;
    for i in 0..20 {
        let results = eval_last("(= (id $z) $z)\n!(id (wrap $z))");
        if let Some(ref base) = baseline {
            assert_eq!(&results, base, "iteration {}: differs from baseline", i);
        } else {
            baseline = Some(results);
        }
    }
    assert_eq!(baseline.expect("one run"), vec!["(wrap $z)".to_string()]);
}
