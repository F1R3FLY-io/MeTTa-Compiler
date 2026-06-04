//! Integration tests for the PeTTa-compatible helper built-ins added in
//! `feature/pln-support`:
//!
//! - `is-member`, `append`, `length`, `exclude-item` (B5 — aliases of
//!   existing operators with possible arg-order swaps)
//! - `msort` (B5 — new numeric ascending sort)
//! - `progn` (B5 — sequential evaluation, returns last value)
//! - `reduce` (B9 — passthrough; identity in applicative-order eval)
//! - `cut` (B9 — no-op returning Unit)
//! - `foldl-atom` 3-arg form (B9 — left fold)
//! - `unique-atom` (B10 — STRUCTURAL equality, PeTTa-compatible)
//! - `alpha-unique-atom` (B10 — alpha-equivalence, MeTTa HE-compatible)
//!
//! Plus regression tests for the bidirectional-unification + transitive
//! environment-trimming fix that landed alongside these helpers.

use mettatron::{compile, eval, new_env};

/// Run a MeTTa source program and return the result list of the LAST
/// top-level expression as `Display`-formatted strings.
///
/// All preceding expressions (rule definitions, KB inserts, etc.) are
/// evaluated first so they take effect before the last `!(...)` runs.
///
/// Filters Empty sentinels so the test sees the same multiset the
/// directive-level (`!`) output emits — `main.rs:743` applies the same
/// filter before printing.
fn run_one(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    // Snapshot the source to release the mutex before calling eval()
    // (eval() may take the GC lock, which would deadlock against the
    // source mutex if held across the call).
    let exprs: Vec<_> = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    // Evaluate every expression in order; return the result of the last.
    let mut env = env;
    let mut last_results: Vec<mettatron::MettaValue> = Vec::new();
    for expr in &exprs {
        let (results, new_env, ..) = eval(*expr, env, &state);
        env = new_env;
        last_results = results.into_iter().collect();
    }
    last_results
        .iter()
        .filter(|v| !v.is_empty_sentinel())
        .map(|v| format!("{}", v))
        .collect()
}

// =============================================================================
// B5: is-member
// =============================================================================

#[test]
fn is_member_found() {
    let r = run_one("!(is-member b (a b c))");
    assert_eq!(r, vec!["true"]);
}

#[test]
fn is_member_not_found() {
    let r = run_one("!(is-member z (a b c))");
    assert_eq!(r, vec!["false"]);
}

#[test]
fn is_member_empty_tuple() {
    let r = run_one("!(is-member a ())");
    assert_eq!(r, vec!["false"]);
}

// =============================================================================
// B5: append
// =============================================================================

#[test]
fn append_two_nonempty() {
    let r = run_one("!(append (1 2) (3 4))");
    assert_eq!(r, vec!["(1 2 3 4)"]);
}

#[test]
fn append_left_empty() {
    let r = run_one("!(append () (3 4))");
    assert_eq!(r, vec!["(3 4)"]);
}

#[test]
fn append_right_empty() {
    let r = run_one("!(append (1 2) ())");
    assert_eq!(r, vec!["(1 2)"]);
}

// =============================================================================
// B5: length
// =============================================================================

#[test]
fn length_three() {
    let r = run_one("!(length (a b c))");
    assert_eq!(r, vec!["3"]);
}

#[test]
fn length_empty() {
    let r = run_one("!(length ())");
    assert_eq!(r, vec!["0"]);
}

// =============================================================================
// B5: exclude-item (PeTTa: elem first, tuple second; reverse of `without`)
// =============================================================================

#[test]
fn exclude_item_present() {
    let r = run_one("!(exclude-item b (a b c b d))");
    assert_eq!(r, vec!["(a c d)"]);
}

#[test]
fn exclude_item_absent() {
    let r = run_one("!(exclude-item z (a b c))");
    assert_eq!(r, vec!["(a b c)"]);
}

// =============================================================================
// B5: msort
// =============================================================================

#[test]
fn msort_basic() {
    let r = run_one("!(msort (3 1 4 1 5 9 2 6))");
    assert_eq!(r, vec!["(1 1 2 3 4 5 6 9)"]);
}

#[test]
fn msort_empty() {
    let r = run_one("!(msort ())");
    assert_eq!(r, vec!["()"]);
}

#[test]
fn msort_already_sorted() {
    let r = run_one("!(msort (1 2 3))");
    assert_eq!(r, vec!["(1 2 3)"]);
}

#[test]
fn msort_floats() {
    // Spec §02 canonical float form (commit 8fd3a9a): whole-number floats
    // emit `.0` so `parse(format(v))` round-trips to `Float(v)`, not
    // `Long(v as i64)`. Pre-spec-alignment, this expected `(0.5 1.5 2)`.
    let r = run_one("!(msort (1.5 2.0 0.5))");
    assert_eq!(r, vec!["(0.5 1.5 2.0)"]);
}

// =============================================================================
// B5: progn
// =============================================================================

#[test]
fn progn_returns_last() {
    let r = run_one("!(progn 1 2 3)");
    assert_eq!(r, vec!["3"]);
}

#[test]
fn progn_single() {
    let r = run_one("!(progn 42)");
    assert_eq!(r, vec!["42"]);
}

// =============================================================================
// B9: reduce
// =============================================================================

#[test]
fn reduce_passthrough_atom() {
    let r = run_one("!(reduce x)");
    assert_eq!(r, vec!["x"]);
}

#[test]
fn reduce_passthrough_arithmetic() {
    // (+ 1 2) gets pre-evaluated to 3 by applicative-order, then reduce is
    // an identity passthrough.
    let r = run_one("!(reduce (+ 1 2))");
    assert_eq!(r, vec!["3"]);
}

// =============================================================================
// B9: cut
// =============================================================================

#[test]
fn cut_returns_unit() {
    let r = run_one("!(cut)");
    assert_eq!(r, vec!["()"]);
}

// =============================================================================
// `unique-atom` — alpha-equivalence dedup (matches MeTTa HE)
// =============================================================================
//
// MeTTaTron's `unique-atom` uses alpha-equivalence dedup, matching MeTTa HE's
// `UniqueAtomOp`. `alpha-unique-atom` is an explicit alias with identical
// semantics. For PeTTa-style structural dedup, see `struct-unique-atom`.

#[test]
fn unique_atom_dedups_ground() {
    let r = run_one("!(unique-atom (a b a c b))");
    assert_eq!(r, vec!["(a b c)"]);
}

#[test]
fn unique_atom_collapses_alpha_equivalent_vars() {
    // ALPHA-EQUIVALENCE (HE-faithful): $x and $y are both single free
    // variables — alpha-equivalent — so they collapse to a single
    // representative. Compare with struct-unique-atom below which keeps
    // them distinct because their names differ structurally.
    let r = run_one("!(unique-atom ($x $y))");
    assert_eq!(r, vec!["($x)"]);
}

#[test]
fn unique_atom_dedups_repeated_var() {
    // Three occurrences: $x, $x, $y. All three are alpha-equivalent (each
    // is a single free variable), so the result is a single representative.
    let r = run_one("!(unique-atom ($x $x $y))");
    assert_eq!(r, vec!["($x)"]);
}

// =============================================================================
// `alpha-unique-atom` — explicit alias of `unique-atom`
// =============================================================================

#[test]
fn alpha_unique_atom_dedups_ground() {
    let r = run_one("!(alpha-unique-atom (a b a c b))");
    assert_eq!(r, vec!["(a b c)"]);
}

#[test]
fn alpha_unique_atom_collapses_renamed_vars() {
    // ALPHA: $x and $y are alpha-equivalent (both single free vars)
    // → only one kept.
    let r = run_one("!(alpha-unique-atom ($x $y))");
    assert_eq!(r, vec!["($x)"]);
}

// =============================================================================
// `struct-unique-atom` — PeTTa-compatible structural-equality dedup
// =============================================================================

#[test]
fn struct_unique_atom_dedups_ground() {
    // Ground inputs: structural and alpha-equivalence agree.
    let r = run_one("!(struct-unique-atom (a b a c b))");
    assert_eq!(r, vec!["(a b c)"]);
}

#[test]
fn struct_unique_atom_keeps_distinct_named_vars() {
    // STRUCTURAL: $x and $y are byte-distinct, so both are kept.
    // Compare with `unique-atom` (alpha-equivalence) which collapses
    // them to a single representative.
    let r = run_one("!(struct-unique-atom ($x $y))");
    assert_eq!(r, vec!["($x $y)"]);
}

#[test]
fn struct_unique_atom_dedups_byte_identical_vars() {
    // Byte-identical occurrences of $x are deduped; $y is kept.
    let r = run_one("!(struct-unique-atom ($x $x $y))");
    assert_eq!(r, vec!["($x $y)"]);
}

// =============================================================================
// Divergence between unique-atom (alpha) and struct-unique-atom (structural)
// =============================================================================

#[test]
fn unique_atom_vs_struct_unique_atom_diverge_on_distinct_vars() {
    // The two functions MUST diverge on inputs containing distinct free
    // variables. unique-atom (alpha) collapses them; struct-unique-atom
    // (structural) keeps them.
    let alpha = run_one("!(unique-atom ($x $y))");
    let struct_ = run_one("!(struct-unique-atom ($x $y))");
    assert_ne!(
        alpha, struct_,
        "unique-atom (alpha-equivalence, HE-faithful) and struct-unique-atom \
         (structural equality, PeTTa) must diverge on inputs containing \
         distinct free variables. Got unique-atom={:?}, struct-unique-atom={:?}",
        alpha, struct_
    );
    assert_eq!(alpha, vec!["($x)"]);
    assert_eq!(struct_, vec!["($x $y)"]);
}

#[test]
fn unique_atom_and_struct_unique_atom_agree_on_ground_terms() {
    // For all-ground tuples, alpha-equivalence and structural equality
    // agree, so all three flavors produce identical results.
    let alpha = run_one("!(unique-atom (a b a c))");
    let struct_ = run_one("!(struct-unique-atom (a b a c))");
    assert_eq!(alpha, struct_);
    assert_eq!(alpha, vec!["(a b c)"]);
}

// =============================================================================
// Bidirectional unification + transitive env trimming regression
// =============================================================================
//
// This is the canonical reproduction of the bug fixed in this branch:
// when a rule has a repeated variable ($A) and the input matches it with
// values that contain free variables ($1), bidirectional unification must
// fire (binding $1 to the first occurrence's value) AND the result must
// be transitively substituted (so $1 → Anna everywhere in the RHS).

#[test]
fn modus_ponens_repeated_var_bidirectional_unify() {
    // Modus Ponens rule with two occurrences of $A:
    //   (= (|- ($A $T1) ((Implication $A $B) $T2))
    //      ($B (mp $T1 $T2)))
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
    // The free variable $1 in the implication must be unified with Anna
    // from the first occurrence of $A, then transitively substituted
    // through the RHS template.
    assert_eq!(
        r,
        vec!["((Inheritance Anna (IntSet cancerous)) (mp (stv 1 0.9) (stv 0.6 0.9)))"]
    );
}

#[test]
fn modus_ponens_concrete_first_arg() {
    // The same rule applied to a concrete first argument should also work.
    let r = run_one(
        r#"
        (= (|- ($A $T1) ((Implication $A $B) $T2))
           ($B (mp $T1 $T2)))
        !(|- ((Inheritance Anna (IntSet smokes)) (stv 1 0.9))
             ((Implication (Inheritance Anna (IntSet smokes))
                           (Inheritance Anna (IntSet cancerous)))
              (stv 0.6 0.9)))
    "#,
    );
    assert_eq!(
        r,
        vec!["((Inheritance Anna (IntSet cancerous)) (mp (stv 1 0.9) (stv 0.6 0.9)))"]
    );
}

// =============================================================================
// Phase 1.2: 2-arg `(if Cond Then)` PT-style — emits Empty on False
// =============================================================================

#[test]
fn if_2arg_true_returns_then() {
    let r = run_one("!(if True yes)");
    assert_eq!(r, vec!["yes"]);
}

#[test]
fn if_2arg_false_branch_drops() {
    // PHE-finer #3: 2-arg if on False emits Empty so the trampoline
    // branch-drops the directive (no result line emitted).
    // Cross-validated against live PeTTa: `!(if False yes)` produces
    // no output. Different from MTT's earlier `[()]` extension.
    let r = run_one("!(if False yes)");
    assert!(r.is_empty(), "expected branch drop, got {:?}", r);
}

#[test]
fn if_3arg_remains_unchanged() {
    let r = run_one("!(if False yes no)");
    assert_eq!(r, vec!["no"]);
}

// =============================================================================
// Phase 1.3: `&name` named-space auto-creation
// =============================================================================

// =============================================================================
// Phase 1.7: unify lazy-branch + superpose permissive (finer #11, #13)
// =============================================================================

#[test]
fn unify_lazy_branch_then() {
    // `(unify A B then else)`: when A unifies with B, return `then`.
    // MTT-extension over PT (PT doesn't have native `unify`).
    let r = run_one("!(unify foo foo matched not)");
    assert_eq!(r, vec!["matched"]);
}

#[test]
fn unify_lazy_branch_else() {
    // When A does NOT unify with B, return `else`.
    let r = run_one("!(unify foo bar matched not)");
    assert_eq!(r, vec!["not"]);
}

#[test]
fn superpose_permissive_list() {
    // PT translator: superpose enumerates its list contents as separate
    // result branches.
    let r = run_one("!(superpose (1 2 3))");
    assert_eq!(r.len(), 3);
    let mut sorted = r.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["1", "2", "3"]);
}

#[test]
fn named_space_auto_create() {
    // Per Amendments §A3: PeTTa REPL auto-creates `&mykb` lazily on first
    // reference. MTT mirrors this via preprocess_space_refs_generic.
    // `get-atoms` enumerates each atom as a separate result.
    let r = run_one(
        r#"
        !(add-atom &mykb (alpha))
        !(add-atom &mykb (beta))
        !(get-atoms &mykb)
        "#,
    );
    assert_eq!(r.len(), 2, "expected two atoms in space, got {:?}", r);
    let combined = r.join(" ");
    assert!(
        combined.contains("(alpha)") && combined.contains("(beta)"),
        "expected both atoms in space, got {:?}",
        r
    );
}

// =============================================================================
// Phase 1.4: `add-atom &kb (= H B)` PT dual-storage — rule fires globally
// =============================================================================

#[test]
fn add_atom_kb_rule_fires_globally() {
    // PT semantics: `assertz(H :- B)` from `add-atom &kb (= H B)` puts the
    // clause into the global Prolog database. MTT mirrors this by routing
    // (= H B) atoms through env.add_to_space() in both the trampoline and
    // bytecode VM's add-atom paths, so `(qux 9)` at top-level dispatches to
    // the rule even though it was added to a named space.
    let r = run_one(
        r#"
        !(add-atom &kb (= (qux $y) ($y zip)))
        !(qux 9)
        "#,
    );
    assert_eq!(r, vec!["(9 zip)"]);
}

#[test]
fn add_atom_self_rule_still_fires() {
    // Regression: &self path preserved.
    let r = run_one(
        r#"
        !(add-atom &self (= (bar $x) ($x baz)))
        !(bar 5)
        "#,
    );
    assert_eq!(r, vec!["(5 baz)"]);
}
