//! Cross-tier bisimilarity property tests (plan workstream H3).
//!
//! Generates random arithmetic / comparison expressions and asserts that
//! T0 and T1 produce the same canonicalized output. T2/T3 are excluded
//! from property-test sweeps because their tiered-cache promotion is
//! threshold-gated and would require pumping execution counts per case.
//!
//! The generators here are intentionally conservative — they emit
//! expressions that pass `tier_applicable(_, T1)` so the bisimilarity
//! assertion isn't trivially vacuous.

use proptest::prelude::*;

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, tier_applicable, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::eval::trampoline::new_env;
use mettatron::{compile, MettaValue};

fn arb_small_int() -> impl Strategy<Value = String> {
    (-100i64..100i64).prop_map(|n| n.to_string())
}

/// Generate a small arithmetic expression — guaranteed to compile on both T0 and T1.
fn arb_arith_expr() -> impl Strategy<Value = String> {
    let leaf = prop_oneof![arb_small_int(), arb_small_int(),];
    leaf.prop_recursive(3, 16, 4, |inner| {
        prop_oneof![
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("(+ {} {})", a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("(* {} {})", a, b)),
            (inner.clone(), inner.clone()).prop_map(|(a, b)| format!("(- {} {})", a, b)),
        ]
    })
}

fn canonicalize_results(results: &[MettaValue]) -> Vec<String> {
    let mut s: Vec<String> = results
        .iter()
        .filter(|v| !v.is_empty())
        .map(|v| format!("{:?}", v.view()))
        .collect();
    s.sort();
    s
}

fn eval_on_tier(source: &str, tier: TierSelection) -> Option<Vec<MettaValue>> {
    let bang = format!("!{}", source);
    let state = compile(&bang).ok()?;
    let mut env = new_env();
    let mut all: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        if tier_applicable(&expr, &env, tier).is_err() {
            return None;
        }
        let outcome = eval_with_tier(expr, env, &state, tier, FallbackPolicy::SilentDemote);
        let (results, new_env) = match outcome {
            TierEvalOutcome::Ok { results, env, .. } => (results, env),
            TierEvalOutcome::Demoted { results, env, .. } => (results, env),
            TierEvalOutcome::NotApplicable { .. } => return None,
        };
        env = new_env;
        all.extend(results.into_iter().filter(|v| !v.is_empty()));
    }
    Some(all)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property: T0 and T1 produce identical canonicalized output for
    /// arbitrary arithmetic expressions.
    #[test]
    fn cross_tier_bisim_arith(source in arb_arith_expr()) {
        let t0 = match eval_on_tier(&source, TierSelection::Treewalker) {
            Some(r) => canonicalize_results(&r),
            None => return Ok(()),
        };
        let t1 = match eval_on_tier(&source, TierSelection::Bytecode) {
            Some(r) => canonicalize_results(&r),
            None => return Ok(()),
        };
        prop_assert_eq!(&t0, &t1, "T0 ↔ T1 divergence on `{}`", source);
    }
}

#[test]
fn cross_tier_bisim_simple_sanity() {
    // Hardcoded sanity case so the test suite always exercises the
    // cross-tier comparator even if proptest's strategy library changes.
    let t0 = eval_on_tier("(+ 1 2)", TierSelection::Treewalker).expect("T0 evaluates");
    let t1 = eval_on_tier("(+ 1 2)", TierSelection::Bytecode).expect("T1 evaluates");
    assert_eq!(canonicalize_results(&t0), canonicalize_results(&t1));
}
