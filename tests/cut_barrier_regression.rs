//! Regression tests for Phase 1 of the MeTTaTron control substrate:
//! the cut-barrier identity replacement for the broken depth-based cut.
//!
//! These tests lock in PeTTa-correct `(cut)` semantics: a cut commits the
//! ENCLOSING clause's nondeterminism to its FIRST answer (in source order)
//! and discards the remaining alternatives. Before Phase 1, MeTTaTron's
//! depth-based cut was structurally unreachable for the single-rule
//! fast-path + `let*`/`match` fan-out shape — both branches survived.
//!
//! Design (`docs/wam/control-substrate-design.md`): a monotonic *barrier id*
//! identifies each cut scope; every nondeterministic fan-out continuation
//! records the barrier active at its creation and prunes when `(cut)` fires
//! that id. This is scope-precise across the heterogeneous fan-out forest
//! (rule ∨ match ∨ superpose ∨ let*), which a single deepest-depth target
//! could not express. Within an active cut scope all fan-out is forced
//! sequential (parallel cut has no defined "first" branch, and the barrier
//! thread-locals do not propagate to work-pool workers).
//!
//! The canonical reproducer is PeTTa's `examples/cut.metta`. It is
//! transcribed INLINE here (do NOT read it from `/PeTTa/`) so this test is
//! self-contained and pins the exact committed result.
//!
//! Oracle: `cd /home/dylon/Workspace/f1r3fly.io/PeTTa && sh run.sh
//! examples/cut.metta silent` → `(bar 1)` (cut commits to the FIRST match).
//!
//! Adding or modifying tests here requires re-running the full lib/nextest
//! suite + mtt-conformance + the PLN-main examples to confirm no regression.

use mettatron::backend::models::MettaValueTrait;
use mettatron::{compile, eval, new_env};

/// Helper: compile + evaluate a MeTTa source string, threading the environment
/// across top-level expressions (so rules added by `(= ...)` and atoms added
/// by `add-atom` are visible to subsequent `!` queries). Returns the
/// `friendly_repr` strings of every `!`-directive result (Empty filtered out,
/// matching the CLI / conformance harness observation).
fn eval_bang_results(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let mut all: Vec<String> = Vec::new();
    for expr in state.source().iter() {
        let expr = *expr;
        let is_bang = expr
            .as_sexpr()
            .and_then(|items| items.first())
            .and_then(|h| h.as_atom())
            .is_some_and(|s| s == "!");
        let (results, env_after, ..) = eval(expr, env, &state);
        env = env_after;
        if is_bang {
            for v in results {
                if !v.is_empty() {
                    all.push(format!("{}", v.friendly_repr()));
                }
            }
        }
    }
    all
}

/// Helper returning the results of only the LAST top-level expression
/// (regardless of `!`), used by the rule-level cut tests below.
fn eval_last(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let mut last: Vec<String> = Vec::new();
    let expr_count = state.source().len();
    for (idx, expr) in state.source().iter().enumerate() {
        let expr = *expr;
        let (results, env_after, ..) = eval(expr, env, &state);
        env = env_after;
        if idx == expr_count - 1 {
            last = results
                .iter()
                .map(|r| format!("{}", r.friendly_repr()))
                .collect();
        }
    }
    last
}

// The canonical PeTTa cut.metta program, transcribed INLINE (do NOT read it
// from /PeTTa/). `(foo 1)` and `(foo 2)` both match `(foo $1)`; `match-single`
// reads the match into `$x`, then `(cut)` commits the match to its FIRST
// answer, so only `(bar 1)` is added. PeTTa → `(bar 1)`; pre-Phase-1 MTT →
// `(bar 2)` (both survived).
//
// We OMIT cut.metta's own `(test ...)` directive and instead read the space
// back ourselves via `collapse`+`match`, so the assertion is explicit and
// does not depend on the `test` machinery.
const CUT_METTA_PROGRAM: &str = r#"
(foo 1)
(foo 2)

(= (match-single $space $pat $ret)
   (let* (($x (match $space $pat $ret))
          ($temp (cut)))
         $x))

!(let $x (match-single &self (foo $1) $1) (add-atom &self (bar $x)))

!(collapse (match &self (bar $1) (bar $1)))
"#;

/// The cut.metta reproducer must commit to the FIRST match: exactly `(bar 1)`
/// is added to the space; `(bar 2)` must NOT survive.
#[test]
fn cut_metta_commits_to_first_match() {
    let results = eval_bang_results(CUT_METTA_PROGRAM);
    // Directive 1 (`add-atom`) returns the unit `()` (one observable result —
    // the cut committed the inner match to a single value, so `add-atom` ran
    // exactly ONCE; pre-Phase-1 it ran twice → two `()` results). Directive 2
    // (`collapse (match ...)`) returns the single tuple of surviving bars.
    assert_eq!(
        results.len(),
        2,
        "expected two observable directive results — one `()` from the single \
         committed add-atom and one collapse tuple; got: {:?} \
         (a length of 3+ means the cut failed to prune and add-atom ran twice)",
        results
    );
    assert_eq!(
        results[0], "()",
        "directive 1 (add-atom) must produce exactly one `()` — the cut \
         commits to the FIRST match so add-atom runs once; got: {}",
        results[0]
    );
    assert_eq!(
        results[1], "((bar 1))",
        "cut must commit to the FIRST match — the space must hold only \
         (bar 1); got: {}",
        results[1]
    );
    // Defensive: the second match must NOT have leaked into the space.
    let combined = results.join(" ");
    assert!(
        !combined.contains("(bar 2)"),
        "the cut failed to prune the second match — (bar 2) leaked: {}",
        combined
    );
}

/// The cut.metta reproducer must produce the SAME committed result on every
/// run — the cut-barrier prune is order-independent and not a nondeterministic
/// flake. (Models `tests/ghost_branch_regression.rs`'s 20-run determinism
/// pattern.) Before Phase 1's `try_acquire_budget` cut-scope veto, the
/// multi-match fan-out raced across work-pool threads where the cut signal was
/// silently lost; a determinism assertion guards against any such regression.
#[test]
fn cut_metta_deterministic_20_runs() {
    let mut baseline: Option<Vec<String>> = None;
    for i in 0..20 {
        let mut results = eval_bang_results(CUT_METTA_PROGRAM);
        results.sort();
        if let Some(ref base) = baseline {
            assert_eq!(
                &results, base,
                "iteration {}: cut.metta result differs from baseline — \
                 cut commitment is not deterministic",
                i
            );
        } else {
            baseline = Some(results);
        }
    }
    let base = baseline.expect("at least one run");
    // Sorted lexicographically: `"((bar 1))"` < `"()"` (the inner `(` 0x28
    // sorts before `)` 0x29 at the second byte).
    assert_eq!(
        base.len(),
        2,
        "expected exactly two results every run (one `()` add-atom + one \
         collapse tuple); got: {:?}",
        base
    );
    assert_eq!(
        base,
        vec!["((bar 1))".to_string(), "()".to_string()],
        "baseline must be the single committed add-atom `()` plus the \
         committed first match (bar 1); got: {:?}",
        base
    );
}

/// Rule-dispatch-level cut: a multi-rule function `(pick)` produces three
/// answers; a clause that reads one into `$x` then cuts must commit to the
/// FIRST. This exercises the cut barrier opened by the single-rule
/// `first-pick` dispatch flowing through the `(pick)` multi-match fork and the
/// `let*` value-expr fan-out. Oracle (live PeTTa): `1`.
#[test]
fn rule_dispatch_cut_commits_first_pick() {
    let source = r#"
        (= (pick) 1)
        (= (pick) 2)
        (= (pick) 3)
        (= (first-pick) (let* (($x (pick)) ($t (cut))) $x))
        !(first-pick)
    "#;
    let results = eval_last(source);
    assert_eq!(
        results,
        vec!["1".to_string()],
        "cut must commit (pick) to its first answer; got: {:?}",
        results
    );
}

/// Rule-dispatch cut determinism across 20 runs (the multi-match fork must be
/// pruned to a single committed answer every time — guards the parallel-veto).
#[test]
fn rule_dispatch_cut_deterministic_20_runs() {
    let source = r#"
        (= (pick) 1)
        (= (pick) 2)
        (= (pick) 3)
        (= (first-pick) (let* (($x (pick)) ($t (cut))) $x))
        !(first-pick)
    "#;
    let mut baseline: Option<Vec<String>> = None;
    for i in 0..20 {
        let mut results = eval_last(source);
        results.sort();
        if let Some(ref base) = baseline {
            assert_eq!(
                &results, base,
                "iteration {}: rule-dispatch cut result differs from baseline",
                i
            );
        } else {
            baseline = Some(results);
        }
    }
    let base = baseline.expect("at least one run");
    assert_eq!(
        base,
        vec!["1".to_string()],
        "baseline must be the committed first answer; got: {:?}",
        base
    );
}

/// A cut INSIDE a `quote` is data, not a control cut — it must NOT open a
/// barrier or prune anything. `(quote (cut))` is preserved verbatim and the
/// multi-rule `(pick)` fans out fully. This pins the quote-awareness of
/// `expr_contains_cut`.
#[test]
fn quoted_cut_does_not_prune() {
    let source = r#"
        (= (pick) 1)
        (= (pick) 2)
        (= (pick) 3)
        (= (with-quoted-cut $x) (let* (($q (quote (cut)))) $x))
        !(collapse (with-quoted-cut (pick)))
    "#;
    let results = eval_last(source);
    assert_eq!(
        results.len(),
        1,
        "expected one collapse tuple: {:?}",
        results
    );
    let s = &results[0];
    // No cut fired ⇒ all three (pick) answers survive into the collapse.
    assert!(
        s.contains('1') && s.contains('2') && s.contains('3'),
        "quoted (cut) must NOT prune — expected all of 1,2,3; got: {}",
        s
    );
}

/// NESTED cut scopes with a TEXTUALLY-IDENTICAL `(cut)`: an outer clause
/// `(topn)` whose `let*` calls a SEPARATE cut-bearing rule `(pick)` and THEN
/// cuts. Both clauses contain the literal `(cut)`. The inner `(pick)` runs
/// first and commits its own multi-match; the OUTER `(cut)` must still fire and
/// commit `(topn)`'s `$a` fan-out to its FIRST answer.
///
/// Root cause this pins (2026-05-26): `(cut)` was NOT in `is_impure_head`, so
/// `should_memoize((cut))` returned true. The inner `(pick)`'s `(cut)` cached
/// its `(unit)` result; the outer `(topn)`'s identical `(cut)` then hit the
/// memo and returned the cached `(unit)` WITHOUT re-firing `set_cut_active`, so
/// the outer cut never pruned and `$a` committed to its LAST answer (`(pr 2 1)`
/// instead of `(pr 1 1)`). Marking `cut` impure forces every `(cut)` to
/// re-evaluate. Oracle (live PeTTa): the outer cut commits to the first `$a`.
#[test]
fn cut_nested_inner_cut_rule_not_memoized() {
    let source = r#"
        (foo 1)
        (foo 2)
        (gp 1)
        (gp 2)
        (= (pick) (let* (($v (match &self (gp $g) $g)) ($t (cut))) $v))
        (= (topn) (let* (($a (match &self (foo $f) $f)) ($b (pick)) ($z (cut))) (pr $a $b)))
        !(collapse (topn))
    "#;
    let results = eval_last(source);
    assert_eq!(
        results.len(),
        1,
        "expected one collapse tuple: {:?}",
        results
    );
    let s = &results[0];
    assert!(
        s.contains("(pr 1 1)"),
        "outer cut must commit $a to its FIRST answer (pr 1 1); got: {}",
        s
    );
    assert!(
        !s.contains("(pr 2"),
        "outer cut failed to prune — the second $a answer (pr 2 _) leaked \
         (the inner cut-rule's identical (cut) was memoized, skipping the \
         outer cut's set_cut_active side-effect): {}",
        s
    );
}

/// 20-run determinism for the nested-cut case — the memoization fix must hold
/// regardless of evaluation order / cache warmth.
#[test]
fn cut_nested_deterministic_20_runs() {
    let source = r#"
        (foo 1)
        (foo 2)
        (gp 1)
        (gp 2)
        (= (pick) (let* (($v (match &self (gp $g) $g)) ($t (cut))) $v))
        (= (topn) (let* (($a (match &self (foo $f) $f)) ($b (pick)) ($z (cut))) (pr $a $b)))
        !(collapse (topn))
    "#;
    let mut baseline: Option<Vec<String>> = None;
    for i in 0..20 {
        let mut results = eval_last(source);
        results.sort();
        if let Some(ref base) = baseline {
            assert_eq!(
                &results, base,
                "iteration {}: nested-cut result differs from baseline",
                i
            );
        } else {
            baseline = Some(results);
        }
    }
    let base = baseline.expect("at least one run");
    assert_eq!(
        base.len(),
        1,
        "expected one collapse tuple every run: {:?}",
        base
    );
    assert!(
        base[0].contains("(pr 1 1)") && !base[0].contains("(pr 2"),
        "baseline must be the committed first answer (pr 1 1); got: {:?}",
        base
    );
}

/// A clause whose body does NOT contain `(cut)` must keep full
/// nondeterminism — Phase 1 must not accidentally commit cut-free clauses.
/// This guards against the barrier being opened/inherited too eagerly.
#[test]
fn no_cut_keeps_full_nondeterminism() {
    let source = r#"
        (= (pick) 1)
        (= (pick) 2)
        (= (pick) 3)
        (= (all-picks) (let* (($x (pick))) $x))
        !(collapse (all-picks))
    "#;
    let results = eval_last(source);
    assert_eq!(
        results.len(),
        1,
        "expected one collapse tuple: {:?}",
        results
    );
    let s = &results[0];
    assert!(
        s.contains('1') && s.contains('2') && s.contains('3'),
        "cut-free clause must keep all answers; got: {}",
        s
    );
}
