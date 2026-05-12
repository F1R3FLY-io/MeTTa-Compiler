//! Z.A.6 regression: HE-compat primitives in MeTTaTron.
//!
//! Covers:
//! - `PI` and `EXP` constants registered in the default environment.
//! - `=alpha` for alpha-equivalence (was already present; this is the
//!   anchor that it's correct against HE).
//! - `git-module!` as HE-canonical alias of `git-import!`.

use mettatron::{compile, eval, new_env};

fn run_one(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    let exprs: Vec<_> = state.source_snapshot();
    let mut env = env;
    let mut last = Vec::new();
    for expr in &exprs {
        let (results, new_env) = eval(*expr, env, &state);
        env = new_env;
        last = results.into_iter().collect();
    }
    last.iter().map(|v| format!("{}", v)).collect()
}

#[test]
fn pi_constant_resolves_to_f64_pi() {
    let r = run_one("!PI");
    assert_eq!(r.len(), 1);
    let pi: f64 = r[0].parse().expect("PI should parse as f64");
    assert!(
        (pi - std::f64::consts::PI).abs() < 1e-12,
        "PI should be std::f64::consts::PI, got {}",
        pi
    );
}

#[test]
fn exp_constant_resolves_to_f64_e() {
    let r = run_one("!EXP");
    assert_eq!(r.len(), 1);
    let e: f64 = r[0].parse().expect("EXP should parse as f64");
    assert!(
        (e - std::f64::consts::E).abs() < 1e-12,
        "EXP should be std::f64::consts::E, got {}",
        e
    );
}

#[test]
fn pi_usable_in_arithmetic() {
    let r = run_one("!(* 2.0 PI)");
    assert_eq!(r.len(), 1);
    let tau: f64 = r[0].parse().expect("2*PI should parse as f64");
    assert!(
        (tau - 2.0 * std::f64::consts::PI).abs() < 1e-12,
        "2*PI mismatch: {}",
        tau
    );
}

#[test]
fn alpha_eq_true_for_consistent_rename() {
    let r = run_one("!(=alpha ($x $y) ($a $b))");
    assert_eq!(r, vec!["True"]);
}

#[test]
fn alpha_eq_false_for_inconsistent_rename() {
    let r = run_one("!(=alpha ($x $x) ($a $b))");
    assert_eq!(r, vec!["False"]);
}

#[test]
fn alpha_eq_false_for_different_atoms() {
    let r = run_one("!(=alpha (foo $x) (bar $x))");
    assert_eq!(r, vec!["False"]);
}

#[test]
fn alpha_eq_true_for_same_atom() {
    let r = run_one("!(=alpha (foo $x) (foo $y))");
    assert_eq!(r, vec!["True"]);
}

#[test]
fn git_module_is_alias_of_git_import() {
    // Just verify the form is recognized — actual cloning would need
    // network access. The handler emits an Error atom on git failure,
    // which is the expected behaviour for a malformed URL too.
    let r = run_one(r#"!(git-module! "not-a-real-git-uri")"#);
    assert_eq!(r.len(), 1);
    // Either Unit (success — unlikely) or Error (expected for bad URL).
    assert!(
        r[0].starts_with("()") || r[0].starts_with("(Error"),
        "git-module! should return Unit or Error, got: {}",
        r[0]
    );
}

#[test]
fn print_alternatives_is_alias_of_println() {
    // Single-result variant: prints "5" to stdout, returns Unit.
    let r = run_one(r#"!(print-alternatives! 5)"#);
    assert_eq!(r, vec!["()"]);
}

#[test]
fn print_alternatives_with_deterministic_arithmetic_arg() {
    // Single-result computed arg: prints "3" to stdout, returns Unit.
    let r = run_one(r#"!(print-alternatives! (+ 1 2))"#);
    assert_eq!(r, vec!["()"]);
}

#[test]
fn capture_evaluates_argument_like_eval() {
    let r = run_one(r#"!(capture (+ 1 2))"#);
    assert_eq!(r, vec!["3"]);
}

#[test]
fn capture_preserves_nondet_results() {
    let r = run_one(r#"!(capture (superpose (1 2 3)))"#);
    let mut sorted = r.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["1", "2", "3"]);
}

#[test]
fn register_module_routes_via_include() {
    // register-module! is HE's path-based module loader; MeTTaTron aliases
    // to `include` for shared loading semantics. Nonexistent file returns
    // an Error atom (matches HE's load failure behaviour).
    let r = run_one(r#"!(register-module! "this-file-does-not-exist.metta")"#);
    assert_eq!(r.len(), 1);
    assert!(
        r[0].starts_with("(Error"),
        "register-module! on nonexistent file should return Error atom, got: {}",
        r[0]
    );
}
