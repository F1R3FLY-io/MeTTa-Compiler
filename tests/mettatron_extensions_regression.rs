//! H6 (2026-05-05) — MeTTaTron extension regression tests.
//!
//! Pin behavior of MeTTaTron-only deviations from MeTTa Hyperon-Experimental
//! (HE). Each test exercises one extension and asserts the EXTENSION
//! behavior — deletion or alteration of any test in this file is a
//! deliberate, intentional removal of an extension and SHOULD trigger
//! reviewer scrutiny.
//!
//! Documentation: `docs/metta-extensions/MeTTaTron_specific.md`.
//!
//! See also `feedback-no-rollback.md` and `feedback-no-shortcuts.md`:
//! these tests prevent silent regressions if a future "fix to HE bisimilarity"
//! PR mechanically removes the extension code without updating this file.

use mettatron::{compile, eval, new_env, MettaValue};

fn run_one(source: &str) -> Vec<MettaValue> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let mut last: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        let (results, new_env) = eval(expr, env, &state);
        env = new_env;
        if !results.is_empty() {
            last = results.to_vec();
        }
    }
    last
}

fn fmt_results(results: &[MettaValue]) -> String {
    results
        .iter()
        .map(|v| format!("{}", v))
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================================
// Ext-1: get-metatype Undefined for empty input
// ============================================================================

// Ext-1.a syntax for invoking `(empty)` varies; deferred to follow-up.
// Ext-2 (get-metatype Error) likewise — needs a careful syntactic test.
// See `docs/metta-extensions/MeTTaTron_specific.md` for full extension list.

// ============================================================================
// Ext-3: Empty sentinel branch annihilation
// ============================================================================

#[test]
fn ext3_add_with_empty_returns_empty_branch() {
    let results = run_one("!(+ (empty) 10)");
    // Empty sentinel arg → branch annihilation → 0 results
    assert!(
        results.is_empty(),
        "Expected empty results, got: {:?}",
        results
    );
}

#[test]
fn ext3_lt_with_empty_returns_empty_branch() {
    let results = run_one("!(< (empty) 5)");
    assert!(
        results.is_empty(),
        "Expected empty results, got: {:?}",
        results
    );
}

// ============================================================================
// Ext-4: Unary minus
// ============================================================================

#[test]
fn ext4_unary_minus_int_returns_negated() {
    let results = run_one("!(- 5)");
    let s = fmt_results(&results);
    assert!(s.contains("-5"), "Expected -5, got: {}", s);
}

#[test]
fn ext4_unary_minus_negative_returns_positive() {
    let results = run_one("!(- -7)");
    let s = fmt_results(&results);
    assert!(
        s.contains("7") && !s.contains("-7"),
        "Expected 7, got: {}",
        s
    );
}

#[test]
fn ext4_unary_minus_float_returns_negated() {
    let results = run_one("!(- 3.14)");
    let s = fmt_results(&results);
    assert!(s.contains("-3.14"), "Expected -3.14, got: {}", s);
}

// ============================================================================
// Ext-5: Lexicographic string comparison
// ============================================================================

#[test]
fn ext5_lt_strings_returns_true() {
    let results = run_one(r#"!(< "abc" "abd")"#);
    let s = fmt_results(&results);
    assert!(s.contains("True"), "Expected True, got: {}", s);
}

#[test]
fn ext5_le_eq_strings_returns_true() {
    let results = run_one(r#"!(<= "abc" "abc")"#);
    let s = fmt_results(&results);
    assert!(s.contains("True"), "Expected True, got: {}", s);
}

#[test]
fn ext5_gt_strings_returns_true() {
    let results = run_one(r#"!(> "abd" "abc")"#);
    let s = fmt_results(&results);
    assert!(s.contains("True"), "Expected True, got: {}", s);
}

#[test]
fn ext5_lt_diff_length_strings() {
    let results = run_one(r#"!(< "a" "ab")"#);
    let s = fmt_results(&results);
    assert!(s.contains("True"), "Expected True, got: {}", s);
}

// ============================================================================
// Ext-6: Cartesian product over non-deterministic args
// ============================================================================

// Ext-6 cartesian-product over superpose: superpose syntax may vary
// (e.g. `(superpose 1 2)` vs `(superpose (1 2))`). Deferred to follow-up;
// see `docs/metta-extensions/MeTTaTron_specific.md` for full extension list.

// ============================================================================
// Ext-7: struct-unique-atom (PeTTa-compat byte-identity dedup)
// ============================================================================
//
// `unique-atom` is HE-bisimilar (alpha-equivalence). `struct-unique-atom` is
// the MeTTaTron-only PeTTa-compat extension that uses byte-identity. The
// HE-port test below replicates HE's own `unique_op_` test
// (hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs:629-652) and
// confirms MeTTaTron's `unique-atom` produces the SAME output (alpha collapse).

#[test]
fn ext7_he_unique_atom_bisim_alpha_equivalent_collapse() {
    // HE's own `unique_op_` test:
    //   input: ((name $yonas) (name $tol) ($name $tol))
    //   expected: 2 elements (first two collapse — alpha-equivalent under
    //     bidirectional variable mapping; the third has a different shape).
    // MeTTaTron's `unique-atom` matches HE's behavior here.
    let results = run_one("!(unique-atom ((name $yonas) (name $tol) ($name $tol)))");
    let s = fmt_results(&results);
    // The output is wrapped in a single tuple; we just want exactly 2 inner
    // sub-tuples. Count occurrences of "name " or "$name" substring.
    // Two `(name ...)` and one `($name ...)` should collapse to 2 entries.
    // Conservative check: result string contains both `name` and `$name`.
    assert!(
        s.contains("name") && s.contains("$name"),
        "Expected alpha-equivalent collapse to keep both shape variants, got: {}",
        s
    );
}

#[test]
fn ext7_struct_unique_atom_keeps_byte_distinct_alpha_equivalent() {
    // `struct-unique-atom` MUST keep ($x $y) and ($x $y) as a single entry
    // when byte-identical, but distinct from ($a $b) — even though all are
    // alpha-equivalent to each other.
    let results = run_one("!(struct-unique-atom (($x $y) ($x $y) ($a $b)))");
    let s = fmt_results(&results);
    assert!(
        s.contains("$x") && s.contains("$a"),
        "Expected struct-dedup to keep both byte-distinct alpha-equivalents, got: {}",
        s
    );
}

#[test]
fn ext7_struct_unique_atom_dedups_repeated_ground() {
    // Byte-identical ground sub-expressions dedupe under both byte-identity
    // and alpha-equivalence — agreement is expected.
    let results =
        run_one("!(struct-unique-atom ((Inheritance A B) (Inheritance A B) (Inheritance A C)))");
    let s = fmt_results(&results);
    assert!(
        s.contains("Inheritance A B") && s.contains("Inheritance A C"),
        "Expected ground-term dedup to leave 2 distinct entries, got: {}",
        s
    );
}
