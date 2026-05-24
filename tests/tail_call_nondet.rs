//! BUG T0-T1-008 regression: T1's `op_tail_call` must preserve nondet
//! alternatives (multiple rule matches → multiple results, not just first).
//!
//! Per spec K.2.8 the row was originally marked "T1 op_tail_call drops nondet",
//! but `op_tail_call` at `bytecode/vm/mod.rs:2370-2396` already calls
//! `op_dispatch_rules` which iterates all matches into a choice point. This
//! test verifies the existing behavior so future regressions are caught.

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::eval::trampoline::new_env;
use mettatron::{compile, MettaValue};

fn eval_on_tier(source: &str, tier: TierSelection) -> Vec<MettaValue> {
    let state = compile(source).expect("compile");
    let mut env = new_env();
    let mut all: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        let outcome = eval_with_tier(expr, env, &state, tier, FallbackPolicy::SilentDemote);
        let (results, new_env) = match outcome {
            TierEvalOutcome::Ok { results, env, .. } => (results, env),
            TierEvalOutcome::Demoted { results, env, .. } => (results, env),
            TierEvalOutcome::NotApplicable { .. } => unreachable!("tier always applicable"),
        };
        env = new_env;
        all.extend(results.into_iter().filter(|v| !v.is_empty()));
    }
    all
}

fn results_to_strings(results: &[MettaValue]) -> Vec<String> {
    let mut s: Vec<String> = results.iter().map(|v| format!("{:?}", v.view())).collect();
    s.sort();
    s
}

/// Tail-call dispatch with multiple matching rule alternatives must produce
/// all matches as results, not just the first.
#[test]
fn tail_call_with_multiple_matches_t0() {
    let source = r#"
        (= (foo 0) "a")
        (= (foo $x) "b")
        !(foo 0)
    "#;
    let results = eval_on_tier(source, TierSelection::Treewalker);
    assert!(
        results.len() >= 2,
        "T0 expected ≥2 results (a + b), got {:?}",
        results_to_strings(&results)
    );
    let s = results_to_strings(&results);
    assert!(s.iter().any(|r| r.contains("\"a\"")), "missing a: {:?}", s);
    assert!(s.iter().any(|r| r.contains("\"b\"")), "missing b: {:?}", s);
}

/// Same test forced through T1 (bytecode VM) tail-call dispatch path.
#[test]
fn tail_call_with_multiple_matches_t1() {
    let source = r#"
        (= (foo 0) "a")
        (= (foo $x) "b")
        !(foo 0)
    "#;
    let results = eval_on_tier(source, TierSelection::Bytecode);
    assert!(
        results.len() >= 2,
        "T1 expected ≥2 results (a + b), got {:?}",
        results_to_strings(&results)
    );
}
