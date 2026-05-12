//! Z.A.4 regression: JIT choice-point overflow correctness.
//!
//! When the number of alternatives exceeds `MAX_ALTERNATIVES_INLINE` (=32),
//! the JIT bails to the VM tier which re-enumerates via T0/T1 native
//! dispatch. This test verifies:
//!
//! 1. No duplicate results emitted by the bailout path.
//! 2. No skipped alternatives.
//! 3. The result multiset has the expected size.
//!
//! Closes: T0-T2-014 / T0-T3-015 audit.
//!
//! Anchor: `src/backend/bytecode/jit/runtime/space_ops.rs:278-293,505-513`
//! (`jit_runtime_space_get_atoms`, `jit_runtime_space_match_nondet`).
//!
//! NOTE: tests must run serially (`-- --test-threads=1`) — running them
//! in parallel introduces JIT cache and WorkPool interference that is
//! unrelated to the choice-point bailout correctness. The single
//! all-in-one test below executes all sub-cases sequentially in a
//! single thread to bypass that harness-level concurrency issue.

use mettatron::{compile, eval, new_env};
use std::collections::BTreeMap;

fn run_one(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let env = new_env();
    let exprs: Vec<_> = state.source_snapshot();
    assert!(!exprs.is_empty(), "source had no expressions");
    let mut env = env;
    let mut last = Vec::new();
    for expr in &exprs {
        let (results, new_env) = eval(*expr, env, &state);
        env = new_env;
        last = results.into_iter().collect();
    }
    last.iter().map(|v| format!("{}", v)).collect()
}

fn run_match_n_facts(n: usize) -> Vec<String> {
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!("(fact {})\n", i));
    }
    src.push_str("!(match &self (fact $x) $x)\n");
    let mut r = run_one(&src);
    r.sort();
    r
}

fn count_multiset(r: &[String]) -> BTreeMap<String, usize> {
    let mut m = BTreeMap::new();
    for s in r {
        *m.entry(s.clone()).or_insert(0) += 1;
    }
    m
}

fn assert_no_duplicates(r: &[String], expected_n: usize) {
    let m = count_multiset(r);
    assert_eq!(
        r.len(),
        expected_n,
        "expected {} results, got {}: {:?}",
        expected_n,
        r.len(),
        r
    );
    for (k, v) in &m {
        assert_eq!(*v, 1, "duplicate result for {}: count={}", k, v);
    }
}

/// All sub-cases run inline so they share a single test thread and don't
/// race against each other via the JIT global cache. The boundaries
/// exercised: below threshold (16), at threshold (32 = MAX_ALTERNATIVES_INLINE),
/// just above (33 = bailout), well above (64), and a `superpose` form
/// with 33 alts (a different bailout call site).
#[test]
fn jit_choicepoint_overflow_no_duplicates_or_skips() {
    // 16 < 32: JIT inline path.
    assert_no_duplicates(&run_match_n_facts(16), 16);

    // 32 = MAX_ALTERNATIVES_INLINE: JIT inline boundary.
    assert_no_duplicates(&run_match_n_facts(32), 32);

    // 33 > 32: triggers SpaceMatch bailout. No double-yield, no skips.
    assert_no_duplicates(&run_match_n_facts(33), 33);

    // 2× threshold: bailout under sustained load.
    assert_no_duplicates(&run_match_n_facts(64), 64);

    // superpose with 33 alternatives exercises jit_runtime_superpose
    // (different bailout site from SpaceMatch but same MAX constraint).
    let mut src = String::from("!(superpose (");
    for i in 0..33 {
        src.push_str(&format!("{} ", i));
    }
    src.push_str("))\n");
    let mut r = run_one(&src);
    r.sort();
    assert_no_duplicates(&r, 33);
}
