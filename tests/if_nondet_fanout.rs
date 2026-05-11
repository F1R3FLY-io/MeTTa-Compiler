//! BUG-T0-009 regression tests: `if` must fan out across multiple nondet
//! condition results, evaluating `then` for each True and `else` for each
//! False alternative.
//!
//! These tests exercise the T0 tree-walker tier specifically. The bytecode
//! VM (T1) and JIT (T2/T3) will get independent fan-out fixes per their own
//! tier-local architecture (plan workstreams T1.A and T2/T3.B).

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::eval::trampoline::new_env;
use mettatron::{compile, MettaValue};

fn eval_t0(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compile");
    let mut env = new_env();
    let mut all: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        let outcome =
            eval_with_tier(expr, env, &state, TierSelection::Treewalker, FallbackPolicy::SilentDemote);
        let (results, new_env) = match outcome {
            TierEvalOutcome::Ok { results, env, .. } => (results, env),
            TierEvalOutcome::Demoted { results, env, .. } => (results, env),
            TierEvalOutcome::NotApplicable { reason } => {
                panic!("tier T0 should always be applicable; got {:?}", reason)
            }
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

#[test]
fn if_single_result_true_returns_then() {
    let results = eval_t0("!(if True yes no)");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].as_atom(), Some("yes"));
}

#[test]
fn if_single_result_false_returns_else() {
    let results = eval_t0("!(if False yes no)");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].as_atom(), Some("no"));
}

/// Core BUG-T0-009 case: multi-result condition fans out across alternatives.
#[test]
fn if_multi_result_condition_fans_out() {
    let results = eval_t0("!(if (superpose (True False)) yes no)");
    assert_eq!(results.len(), 2, "expected fan-out into 2 results; got {:?}", results);
    let s = results_to_strings(&results);
    assert!(s.iter().any(|r| r.contains("yes")), "missing yes: {:?}", s);
    assert!(s.iter().any(|r| r.contains("no")), "missing no: {:?}", s);
}

#[test]
fn if_three_alt_condition_fans_out() {
    let results = eval_t0("!(if (superpose (True True False)) yes no)");
    assert_eq!(results.len(), 3, "expected 3 results, got {:?}", results);
    let s = results_to_strings(&results);
    let yes_count = s.iter().filter(|r| r.contains("yes")).count();
    let no_count = s.iter().filter(|r| r.contains("no")).count();
    assert_eq!(yes_count, 2, "expected 2 yes, got {:?}", s);
    assert_eq!(no_count, 1, "expected 1 no, got {:?}", s);
}

/// Empty superpose ⇒ zero results (branch annihilation preserved).
#[test]
fn if_empty_superpose_returns_empty() {
    let results = eval_t0("!(if (superpose ()) yes no)");
    assert!(
        results.is_empty(),
        "expected zero results from empty superpose; got {:?}",
        results
    );
}
