//! Regression tests for Phase 2 of the MeTTaTron control substrate: the
//! PeTTa `(once X)` form, built as a thin layer over the Phase-1 cut barrier.
//!
//! `once(X)` ≡ Prolog `once(G) = (G, !)` scoped to G: it evaluates X, commits
//! to X's FIRST answer (in source order), and PRUNES X's remaining
//! nondeterministic fan-out. The commitment is scope-local — a `(once …)`
//! inside a clause must NOT prune the enclosing clause's OTHER nondeterminism.
//!
//! Implementation (`docs/wam/control-substrate-design.md`, Phase 2): `(once X)`
//! desugars to the verified cut idiom `(prog1 X (cut))` =
//! `(let $r X (let $_ (cut) $r))` evaluated under a FRESH cut barrier opened by
//! `StartOnce` and owned by `ProcessOnceRestore` (which consumes the once's cut
//! signal and restores the enclosing barrier). `once` is in `is_impure_head`
//! (never memoized — the cut_nested lesson) and is routed to the T0 trampoline
//! by `can_compile_with_env` (the T1 bytecode VM has no `once` lowering).
//!
//! Oracle (live PeTTa): running `examples/once.metta` in the sibling PeTTa
//! checkout with silent output returns `(bar 1)`.
//!
//! Adding or modifying tests here requires re-running the full lib/nextest
//! suite + mtt-conformance + the PLN-main examples to confirm no regression.

use mettatron::backend::models::MettaValueTrait;
use mettatron::{compile, eval, new_env};

/// Compile + evaluate, threading the env across top-level expressions; return
/// the `friendly_repr` of every `!`-directive result (Empty filtered out).
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

/// Results of only the LAST top-level expression.
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

// PeTTa once.metta, transcribed INLINE (do NOT read it from /PeTTa/). `(foo 1)`
// and `(foo 2)` both match; `match-single` reads the match via `(once …)`,
// committing to the FIRST answer → only `(bar 1)` is added. PeTTa → `(bar 1)`;
// pre-Phase-2 MTT → `(bar (once 1)) (bar (once 2))` (once fell through to data).
const ONCE_METTA_PROGRAM: &str = r#"
(foo 1)
(foo 2)

(= (match-single $space $pat $ret)
   (once (match $space $pat $ret)))

!(let $x (match-single &self (foo $1) $1) (add-atom &self (bar $x)))

!(collapse (match &self (bar $1) (bar $1)))
"#;

/// once over a match commits to the FIRST answer: exactly `(bar 1)` is added.
#[test]
fn once_commits_match_to_first() {
    let results = eval_bang_results(ONCE_METTA_PROGRAM);
    // Directive 1 (`add-atom`) returns one `()` — once committed the match to a
    // single value, so add-atom ran ONCE (pre-Phase-2 it ran twice and added a
    // `(once 1)`/`(once 2)` data wrapper). Directive 2 returns the collapse tuple.
    assert_eq!(
        results.len(),
        2,
        "expected two observable results (one `()` + one collapse tuple); got: {:?}",
        results
    );
    assert_eq!(
        results[1], "((bar 1))",
        "once must commit to FIRST match; got: {}",
        results[1]
    );
    let combined = results.join(" ");
    assert!(
        !combined.contains("(bar 2)"),
        "once failed to prune — (bar 2) leaked: {}",
        combined
    );
    assert!(
        !combined.contains("once"),
        "`(once …)` must be evaluated, not left as data: {}",
        combined
    );
}

/// `(once X)` must be observationally identical to the explicit cut idiom
/// `(let* (($x X) ($t (cut))) $x)` when both are wrapped in a RULE body — this
/// is the matchsingle.metta desugar contract. (NB: the cut idiom only commits
/// inside a rule body, whose dispatch opens the cut barrier. `once` is strictly
/// MORE self-contained: it opens its OWN fresh barrier via `StartOnce`, so it
/// also commits at top level — a superior property, intentionally NOT asserted
/// here since the contract is the rule-wrapped equivalence.)
#[test]
fn once_equals_cut_idiom() {
    let once_src = r#"
        (foo 1)
        (foo 2)
        (foo 3)
        (= (f) (once (match &self (foo $1) $1)))
        !(collapse (f))
    "#;
    let cut_src = r#"
        (foo 1)
        (foo 2)
        (foo 3)
        (= (f) (let* (($x (match &self (foo $1) $1)) ($t (cut))) $x))
        !(collapse (f))
    "#;
    let once_res = eval_last(once_src);
    let cut_res = eval_last(cut_src);
    assert_eq!(
        once_res, cut_res,
        "once must equal the rule-wrapped cut idiom; once={:?} cut={:?}",
        once_res, cut_res
    );
    assert_eq!(
        once_res,
        vec!["(1)".to_string()],
        "both must commit to first; got: {:?}",
        once_res
    );
}

/// once over a multi-clause rule commits to the first clause's answer. This is
/// the case that regressed when `once` was wrongly routed to the bytecode tier
/// (which has no `once` lowering → returned `((once 1) 2 3)`).
#[test]
fn once_over_multiclause_rule() {
    let source = r#"
        (= (xs) 1)
        (= (xs) 2)
        (= (xs) 3)
        !(collapse (once (xs)))
    "#;
    let results = eval_last(source);
    assert_eq!(
        results,
        vec!["(1)".to_string()],
        "once must commit a multi-clause rule call to its first answer; got: {:?}",
        results
    );
}

/// Scope-precision: a `(once …)` bound in a `let*` must NOT prune a SIBLING
/// `superpose` in the same clause. `$x` commits to `(g)`'s first answer (1),
/// but `$y` still fans out over both 10 and 20. If the once-barrier leaked to
/// the enclosing scope, only `(pair 1 10)` would survive.
#[test]
fn once_inside_clause_does_not_overprune() {
    let source = r#"
        (= (g) 1)
        (= (g) 2)
        !(collapse (let* (($x (once (g))) ($y (superpose (10 20)))) (pair $x $y)))
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
        s.contains("(pair 1 10)") && s.contains("(pair 1 20)"),
        "once must commit $x=1 but NOT prune the sibling superpose $y∈{{10,20}}; got: {}",
        s
    );
    assert!(
        !s.contains("(pair 2"),
        "once must commit $x to its FIRST answer (1), not 2; got: {}",
        s
    );
}

/// Nested `(once (once X))` must commit to the first answer without variable
/// capture between the two epoch-freshened desugars.
#[test]
fn nested_once_once() {
    let source = r#"
        (= (g) 1)
        (= (g) 2)
        (= (g) 3)
        !(collapse (once (once (g))))
    "#;
    let results = eval_last(source);
    assert_eq!(
        results,
        vec!["(1)".to_string()],
        "nested once must commit to first answer; got: {:?}",
        results
    );
}

/// `(once X)` with the wrong argument count yields an arity Error (not silent
/// data fall-through).
#[test]
fn once_arity_error() {
    let source = r#"!(once 1 2)"#;
    let results = eval_last(source);
    assert_eq!(results.len(), 1, "expected one error result: {:?}", results);
    assert!(
        results[0].contains("once requires exactly 1 argument"),
        "expected an arity error; got: {}",
        results[0]
    );
}

/// Determinism: the once commitment must be identical on every run (guards the
/// parallel cut-scope veto and the barrier thread-locals against flakes).
#[test]
fn once_deterministic_20_runs() {
    let source = r#"
        (= (xs) 1)
        (= (xs) 2)
        (= (xs) 3)
        !(collapse (once (xs)))
    "#;
    let mut baseline: Option<Vec<String>> = None;
    for i in 0..20 {
        let mut results = eval_last(source);
        results.sort();
        if let Some(ref base) = baseline {
            assert_eq!(
                &results, base,
                "iteration {}: once result differs from baseline",
                i
            );
        } else {
            baseline = Some(results);
        }
    }
    assert_eq!(
        baseline.expect("at least one run"),
        vec!["(1)".to_string()],
        "baseline must be the committed first answer"
    );
}
