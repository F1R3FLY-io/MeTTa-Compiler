//! Conformance tests for MTT-HE-022 (`is-error`) and MTT-HE-023 (`catch`)
//! per the new spec rows in `mettatron-specification/spec/I-divergences.md §I.1`
//! and fixture files in `conformance/M11-bisimilarity-he/030-*`, `031-*`.
//!
//! These tests assert MTT-only primitive behavior. The fixtures in the
//! spec repo document HE-equivalent observation (via `if-error` shim);
//! these tests pin the MTT primitive behavior cross-tier.

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
            TierEvalOutcome::NotApplicable { .. } => unreachable!("T0 always applicable"),
        };
        env = new_env;
        all.extend(results.into_iter().filter(|v| !v.is_empty()));
    }
    all
}

#[test]
fn mtt_he_022_is_error_with_error() {
    let r = eval_t0("!(is-error (Error msg detail))");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].as_bool(), Some(true));
}

#[test]
fn mtt_he_022_is_error_with_non_error() {
    let r = eval_t0("!(is-error 42)");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].as_bool(), Some(false));
}

#[test]
fn mtt_he_023_catch_with_error_returns_fallback() {
    let r = eval_t0("!(catch (/ 1 0) \"fallback\")");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].as_string(), Some("fallback"));
}

#[test]
fn mtt_he_023_catch_with_success_returns_value() {
    let r = eval_t0("!(catch 42 \"fallback\")");
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].as_long(), Some(42));
}
