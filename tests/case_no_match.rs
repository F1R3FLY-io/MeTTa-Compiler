//! A.5 investigation: does `(case <unmatched> ...)` return 0 results
//! (matching MeTTa HE) or 1 Unit result (a MeTTaTron quirk)?
//!
//! This test was written to verify the root-cause of a CLI display
//! divergence: MeTTa HE prints `[]` for case no-match while MeTTaTron
//! prints `[()]`. The question is whether MeTTaTron's trampoline really
//! returns a `Vec[Unit]` (which would re-pollute PLN's deriver task
//! queue) or whether the issue is purely cosmetic at the CLI display
//! layer.
//!
//! See plan: `/Users/dylon/.claude/plans/greedy-squishing-starfish.md`
//! Phase A.5.

use mettatron::{compile, eval, new_env};

fn run_one(source: &str) -> Vec<mettatron::backend::models::MettaValue> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    let exprs = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    // Evaluate every expression in order; return the result of the last.
    let mut env = env;
    let mut last_results = Vec::new();
    for expr in &exprs {
        let (results, new_env, ..) = eval(*expr, env, &state);
        env = new_env;
        last_results = results.into_iter().collect();
    }
    last_results
}

#[test]
fn case_no_match_directly_via_eval() {
    // No `!` wrapper — evaluate the case expression directly via the
    // top-level eval loop. If the bang-eval CLI display layer adds the Unit,
    // this test would still see 0 results.
    let results = run_one("(case 42 ((($x $y) (sentence $x $y))))");
    eprintln!("[direct-eval] results.len() = {}", results.len());
    for (i, r) in results.iter().enumerate() {
        eprintln!("[direct-eval] [{}] = {:?}", i, r);
    }
}

#[test]
fn case_no_match_on_literal_returns_empty_results_not_unit() {
    // (case 42 ((($x $y) (sentence $x $y))))
    // 42 is a Long literal; it does not match the s-expr pattern ($x $y).
    // MeTTa HE returns [] (zero results). MeTTaTron should match.
    let results = run_one("!(case 42 ((($x $y) (sentence $x $y))))");
    assert_eq!(
        results.len(),
        0,
        "case no-match should return 0 results, got {} results: {:?}",
        results.len(),
        results.iter().map(|v| format!("{}", v)).collect::<Vec<_>>()
    );
}

#[test]
fn case_no_match_on_atom_returns_empty_results_not_unit() {
    // (case some-atom ((($x $y) (sentence $x $y))))
    // some-atom is an atom; pattern ($x $y) matches s-exprs only.
    let results = run_one("!(case some-atom ((($x $y) (sentence $x $y))))");
    assert_eq!(
        results.len(),
        0,
        "case no-match on atom should return 0 results, got {} results: {:?}",
        results.len(),
        results.iter().map(|v| format!("{}", v)).collect::<Vec<_>>()
    );
}

#[test]
fn case_match_succeeds_returns_one_result() {
    // Control: a case that DOES match should return exactly 1 result.
    let results = run_one("!(case (a b) ((($x $y) (sentence $x $y))))");
    assert_eq!(results.len(), 1);
    assert_eq!(format!("{}", results[0]), "(sentence a b)");
}

#[test]
fn case_via_catch_all_to_empty_returns_no_results() {
    // The exact PLN-deriver pattern: a function with both a specific clause
    // and a catch-all (-> Empty), invoked on an arg that misses the
    // specific clause. The catch-all produces Empty, the case filters Empty,
    // leaving 0 results.
    let results = run_one(
        r#"
        (= (foo bar) ((Inheritance bar baz) (stv 1.0 1.0)))
        (= (foo $_x) Empty)
        !(case (foo other) ((($x $y) (sentence $x $y))))
        "#,
    );
    assert_eq!(
        results.len(),
        0,
        "case on a function with only Empty result should return 0 results, got {} results: {:?}",
        results.len(),
        results.iter().map(|v| format!("{}", v)).collect::<Vec<_>>()
    );
}
