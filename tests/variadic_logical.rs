//! Variadic semantics for the `(and ...)` and `(or ...)` grounded ops,
//! introduced by Z.A.1 (2026-05-12) to align with HE's empirical behaviour.
//!
//! Anchors: `src/backend/grounded/logical.rs::AndOp`/`OrOp` and
//! `variadic_logical_step`. Promotes `conformance/M09e-stdlib-logical/{002,003}`.

use mettatron::{compile, eval, new_env};

fn run_one(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    let exprs: Vec<_> = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    let mut env = env;
    let mut last = Vec::new();
    for expr in &exprs {
        let (results, new_env, ..) = eval(*expr, env, &state);
        env = new_env;
        last = results.into_iter().collect();
    }
    last.iter().map(|v| format!("{}", v)).collect()
}

#[test]
fn and_three_args_short_circuits_on_false() {
    assert_eq!(run_one("!(and True True False)"), vec!["false"]);
}

#[test]
fn or_three_args_short_circuits_on_true() {
    assert_eq!(run_one("!(or False False True)"), vec!["true"]);
}

#[test]
fn and_empty_returns_identity_true() {
    assert_eq!(run_one("!(and)"), vec!["true"]);
}

#[test]
fn or_empty_returns_identity_false() {
    assert_eq!(run_one("!(or)"), vec!["false"]);
}

#[test]
fn and_single_arg_returns_arg() {
    assert_eq!(run_one("!(and True)"), vec!["true"]);
    assert_eq!(run_one("!(and False)"), vec!["false"]);
}

#[test]
fn or_single_arg_returns_arg() {
    assert_eq!(run_one("!(or True)"), vec!["true"]);
    assert_eq!(run_one("!(or False)"), vec!["false"]);
}

#[test]
fn and_all_true_returns_true() {
    assert_eq!(run_one("!(and True True True True)"), vec!["true"]);
}

#[test]
fn or_all_false_returns_false() {
    assert_eq!(run_one("!(or False False False False)"), vec!["false"]);
}

#[test]
fn and_with_nondet_operand_carries_cartesian_product() {
    // `(superpose (True False))` produces two operand results.
    // (and (superpose (True False)) True) → cartesian {T∧T, F∧T} = {True, False}.
    let r = run_one("!(and (superpose (True False)) True)");
    assert_eq!(r.len(), 2);
    let mut sorted = r.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["false", "true"]);
}

#[test]
fn or_with_nondet_operand_carries_cartesian_product() {
    let r = run_one("!(or (superpose (True False)) False)");
    assert_eq!(r.len(), 2);
    let mut sorted = r.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["false", "true"]);
}

#[test]
fn and_non_bool_arg_returns_runtime_error() {
    let r = run_one("!(and True 5)");
    assert_eq!(r.len(), 1);
    assert!(
        r[0].starts_with("(Error"),
        "expected Error atom, got: {}",
        r[0]
    );
}

#[test]
fn or_non_bool_arg_returns_runtime_error() {
    let r = run_one("!(or False \"hi\")");
    assert_eq!(r.len(), 1);
    assert!(
        r[0].starts_with("(Error"),
        "expected Error atom, got: {}",
        r[0]
    );
}

#[test]
fn nested_variadic_logical_evaluates() {
    assert_eq!(
        run_one("!(and (or False True) (and True True True))"),
        vec!["true"]
    );
    assert_eq!(
        run_one("!(or (and True False) (and False False))"),
        vec!["false"]
    );
}
