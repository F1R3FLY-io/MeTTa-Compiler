//! Regression smoke tests for the PLN.Query bindings-flow pipeline.
//!
//! These tests anchor the P2+P3 fixes (commit b359684) that resolved
//! repeated-var rule dispatch in T1: bidirectional unification's
//! query-side bindings (e.g. `$1 → Anna`) must flow through to the
//! compiled-RHS BindingFrame so PushVariable resolves transitively.
//!
//! See the PLN performance plan ledger for the full diagnosis
//! (R1: binding-loss between rule-match and case body).

use mettatron::{compile, eval, new_env};

/// Run a MeTTa source program and return the result list of the LAST
/// top-level expression as `Display`-formatted strings.
fn run_one(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    let exprs: Vec<_> = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    let mut env = env;
    let mut last_results = Vec::new();
    for expr in &exprs {
        let (results, new_env, ..) = eval(*expr, env, &state);
        env = new_env;
        last_results = results.into_iter().collect();
    }
    last_results.iter().map(|v| format!("{}", v)).collect()
}

/// PLN modus ponens with a free variable on the query side that must
/// unify with the rule's already-bound repeated variable. This is the
/// canonical case that drove the P2+P3 fix.
#[test]
fn modus_ponens_repeated_var_substitutes_through_rhs() {
    let r = run_one(
        r#"
        (= (|- ($A $T1) ((Implication $A $B) $T2))
           ($B (mp $T1 $T2)))
        !(|- ((Inheritance Anna (IntSet smokes)) (stv 1 0.9))
             ((Implication (Inheritance $1 (IntSet smokes))
                           (Inheritance $1 (IntSet cancerous)))
              (stv 0.6 0.9)))
    "#,
    );
    assert_eq!(
        r,
        vec!["((Inheritance Anna (IntSet cancerous)) (mp (stv 1 0.9) (stv 0.6 0.9)))"]
    );
}

/// Simpler reproducer: a rule whose RHS embeds one rule-side variable
/// whose bound value contains a query-side variable that must resolve
/// through the SAME bindings frame transitively.
#[test]
fn rule_rhs_var_contains_query_var_substitutes() {
    let r = run_one(
        r#"
        (= (f ($X $Y)) (got $X $Y))
        !(f ((Inheritance $1 toy) $1))
    "#,
    );
    // $X → (Inheritance $1 toy), $Y → $1
    // No bidirectional unify here, so the bindings are simple.
    // The RHS (got $X $Y) → (got (Inheritance $1 toy) $1) — $1 is free.
    assert_eq!(r, vec!["(got (Inheritance $1 toy) $1)"]);
}

/// Two-sentence PLN-style Truth_Revision smoke. Verifies that a free
/// variable on the query side flows through a rule whose body has
/// nontrivial structure (multiple case branches, let* bindings, etc.).
/// Kept small (no PLN.Query, no full inference loop) so it runs in
/// well under 1 second.
#[test]
fn pln_style_repeated_var_through_let() {
    let r = run_one(
        r#"
        (= (extract ($T $V))
           (let* (($name $T) ($val $V))
             (sentence $name $val)))
        !(extract ((color $X) red))
    "#,
    );
    assert_eq!(r, vec!["(sentence (color $X) red)"]);
}
