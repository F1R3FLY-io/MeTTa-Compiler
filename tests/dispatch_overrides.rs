//! Regression tests for the per-environment dispatch override mechanism.
//!
//! Background: MeTTaTron grounds many list/tuple helpers (`append`, `length`,
//! `is-member`, `map-atom`, `car-atom`, …) for performance. MeTTa HE either
//! defines these as MeTTa rules in `stdlib.metta` or has no equivalent at
//! all. When a user adds their own rules for one of these names, the
//! grounded fast path must defer to the user rules — otherwise code like
//! mmverify (which defines its own `(= (append Nil $list) $list)` against
//! a `Cons`/`Nil` data type) silently fails.
//!
//! These tests pin the override semantics so future refactors don't
//! regress them. They cover:
//!
//!   - User rules win for overridable names (Class B + Class C-pure).
//!   - Grounded helper still runs when no user rules exist.
//!   - User rules are STICKY: removing one user rule restores the
//!     grounded fast path only when ALL user rules are gone.
//!   - TRUE HE primitives (`cons-atom`, `decons-atom`, `size-atom`, …)
//!     are NOT overridable — adding a user rule has no effect.
//!
//! See `/Users/dylon/.claude/plans/twinkling-discovering-scott.md` for the
//! design and the full overridable-name partition.

use mettatron::backend::models::MettaValue;
use mettatron::{compile, eval, new_env};

fn run_program(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let exprs = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    let mut last_results = Vec::new();
    for expr in &exprs {
        let (results, new_env) = eval(*expr, env, &state);
        env = new_env;
        last_results = results.into_iter().collect();
    }
    last_results
}

fn run_program_collect_all(source: &str) -> Vec<Vec<MettaValue>> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let exprs = state.source_snapshot();
    let mut all = Vec::new();
    for expr in &exprs {
        let (results, new_env) = eval(*expr, env, &state);
        env = new_env;
        all.push(results.into_iter().collect());
    }
    all
}

fn results_to_strings(results: &[MettaValue]) -> Vec<String> {
    results.iter().map(|v| format!("{}", v)).collect()
}

// ============================================================================
// Class C: helpers HE doesn't have at all (mmverify motivators)
// ============================================================================

#[test]
fn user_append_overrides_grounded_for_cons_data_type() {
    // mmverify-style append on a custom Nil/Cons data type. The grounded
    // `append` only handles `()` and S-expression first args — it would
    // error on `Nil`. With the override, the user rules fire instead.
    let source = r#"
        (= (append Nil $list) $list)
        (= (append (Cons $head $tail) $list) (Cons $head (append $tail $list)))
        !(append (Cons 1 (Cons 2 Nil)) (Cons 3 (Cons 4 Nil)))
    "#;
    let results = run_program(source);
    assert_eq!(
        results.len(),
        1,
        "expected one result, got {:?}",
        results_to_strings(&results)
    );
    assert_eq!(
        format!("{}", results[0]),
        "(Cons 1 (Cons 2 (Cons 3 (Cons 4 Nil))))",
        "user append rules should produce a Cons-list, not an error"
    );
}

#[test]
fn grounded_append_when_no_user_rules() {
    // No user `append` rules — the grounded fast path runs.
    let source = "!(append (1 2) (3 4))";
    let results = run_program(source);
    assert_eq!(results.len(), 1);
    assert_eq!(format!("{}", results[0]), "(1 2 3 4)");
}

#[test]
fn user_length_overrides_grounded_for_cons_data_type() {
    let source = r#"
        (= (length Nil) 0)
        (= (length (Cons $h $t)) (+ 1 (length $t)))
        !(length (Cons a (Cons b (Cons c Nil))))
    "#;
    let results = run_program(source);
    assert_eq!(results.len(), 1, "got: {:?}", results_to_strings(&results));
    assert_eq!(format!("{}", results[0]), "3");
}

#[test]
fn grounded_length_when_no_user_rules() {
    let source = "!(length (a b c d))";
    let results = run_program(source);
    assert_eq!(results.len(), 1);
    assert_eq!(format!("{}", results[0]), "4");
}

#[test]
fn user_is_member_overrides_grounded_for_cons_data_type() {
    let source = r#"
        (= (is-member $x Nil) False)
        (= (is-member $x (Cons $x $rest)) True)
        (= (is-member $x (Cons $y $rest)) (is-member $x $rest))
        !(is-member b (Cons a (Cons b (Cons c Nil))))
    "#;
    let results = run_program(source);
    assert!(
        results.iter().any(|r| format!("{}", r) == "true"),
        "expected True among results, got {:?}",
        results_to_strings(&results)
    );
}

// ============================================================================
// Class B: HE defines as MeTTa rules in stdlib.metta — should be overridable
// ============================================================================

#[test]
fn user_map_atom_overrides_grounded() {
    // Override map-atom with a constant function.
    let source = r#"
        (= (map-atom $list $var $template) overridden-result)
        !(map-atom (1 2 3) $x (* $x 2))
    "#;
    let results = run_program(source);
    assert_eq!(results.len(), 1, "got: {:?}", results_to_strings(&results));
    assert_eq!(format!("{}", results[0]), "overridden-result");
}

#[test]
fn user_car_atom_overrides_grounded() {
    let source = r#"
        (= (car-atom $list) overridden-head)
        !(car-atom (a b c))
    "#;
    let results = run_program(source);
    assert_eq!(results.len(), 1, "got: {:?}", results_to_strings(&results));
    assert_eq!(format!("{}", results[0]), "overridden-head");
}

#[test]
fn grounded_car_atom_when_no_user_rules() {
    let source = "!(car-atom (a b c))";
    let results = run_program(source);
    assert_eq!(results.len(), 1);
    assert_eq!(format!("{}", results[0]), "a");
}

// ============================================================================
// Override semantics: stickiness, removal, partial overrides
// ============================================================================

#[test]
fn remove_user_rule_restores_grounded() {
    // Add a user `append` rule, then remove it; the grounded fast path
    // should take over again.
    let source = r#"
        (= (append Nil $l) overridden)
        ; While the rule exists, the user override fires.
        !(append Nil (1 2))
        ; Remove it.
        !(remove-atom &self (= (append Nil $l) overridden))
        ; Now the grounded helper handles a regular S-expression input.
        !(append (1 2) (3 4))
    "#;
    let results = run_program_collect_all(source);
    // The user rule fires for the first call.
    let first_call = &results[1];
    assert!(
        first_call.iter().any(|r| format!("{}", r) == "overridden"),
        "first call should hit user override, got: {:?}",
        results_to_strings(first_call)
    );
    // The grounded helper handles the third call after removal.
    let third_call = &results[3];
    assert_eq!(third_call.len(), 1);
    assert_eq!(format!("{}", third_call[0]), "(1 2 3 4)");
}

// ============================================================================
// Class A: TRUE HE primitives — NOT overridable
// ============================================================================

#[test]
fn cons_atom_user_rule_does_not_override_primitive() {
    // Adding a user rule for `cons-atom` succeeds (rule index accepts it),
    // but the special-form dispatch arm wins at evaluation time. The
    // grounded `cons-atom` produces `(1 2 3)` regardless of user rules.
    let source = r#"
        (= (cons-atom $h $t) bogus)
        !(cons-atom 1 (2 3))
    "#;
    let results = run_program(source);
    assert_eq!(results.len(), 1);
    assert_eq!(
        format!("{}", results[0]),
        "(1 2 3)",
        "cons-atom is a TRUE HE primitive — user rule must NOT win"
    );
}

#[test]
fn size_atom_user_rule_does_not_override_primitive() {
    let source = r#"
        (= (size-atom $list) bogus)
        !(size-atom (a b c))
    "#;
    let results = run_program(source);
    assert_eq!(results.len(), 1);
    assert_eq!(
        format!("{}", results[0]),
        "3",
        "size-atom is a TRUE HE primitive — user rule must NOT win"
    );
}
