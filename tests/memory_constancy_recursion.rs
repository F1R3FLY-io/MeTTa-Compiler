//! **Task #6 Phase 8 (2026-05-18) — Memory-constancy regression tests**
//!
//! These tests are the codified contract for Task #6: under tight infinite
//! self-recursion (`(= (rec) (rec)) !(rec)`, variable-arg variants, and
//! mutual recursion), the T0 trampoline MUST terminate quickly and with
//! bounded RSS growth.
//!
//! Historical regression: a tight `(= (rec) (rec)) !(rec)` previously
//! consumed 75+ GB RAM and was OOM-killed. After Phases 1-7 of Task #6
//! (TCO at rule-RHS push sites + hash-cycle cap on the deterministic
//! chain + continuation collapse), the same fixture returns `[]` (via
//! subgoal-tabling cycle detection) within a fraction of a second at T0.
//!
//! **Scope**: these tests exercise BOTH the T0 (Treewalker) tier (via
//! `eval_with_tier(TierSelection::Treewalker, FallbackPolicy::SilentDemote)`)
//! AND the default `auto` tier (via `eval()`). The auto-tier variants
//! verify Task #6 Phase 9 — bytecode-VM cycle detection via the trampoline-
//! shared `cesk::tabling::ACTIVE_EVAL_SET` — so the VM's `op_dispatch_rules`
//! short-circuits on self-recursion instead of growing the call_stack
//! unbounded.
//!
//! Stack-safety mandate (see [[feedback-stack-safety-mandate]]): NO
//! artificial depth limits — termination comes from STRUCTURAL cycle
//! detection (`is_actively_evaluating` at the trampoline + hash-cycle
//! cap inside `try_deterministic_chain`).
//!
//! ## What these tests assert
//!
//! 1. Self-recursive ground rule: `(= (rec) (rec)) !(rec) → []`
//! 2. Self-recursive variable-arg rule: `(= (rec $x) (rec $x)) !(rec 0) → []`
//! 3. Mutual recursion: `(= (a) (b)) (= (b) (a)) !(a) → []`
//! 4. Repeated invocation (100x tight loop) — total wall time bounded
//!
//! Each test asserts termination within `MAX_EVAL_WALL_SECS` and (where
//! feasible) bounded RSS-delta via /proc/self/statm sampling.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use mettatron::backend::eval::tier_forced::{eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection};
use mettatron::backend::models::MettaValue;
use mettatron::{compile, eval, new_env};

/// Hard wall-clock budget per fixture. Phases 2-7 of Task #6 make all of
/// these terminate in milliseconds at T0; we set 5 seconds as the regression
/// canary — if the budget is ever blown, the trampoline has lost a TCO
/// site or the chain cycle cap.
const MAX_EVAL_WALL_SECS: u64 = 5;

/// Maximum acceptable RSS-delta in MB for a single evaluation.
/// Historical regression was 75_000+ MB. After Phase 5+ the actual delta
/// is well under 50 MB; we set 200 MB as the regression canary to absorb
/// slab-allocator warmup variance.
const MAX_RSS_DELTA_MB: u64 = 200;

/// Read RSS in MB from `/proc/self/statm`. Field 1 is resident pages.
#[cfg(target_os = "linux")]
fn rss_mb() -> u64 {
    use std::fs;
    let s = match fs::read_to_string("/proc/self/statm") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let parts: Vec<&str> = s.split_whitespace().collect();
    if parts.len() < 2 {
        return 0;
    }
    let resident_pages: u64 = parts[1].parse().unwrap_or(0);
    let page_size = 4096u64; // Linux x86_64 standard
    (resident_pages * page_size) / (1024 * 1024)
}

#[cfg(not(target_os = "linux"))]
fn rss_mb() -> u64 {
    0
}

/// Run `source` through the in-process compile + eval_with_tier(T0)
/// pipeline on a background thread, returning the result list (or
/// panicking on timeout). T0 forced to avoid the separate auto-tier
/// bytecode-VM leak path.
fn eval_t0_with_timeout(source: &'static str, label: &'static str) -> Vec<MettaValue> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(format!("mem-constancy-{}", label))
        .spawn(move || {
            let state = compile(source).expect("compile failed");
            let env = new_env();
            let exprs = state.source_snapshot();
            let mut env = env;
            let mut last_results: Vec<MettaValue> = Vec::new();
            for expr in &exprs {
                let outcome = eval_with_tier(
                    *expr,
                    env,
                    &state,
                    TierSelection::Treewalker,
                    FallbackPolicy::SilentDemote,
                );
                match outcome {
                    TierEvalOutcome::Ok { results, env: new_env, .. }
                    | TierEvalOutcome::Demoted { results, env: new_env, .. } => {
                        last_results = results.into_iter().collect();
                        env = new_env;
                    }
                    TierEvalOutcome::NotApplicable { reason } => {
                        panic!("T0 NotApplicable for `{}`: {:?}", source, reason);
                    }
                }
            }
            let _ = tx.send(last_results);
        })
        .expect("spawn eval thread");

    match rx.recv_timeout(Duration::from_secs(MAX_EVAL_WALL_SECS)) {
        Ok(results) => results,
        Err(_) => panic!(
            "Task #6 regression: `{}` exceeded {}s wall budget at T0 — TCO/cycle-cap likely lost",
            label, MAX_EVAL_WALL_SECS
        ),
    }
}

#[test]
fn self_recursive_ground_rule_terminates_empty_t0() {
    let rss_before = rss_mb();
    let results = eval_t0_with_timeout(
        "(= (rec) (rec))\n!(rec)\n",
        "rec-ground",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    // HE-bisim: tight self-recursion with no productive RHS step returns
    // an empty result set (cycle detected, no convergent value).
    assert_eq!(
        results.len(),
        0,
        "expected 0 results from `(= (rec) (rec)) !(rec)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "RSS delta {} MB exceeds budget {} MB for `(rec)` — Phase 5 cycle cap may have regressed",
        delta,
        MAX_RSS_DELTA_MB
    );
}

#[test]
fn self_recursive_variable_arg_rule_terminates_empty_t0() {
    let rss_before = rss_mb();
    let results = eval_t0_with_timeout(
        "(= (rec $x) (rec $x))\n!(rec 0)\n",
        "rec-var",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    // Phase 4 specifically covers this variable-arg case via the
    // `EvalWithBindings` deferred-chain TCO at eval_loop.rs Site C.
    assert_eq!(
        results.len(),
        0,
        "expected 0 results from `(= (rec $x) (rec $x)) !(rec 0)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "RSS delta {} MB exceeds budget {} MB for variable-arg `(rec)` — Phase 4 EvalWithBindings TCO may have regressed",
        delta,
        MAX_RSS_DELTA_MB
    );
}

#[test]
fn mutual_recursion_terminates_empty_t0() {
    let rss_before = rss_mb();
    let results = eval_t0_with_timeout(
        "(= (a) (b))\n(= (b) (a))\n!(a)\n",
        "mutual",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    // Mutual recursion `(a) → (b) → (a)` must also be detected via
    // subgoal-tabling cycle detection (`is_actively_evaluating`) AND
    // the deterministic-chain hash-cycle cap (Phase 5).
    assert_eq!(
        results.len(),
        0,
        "expected 0 results from mutual `(a) ↔ (b)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "RSS delta {} MB exceeds budget {} MB for mutual recursion",
        delta,
        MAX_RSS_DELTA_MB
    );
}

// =========================================================================
// Auto-tier coverage (Phase 9 — bytecode VM cycle detection)
// =========================================================================

/// Run `source` through the in-process compile + default `eval()` (auto-tier)
/// pipeline on a background thread. Auto-tier exercises the bytecode VM's
/// `op_dispatch_rules` cycle-detection path (Phase 9) — without that fix,
/// `(rec) → (rec)` OOM-kills the process within seconds.
fn eval_auto_with_timeout(source: &'static str, label: &'static str) -> Vec<MettaValue> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name(format!("mem-constancy-auto-{}", label))
        .spawn(move || {
            let state = compile(source).expect("compile failed");
            let env = new_env();
            let exprs = state.source_snapshot();
            let mut env = env;
            let mut last_results: Vec<MettaValue> = Vec::new();
            for expr in &exprs {
                let (results, new_env) = eval(*expr, env, &state);
                env = new_env;
                last_results = results.into_iter().collect();
            }
            let _ = tx.send(last_results);
        })
        .expect("spawn eval thread");

    match rx.recv_timeout(Duration::from_secs(MAX_EVAL_WALL_SECS)) {
        Ok(results) => results,
        Err(_) => panic!(
            "Task #6 regression: `{}` exceeded {}s wall budget at AUTO tier — Phase 9 VM cycle detection likely lost",
            label, MAX_EVAL_WALL_SECS
        ),
    }
}

#[test]
fn self_recursive_ground_rule_terminates_empty_auto() {
    let rss_before = rss_mb();
    let results = eval_auto_with_timeout(
        "(= (rec) (rec))\n!(rec)\n",
        "rec-ground",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    assert_eq!(
        results.len(),
        0,
        "AUTO tier: expected 0 results from `(= (rec) (rec)) !(rec)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "AUTO tier: RSS delta {} MB exceeds budget {} MB for `(rec)` — Phase 9 VM cycle cap may have regressed",
        delta,
        MAX_RSS_DELTA_MB
    );
}

#[test]
fn self_recursive_variable_arg_rule_terminates_empty_auto() {
    let rss_before = rss_mb();
    let results = eval_auto_with_timeout(
        "(= (rec $x) (rec $x))\n!(rec 0)\n",
        "rec-var",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    assert_eq!(
        results.len(),
        0,
        "AUTO tier: expected 0 results from `(= (rec $x) (rec $x)) !(rec 0)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "AUTO tier: RSS delta {} MB exceeds budget {} MB for variable-arg `(rec)`",
        delta,
        MAX_RSS_DELTA_MB
    );
}

#[test]
fn mutual_recursion_terminates_empty_auto() {
    let rss_before = rss_mb();
    let results = eval_auto_with_timeout(
        "(= (a) (b))\n(= (b) (a))\n!(a)\n",
        "mutual",
    );
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    // Mutual recursion is the killer case for VM-local cycle detection:
    // (a) calls (b), (b) calls (a). With Phase 9's shared cesk::tabling
    // set, both the trampoline and VM observe the same mark — when (a)'s
    // outer dispatch marks `hash((a))` active, the inner (b)→(a) re-entry
    // sees the mark and returns empty.
    assert_eq!(
        results.len(),
        0,
        "AUTO tier: expected 0 results from mutual `(a) ↔ (b)`, got {} ({:?})",
        results.len(),
        results
    );
    assert!(
        delta < MAX_RSS_DELTA_MB,
        "AUTO tier: RSS delta {} MB exceeds budget {} MB for mutual recursion",
        delta,
        MAX_RSS_DELTA_MB
    );
}

#[test]
fn repeated_invocation_total_rss_bounded_t0() {
    // 100 fresh evaluations of `(= (rec) (rec)) !(rec)` — verifies the
    // session-tear-down + GC reclaim path correctly releases allocations
    // between invocations. If continuations were leaking globally,
    // total RSS-delta would scale linearly.
    let rss_before = rss_mb();
    let start = Instant::now();
    for i in 0..100 {
        let label = if i == 0 { "loop-0" } else { "loop-N" };
        let results = eval_t0_with_timeout("(= (rec) (rec))\n!(rec)\n", label);
        assert_eq!(
            results.len(),
            0,
            "iteration {} expected 0 results, got {}",
            i,
            results.len()
        );
    }
    let elapsed = start.elapsed();
    let rss_after = rss_mb();
    let delta = rss_after.saturating_sub(rss_before);

    // 100 iterations × per-iter overhead → must still fit in cumulative
    // budget; if we leak per-session continuations the delta blows up.
    let cumulative_budget_mb = MAX_RSS_DELTA_MB * 2;
    assert!(
        delta < cumulative_budget_mb,
        "100-iter cumulative RSS delta {} MB exceeds budget {} MB — per-session GC leak suspected",
        delta,
        cumulative_budget_mb
    );
    assert!(
        elapsed < Duration::from_secs(MAX_EVAL_WALL_SECS * 4),
        "100-iter cumulative wall time {:?} exceeds budget — TCO regression suspected",
        elapsed
    );
}
