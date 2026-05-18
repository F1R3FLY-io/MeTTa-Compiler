//! Generic Trampoline Engine
//!
//! This module provides a truly generic trampoline evaluation engine that works with
//! any value type implementing `MettaValueTrait`. This enables the same evaluation
//! logic to work with both heap-allocated (`MettaValue`) and arena-allocated
//! (`MettaValue`) values.
//!
//! ## Design
//!
//! The generic engine:
//! - Uses `WorkItem<V, F>` and `Continuation<V, F>` for work tracking
//! - Calls `eval_step_generic` for single-step evaluation
//! - Uses `GenericEnvironment<V, F>` for all environment operations
//!
//! ## Entry Points
//!
//! - `eval_trampoline`: Generic evaluation for any `EvalContext`
//!
//! The generic engine is parameterized by the `EvalContext` trait, which determines
//! the value type and factory. The production implementation uses `StaticEvalContext`
//! with arena-allocated `MettaValue` values.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;

// ── Background Drop Worker ─────────────────────────────────────────────
//
// A dedicated thread that receives batches of deferred environment drops
// and destructs them off the hot evaluation path. The PathMap/MettaTrie
// trie cascade drops (33% inclusive CPU) now happen here instead of on
// the eval thread. Spawned lazily on first use.

type SharedEnvArc =
    std::sync::Arc<crate::backend::environment::GenericEnvironmentShared<MettaValue>>;

// ── H11 (2026-05-05) — Worker-side cooperative GC drop ─────────────────
//
// `IS_PARALLEL_WORKER` flags threads spawned by `parallel_branch_eval` /
// `parallel_collapse_eval` so non-trampoline tiers (bytecode VM, JIT,
// grounded ops) know when they're running inside a parallel worker and
// must surrender the EvalGuard at tier-return edges to let GC fire.
//
// Pre-H11, only the trampoline's per-4096-iter safepoint cadence ever
// surrendered ACTIVE_EVALUATORS. When a worker entered JIT/bytecode and
// stayed there for tens of milliseconds, the parent's cooperative-drop
// in the wait-loop (eval_loop.rs:1693+) saw ACTIVE_EVALUATORS stuck at
// >=1 — `maybe_quiescent_gc` requires 0 — and the convergence loop
// timed out. Under workloads with deep JIT/grounded spans (DeductionRevision,
// FlyingRaven, RavenInduction), GC starvation drove `committed_bytes` to
// 700×+ over threshold and the test budget expired.
//
// Tier-return hooks read this thread-local and call
// `worker_cooperative_safepoint(roots)` when GC is requested. The helper
// performs the same root-register / drop-guard / wait-quiescence /
// reacquire dance as `ParallelBranchContext::perform_safepoint`.

thread_local! {
    /// True iff the current thread is a parallel-branch worker. Set by
    /// `WorkerEvalScope::enter()` at the top of each worker closure;
    /// reset to false in the scope's Drop impl.
    pub(crate) static IS_PARALLEL_WORKER: Cell<bool> = const { Cell::new(false) };

    /// Phase 11.C (2026-05-17) — sample counter for
    /// `cache.record_execution` calls in the per-step hot path
    /// (`should_memoize_with_env` branch below). The per-step gate
    /// hashes the entire sub-expression tree (xxh3) plus a DashMap
    /// lookup, costing hundreds of nanoseconds per step. For
    /// PLN-style workloads with thousands of cold sub-expressions per
    /// inference, 100% sampling burns minutes on expressions that
    /// will never reach the tier-promotion threshold. Sampling at
    /// `SAMPLE_RATE = 32` (configurable via
    /// `METTATRON_EXEC_SAMPLE_RATE`) skips ~97% of these calls,
    /// matching the historical baseline before this gate was added.
    ///
    /// Skipping is HE-correctness-irrelevant: when sampled out, the
    /// trampoline falls through to the standard rule-dispatch path
    /// (instead of attempting bytecode dispatch on this sub-step).
    /// Tier promotion is delayed by `SAMPLE_RATE`× but eventually
    /// fires for steady-state hot expressions (mmverify, etc.). Cold
    /// sub-expressions that would never reach the threshold avoid the
    /// per-call cost entirely.
    static EXEC_SAMPLE_COUNTER: Cell<u64> = const { Cell::new(0) };
}

/// Phase 11.C — sample rate for `record_execution` in the per-step
/// hot path. Read once at process start from
/// `METTATRON_EXEC_SAMPLE_RATE` (default 32). Setting to 1 disables
/// sampling and restores the pre-Phase-11.C 100%-sampling behavior.
static EXEC_SAMPLE_RATE: OnceLock<u64> = OnceLock::new();

#[inline]
fn exec_sample_rate() -> u64 {
    *EXEC_SAMPLE_RATE.get_or_init(|| {
        std::env::var("METTATRON_EXEC_SAMPLE_RATE")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(32)
    })
}

/// Phase 11.C — returns `true` iff this thread's per-step
/// `record_execution` call should fire on this iteration. Counter
/// rolls over with `SAMPLE_RATE`; called from the per-step
/// `should_memoize_with_env` branch in the trampoline.
#[inline]
fn should_record_execution_sample() -> bool {
    let rate = exec_sample_rate();
    if rate <= 1 {
        return true;
    }
    EXEC_SAMPLE_COUNTER.with(|c| {
        let n = c.get().wrapping_add(1);
        c.set(n);
        n % rate == 0
    })
}

/// RAII guard that flips the thread-local `IS_PARALLEL_WORKER` flag for
/// the lifetime of a parallel-branch worker closure.
///
/// MUST drop BEFORE the EvalGuard so the flag is reset before the
/// thread can be reused for non-worker work (the work pool reuses
/// threads across tasks).
pub(crate) struct WorkerEvalScope {
    prior: bool,
}

impl WorkerEvalScope {
    #[inline]
    pub(crate) fn enter() -> Self {
        let prior = IS_PARALLEL_WORKER.with(|f| {
            let p = f.get();
            f.set(true);
            p
        });
        Self { prior }
    }
}

impl Drop for WorkerEvalScope {
    fn drop(&mut self) {
        IS_PARALLEL_WORKER.with(|f| f.set(self.prior));
    }
}

/// H11: cooperative safepoint for parallel-branch workers in non-trampoline
/// tiers. Called from JIT/bytecode/grounded tier-return edges when
/// `is_gc_requested()` returns true.
///
/// Surrenders the worker's EvalGuard so `maybe_quiescent_gc` can fire,
/// waits for the cycle to complete, then re-acquires the guard. Mirrors
/// the existing trampoline-safepoint protocol used by
/// `ParallelBranchContext::perform_safepoint`.
///
/// `extra_roots` are any tier-supplied "hot values" (e.g., the
/// MettaValue currently being processed) that the caller knows are
/// reachable but that the trampoline-level root walker cannot see while
/// the call is parked outside the trampoline loop. Frame-chain roots
/// are collected automatically.
///
/// CALLER OBLIGATIONS: every tier that hooks this MUST pass its full set
/// of live MettaValues as `extra_roots`. The bytecode VM has its operand
/// stack + locals + choice_points; the JIT runtime has its register file;
/// grounded ops have their pending result. Calling with `&[]` while
/// holding live unrooted values causes use-after-free when the next read
/// hits a freed slab slot. Tier-edge hooks are NOT yet wired pending each
/// tier's root-collection helper — reserved for a follow-up commit.
#[inline]
#[allow(dead_code)]
pub(crate) fn worker_cooperative_safepoint(extra_roots: &[MettaValue]) {
    use crate::backend::models::gc_allocator;

    // Fast path — no GC pressure, return immediately. One Relaxed load.
    if !gc_allocator::is_gc_requested() {
        return;
    }
    // Collect parent-class roots: walk the frame chain that this thread
    // entered through (matches what the trampoline safepoint registers).
    let mut roots: Vec<MettaValue> = Vec::with_capacity(extra_roots.len() + 16);
    crate::backend::eval::frame_chain::collect_frame_chain_roots(&mut roots);
    roots.extend_from_slice(extra_roots);

    clear_aba_sensitive_caches();
    // Phase 9: keep the temporary-root registration (the safepoint's
    // legitimate effect — exposes parent-class roots to mark-sweep) but
    // drop the EvalGuard dance. GC is purely async; the trampoline never
    // waits on quiescence. The `_root_handle` keeps the roots registered
    // until the caller's frame returns.
    let _root_handle = gc_allocator::register_temporary_roots(roots);
}

/// Clear all caches whose keys are slab pointers, before a GC sweep can run.
///
/// Without this, the slab GC may free a slot whose pointer is still cached
/// in one of these tables; if the slot is later reused by a new allocation
/// (ABA), cache lookups would alias the new value as the old one — silent
/// memory-safety violation in pointer-keyed data structures.
///
/// Called from every safepoint path that may trigger `maybe_quiescent_gc()`:
/// the trampoline's old-gen safepoint block, `ParallelBranchContext`'s
/// `perform_safepoint`, and the parent's cooperative drop in
/// `parallel_branch_eval`'s wait loop.
pub(crate) fn clear_aba_sensitive_caches() {
    // Thread-local MORK serialization caches.
    crate::backend::environment::rule_management::clear_mork_bytes_cache();
    crate::backend::mork_convert::clear_ground_fragment_cache();

    // Value hash cache — pointer-keyed.
    crate::backend::models::metta_value::clear_value_hash_cache();

    // Hash-consing table — entries reference slab pointers.
    crate::backend::models::gc_allocator::clear_hash_cons_table();

    // Operator dispatch cache — keyed by interned atom string pointers.
    clear_operator_cache();

    // Do not clear the normal-form bloom here. It is content-addressed and
    // records within-query semantic state for freeze-tuple/collapse handling;
    // query boundaries and rule mutations invalidate it explicitly.
}

static DROP_SENDER: OnceLock<std::sync::mpsc::Sender<Vec<SharedEnvArc>>> = OnceLock::new();

fn get_drop_sender() -> &'static std::sync::mpsc::Sender<Vec<SharedEnvArc>> {
    DROP_SENDER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<SharedEnvArc>>();
        std::thread::Builder::new()
            .name("mettatron-drop-worker".into())
            .spawn(move || {
                while let Ok(batch) = rx.recv() {
                    drop(batch);
                }
            })
            .expect("failed to spawn drop worker thread");
        tx
    })
}

use smallvec::{smallvec, SmallVec};
use tracing::trace;

use super::super::list_ops::substitute_variable_generic;
use super::super::processing::{process_collected_sexpr_generic, GenericProcessedSExpr};
use super::super::step::{eval_step_generic, GenericEvalStep};
use super::context::{EvalContext, MettaEnvironment, SharedEnv};
use super::engine::{
    apply_bindings, eval_switch, is_boolean_check_pattern, pattern_match,
    try_deferred_deterministic_chain, try_match_all_rules, DeferredChainResult, SwitchResult,
};
use super::types::{
    bv, bv_with, empty_shared_bindings, values_of, BoundValue, Continuation,
    EvalResult, SharedBindings, WorkItem,
};
use crate::backend::models::gc_allocator::RootProvider;

use crate::backend::eval::types::{
    extract_type_constraint, get_ground_type, infer_type_generic, is_pattern_type_compatible,
    types_match_generic, types_match_with_subtypes,
};
use crate::backend::grounded::{exec_error_to_value, execute_grounded_op, ExecError, GroundedWork};
use crate::backend::models::metta_value::is_variable_str;
use crate::backend::models::work_pool::global_eval_pool;
use crate::backend::models::{
    EvalGuard, GcFactory, GenericMultiplicityMatch, MettaValue, MettaValueFactory, MettaValueTrait,
    SpaceHandle,
};
use crate::backend::priority_scheduler::{priority_levels, TaskTypeId};

// Evaluation memoization and type-driven dispatch helpers extracted to `dispatch_hints`
// module for icache locality. Re-import the functions used in this file.
use super::dispatch_hints::{
    clear_operator_cache, collect_eval_memo_roots, collect_match_result_roots,
    derive_arg_expected_type, enter_fork_scope, eval_memo_get, eval_memo_put,
    increment_mutation_epoch, is_memoized_normal_form, is_normal_form_bounded, leave_fork_scope,
    memoize_normal_form, mutation_epoch, next_branch_scope, set_mutation_epoch,
    should_memoize_with_env,
};
use super::dispatch_hints::{is_embedded_kernel_op, is_reducible_head};
use super::engine::{try_deterministic_chain, try_match_rules_with_bindings};

// =============================================================================
// WPDS Context Hashing (Layer 3)
// =============================================================================
//
// Maps the top-k continuation frames to SchedulerStackSymbol values for
// context-aware weight refinement via the WPDS precomputed weight table.

/// Map a continuation discriminant to a SchedulerStackSymbol for WPDS context hashing.
///
/// Only inspects the variant tag (O(1)), not the contained data.
fn continuation_to_stack_symbol(
    cont: &Continuation,
) -> crate::backend::scheduler::wpds::SchedulerStackSymbol {
    use crate::backend::scheduler::wpds::SchedulerStackSymbol;

    match cont {
        Continuation::Done => SchedulerStackSymbol::Root,
        Continuation::ProcessRuleMatches { .. } | Continuation::ProcessRuleMatchesLazy { .. } => {
            SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 0,
            }
        }
        Continuation::ProcessGroundedOp { .. }
        | Continuation::ProcessGroundedOpFanout { .. }
        | Continuation::CollectFreezeArgs { .. } => SchedulerStackSymbol::GroundedOp,
        Continuation::ProcessCombinations { .. } => SchedulerStackSymbol::Combinations,
        Continuation::ProcessCombinationsBound { .. } => SchedulerStackSymbol::Combinations,
        Continuation::ProcessLet { .. } | Continuation::ProcessLetStar { .. } => {
            SchedulerStackSymbol::LetChain { depth: 0 }
        }
        Continuation::CollectSExpr { .. } | Continuation::CollectGroundedArg { .. } => {
            SchedulerStackSymbol::ArgEval { position: 0 }
        }
        Continuation::ProcessIfCondition { .. } => SchedulerStackSymbol::Conditional { branch: 0 },
        Continuation::ProcessCaseAtom { .. }
        | Continuation::ProcessCaseEvalScrutineeResults { .. } => SchedulerStackSymbol::CaseSwitch,
        Continuation::ProcessCollapseEvalResults { .. } => SchedulerStackSymbol::Collapse,
        Continuation::MemoizeResult { .. }
        | Continuation::CompleteSubgoal { .. }
        | Continuation::CompleteThunk { .. } => SchedulerStackSymbol::Memoize,
        _ => SchedulerStackSymbol::Root,
    }
}

/// Hash the top-3 continuation frames for WPDS context weight lookup.
///
/// Returns a 64-bit context hash suitable for the SchedulerAutomaton's
/// context_weight() method.
fn hash_continuation_context(continuations: &[Continuation]) -> u64 {
    let len = continuations.len();
    let limit = len.min(3);
    let mut packed = [0u32; 3];
    for i in 0..limit {
        // Top of stack is last element
        let idx = len - 1 - i;
        packed[i] = continuation_to_stack_symbol(&continuations[idx]).pack();
    }
    crate::backend::scheduler::wpds::hash_context_packed(&packed[..limit])
}

// =============================================================================
// Parallel Nondeterministic Branching
// =============================================================================
//
// MeTTa HE defines nondeterministic results as **unordered sets**. Evaluating
// branches in parallel and collecting results in any order produces semantically
// identical results. Programs needing ordering must use sequential combinators
// (`chain`, `let*`).

/// Maximum number of depth levels for per-depth budget quotas.
const MAX_DEPTH_LEVELS: usize = 8;

/// Per-depth budget quotas as percentage of total budget.
///
/// Phase 10.C (2026-05-17): smoothly decreasing schedule replacing the
/// previous `[50, 30, 15, 5, 0, 0, 0, 0]`. The old schedule actively
/// starved depths 4-7 even when worker capacity existed, but PLN.Derive
/// (and similar deeply-nested workloads) routinely fork at depths 4-7.
/// The new schedule
///
///   `[40, 25, 15, 8, 5, 4, 2, 1]`
///
/// keeps the bulk at shallow depths (where wider fan-outs are most
/// common) but never zeroes deeper levels — sums to 100%.
///
/// This replaces the exponential `4^(-depth)` decay with configurable
/// quotas, allowing inner forks to exploit more parallelism when outer
/// levels are idle.
const DEPTH_QUOTA_PERCENTS: [u32; MAX_DEPTH_LEVELS] = [40, 25, 15, 8, 5, 4, 2, 1];

/// Per-depth parallel branch budget quotas (Phase 3.6).
///
/// Each depth level has its own atomic budget counter, independently acquired
/// and released. This prevents shallow forks from exhausting all budget and
/// starving deeper levels.
struct DepthBudgets {
    /// Budget counters per depth level. Index = min(depth, MAX_DEPTH_LEVELS-1).
    quotas: [AtomicU32; MAX_DEPTH_LEVELS],
    /// Total budget across all levels (kept for future diagnostics; not
    /// currently consumed).
    #[allow(dead_code)]
    total: u32,
}

static DEPTH_BUDGETS: OnceLock<DepthBudgets> = OnceLock::new();

/// CPU-fanout budget for nondeterministic branch dispatch.
///
/// **Renamed 2026-05-15** from `MAX_PARALLEL_DEPTH` to clarify intent:
/// after the stack-safety trampolinization (Phases 1-5 of the mandate plan),
/// this cap bounds the **parallelism degree** of branch fan-out, NOT the
/// stack. C-stack depth in the parallel-dispatch path is now bounded by a
/// small constant independent of this knob.
///
/// Higher values increase potential CPU parallelism but contend for the
/// work-pool's fixed thread count; default `3` is a reasonable balance for
/// typical workloads. Set to `0` to disable parallel branching entirely
/// (forces all dispatch sequential).
///
/// Cached from `METTATRON_PARALLEL_FANOUT_DEPTH` (preferred name) with
/// fallback to `METTATRON_MAX_PARALLEL_DEPTH` for backwards compatibility.
static MAX_PARALLEL_DEPTH: OnceLock<u32> = OnceLock::new();

fn max_parallel_depth() -> u32 {
    *MAX_PARALLEL_DEPTH.get_or_init(|| {
        // Preferred name (post-2026-05-15 stack-safety refactor).
        std::env::var("METTATRON_PARALLEL_FANOUT_DEPTH")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            // Backwards-compat alias.
            .or_else(|| {
                std::env::var("METTATRON_MAX_PARALLEL_DEPTH")
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
            })
            // Phase 10.H (2026-05-17): default raised from 8 → u32::MAX
            // (effectively no cap). The per-depth quota schedule at
            // `DEPTH_QUOTA_PERCENTS` clamps to the deepest level for
            // depths ≥ MAX_DEPTH_LEVELS, so the hard cap was redundant
            // and forced deep-recursion workloads (PLN.Derive nests to
            // depth 17+) to lose ALL parallelism past depth 7. Now the
            // 1% quota at depth 7 governs everything from depth 7
            // upward, while the budget pool naturally throttles total
            // fan-out. Set `METTATRON_PARALLEL_FANOUT_DEPTH=8` to
            // restore the Phase 10.C cap.
            .unwrap_or(u32::MAX)
    })
}

/// Minimum number of nondeterministic branches to trigger parallel dispatch.
/// Below this threshold, branches are always evaluated sequentially (no WorkPool
/// overhead). Default: 4 — ensures 2-3 branch cases (like match-atom with 2
/// overlapping rules) stay sequential, avoiding Arc/mutex/condvar overhead for
/// branches where 97% are dead ends returning empty immediately.
static MIN_PARALLEL_BRANCHES: OnceLock<usize> = OnceLock::new();

fn min_parallel_branches() -> usize {
    *MIN_PARALLEL_BRANCHES.get_or_init(|| {
        std::env::var("METTATRON_MIN_PARALLEL_BRANCHES")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            // Phase 10.B (2026-05-17): default lowered from 4 → 2. PLN's
            // `(superpose ((case (|- $x $y) ...) (case (|- $y $x) ...)))`
            // pattern is exactly 2 branches; 4 forced it sequential. Set
            // `METTATRON_MIN_PARALLEL_BRANCHES=4` to restore old behavior.
            .unwrap_or(2)
    })
}

fn depth_budgets() -> &'static DepthBudgets {
    DEPTH_BUDGETS.get_or_init(|| {
        let cpus = num_cpus::get() as u32;
        // Phase 10.I (2026-05-17): total budget raised from cpus*2
        // → cpus*4 (capped at 128). The old `cpus*2` produced only
        // 8 slots on a 4-CPU machine, so the 1% quota at depth 7
        // floored at a single slot — serializing depth-7+ dispatches.
        // The factor of 4 is the empirical sweet spot: cpus*8 OOMed
        // Robot.metta within 13 s by allowing 240+ in-flight items,
        // while cpus*2 starved deep depths.
        let total = cpus.saturating_mul(4).min(128).max(16);

        // Initialize per-depth quotas. Can't use array init with AtomicU32
        // directly, so initialize each element.
        //
        // Phase 10.I (2026-05-17): floor lifted from 1 → 2 for active
        // depth levels — deep PLN.Derive iterations need at least
        // 2 concurrent dispatches per depth to overlap with the
        // sequential parent merge of the previous iteration.
        let quotas = std::array::from_fn(|i| {
            let pct = DEPTH_QUOTA_PERCENTS[i];
            let quota = (total * pct) / 100;
            AtomicU32::new(if pct > 0 { quota.max(2) } else { 0 })
        });

        DepthBudgets { quotas, total }
    })
}

/// Phase 10.F (2026-05-17) — per-cause rejection counters for
/// `try_acquire_budget`. Atomically incremented at the call site
/// when the gate fires. Use the snapshot helpers
/// `budget_rejection_*_count()` for telemetry / lint reports.
static BUDGET_REJ_QUEUE_PRESSURE: AtomicU64 = AtomicU64::new(0);
static BUDGET_REJ_DEPTH_QUOTA_EMPTY: AtomicU64 = AtomicU64::new(0);
static BUDGET_GRANTED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Snapshot of per-cause rejection / grant counters. Returned as
/// `(queue_pressure, depth_quota_empty, granted)`.
#[allow(dead_code)]
pub(crate) fn budget_counters_snapshot() -> (u64, u64, u64) {
    (
        BUDGET_REJ_QUEUE_PRESSURE.load(Ordering::Relaxed),
        BUDGET_REJ_DEPTH_QUOTA_EMPTY.load(Ordering::Relaxed),
        BUDGET_GRANTED_COUNT.load(Ordering::Relaxed),
    )
}

/// Phase 10.F (2026-05-17): queue-pressure factor, cached from
/// `METTATRON_QUEUE_PRESSURE_FACTOR`. Default loosened from the
/// historical `2` to `4` — under Phase 10.A-G the dispatch sites can
/// saturate the queue legitimately with short, deep tasks. The
/// factor of 4 is the empirical sweet spot: a factor of 8 OOMed
/// Robot.metta within 13 s, while 2 rejected useful dispatches.
/// Set to `2` to restore the pre-Phase-10 behavior.
static QUEUE_PRESSURE_FACTOR: OnceLock<u32> = OnceLock::new();

#[inline]
fn queue_pressure_factor() -> u32 {
    *QUEUE_PRESSURE_FACTOR.get_or_init(|| {
        std::env::var("METTATRON_QUEUE_PRESSURE_FACTOR")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|&n| n >= 1)
            .unwrap_or(4)
    })
}

/// Try to acquire N budget slots at the given nesting depth.
///
/// Phase 3.6: Uses per-depth quota system instead of exponential `4^(-depth)` decay.
/// Each depth level has its own budget pool, preventing shallow forks from
/// starving deeper levels.
///
/// Budget gate: when the work pool queue is saturated
/// (`queue_depth > active_workers * queue_pressure_factor()`), no
/// budget is granted. Factor defaulted 2 → MAX_DEPTH_LEVELS (8) in
/// Phase 10.F.
///
/// Returns actual slots acquired (0..=N).
fn try_acquire_budget(n: u32, depth: u32) -> u32 {
    // Phase 10.A — Stage 1e closure (2026-05-17):
    //
    // The historical `in_collapse_bind_scope() → 0` veto used to live
    // here. Rationale was: BINDING_CAPTURE_STACK is a thread-local, so
    // workers couldn't see the parent's tracked-variable set and
    // rule-match projection would observe an empty set and strip
    // bindings that the caller's collapse-bind expected to capture.
    //
    // That veto is no longer needed: all 5 callers of this function
    // (`dispatch_rule_matches`, `StartAmb`/superpose, `ProcessLet` body
    // fan-out, `ProcessCollapse`, `ProcessCollapseBind`) feed
    // `parallel_dispatch` / `parallel_collapse_dispatch`, both of which
    // now snapshot the parent's tracked-vars and re-establish them on
    // each worker via `WorkerCaptureScope::enter`. So
    // `in_collapse_bind_scope()` and `active_tracked_vars()` return the
    // right answers on the worker thread, and rule-match projection
    // sees the correct set.
    //
    // The veto stays out of the queue-pressure / quota path below — if
    // a future callsite is added that does NOT thread the hint, it
    // must spin its own veto at the callsite (the gate API stays
    // minimal).

    let budgets = depth_budgets();

    // Dynamic budget gate: check queue pressure.
    let pool = global_eval_pool();
    let queue_depth = pool.queue_len();
    let active = pool.active_workers();
    if active > 0 && queue_depth > active * queue_pressure_factor() as usize {
        BUDGET_REJ_QUEUE_PRESSURE.fetch_add(1, Ordering::Relaxed);
        return 0;
    }

    let level = (depth as usize).min(MAX_DEPTH_LEVELS - 1);
    let quota = &budgets.quotas[level];

    let mut current = quota.load(Ordering::Relaxed);
    loop {
        let granted = n.min(current);
        if granted == 0 {
            BUDGET_REJ_DEPTH_QUOTA_EMPTY.fetch_add(1, Ordering::Relaxed);
            return 0;
        }
        match quota.compare_exchange_weak(
            current,
            current - granted,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                BUDGET_GRANTED_COUNT.fetch_add(granted as u64, Ordering::Relaxed);
                return granted;
            }
            Err(actual) => current = actual,
        }
    }
}

/// Release N budget slots back to the given depth level.
fn release_budget(n: u32, depth: u32) {
    let budgets = depth_budgets();
    let level = (depth as usize).min(MAX_DEPTH_LEVELS - 1);
    budgets.quotas[level].fetch_add(n, Ordering::Release);
}

/// Dispatch nondeterministic rule matches either in parallel or sequentially.
///
/// Unified entry point for all code paths that produce `Vec<(V, GenericBindings<V>)>`
/// rule matches. Checks the 4 parallel gate conditions and either:
/// - **Parallel**: applies bindings to all matches, dispatches to work pool,
///   work-steals until complete, pushes Resume with merged results.
/// - **Sequential**: pops first match, pushes `ProcessRuleMatches` continuation
///   for remaining, pushes Eval for first branch's instantiated RHS.
///
/// Filter bindings to those that should propagate across fold iterations.
///
/// **Retained**: user-level bindings (not `$__fr_` prefixed) AND any
/// freshened bindings whose name appears as a free variable in
/// `propagate_keys`. The latter captures outer-rule freshened vars
/// (like `$__fr_7_b` from an enclosing rule's body) that are shared
/// across fold iterations via the fold's item list.
///
/// **Filtered out**: freshened bindings whose name does NOT appear in
/// `propagate_keys`. Those are the op's own per-invocation rule-match
/// bindings (e.g. Truth_ModusPonens's `$__fr_0_*` pattern vars) — they
/// are per-call state and DO NOT propagate between iterations.
///
/// MeTTaTron freshens rule variables ONCE at rule load time, so each
/// invocation of the same rule contributes identically-named freshened
/// bindings with potentially-different values. Composing an iteration
/// K's op-match bindings with iteration K-1's would produce spurious
/// ground/ground conflicts with no HE analogue (HE freshens per-
/// invocation, so equivalent names don't collide). The
/// `propagate_keys` set is the ITEMS' free variables — those are
/// shared across iterations by construction and must thread.
#[inline]
fn filter_fold_propagating_bindings(
    bindings: &crate::backend::models::GenericBindings<MettaValue>,
    propagate_keys: &[crate::backend::models::BindingName],
) -> crate::backend::models::GenericBindings<MettaValue> {
    let mut out = crate::backend::models::GenericBindings::new();
    for (name, val) in bindings.iter() {
        let is_user = !name.starts_with("$__fr_");
        let is_caller_freshened = propagate_keys.iter().any(|key| key.matches(name));
        if is_user || is_caller_freshened {
            out.insert_or_replace(name, val.clone());
        }
    }
    out
}

/// S14b (2026-05-14): HE-compatible format-args via dyn-fmt semantics.
/// `{}` consumed sequentially; `{N}` indexed for back-compat;
/// `{{` / `}}` escape literal braces; strings stripped of surrounding
/// quotes via `to_display_string` (matches HE's `atom_to_string`).
fn format_args_he(fmt: &str, args: &[&MettaValue]) -> String {
    let mut out = String::with_capacity(fmt.len());
    let mut iter = fmt.chars().peekable();
    let mut next_pos = 0usize;
    while let Some(c) = iter.next() {
        match c {
            '{' if iter.peek() == Some(&'{') => { iter.next(); out.push('{'); }
            '}' if iter.peek() == Some(&'}') => { iter.next(); out.push('}'); }
            '{' => {
                let mut idx_str = String::new();
                let mut closed = false;
                for nc in iter.by_ref() {
                    if nc == '}' { closed = true; break; }
                    idx_str.push(nc);
                }
                if !closed { out.push('{'); out.push_str(&idx_str); continue; }
                let idx = if idx_str.is_empty() {
                    let i = next_pos; next_pos += 1; i
                } else if let Ok(n) = idx_str.parse::<usize>() { n }
                else { out.push('{'); out.push_str(&idx_str); out.push('}'); continue; };
                if let Some(a) = args.get(idx) { out.push_str(&a.to_display_string()); }
            }
            _ => out.push(c),
        }
    }
    out
}

#[inline]
fn project_carrying_for_consumer(
    bindings: &SharedBindings,
    consumer: &MettaValue,
    tracked_vars: Option<&[&'static str]>,
    factory: &GcFactory,
) -> Option<SharedBindings> {
    if bindings.is_empty() {
        return Some(bindings.clone());
    }
    let projected = crate::backend::eval::bindings::project_bindings_for_consumer_generic(
        bindings,
        &[consumer],
        tracked_vars,
        factory,
    )?;
    if projected.is_empty() {
        Some(empty_shared_bindings())
    } else if projected == **bindings {
        Some(bindings.clone())
    } else {
        Some(std::sync::Arc::new(projected))
    }
}

#[inline]
fn project_owned_bindings_for_consumer(
    bindings: &crate::backend::models::GenericBindings<MettaValue>,
    consumer: &MettaValue,
    tracked_vars: Option<&[&'static str]>,
    factory: &GcFactory,
) -> Option<crate::backend::models::GenericBindings<MettaValue>> {
    crate::backend::eval::bindings::project_bindings_for_consumer_generic(
        bindings,
        &[consumer],
        tracked_vars,
        factory,
    )
}

/// # Precondition
///
/// `matches` must be non-empty. The caller must handle the empty-matches case
/// before calling this function.
/// Check if a value is already in normal form (no further evaluation possible).
///
/// A value is in normal form if:
/// 1. It has no variables (ground)
/// 2. It's not an S-expression whose head has user-defined rules
/// 3. It's not a special form (if, let, case, etc.)
///
/// This avoids pushing an unnecessary Eval work item for values that would
/// immediately return themselves from the trampoline.
#[inline]
fn dispatch_rule_matches<C: EvalContext>(
    mut matches: Vec<(
        MettaValue,
        crate::backend::models::GenericBindings<MettaValue>,
    )>,
    base_results: SmallVec<[BoundValue; 2]>,
    env: SharedEnv,
    depth: usize,
    ctx: &C,
    work_stack: &mut Vec<WorkItem>,
    continuations: &mut Vec<Continuation>,
    demand: Option<crate::backend::eval::cesk::coroutine::Demand>,
    outer_carrying: &crate::backend::models::GenericBindings<MettaValue>,
) {
    // Task #6 Phase 3 (2026-05-18): `env` arrives as `SharedEnv` (Arc-
    // wrapped) directly from the caller, instead of being passed by value
    // and re-Arc'd per call. Caller now does `Arc::clone(&env)` (refcount
    // bump) rather than `(*env).clone()` (full `MettaEnvironment` value
    // clone, which allocates a fresh PathBuf and other shallow fields).
    // For self-recursive rules like `(rec) → (rec)`, this eliminates the
    // per-iteration `Arc::new(MettaEnvironment)` allocation — Explore-
    // identified root cause #3 — preserving constant memory across the
    // recursion.

    debug_assert!(
        !matches.is_empty(),
        "dispatch_rule_matches called with empty matches"
    );

    // ── Single-match fast path ──
    // 93.3% of rule matches produce exactly 1 result. When there's exactly 1
    // match and no accumulated base_results, skip the ProcessRuleMatches
    // continuation entirely: no env clone, no VecDeque, no trace overhead.
    if matches.len() == 1 && base_results.is_empty() {
        let (rhs, bindings) = matches.pop().expect("matches has exactly 1 element");

        // Trace: RuleApplication (single match — no fork)
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let bindings_tv: Vec<(String, trace_format::TraceValue)> = bindings
                    .iter()
                    .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                    .collect();
                let trace_rhs = crate::backend::trace::trace_value_generic(&rhs);
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    trace_rhs.clone(),
                    vec![trace_rhs.clone()],
                    None,
                    trace_format::TraceEventKind::RuleApplication {
                        rule_lhs: trace_rhs.clone(),
                        rule_rhs: trace_rhs,
                        bindings: bindings_tv,
                        rule_span: None,
                    },
                );
            }
        }

        // Stage 1c: inside a collapse-bind scope, the single match still needs
        // its bindings composed onto the RHS result. We push a minimal
        // ProcessRuleMatches continuation (no remaining_matches, no actual
        // fork) that only performs the COMPOSE_MATCH step when the RHS result
        // bubbles back. Outside collapse-bind, this is skipped (zero overhead).
        // Stage 1d-revised: the shim's current_branch_bindings = compose of
        // outer_carrying (ambient from caller, e.g., CollectSExpr's merged
        // child bindings) with this match's bindings — so the RHS result
        // gets tagged with the UNION of both.
        if in_collapse_bind_scope() || !outer_carrying.is_empty() {
            let tracked_vars_hint = active_tracked_vars().map(std::sync::Arc::new);
            // Phase 2.B Issue #5 fix: strict compose — conflict between
            // outer_carrying and this match's bindings means the branch is
            // inconsistent. Emit zero results and return (HE-bisimilar
            // silent pruning). The old unchecked compose produced empty
            // bindings that attached to the RHS → ghost branch downstream.
            let composed = if outer_carrying.is_empty() {
                bindings.clone()
            } else {
                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                    outer_carrying,
                    &bindings,
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => {
                        work_stack.push(WorkItem::Resume {
                            result: (base_results, env),
                        });
                        return;
                    }
                }
            };
            let current_branch_bindings = std::sync::Arc::new(composed);
            continuations.push(Continuation::ProcessRuleMatches {
                remaining_matches: Vec::new().into_iter(),
                results: Vec::new(),
                env: env.clone(),
                depth,
                pre_fork_epoch: mutation_epoch(),
                pre_fork_gen: 0, // not entering a fork scope — no CP to restore
                fork_depth: 0,   // no fork — cut targeting irrelevant
                current_branch_bindings,
                outer_carrying: std::sync::Arc::new(outer_carrying.clone()),
                tracked_vars_hint,
                #[cfg(feature = "trace")]
                branch_span_id: 0,
                #[cfg(feature = "trace")]
                branch_start_ns: 0,
                #[cfg(feature = "trace")]
                branch_index: 0,
                #[cfg(feature = "trace")]
                total_branches: 1,
                // H7 Stage 1: shim path is NOT a real fork — suppress BranchEnd.
                #[cfg(feature = "trace")]
                is_real_fork: false,
            });
        }

        // Stage 1d-revised + HE-faithful rule-match propagation (split by RHS shape):
        //
        //   Ground RHS (`!rhs.has_variables_fast()`):
        //     The rule's `bindings` contains unifications like `$b=c` where
        //     `$b` is a CALLER-LEVEL variable bound to a rule-LHS literal.
        //     Propagate bindings composed with outer_carrying so enclosing
        //     handlers (`ProcessLet` etc.) can observe the caller-level
        //     binding and thread it through subsequent expressions.
        //     Freshened rule-body vars (`$__fr_*`) are filtered out to
        //     prevent unbounded accumulation along deep call chains.
        //
        //   Variable RHS (`rhs.has_variables_fast()`):
        //     `bindings` is applied via `EvalWithBindings` template
        //     substitution (line 440-448). Pass only `outer_carrying` as
        //     ambient — duplicating bindings would grow carrying linearly
        //     per level (mmverify has ~200 nested dispatches → quadratic).
        //
        //   Collapse-bind scope: compose ALL bindings; sidecar needs them.
        //
        // Phase 2.B Issue #5 fix: strict compose. If outer_carrying and
        // bindings conflict (e.g., outer says $a=b, match says $a=a),
        // this rule match is inconsistent with the caller's context —
        // emit zero results and return (HE-bisimilar silent pruning).
        let rhs_carrying: crate::backend::models::GenericBindings<MettaValue> =
            if outer_carrying.is_empty() && bindings.is_empty() {
                crate::backend::models::GenericBindings::new()
            } else if in_collapse_bind_scope() {
                if outer_carrying.is_empty() {
                    bindings.clone()
                } else if bindings.is_empty() {
                    outer_carrying.clone()
                } else {
                    match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                        outer_carrying,
                        &bindings,
                        ctx.factory(),
                    ) {
                        Some(b) => b,
                        None => {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                            return;
                        }
                    }
                }
            } else {
                if outer_carrying.is_empty() && bindings.is_empty() {
                    crate::backend::models::GenericBindings::new()
                } else if outer_carrying.is_empty() {
                    bindings.clone()
                } else if bindings.is_empty() {
                    outer_carrying.clone()
                } else {
                    match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                        outer_carrying,
                        &bindings,
                        ctx.factory(),
                    ) {
                        Some(b) => b,
                        None => {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                            return;
                        }
                    }
                }
            };
        // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings.
        // When RHS has no variables, push as Eval directly (O(1) pointer copy).
        //
        // Task #6 Phase 2 (2026-05-18): Mark rule-RHS evaluations as
        // tail-calls. A user-defined rule's RHS is in tail position
        // relative to its caller — the dispatch returns directly to
        // whoever invoked the rule's head, with no intervening work.
        // The `is_tail_call: true` flag is read by Phase 3's TCO-aware
        // push to collapse self-recursive `Eval`s into the surrounding
        // continuation instead of growing the work_stack.
        //
        // Task #6 Phase 3a (2026-05-18): per-iter `Arc::new(rhs_carrying)`
        // is the Explore-identified leak source for `(rec) → (rec)`
        // self-recursion. When `rhs_carrying` is the Empty variant
        // (the overwhelmingly common case for pure-fact-style rules
        // with no outer carrying context), reuse the cached
        // `empty_shared_bindings()` Arc instead of allocating a fresh
        // one each iteration. Refcount bump replaces heap allocation.
        let rhs_carrying_arc = if rhs_carrying.is_empty() {
            crate::backend::eval::trampoline::types::empty_shared_bindings()
        } else {
            std::sync::Arc::new(rhs_carrying)
        };
        // Task #6 Phase 4 (2026-05-18): mirror the `rhs_carrying_arc` cache
        // for `bindings` itself. Self-recursive rules with no match bindings
        // (e.g. `(= (rec) (rec))`) reach this push with `bindings.is_empty()
        // == true` every iteration; reusing the empty Arc replaces a fresh
        // heap allocation with a refcount bump on the hot recursion path.
        let bindings_arc = if bindings.is_empty() {
            crate::backend::eval::trampoline::types::empty_shared_bindings()
        } else {
            std::sync::Arc::new(bindings)
        };
        if rhs.has_variables_fast() {
            work_stack.push(WorkItem::EvalWithBindings {
                template: rhs,
                bindings: bindings_arc,
                env,
                depth: depth + 1,
                is_tail_call: true,
                expected_type: None,
                carrying_bindings: rhs_carrying_arc,
            });
        } else {
            // Normal-form short-circuit for ground RHS
            if is_memoized_normal_form(&rhs) {
                work_stack.push(WorkItem::Resume {
                    result: (
                        smallvec![bv_with(rhs, (*rhs_carrying_arc).clone())],
                        env,
                    ),
                });
            } else if is_normal_form_bounded(&rhs, &*env, 2) {
                memoize_normal_form(&rhs);
                work_stack.push(WorkItem::Resume {
                    result: (
                        smallvec![bv_with(rhs, (*rhs_carrying_arc).clone())],
                        env,
                    ),
                });
            } else {
                // Phase F: Tight deterministic chain — if the ground RHS is itself
                // a deterministic operator, chain through without pushing to the
                // work stack. This eliminates trampoline pop/dispatch/push overhead
                // for chains of deterministic user-defined functions.
                if let Some(chained) = try_deterministic_chain(&rhs, &*env, ctx.factory()) {
                    // The chain resolved one or more steps. The result still needs
                    // evaluation (may be a special form, nondeterministic, etc.)
                    work_stack.push(WorkItem::Eval {
                        value: chained,
                        env,
                        depth: depth + 1,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: rhs_carrying_arc,
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env,
                        depth: depth + 1,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: rhs_carrying_arc,
                    });
                }
            }
        }
        return;
    }

    // ── I-15: Lazy demand-driven dispatch via BranchCoroutine ──
    // When demand is explicitly set to non-All, evaluate branches one-at-a-time
    // via ProcessRuleMatchesLazy, stopping when demand is satisfied.
    // Default is All (full nondeterministic evaluation) — callers opt in to
    // pruning by setting demand on their WorkItem::Eval.
    let effective_demand = demand.unwrap_or(crate::backend::eval::cesk::coroutine::Demand::All);
    if !effective_demand.is_all() && matches.len() > 1 {
        let mut coroutine =
            crate::backend::eval::cesk::coroutine::BranchCoroutine::new(matches, effective_demand);
        // BranchCoroutine with non-empty matches always has at least one branch.
        let (rhs, bindings) = coroutine
            .next_branch()
            .expect("BranchCoroutine::new with non-empty matches must have first branch");
        // Push the lazy continuation to collect results incrementally.
        // Stage 1c: stash this branch's match bindings so incoming sub-eval
        // results get composed with them (per-branch provenance).
        // Stage 1d-revised: compose with outer_carrying (caller's ambient).
        // Phase 2.B Issue #5 fix: strict compose — drop branch on conflict.
        let current_branch_bindings = std::sync::Arc::new(if outer_carrying.is_empty() {
            bindings.clone()
        } else {
            match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                outer_carrying,
                &bindings,
                ctx.factory(),
            ) {
                Some(b) => b,
                None => {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), env),
                    });
                    return;
                }
            }
        });
        let tracked_vars_hint = active_tracked_vars().map(std::sync::Arc::new);
        continuations.push(Continuation::ProcessRuleMatchesLazy {
            coroutine: Box::new(coroutine),
            results: base_results.into_vec(),
            env: env.clone(),
            depth,
            current_branch_bindings,
            outer_carrying: std::sync::Arc::new(outer_carrying.clone()),
            tracked_vars_hint,
        });
        // Stage 1d-revised: Lazy first branch RHS carrying = compose(outer, match).
        // Reuse the same strict-composed value (already validated non-conflict above).
        let lazy_carrying: crate::backend::models::GenericBindings<MettaValue> =
            if outer_carrying.is_empty() && !in_collapse_bind_scope() {
                crate::backend::models::GenericBindings::new()
            } else if outer_carrying.is_empty() {
                bindings.clone()
            } else {
                // Already composed above for current_branch_bindings; re-derive
                // locally (same strict compose — can't fail here since we got
                // past the first compose, but keep the safety check).
                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                    outer_carrying,
                    &bindings,
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => crate::backend::models::GenericBindings::new(),
                }
            };
        // Evaluate the first branch.
        // Task #6 Phase 2 (2026-05-18): rule-RHS dispatch is in tail
        // position relative to the rule's caller — applies to both the
        // single-match fast path (above) and this multi-match shim
        // (each branch is independently tail-called by its caller).
        //
        // Task #6 Phase 4 (2026-05-18): same empty-Arc reuse on the
        // multi-match first-branch path. Only compute the cached Arc
        // inside the variable-RHS branch — the else branch borrows
        // `bindings` (apply_bindings).
        if rhs.has_variables_fast() {
            let bindings_arc = if bindings.is_empty() {
                crate::backend::eval::trampoline::types::empty_shared_bindings()
            } else {
                std::sync::Arc::new(bindings)
            };
            work_stack.push(WorkItem::EvalWithBindings {
                template: rhs,
                bindings: bindings_arc,
                env,
                depth: depth + 1,
                is_tail_call: true,
                expected_type: None,
                carrying_bindings: std::sync::Arc::new(lazy_carrying),
            });
        } else {
            work_stack.push(WorkItem::Eval {
                value: apply_bindings(&rhs, &bindings, ctx.factory()),
                env,
                depth: depth + 1,
                is_tail_call: true,
                expected_type: None,
                demand: None,
                carrying_bindings: std::sync::Arc::new(lazy_carrying),
            });
        }
        return;
    }

    // ── Parallel nondeterministic branching gate ──
    // Uses the WFST transduction table's parallelism_degree to decide whether
    // branches justify parallel dispatch. Cheap branches (GroundCheap,
    // SymbolicCheap, etc.) have degree=1 and evaluate sequentially inline.
    // Only SymbolicModerate (degree=4) and ParallelPure (degree=8) get dispatched.
    // All forks go through try_acquire_budget — no unconditional bypass.
    let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());

    let wfst_allows_parallel = if matches.len() >= min_parallel_branches() {
        let scheduler = crate::backend::scheduler::global_scheduler();
        let degree_ok = matches.iter().any(|(rhs, _)| {
            let (_, action) = scheduler.classify_and_transduce(rhs);
            action.parallelism_degree > 1
        });
        // H2 (2026-05-05): branch-purity gate per spec §5.6.1 [N, sub-profile ST].
        // Side-effecting branches (containing add-atom/remove-atom/change-state!/
        // bind!/...) must serialize to preserve HE branch-ordering semantics.
        // mmverify's filter'/assign_f_hyp_to_var race demonstrated the corruption.
        //
        // Phase 10.D (2026-05-17): switched from `body_contains_impure` to
        // `body_blocks_parallel_dispatch`, which only blocks on
        // `STATE_MUTATING_HEADS` by default. `IO_HEADS` (println!/print!/
        // trace!) no longer force serialization, so PLN.Derive's
        // `(progn (println! ...) (PLN.Derive ...))` body is now parallel-
        // eligible. Set `METTATRON_STRICT_PRINT_ORDER=1` to restore the
        // historical wide veto if exact print order is required.
        let all_pure = matches.iter().all(|(rhs, _)| {
            !crate::backend::scheduler::classification::body_blocks_parallel_dispatch(rhs, 8)
        });
        degree_ok && all_pure
    } else {
        false
    };

    let budget = if wfst_allows_parallel
        && current_depth < max_parallel_depth()
        && global_eval_pool().active_workers() > 0
    {
        try_acquire_budget((matches.len() - 1) as u32, current_depth)
    } else {
        0
    };

    if budget > 0 {
        // ── Parallel path: dispatch all branches to work pool ──
        let factory = ctx.factory();
        let branch_values: Vec<MettaValue> = matches
            .into_iter()
            .map(|(rhs, bindings)| {
                if rhs.has_variables_fast() {
                    apply_bindings(&rhs, &bindings, factory)
                } else {
                    rhs.clone()
                }
            })
            .collect();

        let metta_env = (*env).clone();

        // Trace: NondeterministicFork (parallel)
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: branch_values.len() as u32,
                    },
                );
            }
        }

        // Phase 4c: Nondeterministic fork → pre-seed tiered cache for each branch.
        // Fork branches are "hot by definition" — pre-seeding eliminates warmup delay.
        {
            let cache = crate::backend::bytecode::tiered_cache::global_tiered_cache();
            for branch in &branch_values {
                cache.preseed_for_immediate_compile(branch.hash_value());
            }
        }

        // Budget was acquired from try_acquire_budget — pass it to parallel_branch_eval
        // for release on completion.
        let actual_budget_acquired = budget;

        // WPDS Layer 3: compute continuation context hash for context-aware scheduling
        let ctx_hash = hash_continuation_context(continuations);
        CONTINUATION_CONTEXT_HASH.with(|h| h.set(ctx_hash));

        // Demand propagation: rule-match dispatch always runs all branches
        // by default (the trampoline's caller decides demand). The `demand`
        // parameter on dispatch_rule_matches threads the caller's demand
        // through; default is `Demand::All`.
        let dispatch_demand = demand.unwrap_or(crate::backend::eval::cesk::coroutine::Demand::All);
        let branches: Vec<ParallelBranch> = branch_values
            .into_iter()
            .map(|branch| (branch, empty_shared_bindings()))
            .collect();

        // **Stack-safety mandate (2026-05-15)**: dispatch is non-blocking;
        // the wait + merge happens in the trampolinized `WaitForParallel`
        // arm. See feedback-stack-safety-mandate in user memory.
        // Phase 8: build the Arc once, share with both `parallel_dispatch`
        // (for the per-dispatch RootProvider) and
        // `stable_branches_snapshot` (for the continuation).
        let stable_branches_snapshot = std::sync::Arc::new(branches);
        let handle = parallel_dispatch(
            std::sync::Arc::clone(&stable_branches_snapshot),
            metta_env,
            actual_budget_acquired,
            current_depth,
            dispatch_demand,
        );
        let outer_carrying_arc: crate::backend::eval::trampoline::types::SharedBindings =
            std::sync::Arc::new(outer_carrying.clone());
        let env_for_resume = env.clone();
        continuations.push(Continuation::WaitForParallel {
            handle,
            merge_mode: crate::backend::eval::trampoline::types::ParallelMergeMode::RuleMatch,
            base_results,
            outer_carrying: outer_carrying_arc,
            env,
            depth,
            budget_acquired: actual_budget_acquired,
            caller_depth: current_depth,
            stable_branches_snapshot,
        });
        // Dummy Resume to fire the WaitForParallel arm on the next trampoline tick.
        work_stack.push(WorkItem::Resume {
            result: (SmallVec::new(), env_for_resume),
        });
    } else {
        // ── Sequential path: consume first match, push ProcessRuleMatches for rest ──
        let mut remaining_iter = matches.into_iter();
        let _total_branches = (remaining_iter.len() + 1) as u32; // +1 matches existing trace convention
        let (rhs, bindings) = remaining_iter.next().expect("matches is non-empty");

        // collapse-bind: capture tracked variable bindings from first match.
        capture_bindings_if_active(&bindings);

        // Trace: NondeterministicFork + BranchStart for first branch
        #[cfg(feature = "trace")]
        let _branch_span_id = {
            if let Some(tc) = ctx.trace_collector() {
                if _total_branches > 1 {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::NondeterministicFork {
                            branch_count: _total_branches,
                        },
                    );
                }
                let span_id = tc.next_span_id();
                let start_ns = tc.elapsed_ns();
                tc.emit_timed(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::BranchStart {
                        branch_index: 0,
                        total_branches: _total_branches,
                    },
                    start_ns,
                    None,
                    Some(span_id),
                );
                span_id
            } else {
                0u64
            }
        };

        let pre_fork_gen = enter_fork_scope();
        let fork_depth = enter_fork();
        // Stage 1c: stash first branch's match_bindings + tracked_vars hint.
        // Stage 1d-revised: compose outer_carrying (ambient from the caller's
        // sexpr construction) with this branch's match bindings so the RHS
        // result inherits the full ancestry.
        let current_branch_bindings = std::sync::Arc::new(if outer_carrying.is_empty() {
            bindings.clone()
        } else {
            crate::backend::eval::bindings::compose_outer_inner_generic(
                outer_carrying,
                &bindings,
                ctx.factory(),
            )
        });
        let tracked_vars_hint = active_tracked_vars().map(std::sync::Arc::new);
        continuations.push(Continuation::ProcessRuleMatches {
            remaining_matches: remaining_iter,
            results: base_results.into_vec(),
            env: env.clone(),
            depth,
            pre_fork_epoch: mutation_epoch(),
            pre_fork_gen,
            fork_depth,
            current_branch_bindings,
            outer_carrying: std::sync::Arc::new(outer_carrying.clone()),
            tracked_vars_hint,
            #[cfg(feature = "trace")]
            branch_span_id: _branch_span_id,
            #[cfg(feature = "trace")]
            branch_start_ns: { ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0) },
            #[cfg(feature = "trace")]
            branch_index: 0,
            #[cfg(feature = "trace")]
            total_branches: _total_branches,
            // H7 Stage 1: real fork (paired with BranchStart at line ~853)
            #[cfg(feature = "trace")]
            is_real_fork: true,
        });

        // Trace: RuleApplication (first match)
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let bindings_tv: Vec<(String, trace_format::TraceValue)> = bindings
                    .iter()
                    .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                    .collect();
                let trace_rhs = crate::backend::trace::trace_value_generic(&rhs);
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    trace_rhs.clone(),
                    vec![trace_rhs.clone()],
                    None,
                    trace_format::TraceEventKind::RuleApplication {
                        rule_lhs: trace_rhs.clone(),
                        rule_rhs: trace_rhs,
                        bindings: bindings_tv,
                        rule_span: None,
                    },
                );
            }
        }

        // Stage 1d-revised: compose outer_carrying with this branch's match
        // bindings so the first branch's RHS result carries the full ancestry.
        //
        // Phase 2.B Issue #5 fix: use strict compose. On conflict (outer vs
        // match bindings), emit zero for this match and fall through to the
        // next. This is the SEQUENTIAL-first-branch path — the ProcessRuleMatches
        // continuation has already been pushed, so emitting empty Resume
        // advances to the next branch via the continuation's iterator.
        let seq_carrying: crate::backend::models::GenericBindings<MettaValue> =
            if outer_carrying.is_empty() && !in_collapse_bind_scope() {
                crate::backend::models::GenericBindings::new()
            } else if outer_carrying.is_empty() {
                bindings.clone()
            } else {
                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                    outer_carrying,
                    &bindings,
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => {
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env),
                        });
                        return;
                    }
                }
            };
        // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings
        if rhs.has_variables_fast() {
            work_stack.push(WorkItem::EvalWithBindings {
                template: rhs,
                bindings: std::sync::Arc::new(bindings),
                env,
                depth,
                is_tail_call: true,
                expected_type: None,
                carrying_bindings: std::sync::Arc::new(seq_carrying),
            });
        } else {
            if is_memoized_normal_form(&rhs) {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(rhs, seq_carrying)], env),
                });
            } else if is_normal_form_bounded(&rhs, &*env, 2) {
                memoize_normal_form(&rhs);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(rhs, seq_carrying)], env),
                });
            } else {
                work_stack.push(WorkItem::Eval {
                    value: rhs,
                    env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: std::sync::Arc::new(seq_carrying),
                });
            }
        }
    }
}

// Thread-local depth counter for parallel branch evaluation.
//
// Tracks the current nesting depth of `parallel_branch_eval` on this thread.
// At depth >= `MAX_PARALLEL_DEPTH`, the gate falls through to the sequential
// path. Budget slots are scaled by `4^(-depth)` at each nesting level to
// prevent exponential thread explosion while allowing inner forks to exploit
// available parallelism.
thread_local! {
    static PARALLEL_BRANCH_DEPTH: Cell<u32> = const { Cell::new(0) };

    /// Current evaluation demand level on this thread.
    ///
    /// Set when the trampoline pops a `WorkItem::Eval { demand: Some(d), .. }`,
    /// reset when that work item completes. Read by parallel-dispatch sites
    /// (`dispatch_rule_matches` for rule fanout, StartAmb for superpose) to
    /// decide whether to spawn a `CancelToken` that lets the first satisfying
    /// branch terminate its siblings.
    ///
    /// When a cardinality-multiplying form is entered (collapse, collapse-bind,
    /// superpose-bind, case), the demand is shadowed to `Demand::All` for the
    /// duration of that subtree. The shadow is restored when the form's
    /// continuation resumes.
    ///
    /// `None` = inherit-from-default (= `Demand::All`). `Some(d)` = bounded.
    static CURRENT_DEMAND: Cell<Option<crate::backend::eval::cesk::coroutine::Demand>> =
        const { Cell::new(None) };

    /// WPDS continuation context hash for the current parallel dispatch point.
    /// Set by `dispatch_rule_matches` before calling `parallel_branch_eval`,
    /// read by `parallel_branch_eval` to compute context-aware effective priority.
    static CONTINUATION_CONTEXT_HASH: Cell<u64> = const { Cell::new(0) };

    /// Prolog-style cut depth. When `(cut)` is evaluated inside a rule's RHS,
    /// this is set to the current fork depth. The `ProcessRuleMatches`
    /// continuation at the matching fork depth consumes the signal and
    /// discards remaining alternative matches.
    ///
    /// Value of 0 means "no cut active". Values > 0 indicate the fork depth
    /// at which cut should fire. This depth-awareness prevents nested
    /// `dispatch_rule_matches` calls from accidentally consuming the cut
    /// signal meant for an outer dispatch.
    static CUT_TARGET_DEPTH: Cell<u32> = const { Cell::new(0) };

    /// Current nondeterministic fork depth — incremented when entering a
    /// `dispatch_rule_matches` with 2+ matches, decremented when the
    /// corresponding `ProcessRuleMatches` continuation completes.
    static FORK_DEPTH: Cell<u32> = const { Cell::new(0) };
}

struct DemandScope {
    previous: Option<crate::backend::eval::cesk::coroutine::Demand>,
}

impl DemandScope {
    #[inline]
    fn enter(demand: crate::backend::eval::cesk::coroutine::Demand) -> Self {
        let previous = CURRENT_DEMAND.with(|current| {
            let previous = current.get();
            current.set(Some(demand));
            previous
        });
        Self { previous }
    }
}

impl Drop for DemandScope {
    #[inline]
    fn drop(&mut self) {
        CURRENT_DEMAND.with(|current| current.set(self.previous));
    }
}

// ── Binding Capture for collapse-bind ─────────────────────────────────
//
// When a `collapse-bind` evaluation is active, the thread-local capture
// stack records variable bindings at nondeterministic branch points.
// This is opt-in: zero overhead when no `collapse-bind` is in progress
// (single `RefCell::borrow` + `Vec::is_empty` check).
//
// Architecture: per-branch capture with inverse mapping.
//
// Layer 1 (inverse map): At the top-level dispatch for the collapse-bind
// expression, rule-var bindings are {$fr_a → $who} (rule var keys). The
// inverse map records {$who → $fr_a} so that when $fr_a is later resolved
// to a ground value, we can populate $who's binding.
//
// Layer 2 (per-result snapshots): Each nondeterministic result gets its
// own binding snapshot. When ProcessRuleMatches accumulates results from
// a completed branch, the current bindings are snapshotted for each result.
//
// Layer 3 (direct + indirect capture): At each dispatch_rule_matches,
// bindings are checked for: (a) direct tracked-var keys with ground values,
// (b) rule-var keys that map to tracked vars via the inverse map.

/// Scope marker for an active `collapse-bind` evaluation.
///
/// Stage 1b+: the frame is minimal — it only records WHICH variables are
/// being tracked and the fork depth at scope entry. Per-branch bindings
/// are NOT stored here; they travel with each `BoundValue` via its `.1`
/// (MeTTa-HE-faithful propagation, see `Continuation::ProcessRuleMatches.
/// current_branch_match_bindings`).
///
/// The old fields (`current_bindings`, `per_result_bindings`, `inverse_map`)
/// were removed: they encoded shared-mutable state that couldn't correctly
/// distinguish sibling branches at nested fork depths. Per-branch bindings
/// now flow via the BoundValue pipeline instead.
struct BindingCaptureFrame {
    /// Free variable names from the original `collapse-bind` expression.
    /// Used to project match bindings to just the variables the caller
    /// cares about (keeps carried bindings small).
    tracked_vars: SmallVec<[&'static str; 4]>,
    /// Fork depth at which the collapse-bind was entered (kept for debug
    /// and future depth-aware cancellation logic; not currently consumed).
    #[allow(dead_code)]
    collapse_fork_depth: u32,
}

thread_local! {
    /// Stack of scope markers for nested `collapse-bind` calls.
    /// Empty when no `collapse-bind` is active.
    static BINDING_CAPTURE_STACK: RefCell<Vec<BindingCaptureFrame>> = const { RefCell::new(Vec::new()) };

    /// Monotonic counter for per-Resume-boundary `flow_id`s used by
    /// eval-trace binding-flow instrumentation. Enter/Emit/Dropped/
    /// ExitNoResume events for the same boundary share a flow_id.
    #[cfg(feature = "trace")]
    static FLOW_ID_COUNTER: Cell<u64> = const { Cell::new(0) };
}

#[cfg(feature = "trace")]
fn next_flow_id() -> u64 {
    FLOW_ID_COUNTER.with(|c| {
        let id = c.get();
        c.set(id.wrapping_add(1));
        id
    })
}

/// Push a new scope marker when entering `collapse-bind`.
/// `tracked_vars` are the free variable names from the inner expression.
fn push_binding_capture_frame(tracked_vars: SmallVec<[&'static str; 4]>) {
    let fork_depth = FORK_DEPTH.with(|c| c.get());
    BINDING_CAPTURE_STACK.with(|stack| {
        stack.borrow_mut().push(BindingCaptureFrame {
            tracked_vars,
            collapse_fork_depth: fork_depth,
        });
    });
}

/// Pop and return the top scope marker when `collapse-bind` completes.
fn pop_binding_capture_frame() -> Option<BindingCaptureFrame> {
    BINDING_CAPTURE_STACK.with(|stack| stack.borrow_mut().pop())
}

/// Returns true iff at least one `collapse-bind` is active on this thread.
/// Called at rule-dispatch sites to decide whether to filter match bindings.
#[inline]
fn in_collapse_bind_scope() -> bool {
    BINDING_CAPTURE_STACK.with(|stack| !stack.borrow().is_empty())
}

/// Phase 10.A — RAII guard that pushes a shadow `BindingCaptureFrame` on
/// worker entry and pops it on drop (normal exit AND panic-unwind).
///
/// Workers spawn on different threads and therefore have empty
/// `BINDING_CAPTURE_STACK` by default. Without this shadow frame,
/// `in_collapse_bind_scope()` and `active_tracked_vars()` would return
/// the wrong answers — workers would think they're outside any
/// collapse-bind, strip bindings they were meant to track, and produce
/// wrong rule-match projections.
///
/// The shadow frame records only the `tracked_vars` set (the parent's
/// union). It does NOT carry any mutation hooks back to the parent's
/// stack — workers cannot mutate the parent's captures. They only
/// READ via `active_tracked_vars()`.
///
/// Use: `let _scope = WorkerCaptureScope::enter(handle.tracked_vars_hint.clone());`
pub(crate) struct WorkerCaptureScope {
    pushed: bool,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl WorkerCaptureScope {
    pub(crate) fn enter(hint: Option<Arc<SmallVec<[&'static str; 4]>>>) -> Self {
        let pushed = if let Some(vars) = hint {
            // Clone the SmallVec out of the Arc — the worker pushes its
            // OWN frame, not a reference to the parent's. This keeps the
            // worker's frame independent and panic-safe.
            push_binding_capture_frame((*vars).clone());
            true
        } else {
            false
        };
        Self { pushed, _not_send: std::marker::PhantomData }
    }
}

impl Drop for WorkerCaptureScope {
    fn drop(&mut self) {
        if self.pushed {
            // SAFETY: we pushed exactly one frame on entry; pop it now.
            // If the worker pushed additional frames during eval (e.g.,
            // nested collapse-bind), those must have been popped before
            // this guard runs (RAII ordering).
            let _ = pop_binding_capture_frame();
        }
    }
}

/// Union of tracked variables across all active collapse-bind frames on
/// this thread. Returned as a sorted-deduped `SmallVec`. Used to project
/// match bindings at dispatch sites to just the relevant set.
///
/// Returns `None` when no collapse-bind is active (the caller should skip
/// filtering entirely for zero overhead on the hot path).
fn active_tracked_vars() -> Option<SmallVec<[&'static str; 4]>> {
    BINDING_CAPTURE_STACK.with(|stack| {
        let stack = stack.borrow();
        if stack.is_empty() {
            return None;
        }
        let mut out: SmallVec<[&'static str; 4]> = SmallVec::new();
        for frame in stack.iter() {
            for &v in frame.tracked_vars.iter() {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
        }
        Some(out)
    })
}

/// Stage 1b no-op: capture logic was removed. Retained as a stub to
/// minimize churn at dispatch sites during the migration. Per-branch
/// bindings now flow via `ProcessRuleMatches.current_branch_match_bindings`
/// (Stage 1c).
#[inline]
fn capture_bindings_if_active(
    _match_bindings: &crate::backend::models::GenericBindings<MettaValue>,
) {
}

/// Encode bindings as an S-expression: `(Bindings ($var val) ...)`.
/// Used by collapse-bind to pair each result with its captured bindings.
///
/// Trampoline-tier wrapper — forwards to the generic implementation in
/// [`crate::backend::eval::bindings::encode_bindings_as_sexpr_generic`].
/// The bytecode VM (Phase C) and JIT (Phase D) use the generic function
/// directly, ensuring a single encoding rule across all three tiers.
pub(crate) fn encode_bindings_as_sexpr(
    bindings: &crate::backend::models::GenericBindings<MettaValue>,
    factory: &crate::backend::models::gc_allocator::GcFactory,
) -> MettaValue {
    crate::backend::eval::bindings::encode_bindings_as_sexpr_generic(bindings, factory)
}

/// Decode bindings from an S-expression `(Bindings ($var val) ...)` back to
/// `GenericBindings<MettaValue>`. Used by `ground-with-bindings` and
/// `superpose-bind` (S5) to reconstruct bindings from their serialized form.
pub fn decode_bindings_from_sexpr(
    sexpr: &MettaValue,
    factory: &crate::backend::models::gc_allocator::GcFactory,
) -> crate::backend::models::GenericBindings<MettaValue> {
    decode_bindings_from_sexpr_generic(sexpr, factory)
}

/// Generic decoder for the `(Bindings ($var val) ...)` sidecar shape.
///
/// Shared across trampoline, bytecode-VM (S5 op_superpose_bind), and JIT
/// (S5 jit_runtime_superpose_bind) tiers so all three reconstruct the same
/// `GenericBindings<V>` map from the encoded sexpr produced by
/// `encode_bindings_as_sexpr_generic`.
pub fn decode_bindings_from_sexpr_generic<V, F>(
    sexpr: &V,
    factory: &F,
) -> crate::backend::models::GenericBindings<V>
where
    V: crate::backend::models::MettaValueTrait + Clone,
    F: crate::backend::models::MettaValueFactory<V>,
{
    let mut bindings = crate::backend::models::GenericBindings::new();
    if let Some(items) = sexpr.as_sexpr() {
        // Skip head "Bindings" atom
        for pair in items.iter().skip(1) {
            if let Some(pair_items) = pair.as_sexpr() {
                if pair_items.len() == 2 {
                    if let Some(name) = pair_items[0].as_atom() {
                        bindings.insert(name, pair_items[1].clone());
                    }
                }
            }
        }
    }
    let _ = factory; // factory available for future use
    bindings
}

/// Stage 1b: the capture frame no longer stores MettaValues — bindings now
/// travel with each `BoundValue` and are walked via the standard WorkItem/
/// Continuation GC root collectors. This function is retained as a no-op
/// to preserve the callable signature; existing call sites remain unchanged
/// and contribute zero roots.
pub fn collect_binding_capture_roots(_roots: &mut Vec<MettaValue>) {}

/// Set the cut signal — called by `eval_cut_generic` when `(cut)` is evaluated.
/// Records the current fork depth so the correct `ProcessRuleMatches`
/// continuation consumes it.
#[inline]
pub fn set_cut_active() {
    let depth = FORK_DEPTH.with(|c| c.get());
    CUT_TARGET_DEPTH.with(|c| c.set(depth));
}

/// Check if a cut signal is pending for the given fork depth. If so,
/// consume it and return `true`.
#[inline]
fn take_cut_at_depth(depth: u32) -> bool {
    CUT_TARGET_DEPTH.with(|c| {
        if c.get() == depth && depth > 0 {
            c.set(0);
            true
        } else {
            false
        }
    })
}

/// Increment fork depth — called when entering a nondeterministic dispatch.
#[inline]
fn enter_fork() -> u32 {
    FORK_DEPTH.with(|c| {
        let d = c.get() + 1;
        c.set(d);
        d
    })
}

/// Decrement fork depth — called when a `ProcessRuleMatches` completes.
#[inline]
fn leave_fork() {
    FORK_DEPTH.with(|c| {
        let d = c.get();
        if d > 0 {
            c.set(d - 1);
        }
    });
}

pub(crate) type ParallelEvalResults =
    std::sync::Arc<std::sync::Mutex<Vec<Option<Vec<BoundValue>>>>>;
pub(crate) type ParallelBranch = (MettaValue, SharedBindings);

pub(crate) struct ParallelBranchRootFrame {
    pub(crate) branches: Vec<ParallelBranch>,
    pub(crate) results: ParallelEvalResults,
}

pub(crate) struct ParallelCollapseRootFrame {
    pub(crate) items: Vec<BoundValue>,
    pub(crate) results: ParallelEvalResults,
}

fn collect_bound_value_roots(out: &mut Vec<MettaValue>, value: &BoundValue) {
    let (val, bindings) = value;
    out.push(val.clone());
    for (_, bound) in bindings.iter() {
        out.push(bound.clone());
    }
}

fn collect_parallel_result_roots(results: &ParallelEvalResults, out: &mut Vec<MettaValue>) {
    let guard = results.lock().expect("parallel results mutex poisoned");
    for slot in guard.iter().flatten() {
        for value in slot.iter() {
            collect_bound_value_roots(out, value);
        }
    }
}

unsafe fn collect_parallel_branch_frame_roots(data: *const (), out: &mut Vec<MettaValue>) {
    let frame = unsafe { &*(data as *const ParallelBranchRootFrame) };
    for (value, bindings) in frame.branches.iter() {
        out.push(value.clone());
        for (_, bound) in bindings.iter() {
            out.push(bound.clone());
        }
    }
    collect_parallel_result_roots(&frame.results, out);
}

unsafe fn collect_parallel_collapse_frame_roots(data: *const (), out: &mut Vec<MettaValue>) {
    let frame = unsafe { &*(data as *const ParallelCollapseRootFrame) };
    for item in frame.items.iter() {
        collect_bound_value_roots(out, item);
    }
    collect_parallel_result_roots(&frame.results, out);
}

/// **Stack-safety mandate (2026-05-15)**: Non-blocking parallel-dispatch.
///
/// Spawns all N branches to the work pool (NO inline branch-0) and returns
/// immediately with a `ParallelDispatchHandle`. The caller pushes a
/// `Continuation::WaitForParallel` to the heap-allocated continuation stack
/// and yields to the trampoline outer loop, which pumps the wait one tick
/// at a time without growing the C stack.
///
/// This replaces the old `parallel_branch_eval` synchronous wait + inline
/// branch-0 + work-stealing-on-stack pattern that caused unbounded C-stack
/// recursion (Robot.metta crash, PID 466461). See [[feedback-stack-safety-mandate]]
/// in user memory.
///
/// # Arguments
/// - `branches`: Pre-instantiated RHS values (bindings already applied)
/// - `env`: The evaluation environment (cloned per branch)
/// - `budget_acquired`: Number of budget slots — released by the WaitForParallel
///   arm on completion
/// - `caller_depth`: Caller's `PARALLEL_BRANCH_DEPTH` snapshot (informational)
/// - `demand`: Cancellation demand (e.g., `Exactly(1)` for `match-atom` fast-exit)
///
/// # Returns
/// `ParallelDispatchHandle` carrying all shared state. The handle owns the
/// frame_chain registration via `_root_guard`, popped on drop.
/// `branches` is taken as `Arc<Vec<...>>` (not `Vec<...>`) so the same
/// allocation can be shared between (a) the per-dispatch
/// `ParallelDispatchRootProvider` registered with `ROOT_REGISTRY`
/// (Phase 8 — closes the worker-INPUT vs GC-pool-walker race) AND
/// (b) the caller's `WaitForParallel.stable_branches_snapshot` field.
/// One allocation, one ref-count chain.
fn parallel_dispatch(
    branches: std::sync::Arc<Vec<ParallelBranch>>,
    env: crate::backend::environment::core::MettaEnvironment,
    // `budget_acquired` is threaded through the call sites and stored in
    // `Continuation::WaitForParallel` so the WaitForParallel arm can release
    // it on completion. The dispatch function itself doesn't use it (it's
    // not the one releasing); keep the parameter for ABI consistency with
    // the now-deleted `parallel_branch_eval`.
    _budget_acquired: u32,
    caller_depth: u32,
    demand: crate::backend::eval::cesk::coroutine::Demand,
) -> crate::backend::eval::trampoline::types::ParallelDispatchHandle {
    use std::sync::{Arc, Condvar, Mutex};

    use super::context::ParallelBranchContext;
    use super::types::{ParallelDispatchHandle, StallState};

    let num_branches = branches.len();
    debug_assert!(
        num_branches >= 2,
        "parallel_dispatch requires at least 2 branches"
    );

    // Trace: ParallelDispatch enter (lifted from parallel_branch_eval:1585-1607)
    #[cfg(feature = "trace")]
    {
        crate::backend::trace::with_trace_collector_ref(|tc| {
            let branch_tvs: Vec<trace_format::TraceValue> = branches
                .iter()
                .take(4)
                .map(|(b, _)| crate::backend::trace::trace_value_generic(b))
                .collect();
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                caller_depth,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::ParallelDispatch {
                    branch_count: num_branches as u32,
                    branch_exprs: branch_tvs,
                    parallel_depth: PARALLEL_BRANCH_DEPTH.with(|d| d.get()),
                    phase: "enter".to_string(),
                },
            );
        });
    }

    // **Stack-safety**: `remaining = num_branches` (all branches go to pool;
    // no inline branch-0 means every spawned worker decrements). Previously
    // `num_branches - 1` because branch-0 ran inline and didn't decrement.
    let results: ParallelEvalResults = Arc::new(Mutex::new(vec![None; num_branches]));
    let remaining = Arc::new(std::sync::atomic::AtomicU32::new(num_branches as u32));
    let done_pair = Arc::new((Mutex::new(false), Condvar::new()));

    let cancel_token = Arc::new(crate::backend::eval::cesk::coroutine::CancelToken::new(
        demand,
    ));

    let pool = global_eval_pool();
    let child_depth = caller_depth + 1;

    // Allocate the root frame on the heap. The Box's address is registered
    // with the frame_chain via push_custom; the resulting EvalFrameGuard is
    // stored in `handle._root_guard` so the registration outlives this
    // function and is popped only when the handle drops (i.e., when
    // WaitForParallel is consumed at completion).
    //
    // `branches` is now `Arc<Vec<...>>`; `(*branches).clone()` materializes
    // a Vec for the root_frame (the frame_chain's
    // `collect_parallel_branch_frame_roots` walker requires the heap-pinned
    // Vec, not an Arc indirection, per the unsafe push_custom contract).
    let root_frame = Box::new(ParallelBranchRootFrame {
        branches: (*branches).clone(),
        results: Arc::clone(&results),
    });
    let root_frame_ptr =
        &*root_frame as *const ParallelBranchRootFrame as *const ();
    // SAFETY: `root_frame` lives as long as the returned handle (stored
    // inside it). The frame_chain entry is popped on `_root_guard` drop,
    // which happens before `root_frame` (declaration order in the struct).
    let root_guard = unsafe {
        crate::backend::eval::frame_chain::EvalFrameGuard::push_custom(
            crate::backend::eval::frame_chain::FrameLabel::Custom("parallel-branch"),
            root_frame_ptr,
            collect_parallel_branch_frame_roots,
        )
    };

    // Phase 10.A: capture parent's tracked-vars union ONCE before the spawn
    // loop. Each worker closure gets a cheap Arc::clone — the union is
    // re-established on the worker thread via `WorkerCaptureScope::enter`
    // so `in_collapse_bind_scope()` and `active_tracked_vars()` return the
    // correct answers on the worker.
    let parent_tracked_vars: Option<Arc<SmallVec<[&'static str; 4]>>> =
        active_tracked_vars().map(Arc::new);

    // Spawn ALL branches to the pool — including branch 0 (stack-safety mandate).
    for (slot, (branch_expr, branch_bindings)) in branches.iter().enumerate() {
        let branch_expr = branch_expr.clone();
        let branch_bindings = branch_bindings.clone();
        let env = env.clone();
        let results = Arc::clone(&results);
        let remaining = Arc::clone(&remaining);
        let done_pair = Arc::clone(&done_pair);
        let cancel_token = Arc::clone(&cancel_token);
        let worker_tracked_vars = parent_tracked_vars.clone();

        // WFST classification: same as the old parallel_branch_eval path.
        let scheduler = crate::backend::scheduler::global_scheduler();
        let (cost_class, _action) = scheduler.classify_and_transduce(&branch_expr);
        let head_str = match branch_expr.view() {
            crate::backend::models::metta_value::ValueView::SExpr(items) if !items.is_empty() => {
                match items[0].view() {
                    crate::backend::models::metta_value::ValueView::Atom(s) => s,
                    _ => "",
                }
            }
            _ => "",
        };
        let head_hash = crate::backend::scheduler::TaskDescriptor::hash_head_symbol(head_str);
        let arity = match branch_expr.view() {
            crate::backend::models::metta_value::ValueView::SExpr(items) if items.len() > 1 => {
                (items.len() - 1).min(15) as u8
            }
            _ => 0u8,
        };
        let depth_bucket = crate::backend::scheduler::TaskDescriptor::depth_to_bucket(child_depth);
        let descriptor =
            crate::backend::scheduler::TaskDescriptor::pack(head_hash, arity, depth_bucket, 0);

        let ctx_hash = CONTINUATION_CONTEXT_HASH.with(|h| h.get());
        let effective_pri = scheduler.effective_priority(cost_class, ctx_hash);

        let closure = move || {
            PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));
            let _region_guard = crate::backend::eval::cesk::RegionGuard::enter();
            let _guard = EvalGuard::enter();
            // Phase 9.1: register `branch_expr` as a per-thread current-iter
            // GC root for the worker's lifetime. Drops on closure exit
            // (normal OR panic-unwind) and clears the cell. Closes the
            // worker-input UAF window that the Phase 9.3 deletion of
            // `safepoint_wait_for_quiescence` would otherwise re-open
            // between Phase 8 dispatch-input snapshots and per-pump-tick
            // safepoint root registrations.
            let _current_iter_scope =
                super::current_iter_root::CurrentIterScope::enter(branch_expr);
            // Phase 10.A — Stage 1e closure: re-establish the parent's
            // collapse-bind tracked-vars on the worker thread so
            // `in_collapse_bind_scope()` and `active_tracked_vars()` return
            // the right answers during worker eval. No-op if parent has no
            // active collapse-bind. RAII: drops on closure exit OR panic.
            let _worker_capture_scope = WorkerCaptureScope::enter(worker_tracked_vars);
            let _demand_scope = DemandScope::enter(demand);
            let _worker_marker = WorkerEvalScope::enter();
            // Cache-root refresh: branch workers evaluate arbitrary rule RHS
            // values and populate thread-local `EVAL_MEMO` /
            // `MATCH_RESULT_CACHE` entries (see `eval/mod.rs:107-119`) that
            // the parent trampoline will re-enter on merge — typical of
            // recursive PLN inference. Refreshing before `EvalGuard` drops
            // snapshots those entries into the safepoint root registry so
            // they survive between-worker GC. Also load-bearing under the
            // `catch_unwind` path below (`:1713-1716`): a cancellation
            // panic unwinds through this drop, ensuring cache-roots are
            // refreshed even on the unwind exit (the closure tail never
            // executes in that case).
            //
            // ASYMMETRY: `parallel_collapse_dispatch` worker at `:2107`
            // intentionally has NO equivalent guard — see the explanatory
            // comment there. Do not mirror this line into the collapse
            // worker without first auditing
            // `enumerate_rules_via_unification` cache-hit rates under PLN
            // load — empirically it inflates compose's freshened-binding
            // chain past the canary at `:6457`.
            let _cache_root_refresh = crate::backend::eval::CacheRootRefreshGuard::new();

            let cancel_outer = Arc::clone(&cancel_token);
            let unwind_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let ctx = ParallelBranchContext::with_cancel(Arc::clone(&cancel_outer));
                eval_trampoline_with_carrying(branch_expr, env, &ctx, branch_bindings)
            }));

            match unwind_result {
                Ok((eval_results, _new_env)) => {
                    let any_non_empty = eval_results.iter().any(|bv| !bv.0.is_empty());
                    if any_non_empty {
                        cancel_token.record_non_empty_branch();
                    }
                    let mut guard = results.lock().expect("results mutex poisoned");
                    guard[slot] = Some(eval_results.into_iter().collect());
                }
                Err(payload) => {
                    if payload
                        .downcast_ref::<crate::backend::eval::cesk::coroutine::BranchCancelled>()
                        .is_none()
                    {
                        let mut guard = results.lock().expect("results mutex poisoned");
                        guard[slot] = None;
                        drop(guard);
                        if remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                            let (lock, cvar) = &*done_pair;
                            let mut done = lock.lock().expect("done mutex poisoned");
                            *done = true;
                            cvar.notify_one();
                        }
                        std::panic::resume_unwind(payload);
                    }
                    let mut guard = results.lock().expect("results mutex poisoned");
                    guard[slot] = None;
                }
            }

            if remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                let (lock, cvar) = &*done_pair;
                let mut done = lock.lock().expect("done mutex poisoned");
                *done = true;
                cvar.notify_one();
            }
        };

        pool.spawn_eval_classified(
            closure,
            TaskTypeId::Eval(0),
            effective_pri,
            cost_class,
            descriptor,
        );
    }

    let started_at_alloc_count = AtomicU64::new(
        crate::backend::models::alloc_count_snapshot(),
    );

    // Register a GC root provider for this dispatch. Closes the
    // worker-write vs GC-pool-walker race (workers write into
    // `results[slot]` between parent pump-ticks; the parent's
    // thread-local `frame_chain` is invisible to GC pool workers on
    // other threads). The Arc stays alive while the handle does;
    // dropping the handle on the trampoline thread frees the provider
    // and the Weak in ROOT_REGISTRY is auto-pruned on next root walk.
    let root_provider = Arc::new(crate::backend::eval::trampoline::types::ParallelDispatchRootProvider {
        results: Arc::clone(&results),
        // Phase 8: share the SAME Arc the caller will use for
        // `WaitForParallel.stable_branches_snapshot`. Single allocation,
        // two strong refs — closes the worker-INPUT root-coverage gap.
        branches: Arc::clone(&branches),
    });
    crate::backend::models::gc_allocator::register_root_provider(
        &(Arc::clone(&root_provider) as Arc<dyn crate::backend::models::gc_allocator::RootProvider>),
    );

    // Phase 10.A: capture the parent's tracked-vars union BEFORE spawning
    // any worker. The union is later re-established as a shadow frame on
    // each worker thread via `WorkerCaptureScope::enter`. If no
    // collapse-bind is active, the hint is None and workers run without
    // pushing a frame (zero overhead).
    let tracked_vars_hint = active_tracked_vars().map(Arc::new);

    ParallelDispatchHandle {
        results,
        remaining,
        done_pair,
        cancel_token,
        num_branches,
        root_frame,
        _root_guard: Some(root_guard),
        started_at_alloc_count,
        stall_state: Mutex::new(StallState::default()),
        _root_provider_arc: root_provider,
        tracked_vars_hint,
    }
}

/// **Stack-safety mandate (2026-05-15)**: one-tick wait pump for the
/// trampolinized parallel-dispatch path.
///
/// Called once per `WaitForParallel` arm invocation by the trampoline outer
/// loop. Each tick:
///   1. Performs a short 1ms condvar wait for branch-completion notification.
///   2. Steals up to 4 tasks from the global queue (with GC-cooperation
///      drop/reacquire dance when `is_gc_requested()` is true).
///   3. Optionally drops the EvalGuard for a cooperative GC quiescence
///      window when `started_at_alloc_count` delta has crossed 500_000 or
///      GC has been explicitly requested.
///   4. Updates stall-detection state and spawns overflow workers if no
///      progress is observed across ≥20 consecutive ticks.
///
/// Crucially, this function returns after one tick. The trampoline's outer
/// `while let Some(work) = work_stack.pop()` loop pumps the continuation
/// again. There is no C-stack recursion across ticks.
fn pump_parallel_wait(
    handle: &crate::backend::eval::trampoline::types::ParallelDispatchHandle,
    stable_branches: &[ParallelBranch],
) {
    use std::time::Duration;

    let pool = global_eval_pool();

    // (1) Short cv wait.
    //
    // Phase 10.E.2 (2026-05-17): timeout dropped from 1 ms → 100 µs.
    // The parent does no productive work inside `pump_parallel_wait`
    // (see stack-safety mandate note below), so each ms of wait is a
    // pure latency-bubble between worker `notify_one()` and parent
    // re-check. At 100 µs the parent unblocks ~10× faster on
    // worker completion, with negligible additional wakeup overhead
    // (one extra context switch every ~900 µs in steady state).
    {
        let (lock, cvar) = &*handle.done_pair;
        let done_guard = lock.lock().expect("done mutex poisoned");
        if *done_guard {
            return;
        }
        let res = cvar
            .wait_timeout(done_guard, Duration::from_micros(100))
            .expect("done condvar wait failed");
        if *res.0 {
            return;
        }
    }

    if handle.cancel_token.is_satisfied() {
        return;
    }

    // **Stack-safety mandate (2026-05-15)**: NO work-stealing inside the
    // pump. The old `parallel_branch_eval` wait loop stole tasks via
    // `queue.try_pop() + task.execute()` to keep the calling thread
    // productive, but `task.execute()` runs a worker closure that itself
    // can dispatch parallel branches via `parallel_dispatch +
    // WaitForParallel`, which re-enters this pump — adding C-stack frames
    // per stolen task. With MAX_PARALLEL_DEPTH=3 the recursion was bounded
    // to ~3 levels, but each level adds ~12 stack frames (`pump →
    // task.execute → worker closure → eval_trampoline_with_carrying →
    // eval_trampoline_inner → process_continuation → next pump`), and in
    // debug builds frames are large enough that 3 levels overflow the
    // 8 MB OS-default stack (Robot.metta repro 2026-05-15).
    //
    // The trampolinization mandate requires bounded C-stack regardless of
    // `MAX_PARALLEL_DEPTH` (a user-tunable env var). Removing inline task
    // execution means the calling thread parks briefly on the condvar;
    // other workers handle the queue. With 64+ workers in the global
    // pool, throughput remains high; the stall-detection branch below
    // spawns overflow workers if all pool workers happen to be parked
    // simultaneously (rare).
    //
    // `stable_branches` is still used below in the GC-drop path as a root
    // source.

    // (2) Periodic cooperative GC drop — gated by alloc-delta or explicit
    //     gc-request. Mirrors the original wait loop's logic at
    //     parallel_branch_eval:1975-2037.
    if super::context::parallel_gc_coop_enabled()
        && handle.remaining.load(Ordering::Acquire) > 0
    {
        let current_allocs = crate::backend::models::alloc_count_snapshot();
        let last = handle.started_at_alloc_count.load(Ordering::Relaxed);
        let gc_pending = crate::backend::models::gc_allocator::is_gc_requested();
        let delta_crossed = current_allocs.wrapping_sub(last) >= 500_000;
        if gc_pending || delta_crossed {
            handle.started_at_alloc_count.store(current_allocs, Ordering::Relaxed);

            let mut parent_roots: Vec<MettaValue> = Vec::with_capacity(64);
            crate::backend::eval::frame_chain::collect_frame_chain_roots(&mut parent_roots);
            for (value, bindings) in stable_branches.iter() {
                parent_roots.push(value.clone());
                for (_, bound) in bindings.iter() {
                    parent_roots.push(bound.clone());
                }
            }
            {
                let g = handle.results.lock().expect("results mutex poisoned");
                for slot in g.iter().flatten() {
                    for (val, bindings) in slot.iter() {
                        parent_roots.push(val.clone());
                        for (_, v) in bindings.iter() {
                            parent_roots.push(v.clone());
                        }
                    }
                }
            }

            // Phase 9: purely-async GC — keep the temporary-root snapshot
            // (covers the parent's `frame_chain` + `stable_branches` +
            // `handle.results` for the duration of this pump frame) and
            // signal GC if needed, but DO NOT block on quiescence. See
            // `current_iter_root` module + Phase 9 plan.
            clear_aba_sensitive_caches();
            let _root_handle = crate::backend::models::register_temporary_roots(parent_roots);
            crate::backend::models::request_gc();
        }
    }

    // (4) Stall detection + overflow spawn — state in `handle.stall_state`.
    let curr_remaining = handle.remaining.load(Ordering::Acquire);
    let mut stall_state = handle.stall_state.lock().expect("stall_state mutex poisoned");
    if curr_remaining > 0 && curr_remaining == stall_state.prev_remaining {
        stall_state.stall_count += 1;
        if stall_state.stall_count >= 20 && !stall_state.overflow_requested {
            pool.spawn_overflow(curr_remaining as usize);
            stall_state.overflow_requested = true;
            tracing::warn!(
                remaining = curr_remaining,
                active_workers = pool.active_workers(),
                overflow = pool.overflow_count(),
                "parallel_dispatch: stall detected, spawned overflow workers"
            );
        }
    } else {
        stall_state.stall_count = 0;
    }
    stall_state.prev_remaining = curr_remaining;
}


/// **Stack-safety mandate (2026-05-15)**: trampolinized one-tick pump for
/// `parallel_collapse_dispatch`. Mirrors `pump_parallel_wait` but operates
/// on a `ParallelCollapseDispatchHandle`. No work-stealing — workers handle
/// the queue. Wired up by the `WaitForParallelCollapse` arm in
/// `process_continuation`.
fn pump_parallel_collapse_wait(
    handle: &crate::backend::eval::trampoline::types::ParallelCollapseDispatchHandle,
    stable_items: &[crate::backend::eval::trampoline::types::BoundValue],
) {
    use std::time::Duration;

    let pool = global_eval_pool();

    // (1) Short cv wait.
    //
    // Phase 10.E.2 (2026-05-17): timeout dropped from 1 ms → 100 µs
    // (see rationale in `pump_parallel_wait`).
    {
        let (lock, cvar) = &*handle.done_pair;
        let done_guard = lock.lock().expect("done mutex poisoned");
        if *done_guard {
            return;
        }
        let res = cvar
            .wait_timeout(done_guard, Duration::from_micros(100))
            .expect("done condvar wait failed");
        if *res.0 {
            return;
        }
    }

    if handle.cancel_token.is_satisfied() {
        return;
    }

    // (2) Periodic cooperative GC drop (no work-stealing per mandate).
    if super::context::parallel_gc_coop_enabled()
        && handle.remaining.load(Ordering::Acquire) > 0
    {
        let current_allocs = crate::backend::models::alloc_count_snapshot();
        let last = handle.started_at_alloc_count.load(Ordering::Relaxed);
        let gc_pending = crate::backend::models::gc_allocator::is_gc_requested();
        let delta_crossed = current_allocs.wrapping_sub(last) >= 500_000;
        if gc_pending || delta_crossed {
            handle.started_at_alloc_count.store(current_allocs, Ordering::Relaxed);

            let mut parent_roots: Vec<MettaValue> = Vec::with_capacity(64);
            crate::backend::eval::frame_chain::collect_frame_chain_roots(&mut parent_roots);
            for (value, bindings) in stable_items.iter() {
                parent_roots.push(value.clone());
                for (_, bound) in bindings.iter() {
                    parent_roots.push(bound.clone());
                }
            }
            {
                let g = handle.results.lock().expect("results mutex poisoned");
                for slot in g.iter().flatten() {
                    for (val, bindings) in slot.iter() {
                        parent_roots.push(val.clone());
                        for (_, v) in bindings.iter() {
                            parent_roots.push(v.clone());
                        }
                    }
                }
            }

            // Phase 9: purely-async GC — same shape as pump_parallel_wait.
            // Keep the temporary-root snapshot; signal GC; do NOT block.
            clear_aba_sensitive_caches();
            let _root_handle = crate::backend::models::register_temporary_roots(parent_roots);
            crate::backend::models::request_gc();
        }
    }

    // (3) Stall detection + overflow.
    let curr_remaining = handle.remaining.load(Ordering::Acquire);
    let mut stall_state = handle.stall_state.lock().expect("stall_state mutex poisoned");
    if curr_remaining > 0 && curr_remaining == stall_state.prev_remaining {
        stall_state.stall_count += 1;
        if stall_state.stall_count >= 20 && !stall_state.overflow_requested {
            pool.spawn_overflow(curr_remaining as usize);
            stall_state.overflow_requested = true;
            tracing::warn!(
                remaining = curr_remaining,
                "parallel_collapse_dispatch: stall detected, spawned overflow workers"
            );
        }
    } else {
        stall_state.stall_count = 0;
    }
    stall_state.prev_remaining = curr_remaining;
}

/// **Stack-safety mandate (2026-05-15)**: non-blocking collapse-dispatch.
///
/// Spawns all N items to the work pool (NO inline item-0) and returns a
/// `ParallelCollapseDispatchHandle`. Caller pushes `WaitForParallelCollapse`
/// to yield to the trampoline outer loop. Used by the ProcessCollapse and
/// ProcessCollapseBind paths (Sites 4 and 5) in `process_continuation`.
/// `items` is taken as `Arc<Vec<...>>` (not `Vec<...>`) so the same
/// allocation can be shared between (a) the per-dispatch
/// `ParallelCollapseRootProvider` registered with `ROOT_REGISTRY`
/// (Phase 8 — input-root coverage) AND (b) the caller's
/// `WaitForParallelCollapse.stable_items_snapshot` field.
fn parallel_collapse_dispatch(
    items: std::sync::Arc<Vec<crate::backend::eval::trampoline::types::BoundValue>>,
    env: crate::backend::environment::core::MettaEnvironment,
    _budget_acquired: u32,
    caller_depth: u32,
    _eval_depth: usize,
) -> crate::backend::eval::trampoline::types::ParallelCollapseDispatchHandle {
    use std::sync::{Arc, Condvar, Mutex};

    use super::context::ParallelBranchContext;
    use super::types::{ParallelCollapseDispatchHandle, StallState};

    let num_items = items.len();
    debug_assert!(
        num_items >= 2,
        "parallel_collapse_dispatch requires at least 2 items"
    );

    let results: ParallelEvalResults = Arc::new(Mutex::new(vec![None; num_items]));
    let remaining = Arc::new(std::sync::atomic::AtomicU32::new(num_items as u32));
    let done_pair = Arc::new((Mutex::new(false), Condvar::new()));
    let cancel_token = Arc::new(crate::backend::eval::cesk::coroutine::CancelToken::new(
        crate::backend::eval::cesk::coroutine::Demand::All,
    ));

    let pool = global_eval_pool();
    let child_depth = caller_depth + 1;

    // `items` is now `Arc<Vec<...>>`; materialize a Vec for the
    // root_frame's heap-pinned slot (frame_chain's collect walker
    // requires a Vec, not an Arc indirection).
    let root_frame = Box::new(ParallelCollapseRootFrame {
        items: (*items).clone(),
        results: Arc::clone(&results),
    });
    let root_frame_ptr =
        &*root_frame as *const ParallelCollapseRootFrame as *const ();
    // SAFETY: see analogous block in `parallel_dispatch`.
    let root_guard = unsafe {
        crate::backend::eval::frame_chain::EvalFrameGuard::push_custom(
            crate::backend::eval::frame_chain::FrameLabel::Custom("parallel-collapse"),
            root_frame_ptr,
            collect_parallel_collapse_frame_roots,
        )
    };

    // Phase 10.A — Stage 1e closure: capture the parent's collapse-bind
    // tracked-vars union ONCE before the spawn loop. Each worker clones
    // the Arc (cheap) and pushes a shadow `BINDING_CAPTURE_STACK` frame
    // via `WorkerCaptureScope::enter`, so `in_collapse_bind_scope()` and
    // `active_tracked_vars()` return the right answers on the worker
    // thread during nested evaluation. No-op when the parent has no
    // active collapse-bind. See `WorkerCaptureScope` docs at `:1380`.
    let parent_tracked_vars: Option<Arc<SmallVec<[&'static str; 4]>>> =
        active_tracked_vars().map(Arc::new);

    // Spawn ALL items to the pool — NO inline item-0 (stack-safety mandate).
    for (slot, (item_expr, item_bindings)) in items.iter().enumerate() {
        let item_expr = item_expr.clone();
        let item_bindings = item_bindings.clone();
        let env = env.clone();
        let results = Arc::clone(&results);
        let remaining = Arc::clone(&remaining);
        let done_pair = Arc::clone(&done_pair);
        let worker_tracked_vars = parent_tracked_vars.clone();

        let scheduler = crate::backend::scheduler::global_scheduler();
        let (cost_class, _action) = scheduler.classify_and_transduce(&item_expr);
        let head_str = match item_expr.view() {
            crate::backend::models::metta_value::ValueView::SExpr(items) if !items.is_empty() => {
                match items[0].view() {
                    crate::backend::models::metta_value::ValueView::Atom(s) => s,
                    _ => "",
                }
            }
            _ => "",
        };
        let head_hash = crate::backend::scheduler::TaskDescriptor::hash_head_symbol(head_str);
        let arity = match item_expr.view() {
            crate::backend::models::metta_value::ValueView::SExpr(items) if items.len() > 1 => {
                (items.len() - 1).min(15) as u8
            }
            _ => 0u8,
        };
        let depth_bucket = crate::backend::scheduler::TaskDescriptor::depth_to_bucket(child_depth);
        let descriptor =
            crate::backend::scheduler::TaskDescriptor::pack(head_hash, arity, depth_bucket, 0);

        let closure = move || {
            PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));
            let _region_guard = crate::backend::eval::cesk::RegionGuard::enter();
            let _guard = EvalGuard::enter();
            // Phase 9.1: register `item_expr` as a per-thread current-iter
            // GC root for the worker's lifetime (mirrors branch worker at
            // `:1730`). See `current_iter_root` module docs.
            let _current_iter_scope =
                super::current_iter_root::CurrentIterScope::enter(item_expr);
            // Phase 10.A — Stage 1e closure: re-establish the parent's
            // collapse-bind tracked-vars on the worker thread so
            // `in_collapse_bind_scope()` and `active_tracked_vars()` return
            // the right answers during worker eval. No-op if parent has no
            // active collapse-bind. RAII: drops on closure exit OR panic.
            let _worker_capture_scope = WorkerCaptureScope::enter(worker_tracked_vars);
            let _demand_scope =
                DemandScope::enter(crate::backend::eval::cesk::coroutine::Demand::All);
            let _worker_marker = WorkerEvalScope::enter();
            // Intentionally NO `CacheRootRefreshGuard` here (contrast the
            // `parallel_dispatch` worker at `:1710`). Three reasons:
            //
            // (a) The `is_memoized_normal_form` short-circuit immediately
            //     below skips eval entirely for the majority of collapse
            //     items — no caches get populated in that case.
            //
            // (b) Eval-path items produce values fed straight into
            //     `results[slot]`, which `ParallelCollapseRootProvider`
            //     (registered at the construction site in `429e798`)
            //     already covers for cross-thread GC visibility. The
            //     parent's merge path drives results into
            //     `ProcessCollapseEvalResults` /
            //     `WaitForParallelCollapse`, which does NOT re-enter
            //     unification — so persisting `MATCH_RESULT_CACHE`
            //     entries past this worker boundary adds memory pressure
            //     with no correctness benefit.
            //
            // (c) `Demand::All` + no `catch_unwind` plumbing means the
            //     worker always exits through the closure tail. Unlike
            //     the branch worker, there is no panic-unwind path that
            //     would skip natural cache cleanup.
            //
            // Empirical confirmation: adding the guard here regresses
            // Robot.metta from 40-85 SELECTED outputs to 19-21 AND trips
            // the `freshened_count < 1024` canary at `:6457` (see
            // commit `429e798` body and the Phase 7 investigation in
            // memory `sigill-fix-2026-05-15.md`).
            //
            // Do not add it without first investigating
            // `enumerate_rules_via_unification` cache-hit rates under
            // PLN load — the asymmetry may be masking a latent bug there
            // (Plan agent's optional Option A follow-up).
            //
            // Option C: HE-faithful re-eval skip for normal-form items.
            let eval_results: smallvec::SmallVec<
                [crate::backend::eval::trampoline::types::BoundValue; 2],
            > = if crate::backend::eval::trampoline::is_memoized_normal_form(&item_expr) {
                smallvec::smallvec![bv_with(item_expr.clone(), item_bindings.clone())]
            } else {
                let ctx = ParallelBranchContext::get();
                let (results, _new_env) = eval_trampoline_with_carrying(
                    item_expr.clone(),
                    env,
                    &ctx,
                    std::sync::Arc::new(item_bindings.clone()),
                );
                results
            };

            {
                let mut guard = results.lock().expect("results mutex poisoned");
                guard[slot] = Some(eval_results.into_iter().collect());
            }

            if remaining.fetch_sub(1, Ordering::AcqRel) == 1 {
                let (lock, cvar) = &*done_pair;
                let mut done = lock.lock().expect("done mutex poisoned");
                *done = true;
                cvar.notify_one();
            }
        };

        pool.spawn_eval_classified(
            closure,
            TaskTypeId::Eval(0),
            priority_levels::NORMAL,
            cost_class,
            descriptor,
        );
    }

    // Register a GC root provider for this dispatch.
    // See `parallel_dispatch` for the rationale and lifetime invariants.
    let root_provider = Arc::new(crate::backend::eval::trampoline::types::ParallelCollapseRootProvider {
        results: Arc::clone(&results),
        // Phase 8: share the Arc the caller will use for
        // `WaitForParallelCollapse.stable_items_snapshot`.
        items: Arc::clone(&items),
    });
    crate::backend::models::gc_allocator::register_root_provider(
        &(Arc::clone(&root_provider) as Arc<dyn crate::backend::models::gc_allocator::RootProvider>),
    );

    ParallelCollapseDispatchHandle {
        results,
        remaining,
        done_pair,
        cancel_token,
        num_branches: num_items,
        root_frame,
        _root_guard: Some(root_guard),
        started_at_alloc_count: AtomicU64::new(
            crate::backend::models::alloc_count_snapshot(),
        ),
        stall_state: Mutex::new(StallState::default()),
        _root_provider_arc: root_provider,
        // Phase 10.A: handed off to the WaitForParallelCollapse continuation
        // for sidecar per-branch binding-projection reconstruction.
        tracked_vars_hint: parent_tracked_vars,
    }
}

/// Minimum number of collapse results to trigger parallel evaluation.
/// Below this threshold, the sequential `ProcessCollapseEvalResults` path
/// is cheaper due to lower overhead (no Arc, no Mutex, no condvar).
///
/// Phase 10.G (2026-05-17): default lowered 16 → 8. PLN's inner Derive
/// step typically produces 4–12 collapse results; the old threshold of
/// 16 meant they almost never dispatched in parallel. The env var
/// `METTATRON_PARALLEL_COLLAPSE_THRESHOLD` overrides at process start.
static PARALLEL_COLLAPSE_THRESHOLD_CACHE: OnceLock<usize> = OnceLock::new();

#[inline]
fn parallel_collapse_threshold() -> usize {
    *PARALLEL_COLLAPSE_THRESHOLD_CACHE.get_or_init(|| {
        std::env::var("METTATRON_PARALLEL_COLLAPSE_THRESHOLD")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|&n| n >= 2)
            .unwrap_or(8)
    })
}


/// Generic trampoline evaluation entry point.
///
/// This function provides a unified evaluation engine that works with any value type
/// implementing `MettaValueTrait`. It uses:
/// - `WorkItem<V, F>` for pending evaluation work
/// - `Continuation<V, F>` for continuation handling
/// - `eval_step_generic` for single-step evaluation
///
/// All environment operations use `GenericEnvironment<V, F>` inherent methods.
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticEvalContext` or `SessionContext`)
///
/// # Arguments
///
/// - `value`: The value to evaluate
/// - `env`: The evaluation environment (`GenericEnvironment<MettaValue, GcFactory>`)
/// - `ctx`: The evaluation context providing the factory
///
/// # Returns
///
/// A tuple of (results, final_environment)
/// I-18: Run eval_trampoline to completion, resuming any yields.
///
/// This is the backward-compatible entry point used by all callers.
/// Internally, the trampoline may yield after exhausting its reduction budget,
/// but this wrapper loops until `Complete`.
pub fn eval_trampoline<C: EvalContext>(
    value: MettaValue,
    env: MettaEnvironment,
    ctx: &C,
) -> EvalResult {
    eval_trampoline_with_carrying(value, env, ctx, empty_shared_bindings())
}

fn eval_trampoline_with_carrying<C: EvalContext>(
    value: MettaValue,
    env: MettaEnvironment,
    ctx: &C,
    carrying_bindings: SharedBindings,
) -> EvalResult {
    // Isolate fork/cut thread-local state so nested trampoline calls
    // (from `test`, `assertEqual`, `collapse` within synchronous ops,
    // etc.) don't corrupt the outer trampoline's cut signaling.
    // Without this, a `(cut)` inside a `test` body could consume the
    // outer dispatch's cut target or vice versa.
    let saved_fork = FORK_DEPTH.with(|c| c.replace(0));
    let saved_cut = CUT_TARGET_DEPTH.with(|c| c.replace(0));

    let mut outcome = eval_trampoline_inner(value, env, ctx, None, None, 0, carrying_bindings);
    let result = loop {
        match outcome {
            crate::backend::eval::cesk::EvalOutcome::Complete(results, env) => {
                break (results, Arc::new(env));
            }
            crate::backend::eval::cesk::EvalOutcome::Yielded(suspended) => {
                // Resume from suspended state with a fresh reduction budget
                outcome = resume_trampoline_inner(suspended, ctx);
            }
        }
    };

    // Restore outer trampoline's fork/cut state.
    FORK_DEPTH.with(|c| c.set(saved_fork));
    CUT_TARGET_DEPTH.with(|c| c.set(saved_cut));

    result
}

/// Resume a suspended trampoline evaluation from saved state.
///
/// Reconstructs the trampoline loop from the saved work_stack and continuations,
/// continuing with a fresh reduction budget slice.
fn resume_trampoline_inner<C: EvalContext>(
    suspended: crate::backend::eval::cesk::SuspendedEval,
    ctx: &C,
) -> crate::backend::eval::cesk::EvalOutcome {
    // Extract environment from the first work item.
    // The `value` and `env` parameters to eval_trampoline_inner are unused when
    // resuming (work_stack is pre-populated). We extract env from saved state.
    let env = match suspended.work_stack.first() {
        Some(WorkItem::Eval { env, .. }) => env.clone(),
        Some(WorkItem::EvalWithBindings { env, .. }) => env.clone(),
        Some(WorkItem::Resume { result }) => result.1.clone(),
        None => panic!("resume_trampoline_inner: SuspendedEval has empty work_stack"),
    };

    eval_trampoline_inner(
        ctx.factory().unit(), // Dummy value — unused since work_stack is pre-populated
        (*env).clone(),
        ctx,
        Some(suspended.work_stack),
        Some(suspended.continuations),
        suspended.total_reductions,
        empty_shared_bindings(),
    )
}

/// Internal trampoline that returns EvalOutcome (may yield).
///
/// When `resume_work_stack` and `resume_continuations` are `Some`, this resumes
/// a previously suspended evaluation from saved state rather than starting fresh.
/// `resume_reductions` carries the lifetime reduction count across yields.
fn eval_trampoline_inner<C: EvalContext>(
    value: MettaValue,
    env: MettaEnvironment,
    ctx: &C,
    resume_work_stack: Option<Vec<WorkItem>>,
    resume_continuations: Option<Vec<Continuation>>,
    resume_reductions: u64,
    initial_carrying_bindings: SharedBindings,
) -> crate::backend::eval::cesk::EvalOutcome {
    // Trace: EvalStart with span correlation + start timestamp.
    // These variables carry the start timestamp and span ID to the EvalEnd site.
    #[cfg(feature = "trace")]
    let (_eval_start_ns, _eval_span_id) = {
        if let Some(tc) = ctx.trace_collector() {
            let span_id = tc.next_span_id();
            let start_ns = tc.elapsed_ns();
            tc.emit_timed(
                trace_format::TraceTier::TreeWalker,
                0,
                crate::backend::trace::trace_value_generic(&value),
                vec![],
                None,
                trace_format::TraceEventKind::EvalStart,
                start_ns,
                None, // duration not known yet — EvalEnd carries it
                Some(span_id),
            );
            (start_ns, span_id)
        } else {
            (0u64, 0u64)
        }
    };

    // Set thread-local trace collector so that type inference (infer_types_generic,
    // types_match_generic) and rule management (add_rule) can emit trace events
    // without requiring an EvalContext parameter.
    #[cfg(feature = "trace")]
    {
        if let Some(tc) = ctx.trace_collector() {
            // The trace collector is behind a shared reference with a 'static-like
            // lifetime (Arc in SessionContext). We store a raw pointer in the
            // thread-local; the trampoline outlives all type inference calls.
            crate::backend::trace::thread_local_sink::set_thread_trace_collector_ref(tc);
        }
    }

    // Initialize work stack and continuations, either from resume state or fresh.
    let is_resuming = resume_work_stack.is_some();
    let mut work_stack: Vec<WorkItem> = if let Some(ws) = resume_work_stack {
        ws
    } else {
        let mut ws = Vec::with_capacity(32);
        ws.push(WorkItem::Eval {
            value,
            env: Arc::new(env.clone()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: initial_carrying_bindings,
        });
        ws
    };

    let mut continuations: Vec<Continuation> = if let Some(cs) = resume_continuations {
        cs
    } else {
        let mut cs = Vec::with_capacity(64);
        cs.push(Continuation::Done);
        cs
    };

    // Final result storage
    let mut final_result: Option<EvalResult> = None;

    // GC safepoint counter: wrapping u16 overflows every 4096 iterations (mask 0xFFF).
    // Increased from u8 (256) to reduce maybe_process_gc_response overhead (4.9% → ~1%).
    let mut gc_counter: u16 = 0;

    // I-18: Reduction counter for cooperative yielding.
    // Initializes from resume_reductions for lifetime tracking across yields.
    let mut reduction_counter = crate::backend::eval::cesk::ReductionCounter::new();
    if resume_reductions > 0 {
        reduction_counter.add_total(resume_reductions);
    }

    // SECK Phase 0.5: Reusable root set for GC safepoints.
    // Allocated once here, cleared and reused across safepoints. This avoids
    // re-allocating a Vec<V> on every safepoint (previously ~every 4096 iterations).
    let mut root_set =
        crate::backend::eval::cesk::RootSet::<MettaValue>::with_estimated_capacity(32, 64, 0);

    // I-4/I-6: Clear subgoal and thunk tables between top-level evaluations
    // to prevent stale cached results from previous evaluations.
    // Skip when resuming — tables were already cleared on the initial call.
    //
    // Phase 3.2: Bump the query_generation counter so EVAL_MEMO and
    // MATCH_RESULT_CACHE entries from the prior top-level `!` are treated
    // as stale on lookup. LRU reclaims them lazily — no bulk clear cost,
    // but cross-query contamination is eliminated. This is the root-cause
    // fix for Direct.metta's order-dependent result sets.
    if !is_resuming {
        crate::backend::eval::cesk::clear_subgoal_table();
        crate::backend::eval::cesk::clear_thunk_table();
        crate::backend::eval::trampoline::dispatch_hints::increment_query_generation();
        // H12 (2026-05-05): clear the normal-form bloom so freeze-tuple
        // memoization from a prior top-level `!` cannot poison this query
        // via hash-cons aliasing or bloom FPR. Within-query optimization
        // is preserved — only cross-query reuse is sacrificed.
        crate::backend::eval::trampoline::dispatch_hints::clear_normal_form_memo_for_new_query();
    }

    // Deferred environment drops: hold Arc clones to dying environments' shared
    // state, deferring the expensive cascading Arc::drop_slow out of the hot path.
    // At each GC safepoint, we call collect_roots() on each deferred env to add
    // their MettaValues to the root_set (so the GC doesn't sweep them), then
    // clear the Vec AFTER the safepoint completes.
    let mut deferred_shared_drops: Vec<
        std::sync::Arc<crate::backend::environment::GenericEnvironmentShared<MettaValue>>,
    > = Vec::new();

    // Main trampoline loop
    #[cfg(feature = "trace")]
    let mut _trampoline_iter: u64 = 0;
    while let Some(work) = work_stack.pop() {
        if crate::backend::interrupt::is_interrupted() {
            let interrupted_env = match &work {
                WorkItem::Eval { env, .. } | WorkItem::EvalWithBindings { env, .. } => env.clone(),
                WorkItem::Resume { result } => result.1.clone(),
            };
            final_result = Some((SmallVec::new(), interrupted_env));
            work_stack.clear();
            break;
        }

        // ── G1 trampoline-tick balance check (every 65536 iterations).
        // ──   Catches threads that hold a leaked `pages.read()` guard
        // ──   while iterating in the trampoline. The 65k tick interval
        // ──   keeps overhead negligible (one cmp every 65k iterations
        // ──   ≈ < 0.001% per-iteration cost) while still catching a
        // ──   leak within ~10ms of the leak point on a typical
        // ──   machine. The `current_thread_has_imbalance()` early-out
        // ──   skips the emission entirely on threads with no held
        // ──   guards.
        // Trace: TrampolineStep (gated by METTA_TRACE_TRAMPOLINE=1)
        #[cfg(feature = "trace")]
        {
            _trampoline_iter += 1;
            if crate::backend::trace::rule_match::should_trace_trampoline() {
                crate::backend::trace::with_trace_collector_ref(|tc| {
                    let (kind, expr, depth) = match &work {
                        WorkItem::Eval { value, depth, .. } => (
                            "Eval",
                            Some(crate::backend::trace::trace_value_generic(value)),
                            *depth as u32,
                        ),
                        WorkItem::EvalWithBindings {
                            template, depth, ..
                        } => (
                            "EvalWithBindings",
                            Some(crate::backend::trace::trace_value_generic(template)),
                            *depth as u32,
                        ),
                        WorkItem::Resume { .. } => ("Resume", None, 0),
                    };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth,
                        expr.clone().unwrap_or(trace_format::TraceValue::Unit),
                        vec![],
                        None,
                        trace_format::TraceEventKind::TrampolineStep {
                            work_kind: kind.to_string(),
                            expression: expr,
                            stack_depth: work_stack.len() as u32,
                            continuation_depth: continuations.len() as u32,
                            iteration: _trampoline_iter,
                        },
                    );
                });
            }
        }

        // Incremental deferred-drop drain: pop 1 environment every 64 iterations.
        // Drops happen on the eval thread (amortized, not spiked).
        gc_counter = gc_counter.wrapping_add(1);
        if gc_counter & 0x3F == 0 {
            deferred_shared_drops.pop();
        }

        // Periodic GC safepoint check (every 4096 trampoline iterations).
        //
        // The 4096-iter cadence drives THREE distinct activities:
        //
        //   1. Root collection — needed for both nursery and old-gen GC.
        //   2. Nursery collection — thread-local, no quiescence needed.
        //      MUST run regardless of `ctx.should_safepoint()`, otherwise
        //      `ParallelBranchContext` workers (which inherit the trait's
        //      no-op `should_safepoint() -> false`) would never sweep their
        //      nurseries — bytehound on Smokes showed a 67 MB single
        //      `record_alloc` grow in this exact failure mode.
        //   3. Old-gen safepoint — requires `ctx.should_safepoint()` true
        //      because it drops the EvalGuard and waits for global quiescence.
        if gc_counter & 0xFFF == 0 {
            // SECK Phase 0.5: Algebraic root set collection.
            // Uses reusable RootSet buffer (allocated once before the loop)
            // instead of a fresh Vec on every safepoint.
            //
            // Root formula: roots = addrs_in(C) ∪ addrs_in(K)
            // where C = current work item + work stack, K = continuations.
            // Environment roots (E) are managed separately via RootProvider.
            root_set.clear();
            root_set.collect_from_work_items(&work, &work_stack);
            root_set.collect_from_continuations(&continuations);

            // Collect roots from all caller frames in the thread-local chain.
            // This protects values held by callers of nested trampolines
            // (e.g., compiled expressions in eval_include_generic).
            {
                let concrete_roots = root_set.as_mut_vec();
                crate::backend::eval::frame_chain::collect_frame_chain_roots(concrete_roots);
            }
            // Collect GC roots from the eval memo cache. Cached MettaValue
            // pointers must survive the mark-sweep cycle.
            {
                let concrete_roots = root_set.as_mut_vec();
                collect_eval_memo_roots(concrete_roots);
                collect_match_result_roots(concrete_roots);
                crate::backend::eval::cesk::tabling::collect_subgoal_roots(concrete_roots);
                crate::backend::eval::cesk::thunk::collect_thunk_roots(concrete_roots);

                // Collect GC roots from collapse-bind capture frames.
                collect_binding_capture_roots(concrete_roots);

                // Collect GC roots from deferred environment drops.
                // These environments' MettaValues must be visible to the GC
                // so it doesn't sweep values only reachable through them.
                for deferred_env in &deferred_shared_drops {
                    deferred_env.as_ref().collect_roots(concrete_roots);
                }
            }

            // Clear all pointer-keyed caches before either nursery or old-gen
            // collection can free/reuse slab slots. The nursery collector runs
            // even when ctx.should_safepoint() is false, so this cannot live
            // only inside the old-gen safepoint branch.
            clear_aba_sensitive_caches();

            // Phase 2.2: Incremental nursery collection (thread-local, no quiescence needed).
            // Uses the algebraic root set to determine which nursery values are live.
            //
            // Build a sorted, deduped live-pointer slice (not a HashSet). The root
            // set is bounded by depth × continuation chain (~hundreds, not millions),
            // so sort_unstable + binary_search inside the collector beats HashSet
            // on cache locality and zero-hash cost. SmallVec keeps the slice on
            // the stack for the common case (≤256 roots).
            //
            // Runs regardless of `ctx.should_safepoint()` — see top-of-block
            // comment. Without this, parallel-branch workers grow their
            // per-thread nurseries unboundedly.
            {
                crate::backend::eval::cesk::with_nursery_collector(|collector| {
                    if collector.should_collect() {
                        let concrete_roots = root_set.as_mut_vec();
                        let mut live_ptrs: smallvec::SmallVec<[usize; 256]> = concrete_roots
                            .iter()
                            .map(|v| v.inner_ptr() as usize)
                            .collect();
                        live_ptrs.sort_unstable();
                        live_ptrs.dedup();
                        collector.collect(&live_ptrs);
                    }
                });
            }

            // Old-gen safepoint dance: requires global quiescence.
            // Skipped for contexts whose `should_safepoint()` returns false
            // (StaticEvalContext, ParallelBranchContext) — those evaluators
            // do not coordinate the slab GC's mark-sweep cycle.
            if ctx.should_safepoint() {
                #[cfg(feature = "trace")]
                let _root_count = root_set.len() as u32;
                #[cfg(feature = "trace")]
                let _safepoint_start =
                    { ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0) };
                // Drain roots into a Vec for the GC. The RootSet retains its
                // allocated capacity for reuse at the next safepoint.
                ctx.perform_safepoint(root_set.drain_into_vec());

                // Batch-drain deferred drops AFTER safepoint completes.
                // Their roots were collected into root_set above, so the GC saw them.
                // Send the batch to the background drop worker thread to avoid
                // PathMap/MettaTrie cascade drops on the hot eval path.
                if !deferred_shared_drops.is_empty() {
                    let drain_count = deferred_shared_drops.len().min(32);
                    let batch_start = deferred_shared_drops.len() - drain_count;
                    let batch: Vec<SharedEnvArc> =
                        deferred_shared_drops.drain(batch_start..).collect();
                    // Send concrete Arc<GenericEnvironmentShared<MettaValue>> to background
                    // drop worker. No type erasure needed — concrete dispatch.
                    let _ = get_drop_sender().send(batch);
                }

                // Trace: GcSafepoint with measured pause duration
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let end_ns = tc.elapsed_ns();
                        let duration = end_ns.saturating_sub(_safepoint_start);
                        tc.emit_timed(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::GcSafepoint {
                                root_count: _root_count,
                                allocation_delta_bytes: 0,
                            },
                            _safepoint_start,
                            Some(duration),
                            None, // no span correlation needed for safepoints
                        );
                    }
                }
            } // end: if ctx.should_safepoint()
        } // end: if gc_counter & 0xFFF == 0

        // I-18: Cooperative yield check. When the reduction budget is exhausted,
        // yield if we are on a worker thread (not the main thread), not inside
        // a nested parallel branch, and have continuations to resume.
        // Amortize reduction_counter.tick() to every 4096 iterations by
        // piggybacking on the GC safepoint cadence. This eliminates ~200ns/iter
        // overhead from 2 increments + 1 comparison on every trampoline step.
        if gc_counter & 0xFFF == 0
            && reduction_counter.tick()
            && !continuations.is_empty()
            && crate::backend::eval::cesk::current_worker_id().is_some()
            && PARALLEL_BRANCH_DEPTH.with(|d| d.get()) == 0
        {
            // Push the popped work item back so it can be resumed
            work_stack.push(work);
            let depth_hint = continuations.last().map(|c| c.depth_hint()).unwrap_or(0) as u32;
            return crate::backend::eval::cesk::EvalOutcome::Yielded(
                crate::backend::eval::cesk::SuspendedEval {
                    work_stack,
                    continuations,
                    depth: depth_hint,
                    total_reductions: reduction_counter.total(),
                    worker_id: crate::backend::eval::cesk::current_worker_id(),
                },
            );
        }

        match work {
            WorkItem::Eval {
                value,
                env,
                depth,
                is_tail_call,
                expected_type,
                demand,
                carrying_bindings,
            } => {
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?value, depth, "eval work item");

                // Publish the current eval's demand on the thread-local so
                // downstream parallel-dispatch sites (StartAmb / superpose,
                // dispatch_rule_matches) can read it. Sticky semantics:
                // `Some(d)` overrides; `None` inherits the previously-set
                // demand. Shape-preserving forms (`not`, `==`, `if`'s arg-
                // pushes) push children with `demand: None` so they
                // implicitly inherit. Cardinality-multiplying forms
                // (`StartCollapse`, `StartCollapseBind`, etc.) push children
                // with `demand: Some(Demand::All)` to explicitly shadow.
                if demand.is_some() {
                    CURRENT_DEMAND.with(|d| d.set(demand));
                }

                // Phase 9.5: Normal-form memoization check.
                // If this S-expression has been previously evaluated and reached
                // fixpoint (evaluated to itself), skip evaluation entirely.
                let is_sexpr = value.as_sexpr().is_some();
                if is_sexpr && is_memoized_normal_form(&value) {
                    let cb = &*carrying_bindings;
                    work_stack.push(WorkItem::Resume {
                        result: (
                            smallvec![if cb.is_empty() {
                                bv(value)
                            } else {
                                bv_with(value, cb.clone())
                            }],
                            env,
                        ),
                    });
                    continue;
                }

                // I-4: Subgoal tabling — check if this expression has been
                // previously evaluated (Complete) or is currently being evaluated
                // (Active → cycle detection). Only for S-expressions at depth >= 2
                // that don't contain variables (variable expressions are context-
                // dependent and must not be cached by content hash).
                // Also require should_memoize: impure expressions (those
                // calling add-atom, change-state!, etc.) must not be tabled
                // because repeated calls must re-execute their side effects.
                // H9 (2026-05-05): track whether the subgoal-table path was taken.
                // If so, the line-2780 MemoizeResult push is elided — CompleteSubgoal
                // already writes the result to the canonical cache. Both gates use
                // the same `should_memoize_with_env` predicate, so the writes are
                // dominated by the subgoal-table path. Audit #7c: 5-8% wall savings.
                let mut subgoal_path_taken = false;
                if is_sexpr
                    && depth >= 2
                    && !value.has_variables_fast()
                    && should_memoize_with_env(&value, &*env)
                {
                    subgoal_path_taken = true;
                    let tabling_hash = value.hash_value();

                    // Step 1: Cycle detection via active evaluation set.
                    // True cycle = expression is on its own call stack.
                    if crate::backend::eval::cesk::is_actively_evaluating(tabling_hash) {
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&value),
                                    vec![],
                                    None,
                                    trace_format::TraceEventKind::TablingDecision {
                                        expr_hash: tabling_hash,
                                        decision: trace_format::TablingDecisionKind::CycleDetected,
                                        result_count: Some(0),
                                    },
                                );
                            }
                        }
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env),
                        });
                        continue;
                    }

                    // Step 2: Check memoization cache (Complete results).
                    let lookup =
                        crate::backend::eval::cesk::with_subgoal_table(|t| t.lookup(tabling_hash));
                    match lookup {
                        crate::backend::eval::cesk::TableLookup::Complete(cached) => {
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&value),
                                        cached
                                            .iter()
                                            .map(|v| crate::backend::trace::trace_value_generic(v))
                                            .collect(),
                                        None,
                                        trace_format::TraceEventKind::TablingDecision {
                                            expr_hash: tabling_hash,
                                            decision: trace_format::TablingDecisionKind::CacheHit,
                                            result_count: Some(cached.len() as u32),
                                        },
                                    );
                                }
                            }
                            // Cross-branch isolation is enforced by scope_gen
                            // (per-branch watermark) + query_generation
                            // (per-`!` watermark). A cache hit only occurs
                            // within the same branch (or pre-fork visible to
                            // all branches), so tagging with the retrieving
                            // branch's carrying_bindings is HE-bisimilar.
                            let resumed: smallvec::SmallVec<[BoundValue; 2]> = cached
                                .into_iter()
                                .filter_map(|v| {
                                    let tracked = active_tracked_vars();
                                    project_carrying_for_consumer(
                                        &carrying_bindings,
                                        &v,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    )
                                    .map(|projected| {
                                        if projected.is_empty() {
                                            bv(v)
                                        } else {
                                            bv_with(v, (*projected).clone())
                                        }
                                    })
                                })
                                .collect();
                            work_stack.push(WorkItem::Resume {
                                result: (resumed, env),
                            });
                            continue;
                        }
                        crate::backend::eval::cesk::TableLookup::Absent => {
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&value),
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::TablingDecision {
                                            expr_hash: tabling_hash,
                                            decision: trace_format::TablingDecisionKind::CacheMiss,
                                            result_count: None,
                                        },
                                    );
                                }
                            }
                            // Task #6 Phase 7 (2026-05-18): collapse duplicate
                            // CompleteSubgoal for the SAME hash. CRITICAL: the
                            // mark_eval_active / unmark_eval_active discipline
                            // must remain BALANCED — when we elide a
                            // CompleteSubgoal push we MUST also elide the
                            // matching mark_eval_active so that the single
                            // popping unmark_eval_active doesn't underflow
                            // the refcount for the parent frame.
                            //
                            // Cycle detection (is_actively_evaluating above)
                            // still returns TRUE because the outer frame's
                            // mark_eval_active set count > 0.
                            let already_pending = matches!(
                                continuations.last(),
                                Some(Continuation::CompleteSubgoal { expr_hash: prev, .. })
                                    if *prev == tabling_hash
                            );
                            if !already_pending {
                                // First evaluation: mark active, push CompleteSubgoal.
                                crate::backend::eval::cesk::mark_eval_active(tabling_hash);
                                continuations.push(Continuation::CompleteSubgoal {
                                    expr_hash: tabling_hash,
                                    env: env.clone(),
                                    depth,
                                    start_epoch: mutation_epoch(),
                                });
                            }
                        }
                    }
                }

                // Expression-level memoization: check if we've evaluated this
                // exact expression before (by content hash). Only for MettaValue
                // (compile-time constant after monomorphization) and pure expressions.
                let memo_hash =
                    if is_sexpr && should_memoize_with_env(&value, &*env) {
                        let h = value.hash_value();
                        if let Some(cached_results) = eval_memo_get(h) {
                            // Cache hit — skip evaluation entirely.
                            let cb = &*carrying_bindings;
                            work_stack.push(WorkItem::Resume {
                                result: (
                                    if cb.is_empty() {
                                        cached_results.into_iter().map(bv).collect()
                                    } else {
                                        cached_results
                                            .into_iter()
                                            .filter_map(|v| {
                                                let tracked = active_tracked_vars();
                                                project_carrying_for_consumer(
                                                    &carrying_bindings,
                                                    &v,
                                                    tracked.as_deref(),
                                                    ctx.factory(),
                                                )
                                                .map(|projected| {
                                                    if projected.is_empty() {
                                                        bv(v)
                                                    } else {
                                                        bv_with(v, (*projected).clone())
                                                    }
                                                })
                                            })
                                            .collect()
                                    },
                                    env,
                                ),
                            });
                            continue;
                        }
                        Some(h)
                    } else {
                        None
                    };

                // Sub-expression tiered dispatch.
                //
                // Direct built-in forms still use the cheap per-slot counter path.
                // Closed, pure user-defined calls additionally record structural
                // hotness by content hash so recursive helpers whose calls allocate
                // fresh S-expressions can still warm up.
                if is_sexpr {
                    // HE-faithful binding-propagation guard: when
                    // collapse-bind is active or `carrying_bindings` is
                    // non-empty, per-branch bindings are semantically
                    // load-bearing. The bytecode VM and JIT are
                    // binding-agnostic (choice-point alternatives and
                    // CallNative dispatch carry bare values, not
                    // `BoundValue`s), so they would silently collapse
                    // per-branch bindings into empty bags — the same bug
                    // that `ProcessGroundedOpFanout` fixes in the
                    // tree-walker. Route such sub-expressions through
                    // the trampoline, which correctly fans out.
                    //
                    // Bisimilarity: outside these scopes, every VM-produced
                    // result would have been wrapped via `bv()` (empty
                    // bindings) anyway, so the VM path stays correct in
                    // the common case.
                    let bindings_load_bearing =
                        in_collapse_bind_scope() || !carrying_bindings.is_empty();

                    let has_grounded_args = if let Some(items) = value.as_sexpr() {
                        items
                            .iter()
                            .skip(1)
                            .any(|arg| super::engine::binding_value_needs_eval(arg))
                    } else {
                        false
                    };

                    if !bindings_load_bearing && !has_grounded_args {
                        let direct_compilable = crate::backend::bytecode::can_compile(&value);

                        if direct_compilable {
                            // Merged: increment per-slot exec counter AND read cached compilation hash
                            // in a single thread-local + generation check (vs 2× for separate calls).
                            let compilation_hash =
                                crate::backend::bytecode::tiered_cache::increment_and_get_hash(
                                    value.inner_ptr(),
                                );

                            // Try dispatching to compiled bytecode/JIT.
                            // hash != 0 guard short-circuits before any trait dispatch / DashMap lookup for cold code.
                            if compilation_hash != 0 {
                                if let Some((results, new_env)) =
                                    ctx.try_compiled_dispatch(&value, &env, compilation_hash)
                                {
                                    #[cfg(feature = "trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let output_tvs: Vec<trace_format::TraceValue> = results
                                                .iter()
                                                .map(|v| {
                                                    crate::backend::trace::trace_value_generic(v)
                                                })
                                                .collect();
                                            tc.emit_converted(
                                                trace_format::TraceTier::BytecodeVM,
                                                depth as u32,
                                                crate::backend::trace::trace_value_generic(&value),
                                                output_tvs,
                                                None,
                                                trace_format::TraceEventKind::TierDispatch {
                                                    expression_hash: compilation_hash,
                                                    selected_tier:
                                                        trace_format::TraceTier::BytecodeVM,
                                                    execution_count: 0,
                                                },
                                            );
                                        }
                                    }
                                    work_stack.push(WorkItem::Resume {
                                        result: (
                                            results.into_iter().map(bv).collect(),
                                            Arc::new(new_env),
                                        ),
                                    });
                                    continue; // Skip eval_step_generic — compiled code handled it
                                }
                            }
                        } else if should_record_execution_sample()
                            && should_memoize_with_env(&value, &*env)
                        {
                            // Phase 11.B (2026-05-17) — fetch the
                            // per-expression compilation state once,
                            // then read the three purity predicates
                            // from rule_epoch-tagged caches. On miss,
                            // compute, populate, return. Saves
                            // O(tree × needles × bloom) per sampled
                            // step on repeat-visited expressions
                            // (PLN's `BestCandidate` rule body is
                            // visited thousands of times per
                            // inference; once cached, the next
                            // visit returns in O(1)).
                            let cache = crate::backend::bytecode::global_tiered_cache();
                            let compilation_state = cache.record_execution(&value);
                            let rule_epoch = crate::backend::environment::rule_management::RULE_EPOCH
                                .load(Ordering::Acquire);
                            let has_overridden = compilation_state
                                .cached_has_overridden_grounded_op(rule_epoch)
                                .unwrap_or_else(|| {
                                    let r = crate::backend::eval::expression_has_overridden_grounded_op(
                                        &value, &*env,
                                    );
                                    compilation_state.set_has_overridden_grounded_op(rule_epoch, r);
                                    r
                                });
                            let has_meta_typed = !has_overridden && compilation_state
                                .cached_has_declared_meta_typed(rule_epoch)
                                .unwrap_or_else(|| {
                                    let r = crate::backend::eval::expression_has_declared_meta_typed_params(
                                        &value, &*env,
                                    );
                                    compilation_state.set_has_declared_meta_typed(rule_epoch, r);
                                    r
                                });
                            let involves_impure = !has_overridden && !has_meta_typed && compilation_state
                                .cached_involves_impure_rules(rule_epoch)
                                .unwrap_or_else(|| {
                                    let r = crate::backend::eval::expression_involves_impure_rules(
                                        &value, &*env,
                                    );
                                    compilation_state.set_involves_impure_rules(rule_epoch, r);
                                    r
                                });
                            let safe_to_dispatch = !has_overridden
                                && !has_meta_typed
                                && !involves_impure;

                            let compilable_with_env = safe_to_dispatch && compilation_state
                                .cached_compilable_with_env()
                                .unwrap_or_else(|| {
                                    let result =
                                        crate::backend::bytecode::can_compile_with_env(&value);
                                    compilation_state.set_compilable_with_env(result);
                                    result
                                });

                            if compilable_with_env {
                                let compilation_hash = compilation_state.expr_hash;
                                if let Some((results, new_env)) =
                                    crate::backend::bytecode::tiered_cache::try_sub_expr_env_dispatch_with_hash(
                                        compilation_hash,
                                        &value,
                                        &env,
                                    )
                                {
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    let output_tvs: Vec<trace_format::TraceValue> = results.iter()
                                        .map(|v| crate::backend::trace::trace_value_generic(v))
                                        .collect();
                                    tc.emit_converted(
                                        trace_format::TraceTier::BytecodeVM,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&value),
                                        output_tvs,
                                        None,
                                        trace_format::TraceEventKind::TierDispatch {
                                                    expression_hash: compilation_hash,
                                            selected_tier: trace_format::TraceTier::BytecodeVM,
                                                    execution_count: compilation_state.count(),
                                        },
                                    );
                                }
                            }
                            work_stack.push(WorkItem::Resume {
                                result: (results.into_iter().map(bv).collect(), Arc::new(new_env)),
                            });
                                    continue;
                                }
                            }
                        }
                    }
                }

                // Push MemoizeResult continuation if we got a cache miss on a
                // memoizable expression. When the evaluation resolves, this
                // continuation caches the results for future lookups.
                //
                // H9 (2026-05-05): elide when subgoal-table path already pushed
                // CompleteSubgoal — that handler writes the same hash to the
                // canonical SubgoalTable, dominating the EVAL_MEMO write.
                if let Some(h) = memo_hash {
                    if !subgoal_path_taken {
                        // Task #6 Phase 7 (2026-05-18): collapse duplicate
                        // MemoizeResult for the SAME hash. Self-recursive
                        // descent (`(rec) → (rec)`) would otherwise push
                        // one MemoizeResult per outer iteration (~48B each),
                        // leading to unbounded growth of the continuations
                        // Vec. A single MemoizeResult per distinct hash
                        // suffices: when it fires it caches results — any
                        // prior identical frame is redundant.
                        let already_pending = matches!(
                            continuations.last(),
                            Some(Continuation::MemoizeResult { expr_hash: prev, .. })
                                if *prev == h
                        );
                        if !already_pending {
                            continuations.push(Continuation::MemoizeResult {
                                expr_hash: h,
                                mutation_epoch: mutation_epoch(),
                                env: env.clone(),
                                depth,
                            });
                        }
                    }
                }

                // Save input pointer for fixpoint detection (Phase 9.5)
                let input_ptr = if is_sexpr {
                    value.inner_ptr()
                } else {
                    std::ptr::null()
                };

                // Phase 6 (Bug 1, trace-driven): substitute caller-side
                // bindings into `value` BEFORE eval_step_generic runs.
                //
                // Without this, free variables in `value` (e.g., `$a, $b` from
                // a cartesian-product combo) are NOT visible to rule body
                // materialization during step_sexpr → try_match_all_rules →
                // match_rules_native → apply_bindings_with_rename_scoped:
                // the body-local rename branch then freshens these caller-
                // side vars to `$__fr_<epoch>_*`, registering wildcard-LHS
                // rules via add-atom and triggering Truth_ModusPonens cascade
                // (4M+ self-recursing rule applications, observed via
                // trace-analyzer redundancy on /tmp/oom.trace).
                //
                // Substitution upfront makes the caller-side variables
                // concrete in `value`. The downstream rule body sees `$C`
                // bound to e.g. `(uncle b c)` instead of `(uncle $a $b)`,
                // so the registered rule has the correct concrete LHS.
                //
                // Trace evidence: trace-analyzer dump /tmp/oom.trace event
                // #56 showed `(uncle $__fr_4_a $__fr_4_b)` instead of the
                // expected `(uncle a b)` after the `=>` rule body fired.
                let cb_subst = &*carrying_bindings;
                let value = if cb_subst.is_empty() || !value.has_variables_fast() {
                    value
                } else {
                    crate::backend::eval::trampoline::apply_bindings(
                        &value,
                        cb_subst,
                        ctx.factory(),
                    )
                };

                // Perform one step of evaluation using generic step function
                let step_result = eval_step_generic(value, (*env).clone(), depth, ctx);
                let _ = is_tail_call; // Used to determine depth in push sites
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?step_result);

                // Process the step result
                match step_result {
                    // Direct result - resume continuation
                    GenericEvalStep::Done((values, step_env)) => {
                        // Phase 9.5: Fixpoint detection — if eval returned
                        // the same S-expression (by pointer), memoize it
                        if !input_ptr.is_null()
                            && values.len() == 1
                            && values[0].inner_ptr() == input_ptr
                        {
                            memoize_normal_form(&values[0]);
                        }
                        // Stage 1d-revised: tag each leaf result with the
                        // carrying bindings from this WorkItem::Eval. This
                        // is the HE-faithful propagation point — mirrors
                        // HE's `finished_result(atom, bindings)` where the
                        // current alternative's bindings travel with the
                        // produced atom. `carrying_bindings` was destructured
                        // from the WorkItem::Eval at the top of this arm.
                        let cb = &*carrying_bindings;
                        let result = if cb.is_empty() {
                            (values.into_iter().map(bv).collect(), Arc::new(step_env))
                        } else {
                            (
                                values.into_iter().map(|v| bv_with(v, cb.clone())).collect(),
                                Arc::new(step_env),
                            )
                        };
                        work_stack.push(WorkItem::Resume { result });
                    }

                    // Need to evaluate S-expression sub-items
                    GenericEvalStep::EvalSExpr {
                        items,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if items.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().sexpr(vec![]))], env),
                            });
                        } else {
                            let mut items_iter = items.into_iter();
                            let collect_capacity = items_iter.len(); // total count before consuming first
                            let first = items_iter.next().expect("items is non-empty");

                            continuations.push(Continuation::CollectSExpr {
                                remaining: items_iter,
                                collected: Vec::with_capacity(collect_capacity),
                                original_env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start a TCO grounded operation
                    // Uses static dispatch - works with any V: MettaValueTrait (NO conversion)
                    GenericEvalStep::StartGroundedOp {
                        state,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        let mut state = state;
                        // Use static dispatch - monomorphized for each value type
                        // Clone op_name to avoid borrow conflict with mutable state
                        let op_name = state.op_name.clone();
                        #[cfg(feature = "trace")]
                        let _grounded_start_ns =
                            { ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0) };
                        if let Some(work) = execute_grounded_op(&op_name, &mut state, ctx.factory())
                        {
                            match work {
                                GroundedWork::Done(results) => {
                                    // Results are already in correct type V - NO conversion
                                    let values: Vec<MettaValue> =
                                        results.into_iter().map(|(v, _)| v).collect();
                                    // Trace: GroundedOp success with duration
                                    #[cfg(feature = "trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let end_ns = tc.elapsed_ns();
                                            let duration =
                                                end_ns.saturating_sub(_grounded_start_ns);
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts =
                                                        Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() {
                                                        parts.push(arg.clone());
                                                    }
                                                    parts
                                                }),
                                            );
                                            tc.emit_timed(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                input,
                                                values.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                None,
                                                trace_format::TraceEventKind::GroundedOp {
                                                    op_name: op_name.clone(),
                                                    args: state.args.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                },
                                                _grounded_start_ns,
                                                Some(duration),
                                                None,
                                            );
                                        }
                                    }
                                    let cb = &*carrying_bindings;
                                    work_stack.push(WorkItem::Resume {
                                        result: (
                                            if cb.is_empty() {
                                                values.into_iter().map(bv).collect()
                                            } else {
                                                values
                                                    .into_iter()
                                                    .map(|v| bv_with(v, cb.clone()))
                                                    .collect()
                                            },
                                            env,
                                        ),
                                    });
                                }
                                GroundedWork::EvalArg {
                                    arg_idx,
                                    state: new_state,
                                } => {
                                    continuations.push(Continuation::ProcessGroundedOp {
                                        state: Box::new(new_state.clone()),
                                        pending_arg_idx: arg_idx,
                                        env: env.clone(),
                                        depth,
                                        // Stage 1d-revised: seed arg_bindings from
                                        // the current WorkItem's carrying so ambient
                                        // bindings (e.g., outer rule match) propagate
                                        // through grounded-op output.
                                        arg_bindings: carrying_bindings.clone(),
                                    });

                                    // Arg already in correct type V - NO conversion
                                    let arg_to_eval = new_state.args[arg_idx].clone();
                                    work_stack.push(WorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        demand: None,
                                        carrying_bindings: carrying_bindings.clone(),
                                    });
                                }
                                GroundedWork::Error(e) => {
                                    // Trace: GroundedOpError
                                    #[cfg(feature = "trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let (error_kind, message) = match &e {
                                                ExecError::NoReduce => ("NoReduce", String::new()),
                                                ExecError::Runtime(msg) => ("Runtime", msg.clone()),
                                                ExecError::Arithmetic(msg) => {
                                                    ("Arithmetic", msg.clone())
                                                }
                                                ExecError::IncorrectArgument(msg) => {
                                                    ("IncorrectArgument", msg.clone())
                                                }
                                                ExecError::Tagged(tag) => {
                                                    ("Tagged", tag.to_string())
                                                }
                                                ExecError::BadArgType { pos, expected, got } => (
                                                    "BadArgType",
                                                    format!(
                                                        "arg {} expected {} got {}",
                                                        pos, expected, got
                                                    ),
                                                ),
                                            };
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts =
                                                        Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() {
                                                        parts.push(arg.clone());
                                                    }
                                                    parts
                                                }),
                                            );
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                input,
                                                vec![],
                                                None,
                                                trace_format::TraceEventKind::GroundedOpError {
                                                    op_name: op_name.clone(),
                                                    error_kind: error_kind.to_string(),
                                                    message,
                                                    args: state.args.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                                },
                                            );
                                        }
                                    }
                                    match e {
                                        ExecError::NoReduce => {
                                            // MeTTa HE semantics: return the original expression unreduced
                                            let mut expr_parts =
                                                Vec::with_capacity(1 + state.args.len());
                                            expr_parts.push(ctx.factory().atom(&state.op_name));
                                            for arg in state.args.iter() {
                                                expr_parts.push(arg.clone());
                                            }
                                            let unreduced = ctx.factory().sexpr(expr_parts);
                                            work_stack.push(WorkItem::Resume {
                                                result: (smallvec![bv(unreduced)], env),
                                            });
                                        }
                                        _ => {
                                            // ERR-shape align (2026-05-16):
                                            // route through the centralized
                                            // `exec_error_to_value` so the
                                            // call form is the offending
                                            // operand (HE shape `(Error
                                            // <call> <detail>)`). The
                                            // previous inverted shape
                                            // `(Error ArithmeticError msg)`
                                            // is gone.
                                            let call_form = state.call_form(ctx.factory());
                                            let error_value =
                                                exec_error_to_value(&e, call_form, ctx.factory());
                                            work_stack.push(WorkItem::Resume {
                                                result: (smallvec![bv(error_value)], env),
                                            });
                                        }
                                    }
                                }
                            }
                        } else {
                            // Operation not in generic registry - report error
                            // All 14 standard TCO operations (arithmetic, comparison, logical) are in the
                            // generic registry. Custom operations should be added there, not to the
                            // legacy registry.
                            let error_value = ctx.factory().error(
                                ctx.factory().atom("OperationNotFoundError"),
                                ctx.factory().string(&format!(
                                    "Grounded operation '{}' not found in generic registry",
                                    op_name
                                )),
                            );
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(error_value)], env),
                            });
                        }
                    }

                    // Start let binding
                    GenericEvalStep::StartLetBinding {
                        pattern,
                        value_expr,
                        body,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessLet {
                            pending_values: None,
                            pattern,
                            body,
                            outer_bindings: None,
                            results: Vec::with_capacity(4),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Evaluate if branch (TCO)
                    GenericEvalStep::EvalIfBranch {
                        branch,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        work_stack.push(WorkItem::Eval {
                            value: branch,
                            env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Evaluate rule matches with unevaluated arguments (lazy evaluation)
                    // Note: matches are now in generic type (V, GenericBindings<V>, Option<V>)
                    // Phase 8.7: Prune matches whose rhs_type is incompatible with expected_type
                    GenericEvalStep::EvalRuleMatchesLazy {
                        mut matches,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        // 8.7: Branch pruning — filter out matches whose rhs_type
                        // is known to be incompatible with the expected_type
                        if let Some(ref expected) = expected_type {
                            let before_count = matches.len();

                            #[cfg(feature = "trace")]
                            let mut pruned_types: Vec<
                                Option<trace_format::TraceValue>,
                            > = Vec::new();

                            matches.retain(|(_rhs, _bindings, rhs_type)| {
                                let keep = match rhs_type {
                                    Some(rt) => types_match_generic(rt, expected),
                                    None => true, // Unknown type — don't prune (conservative)
                                };
                                #[cfg(feature = "trace")]
                                if !keep {
                                    pruned_types.push(
                                        rhs_type
                                            .as_ref()
                                            .map(crate::backend::trace::trace_value_generic),
                                    );
                                }
                                keep
                            });

                            // Emit BranchPrune trace event when pruning occurred
                            #[cfg(feature = "trace")]
                            {
                                if before_count != matches.len() {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            crate::backend::trace::trace_value_generic(expected),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::BranchPrune {
                                                expected_type:
                                                    crate::backend::trace::trace_value_generic(
                                                        expected,
                                                    ),
                                                pruned_count: (before_count - matches.len()) as u32,
                                                surviving_count: matches.len() as u32,
                                                pruned_types,
                                            },
                                        );
                                    }
                                }
                            }

                            let _ = before_count; // suppress unused warning when eval-trace is off
                        }

                        if matches.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                        } else {
                            // Strip rhs_type from 3-tuples → 2-tuples for unified dispatch
                            let matches_deque: Vec<_> = matches
                                .into_iter()
                                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                .collect();
                            dispatch_rule_matches(
                                matches_deque,
                                SmallVec::new(),
                                Arc::clone(&env),
                                depth,
                                ctx,
                                &mut work_stack,
                                &mut continuations,
                                demand,
                                &*carrying_bindings,
                            );
                        }
                    }

                    // Evaluate grounded arguments
                    GenericEvalStep::EvalGroundedArgs {
                        items,
                        grounded_indices,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if grounded_indices.is_empty() {
                            work_stack.push(WorkItem::Eval {
                                value: ctx.factory().sexpr(items),
                                env,
                                depth,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            let first_idx = grounded_indices[0];
                            let arg_to_eval = items[first_idx].clone();

                            // Phase 9.2: Derive expected_type from parent op's
                            // builtin signature for branch pruning (Phase 8.7).
                            let arg_expected_type = derive_arg_expected_type::<C>(
                                &items,
                                first_idx,
                                &env,
                                ctx.factory(),
                            );

                            let grounded_count = grounded_indices.len();
                            continuations.push(Continuation::CollectGroundedArg {
                                items,
                                grounded_indices,
                                current_idx: 0,
                                evaluated_results: Vec::with_capacity(grounded_count),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: arg_to_eval,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: arg_expected_type,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start map-atom
                    GenericEvalStep::StartMapAtom {
                        elements,
                        var_name,
                        template,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if elements.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().sexpr(vec![]))], env),
                            });
                        } else {
                            let mut remaining = elements.into_iter();
                            let map_capacity = remaining.len(); // total before consuming first
                            let first = remaining.next().expect("elements is non-empty");

                            continuations.push(Continuation::ProcessMapAtom {
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                template: template.clone(),
                                collected_results: Vec::with_capacity(map_capacity),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                                acc_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
                            });

                            // Substitute variable and evaluate - NO CONVERSION NEEDED
                            let instantiated = substitute_variable_generic(
                                &template,
                                &var_name,
                                &first,
                                ctx.factory(),
                            );

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start filter-atom
                    GenericEvalStep::StartFilterAtom {
                        elements,
                        var_name,
                        predicate,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if elements.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().sexpr(vec![]))], env),
                            });
                        } else {
                            let mut remaining = elements.into_iter();
                            let filter_capacity = remaining.len(); // total before consuming first
                            let first = remaining.next().expect("elements is non-empty");

                            continuations.push(Continuation::ProcessFilterAtom {
                                current_element: Some(first.clone()),
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                predicate: predicate.clone(),
                                filtered_results: Vec::with_capacity(filter_capacity),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                                acc_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
                            });

                            // NO CONVERSION NEEDED - use generic substitute
                            let instantiated = substitute_variable_generic(
                                &predicate,
                                &var_name,
                                &first,
                                ctx.factory(),
                            );

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                // Phase 9.2d: filter-atom predicate should return Bool
                                expected_type: Some(ctx.factory().atom("Bool")),
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start foldl-atom
                    GenericEvalStep::StartFoldlAtom {
                        elements,
                        init,
                        acc_var_name,
                        item_var_name,
                        operation,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if elements.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(init)], env),
                            });
                        } else {
                            let mut remaining = elements.into_iter();
                            let first = remaining.next().expect("elements is non-empty");

                            continuations.push(Continuation::ProcessFoldlAtom {
                                remaining_elements: remaining,
                                acc_var_name: acc_var_name.clone(),
                                item_var_name: item_var_name.clone(),
                                operation: operation.clone(),
                                env: env.clone(),
                                depth,
                                // Stage 1d-revised: seed acc_bindings from carrying.
                                acc_bindings: carrying_bindings.clone(),
                            });

                            // NO CONVERSION NEEDED - use generic substitute for both variables
                            let instantiated = substitute_variable_generic(
                                &operation,
                                &acc_var_name,
                                &init,
                                ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated,
                                &item_var_name,
                                &first,
                                ctx.factory(),
                            );

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start sort-tuple (insertion sort via trampoline)
                    GenericEvalStep::StartSortTuple {
                        elements,
                        var1_name,
                        var2_name,
                        comparator,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if elements.len() <= 1 {
                            // 0 or 1 elements — already sorted
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().sexpr(elements))], env),
                            });
                        } else {
                            // Start insertion sort: first element is trivially sorted,
                            // take the second element as 'current' to insert.
                            let mut unsorted_iter = elements.into_iter();
                            let first = unsorted_iter.next().expect("at least 2 elements");
                            let current = unsorted_iter.next().expect("at least 2 elements");
                            let unsorted: Vec<_> = unsorted_iter.collect();

                            // Compare current vs sorted[0] (= first)
                            let instantiated = substitute_variable_generic(
                                &comparator,
                                &var1_name,
                                &current,
                                ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated,
                                &var2_name,
                                &first,
                                ctx.factory(),
                            );

                            continuations.push(Continuation::ProcessSortTuple {
                                sorted: vec![first],
                                unsorted,
                                current,
                                insert_pos: 0,
                                var1_name,
                                var2_name,
                                comparator,
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start best-candidate (linear scan via trampoline)
                    GenericEvalStep::StartBestCandidate {
                        elements,
                        var_name,
                        rank_fn,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if elements.is_empty() {
                            // Empty tuple — return Unit
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().unit())], env),
                            });
                        } else {
                            let mut remaining = elements.into_iter();
                            let first = remaining.next().expect("non-empty");

                            // Evaluate rank function for first element
                            let instantiated = substitute_variable_generic(
                                &rank_fn,
                                &var_name,
                                &first,
                                ctx.factory(),
                            );

                            continuations.push(Continuation::ProcessBestCandidate {
                                best: None,
                                best_rank: None,
                                remaining,
                                current: first,
                                var_name,
                                rank_fn,
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Evaluate if condition
                    GenericEvalStep::EvalIfCondition {
                        condition,
                        then_branch,
                        else_branch,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        // Fast path: literal Bool condition — skip continuation + work item
                        if let Some(is_true) = condition.as_bool() {
                            let branch = if is_true { then_branch } else { else_branch };
                            work_stack.push(WorkItem::Eval {
                                value: branch,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: Some(crate::backend::eval::cesk::coroutine::Demand::All),
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            let outer_demand = CURRENT_DEMAND
                                .with(|d| d.get())
                                .unwrap_or(crate::backend::eval::cesk::coroutine::Demand::All);

                            continuations.push(Continuation::ProcessIfCondition {
                                then_branch,
                                else_branch,
                                outer_bindings: None,
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                                outer_demand,
                            });

                            // 8.7: if-condition always expects Bool — prune non-Bool branches.
                            // Demand::Exactly(1): `if` only examines the first result
                            // for boolean test (line ~4918: `cond_results.first()`).
                            work_stack.push(WorkItem::Eval {
                                value: condition,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: Some(ctx.factory().atom("Bool")),
                                demand: Some(
                                    crate::backend::eval::cesk::coroutine::Demand::Exactly(1),
                                ),
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Evaluate case atom
                    GenericEvalStep::EvalCaseAtom {
                        atom,
                        cases,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessCaseAtom {
                            cases,
                            outer_bindings: None,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Switch: pattern match WITHOUT evaluating atom
                    GenericEvalStep::SwitchAtom {
                        atom,
                        cases,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        // Switch does NOT evaluate atom - pattern match directly
                        match eval_switch(&atom, &cases, ctx.factory()) {
                            SwitchResult::Match(template, _bindings) => {
                                // Template needs evaluation
                                work_stack.push(WorkItem::Eval {
                                    value: template,
                                    env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                            SwitchResult::Error(err) => {
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![bv(err)], env),
                                });
                            }
                            SwitchResult::NoMatch => {
                                // No case matched - prune branch (MeTTa HE returns Empty)
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), env),
                                });
                            }
                        }
                    }

                    // Evaluate eval (internal full-reduction path — used by
                    // progn, metta, capture, reduce).
                    //
                    // T04/105 (2026-05-17): `original_eval_expr: None` here —
                    // EvalEval is for non-`eval` forms (capture/reduce/progn)
                    // which do NOT undergo HE `metta_call_return`'s
                    // NotReducible→original conversion at this level.
                    GenericEvalStep::EvalEval {
                        arg,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                            original_eval_expr: None,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Plan S4 (2026-05-14) — HE-faithful one-step `(eval X)`.
                    //
                    // Mirrors `hyperon-experimental/lib/src/metta/interpreter.rs::eval_impl`:
                    //   1. Apply outer bindings to `arg`.
                    //   2. If `arg` (post-binding) is a variable or scalar
                    //      grounded value (Bool/Long/Float/String/Unit) → emit
                    //      `NotReducible` finished sentinel. This matches spec
                    //      tests 069 (`!(eval 42)` → `[NotReducible]`) and 070
                    //      (`!(eval $x)` → unconstrained, NotReducible OK).
                    //   3. If `arg` is an S-expression with a variable head →
                    //      emit `NotReducible` (HE `is_variable_op` short-circuit
                    //      at `eval_impl` line 606-611).
                    //   4. Otherwise → defer to the same full-reduction flow as
                    //      `EvalEval`. HE's eval_impl pushes the result back to
                    //      the interpret-loop stack, which produces transitive
                    //      reduction equivalent to MeTTaTron's trampoline-driven
                    //      WorkItem::Eval recursion. Spec tests 003
                    //      (`!(eval (+ 1 2))` → `[3]`) and 005
                    //      (`!(eval (eval (+ 1 2)))` → `[3]`) rely on this
                    //      transitive behavior.
                    //
                    // Risk mitigation: `NotReducible` is emitted ONLY from
                    // variable/scalar/var-head branches, never from `if`
                    // non-Bool, `case` default, or `collapse-bind` cardinality.
                    // This is the precise scope per the S4 risk-mitigation
                    // protocol (see /home/dylon/.claude/projects/.../memory/
                    // feedback-pln-historical-memory.md re: 2026-04-26 OOM).
                    GenericEvalStep::EvalEvalStep {
                        arg,
                        env: step_env,
                        depth,
                    } => {
                        // Apply outer bindings (HE eval_impl line 505 parity).
                        let resolved = if carrying_bindings.is_empty() {
                            arg
                        } else {
                            apply_bindings(&arg, &*carrying_bindings, ctx.factory())
                        };

                        // T04/105 (2026-05-17): construct original `(eval <arg>)`
                        // expression for HE `metta_call_return` parity. When the
                        // eval result is NotReducible, we substitute back to
                        // this expression. Use the RESOLVED arg (post-binding)
                        // so the original captures any caller-side variable
                        // substitutions HE would apply via apply_bindings_to_atom_move
                        // before quoting back. Empirical HE: `(eval (eval 5))`
                        // → `[(eval (eval 5))]` — bindings-applied inner.
                        let original_eval_expr = ctx.factory().sexpr(vec![
                            ctx.factory().atom("eval"),
                            resolved,
                        ]);

                        // Detect nesting: if the top continuation is already
                        // a ProcessEvalEval, this eval is nested under another
                        // eval. In HE, the inner eval's NotReducible propagates
                        // directly to the outer's metta_call_return (no inner
                        // conversion). MTT mirrors this by propagating
                        // NotReducible raw to the outer ProcessEvalEval.
                        //
                        // For standalone/top-level eval (top continuation is
                        // Done/ProcessLet/etc.), we MUST do the conversion
                        // here because there's no outer ProcessEvalEval to do it.
                        let nested_in_eval = matches!(
                            continuations.last(),
                            Some(Continuation::ProcessEvalEval { .. })
                        );

                        // Classify resolved argument.
                        // Use ValueView for exhaustive, NaN-box-ready dispatch.
                        let view = resolved.view();
                        let immediate_not_reducible = match view {
                            // Variable atoms → NotReducible (HE is_variable_op).
                            crate::backend::models::metta_value::ValueView::Atom(name) => {
                                name.starts_with('$')
                            }
                            // Grounded scalars → NotReducible (HE: no
                            // `(= scalar X)` rule matches a scalar literal).
                            crate::backend::models::metta_value::ValueView::Bool(_)
                            | crate::backend::models::metta_value::ValueView::Long(_)
                            | crate::backend::models::metta_value::ValueView::Float(_)
                            | crate::backend::models::metta_value::ValueView::String(_)
                            | crate::backend::models::metta_value::ValueView::Unit => true,
                            // SExpr with variable head → NotReducible
                            // (HE is_variable_op_expr at line 596-602).
                            // Also: `(quote X)` is self-evaluating in HE —
                            // `(eval (quote X))` returns NotReducible at the
                            // kernel level, then `metta_call_return` wraps
                            // back to `(eval (quote X))`. Verified empirically:
                            //   metta-repl '!(eval (quote (+ 1 2)))'
                            //   → [(eval (quote (+ 1 2)))]
                            crate::backend::models::metta_value::ValueView::SExpr(items) => {
                                items.first().map_or(false, |head| {
                                    match head.view() {
                                        crate::backend::models::metta_value::ValueView::Atom(n) => {
                                            n.starts_with('$') || n == "quote"
                                        }
                                        _ => false,
                                    }
                                })
                            }
                            // NotReducible argument is itself NotReducible (idempotent).
                            crate::backend::models::metta_value::ValueView::NotReducible => true,
                            // `(quote X)` parsed as Quoted variant is self-evaluating
                            // in HE — `(eval (quote X))` returns NotReducible at the
                            // kernel level, then `metta_call_return` wraps back to
                            // `(eval (quote X))`. Verified empirically:
                            //   metta-repl '!(eval (quote (+ 1 2)))'
                            //   → [(eval (quote (+ 1 2)))]
                            crate::backend::models::metta_value::ValueView::Quoted(_) => true,
                            // Other variants (Error, Type, Conjunction, Space, etc.)
                            // are passed through to the normal eval path.
                            _ => false,
                        };

                        if immediate_not_reducible {
                            // HE eval_impl line 555 query → NotReducible.
                            // If nested under outer eval, propagate NotReducible
                            // (outer ProcessEvalEval will convert at its level).
                            // Otherwise, convert here (HE metta_call_return
                            // semantics: NotReducible → original eval call).
                            let env: SharedEnv = Arc::new(step_env);
                            let result_value = if nested_in_eval {
                                ctx.factory().not_reducible()
                            } else {
                                original_eval_expr
                            };
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(result_value)], env),
                            });
                            continue;
                        }

                        // Bare non-variable atom (Symbol): query rules
                        // directly. HE's `eval_impl` falls through to
                        // `query(space, ...)` for symbol atoms (line 555).
                        // If no rule matches, HE's `query` returns
                        // `NotReducible` (line 633-634).
                        if let crate::backend::models::metta_value::ValueView::Atom(_) = view {
                            // Non-variable atom (variable case handled above).
                            // Use the trampoline's rule-match machinery via
                            // the standard Eval path; we'll detect "no rule
                            // matched and result == input" downstream in
                            // ProcessEvalEval. For now, the cheapest correct
                            // implementation: look up rules with arity 0.
                            let matches = try_match_rules_with_bindings(
                                &resolved,
                                &*carrying_bindings,
                                resolved.as_atom().expect("Atom view guarantees as_atom"),
                                0,
                                &step_env,
                                ctx.factory(),
                            );
                            match matches {
                                Some(ms) if ms.is_empty() => {
                                    // No rule matched → HE metta_call_return:
                                    // NotReducible. Convert here if standalone,
                                    // propagate raw if nested under outer eval.
                                    let env: SharedEnv = Arc::new(step_env);
                                    let result_value = if nested_in_eval {
                                        ctx.factory().not_reducible()
                                    } else {
                                        original_eval_expr
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        result: (smallvec![bv(result_value)], env),
                                    });
                                    continue;
                                }
                                Some(ms) => {
                                    // Matched — dispatch each RHS as a
                                    // result. Use existing dispatch helper
                                    // for non-det fan-out.
                                    // We push our own ProcessEvalEval (this
                                    // eval is "outermost" for the rule body's
                                    // sub-evaluations). The push happens
                                    // unconditionally regardless of nesting —
                                    // the outer's ProcessEvalEval will see
                                    // OUR ProcessEvalEval's result (not the
                                    // raw NotReducible from a nested case).
                                    let env: SharedEnv = Arc::new(step_env);
                                    continuations.push(Continuation::ProcessEvalEval {
                                        env: env.clone(),
                                        depth,
                                        outer_carrying: carrying_bindings.clone(),
                                        original_eval_expr: Some(original_eval_expr),
                                    });
                                    dispatch_rule_matches(
                                        ms,
                                        SmallVec::new(),
                                        Arc::clone(&env),
                                        depth + 1,
                                        ctx,
                                        &mut work_stack,
                                        &mut continuations,
                                        None,
                                        &*carrying_bindings,
                                    );
                                    continue;
                                }
                                None => {
                                    // Index inconclusive — fall through to
                                    // standard eval path below.
                                }
                            }
                        }

                        // Otherwise → defer to standard full-reduction flow.
                        // HE's interpret-loop produces transitive reduction
                        // through repeated stack push/pop; MeTTaTron mirrors
                        // this via WorkItem::Eval recursion driven by
                        // dispatch_rule_matches' RHS push.
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                            original_eval_expr: Some(original_eval_expr),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: resolved,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Evaluate return
                    GenericEvalStep::EvalReturn {
                        value,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessReturn {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start chain
                    //
                    // Spec §06.6.3 E-CHAIN-SUBST-DONE: chain dispatches expr by
                    // ONE kernel step — if expr's head is in §06.3.4's kernel-op
                    // whitelist (eval/chain/unify/cons-atom/decons-atom/function/
                    // collapse-bind/superpose-bind/metta/call-native/
                    // context-space), evaluate it. Otherwise expr is data and
                    // binds as-is to $var.
                    //
                    // HE bisimilarity: HE's `chain_to_stack → atom_to_stack`
                    // (interpreter.rs:657-673, 640-655) implements the same
                    // gate. The "auto-eval" users observe at the REPL is from
                    // `wrap_atom_by_metta_interpreter` (runner/mod.rs:1214) —
                    // the runner wraps the user atom in `(metta atom %Undefined% space)`
                    // before chain even sees it. Chain itself does NOT auto-eval.
                    //
                    // Without this gate, chain unboundedly cascades user-rule
                    // recursion (PLN Toothbrush consumed 21GB RAM in 19 min).
                    GenericEvalStep::StartChain {
                        expr,
                        var,
                        body,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);

                        let head_is_kernel = expr
                            .as_sexpr()
                            .and_then(|items| items.first())
                            .and_then(|h| h.as_atom())
                            .map(is_embedded_kernel_op)
                            .unwrap_or(false);

                        if head_is_kernel {
                            // Kernel op: dispatch one kernel step via Eval.
                            continuations.push(Continuation::ProcessChainExpr {
                                var,
                                body,
                                outer_bindings: None,
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: expr,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            // Data: substitute $var → expr in body, evaluate body.
                            let var_name = var.as_atom().unwrap_or("");
                            let instantiated =
                                substitute_variable_generic(&body, var_name, &expr, ctx.factory());
                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start function
                    GenericEvalStep::StartFunction {
                        expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessFunction {
                            iteration_count: 1,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Evaluate is-error
                    GenericEvalStep::EvalIsError {
                        expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessIsError {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start catch
                    GenericEvalStep::StartCatch {
                        expr,
                        default,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessCatch {
                            default,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start freeze-tuple: evaluate reducible args, then
                    // construct tuple and mark as normal form.
                    GenericEvalStep::StartFreezeTuple {
                        args,
                        reducible_indices,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        let first_idx = reducible_indices[0];
                        let arg_to_eval = args[first_idx].clone();

                        continuations.push(Continuation::CollectFreezeArgs {
                            args,
                            reducible_indices,
                            current_idx: 0,
                            evaluated_results: Vec::new(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: arg_to_eval,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start conjunction
                    GenericEvalStep::StartConjunction {
                        goals,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if goals.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(ctx.factory().unit())], env),
                            });
                        } else if goals.len() == 1 {
                            work_stack.push(WorkItem::Eval {
                                value: goals.into_iter().next().expect("goals.len() == 1"),
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            let mut remaining = goals.into_iter();
                            let conj_capacity = remaining.len(); // total before consuming first
                            let first_goal = remaining.next().expect("non-empty");

                            continuations.push(Continuation::ProcessConjunction {
                                remaining_goals: remaining,
                                accumulated_results: Vec::with_capacity(conj_capacity),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: first_goal,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Start unify
                    GenericEvalStep::StartUnify {
                        pattern1,
                        pattern2,
                        success_body,
                        failure_body,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessUnifyPattern1 {
                            pattern2,
                            success_body,
                            failure_body,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: pattern1,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start collapse
                    GenericEvalStep::StartCollapse {
                        expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessCollapse {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: Some(crate::backend::eval::cesk::coroutine::Demand::All),
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start collapse-bind
                    GenericEvalStep::StartCollapseBind {
                        expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);

                        // Push a binding capture frame if the expression has
                        // free variables. Free vars are collected from the
                        // EFFECTIVE expression — `expr` after substitution via
                        // `carrying_bindings`. Without this, macro parameters
                        // like `$term` would be captured instead of the
                        // underlying query variables (e.g. `$who`) that the
                        // caller actually wants to track. This matches HE's
                        // behavior where `apply_bindings_to_atom_move` runs
                        // before `collapse` inspects the atom.
                        let effective_expr = if carrying_bindings.is_empty() {
                            expr.clone()
                        } else {
                            crate::backend::eval::bindings::apply_bindings_generic(
                                &expr,
                                &*carrying_bindings,
                                ctx.factory(),
                            )
                        };
                        if effective_expr.has_variables_fast() {
                            let free_vars = effective_expr.free_variables();
                            if !free_vars.is_empty() {
                                let tracked: SmallVec<[&'static str; 4]> =
                                    free_vars.into_iter().collect();
                                push_binding_capture_frame(tracked);
                            }
                        }

                        continuations.push(Continuation::ProcessCollapseBind {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: Some(crate::backend::eval::cesk::coroutine::Demand::All),
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // S5: Start superpose-bind. Decompose a collapse-bind-shaped
                    // argument `((atom (Bindings ...)) (atom (Bindings ...)) ...)`
                    // into bare nondet results, merging each result's saved
                    // bindings with the caller's `carrying_bindings`.
                    //
                    // HE reference: lib/src/metta/interpreter.rs:893-918
                    //   collapsed.into_children().into_iter()
                    //     .map(atom_into_atom_bindings)
                    //     .flat_map(|(atom, b)| b.merge(&bindings) ...)
                    GenericEvalStep::StartSuperposeBind {
                        arg,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);

                        // Decompose the collapsed arg. It should be an SExpr
                        // whose children are `(atom (Bindings ...))` pairs.
                        // Preallocate to children.len() to avoid reallocation.
                        let pairs: Vec<BoundValue> = if let Some(children) = arg.as_sexpr() {
                            let mut out: Vec<BoundValue> = Vec::with_capacity(children.len());
                            for child in children.iter() {
                                if let Some(items) = child.as_sexpr() {
                                    // HE-bisim binding-pair shapes accepted:
                                    //   2 children: `(value (Bindings ...))` — explicit
                                    //                non-empty binding SExpr.
                                    //   3 children: `(value { })` — HE-style empty
                                    //                bindings rendered as two atoms
                                    //                `{` and `}` (T04/025, T04/026).
                                    //                Same encoding used by collapse-bind.
                                    let is_empty_curly = items.len() == 3
                                        && items[1].as_atom() == Some("{")
                                        && items[2].as_atom() == Some("}");
                                    if is_empty_curly {
                                        out.push(bv(items[0].clone()));
                                    } else if items.len() == 2 {
                                        // (atom (Bindings ...))
                                        let atom = items[0].clone();
                                        let bindings_sexpr = &items[1];
                                        let bindings =
                                            crate::backend::eval::trampoline::eval_loop::decode_bindings_from_sexpr(
                                                bindings_sexpr,
                                                ctx.factory(),
                                            );
                                        out.push(crate::backend::eval::trampoline::types::bv_with(
                                            atom, bindings,
                                        ));
                                    } else {
                                        // Other shapes — treat as raw atom with
                                        // empty bindings (forgiving HE).
                                        out.push(bv(child.clone()));
                                    }
                                } else {
                                    out.push(bv(child.clone()));
                                }
                            }
                            out
                        } else {
                            // Non-SExpr arg: treat as a single result with empty bindings.
                            vec![bv(arg.clone())]
                        };

                        if pairs.is_empty() {
                            // Empty collapse-bind: empty nondet output.
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                        } else {
                            // Re-use ProcessAmb's per-branch dispatch machinery:
                            // each (atom, bindings) pair becomes one alt with
                            // its saved bindings composed onto outer_carrying.
                            let amb_capacity = pairs.len();
                            let mut alts_iter = pairs.into_iter();
                            let (first_val, first_b) =
                                alts_iter.next().expect("pairs is non-empty");

                            continuations.push(Continuation::ProcessAmb {
                                remaining_alts: alts_iter,
                                results: Vec::with_capacity(amb_capacity),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            // Compose outer_carrying with the first pair's
                            // saved bindings, mirroring HE's b.merge(&bindings).
                            let alt_carrying: SharedBindings = if first_b.is_empty() {
                                carrying_bindings.clone()
                            } else if carrying_bindings.is_empty() {
                                std::sync::Arc::new(first_b)
                            } else {
                                std::sync::Arc::new(
                                    crate::backend::eval::bindings::compose_outer_inner_generic(
                                        &*carrying_bindings,
                                        &first_b,
                                        ctx.factory(),
                                    ),
                                )
                            };

                            work_stack.push(WorkItem::Eval {
                                value: first_val,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: alt_carrying,
                            });
                        }
                    }

                    // Start amb
                    GenericEvalStep::StartAmb {
                        alternatives,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        if alternatives.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                        } else {
                            // ── Parallel path: when inside a collapse barrier, evaluate
                            // all superpose alternatives concurrently ──
                            let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
                            // Parallelize superpose alternatives when budget allows.
                            // MeTTa evaluation is pure (read-only env during eval),
                            // so alternatives are independent. Budget + depth decay +
                            // queue pressure backoff prevent over-parallelization.
                            // WFST classification: only parallelize when branches
                            // justify the dispatch overhead.
                            let wfst_allows = if alternatives.len() >= 2 {
                                let scheduler = crate::backend::scheduler::global_scheduler();
                                let degree_ok = alternatives.iter().any(|alt| {
                                    let (_, action) = scheduler.classify_and_transduce(alt);
                                    action.parallelism_degree > 1
                                });
                                // H2: branch-purity gate (spec §5.6.1).
                                // Phase 10.D: state-mutation only by
                                // default; opt-in I/O strictness via
                                // METTATRON_STRICT_PRINT_ORDER=1.
                                let all_pure = alternatives.iter().all(|alt| {
                                    !crate::backend::scheduler::classification::body_blocks_parallel_dispatch(alt, 8)
                                });
                                degree_ok && all_pure
                            } else {
                                false
                            };

                            let par_budget = if wfst_allows
                                && current_depth < max_parallel_depth()
                                && global_eval_pool().active_workers() > 0
                            {
                                try_acquire_budget((alternatives.len() - 1) as u32, current_depth)
                            } else {
                                0
                            };

                            if par_budget > 0 {
                                // Trace: NondeterministicFork (parallel amb)
                                #[cfg(feature = "trace")]
                                {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            trace_format::TraceValue::Unit,
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::NondeterministicFork {
                                                branch_count: alternatives.len() as u32,
                                            },
                                        );
                                    }
                                }

                                let metta_env = (*env).clone();
                                // StartAmb (superpose) propagates the outer
                                // demand: if the caller (e.g. `(if (not (==
                                // <superpose-result> ())) ...)`) only needs
                                // a single non-empty witness, the first
                                // satisfying alternative cancels its siblings
                                // via the CancelToken. When wrapped in
                                // `collapse`/`collapse-bind`, those handlers
                                // shadow CURRENT_DEMAND back to `All` before
                                // dispatching the body, so this read sees
                                // `All` in those contexts (no false pruning).
                                let amb_demand = CURRENT_DEMAND
                                    .with(|d| d.get())
                                    .unwrap_or(crate::backend::eval::cesk::coroutine::Demand::All);
                                let branches: Vec<ParallelBranch> = alternatives
                                    .into_iter()
                                    .map(|alt| (alt, empty_shared_bindings()))
                                    .collect();

                                // **Stack-safety mandate (2026-05-15)**:
                                // trampolinized dispatch via WaitForParallel.
                                // Phase 8: share Arc with RootProvider.
                                let stable_branches_snapshot =
                                    std::sync::Arc::new(branches);
                                let handle = parallel_dispatch(
                                    std::sync::Arc::clone(&stable_branches_snapshot),
                                    metta_env,
                                    par_budget,
                                    current_depth,
                                    amb_demand,
                                );
                                let env_for_resume = env.clone();
                                continuations.push(Continuation::WaitForParallel {
                                    handle,
                                    merge_mode:
                                        crate::backend::eval::trampoline::types::ParallelMergeMode::AmbConcat,
                                    base_results: SmallVec::new(),
                                    outer_carrying: carrying_bindings.clone(),
                                    env,
                                    depth,
                                    budget_acquired: par_budget,
                                    caller_depth: current_depth,
                                    stable_branches_snapshot,
                                });
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), env_for_resume),
                                });
                            } else {
                                // ── Sequential path (original) ──
                                // Wrap each raw alt value as a BoundValue with
                                // empty per-branch bindings. All alts at this
                                // level share the outer_carrying; there are no
                                // per-branch bindings to attach here.
                                let amb_capacity = alternatives.len();
                                let mut alts_iter = alternatives
                                    .into_iter()
                                    .map(bv)
                                    .collect::<Vec<_>>()
                                    .into_iter();
                                let (first_val, _first_b) =
                                    alts_iter.next().expect("alternatives is non-empty");

                                continuations.push(Continuation::ProcessAmb {
                                    remaining_alts: alts_iter,
                                    results: Vec::with_capacity(amb_capacity),
                                    env: env.clone(),
                                    depth,
                                    outer_carrying: carrying_bindings.clone(),
                                });

                                work_stack.push(WorkItem::Eval {
                                    value: first_val,
                                    env,
                                    depth: depth + 1,
                                    is_tail_call: false,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                        }
                    }

                    // Start guard
                    GenericEvalStep::StartGuard {
                        condition,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessGuard {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start get-atoms
                    GenericEvalStep::StartGetAtoms {
                        space_ref,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessGetAtoms {
                            space_ref: space_ref.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start memo
                    GenericEvalStep::StartMemo {
                        memo_ref,
                        expr,
                        first_only,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessMemoTable {
                            memo_ref: memo_ref.clone(),
                            expr,
                            first_only,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start new-memo
                    GenericEvalStep::StartNewMemo {
                        name_arg,
                        size_arg,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessNewMemoName {
                            name_arg: name_arg.clone(),
                            size_arg,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: name_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start memo operation
                    GenericEvalStep::StartMemoOp {
                        memo_ref,
                        op_type,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        let is_clear = matches!(op_type, super::super::step::MemoOpType::Clear);
                        continuations.push(Continuation::ProcessMemoOp {
                            memo_ref: memo_ref.clone(),
                            is_clear,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start match
                    GenericEvalStep::StartMatch {
                        space_arg,
                        pattern,
                        template,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessMatchSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            template,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start add-atom
                    GenericEvalStep::StartAddAtom {
                        space_ref,
                        atom,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessAddAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start remove-atom
                    GenericEvalStep::StartRemoveAtom {
                        space_ref,
                        atom,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessRemoveAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start new-state
                    GenericEvalStep::StartNewState {
                        initial_value,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessNewState {
                            initial_value: initial_value.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: initial_value,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start get-state
                    GenericEvalStep::StartGetState {
                        state_ref,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessGetState {
                            state_ref: state_ref.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start change-state
                    GenericEvalStep::StartChangeState {
                        state_ref,
                        new_value,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessChangeStateRef {
                            state_ref: state_ref.clone(),
                            new_value,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start repr
                    GenericEvalStep::StartRepr {
                        atom,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessRepr {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start format-args
                    GenericEvalStep::StartFormatArgs {
                        format_arg,
                        args_arg,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessFormatArgsString {
                            format_arg: format_arg.clone(),
                            args_arg,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: format_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start println
                    GenericEvalStep::StartPrintln {
                        atom,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessPrintln {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start trace
                    GenericEvalStep::StartTrace {
                        message,
                        value_expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessTraceMessage {
                            message: message.clone(),
                            value_expr,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: message,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start get-metatype
                    GenericEvalStep::StartGetMetatype {
                        atom,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessGetMetatype {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start bind
                    GenericEvalStep::StartBind {
                        token,
                        atom_expr,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessBind {
                            token,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom_expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start if-reducible: evaluate expr, then compare to original
                    GenericEvalStep::EvalIfReducible {
                        expr,
                        then_branch,
                        else_branch,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessIfReducible {
                            original_expr: expr.clone(),
                            then_branch,
                            else_branch,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }

                    // Start match-or: evaluate space, then match with default fallback
                    GenericEvalStep::StartMatchOr {
                        space_arg,
                        pattern,
                        default,
                        template,
                        env: step_env,
                        depth,
                    } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessMatchOrSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            default,
                            template,
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                    }
                }
            }

            // ── Lazy binding: evaluate template with deferred bindings ──
            //
            // Instead of eagerly materializing the entire expression tree via
            // `apply_bindings` (O(tree_depth) recursive alloc), we carry
            // `(template, bindings)` and resolve lazily. For nested `let*` chains,
            // this reduces O(N^2) tree materialization to O(N) by composing
            // bindings at each level and only materializing the innermost body.
            WorkItem::EvalWithBindings {
                template,
                bindings,
                env,
                depth,
                is_tail_call,
                expected_type,
                carrying_bindings,
            } => {
                // Fast path: empty bindings or no variables → just Eval
                if bindings.is_empty() || !template.has_variables_fast() {
                    work_stack.push(WorkItem::Eval {
                        value: template,
                        env,
                        depth,
                        is_tail_call,
                        expected_type,
                        demand: None,
                        carrying_bindings: carrying_bindings.clone(),
                    });
                    continue;
                }

                // Compose deferred bindings into carrying_bindings so
                // downstream handlers (especially StartCollapseBind's
                // tracked_vars extraction) can resolve macro parameters
                // like `$term → (grandfather $who c)`. Without this,
                // any handler that inspects carrying_bindings to resolve
                // variables sees only the ambient carrying — deferred
                // bindings would stay invisible until apply_bindings
                // materializes the template at leaf nodes.
                //
                // Spec §04.4.3 A-VE-CLASS-MERGE-INCOMPAT: if outer+inner
                // conflict on a ground-ground binding, the branch dies.
                // Use strict compose; on None, skip this plan item.
                let carrying_bindings: crate::backend::eval::trampoline::types::SharedBindings =
                    if carrying_bindings.is_empty() {
                        std::sync::Arc::new((*bindings).clone())
                    } else {
                        match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                            &*carrying_bindings,
                            &*bindings,
                            ctx.factory(),
                        ) {
                            Some(b) => std::sync::Arc::new(b),
                            None => {
                                // Branch dies (§04.4.3); drop this plan item.
                                continue;
                            }
                        }
                    };
                let tracked = active_tracked_vars();
                let carrying_bindings = match project_carrying_for_consumer(
                    &carrying_bindings,
                    &template,
                    tracked.as_deref(),
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => continue,
                };
                let bindings = match project_owned_bindings_for_consumer(
                    &bindings,
                    &template,
                    tracked.as_deref(),
                    ctx.factory(),
                ) {
                    Some(b) => std::sync::Arc::new(b),
                    None => continue,
                };

                // I-6: ThunkTable lookup — check if (template, bindings) was previously evaluated.
                // Only at depth >= 3 where template has variables.
                // The hash incorporates BOTH template AND bindings to distinguish
                // recursive calls with different arguments.
                if depth >= 3 && template.has_variables_fast() {
                    // Hash template + bindings content for unique identification
                    let mut thunk_hash = template.hash_value();
                    for (name, val) in bindings.iter() {
                        thunk_hash ^= val.hash_value().wrapping_mul(0x9e3779b97f4a7c15);
                        for b in name.bytes() {
                            thunk_hash = thunk_hash.wrapping_mul(31).wrapping_add(b as u64);
                        }
                    }
                    let lookup =
                        crate::backend::eval::cesk::with_thunk_table(|t| t.lookup(thunk_hash));
                    match lookup {
                        crate::backend::eval::cesk::ThunkLookup::Evaluated(cached) => {
                            let cb = &*carrying_bindings;
                            work_stack.push(WorkItem::Resume {
                                result: (
                                    if cb.is_empty() {
                                        cached.into_iter().map(bv).collect()
                                    } else {
                                        cached
                                            .into_iter()
                                            .filter_map(|v| {
                                                let tracked = active_tracked_vars();
                                                project_carrying_for_consumer(
                                                    &carrying_bindings,
                                                    &v,
                                                    tracked.as_deref(),
                                                    ctx.factory(),
                                                )
                                                .map(|projected| {
                                                    if projected.is_empty() {
                                                        bv(v)
                                                    } else {
                                                        bv_with(v, (*projected).clone())
                                                    }
                                                })
                                            })
                                            .collect()
                                    },
                                    env,
                                ),
                            });
                            continue;
                        }
                        crate::backend::eval::cesk::ThunkLookup::Blackhole => {
                            // Infinite recursion detected — return error
                            let error_val = ctx.factory().error(
                                ctx.factory().atom("infinite recursion in EvalWithBindings"),
                                ctx.factory().string("blackhole"),
                            );
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(error_val)], env),
                            });
                            continue;
                        }
                        _ => {
                            // Absent or Suspended — push CompleteThunk, proceed normally
                            continuations.push(Continuation::CompleteThunk {
                                thunk_hash,
                                env: env.clone(),
                                depth,
                                start_epoch: mutation_epoch(),
                            });
                        }
                    }
                }

                // Template is a variable atom → resolve from bindings
                if let Some(var_name) = template.as_atom() {
                    if is_variable_str(var_name) {
                        if let Some(bound) = bindings.get(var_name) {
                            let resolved = bound.clone();
                            if resolved.has_variables_fast() {
                                // Resolved value still has variables → recurse
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: resolved,
                                    bindings,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            } else {
                                work_stack.push(WorkItem::Eval {
                                    value: resolved,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    demand: None,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                        } else {
                            work_stack.push(WorkItem::Eval {
                                value: template,
                                env,
                                depth,
                                is_tail_call,
                                expected_type,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    } else {
                        // Non-variable atom: self-evaluating
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(template)], env),
                        });
                    }
                    continue;
                }

                // Template is an S-expression → check for `let` special form
                if let Some(items) = template.as_sexpr() {
                    if items.is_empty() {
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(template)], env),
                        });
                        continue;
                    }

                    // Resolve head through bindings if it's a variable
                    let head = &items[0];
                    let resolved_head_atom = if let Some(var) = head.as_atom() {
                        if is_variable_str(var) {
                            bindings.get(var).and_then(|v| v.as_atom())
                        } else {
                            Some(var)
                        }
                    } else {
                        None
                    };

                    // ── Grounded sub-expression guard ──
                    //
                    // If the template is a user-defined function call with arguments
                    // whose heads are grounded ops (e.g., (f (+ 1 $x) ...)), OR if
                    // any binding value is a grounded sub-expression, materialize and
                    // push as WorkItem::Eval. This goes through eval_step_generic
                    // Step 2 which pre-evaluates grounded args before rule matching.
                    //
                    // Special forms (if, let, chain, case, etc.) handle their own
                    // argument evaluation, so they're excluded.
                    if super::engine::template_has_grounded_arg_heads(&template)
                        || bindings
                            .iter()
                            .any(|(_, val)| super::engine::binding_value_needs_eval(val))
                    {
                        let materialized = apply_bindings(&template, &bindings, ctx.factory());
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&template),
                                    vec![crate::backend::trace::trace_value_generic(&materialized)],
                                    None,
                                    trace_format::TraceEventKind::BindingsApplied {
                                        template: crate::backend::trace::trace_value_generic(
                                            &template,
                                        ),
                                        bindings: bindings
                                            .iter()
                                            .map(|(k, v)| {
                                                (
                                                    k.to_string(),
                                                    crate::backend::trace::trace_value_generic(v),
                                                )
                                            })
                                            .collect(),
                                        result: crate::backend::trace::trace_value_generic(
                                            &materialized,
                                        ),
                                    },
                                );
                            }
                        }
                        work_stack.push(WorkItem::Eval {
                            value: materialized,
                            env,
                            depth,
                            is_tail_call,
                            expected_type,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── freeze-tuple: materialize args and freeze immediately ──
                    // freeze-tuple must NOT go through the general Eval path
                    // because the Eval path can cache/re-evaluate args.
                    // Materialize args from bindings, construct tuple, freeze.
                    if resolved_head_atom == Some("freeze-tuple") && items.len() >= 2 {
                        let materialized_args: Vec<MettaValue> = items[1..]
                            .iter()
                            .map(|item| {
                                let materialized = apply_bindings(item, &bindings, ctx.factory());
                                if let Some(inner) = materialized.as_quoted() {
                                    inner
                                } else {
                                    materialized
                                }
                            })
                            .collect();
                        let tuple = ctx.factory().sexpr(materialized_args);
                        memoize_normal_form(&tuple);
                        let cb = &*carrying_bindings;
                        work_stack.push(WorkItem::Resume {
                            result: (
                                smallvec![if cb.is_empty() {
                                    bv(tuple)
                                } else {
                                    bv_with(tuple, cb.clone())
                                }],
                                env,
                            ),
                        });
                        continue;
                    }

                    // ── collapse-bind materialization ──
                    // collapse-bind needs its arg fully substituted so that
                    // StartCollapseBind's tracked_vars extraction sees the
                    // actual free variables (e.g. $who) rather than the
                    // deferred binding's key (e.g. $term).
                    if resolved_head_atom == Some("collapse-bind") && items.len() == 2 {
                        let materialized = apply_bindings(&template, &bindings, ctx.factory());
                        work_stack.push(WorkItem::Eval {
                            value: materialized,
                            env,
                            depth,
                            is_tail_call,
                            expected_type,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── `let` with lazy body: the key optimization ──
                    //
                    // For `(let pattern value_expr body)` with pending bindings B:
                    // 1. Keep pattern RAW because it is a binder position.
                    // 2. Materialize only value_expr with B (needed immediately).
                    // 3. Keep body RAW + store B as outer_bindings on ProcessLet.
                    // 4. When ProcessLet produces pattern-match bindings B2:
                    //    compose(B, B2) and push EvalWithBindings{body, compose(B, B2)}
                    //
                    // For nested let* of depth N, the body is never materialized
                    // until the innermost level, giving O(N) instead of O(N^2).
                    if resolved_head_atom == Some("let") && items.len() == 4 {
                        let pattern = items[1].clone();
                        let value_expr = apply_bindings(&items[2], &bindings, ctx.factory());

                        continuations.push(Continuation::ProcessLet {
                            pending_values: None,
                            pattern,
                            body: items[3].clone(), // RAW body — not materialized
                            outer_bindings: Some(bindings),
                            results: Vec::with_capacity(4),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── Phase C: `if` with deferred branches ──
                    //
                    // For `(if cond then else)` with pending bindings B:
                    // 1. Materialize only `cond` with B (needed for condition evaluation)
                    // 2. Keep `then` and `else` RAW + store B as outer_bindings on ProcessIfCondition
                    // 3. When condition resolves to True/False, evaluate only the taken branch
                    //    via EvalWithBindings{branch, B} — the untaken branch is never materialized.
                    if resolved_head_atom == Some("if") && items.len() == 4 {
                        let condition = apply_bindings(&items[1], &bindings, ctx.factory());

                        // Fast path: literal Bool after binding substitution
                        if let Some(is_true) = condition.as_bool() {
                            let branch_raw = if is_true { &items[2] } else { &items[3] };
                            let branch = if branch_raw.has_variables_fast() {
                                apply_bindings(branch_raw, &bindings, ctx.factory())
                            } else {
                                branch_raw.clone()
                            };
                            work_stack.push(WorkItem::Eval {
                                value: branch,
                                env,
                                depth,
                                is_tail_call,
                                expected_type,
                                demand: Some(crate::backend::eval::cesk::coroutine::Demand::All),
                                carrying_bindings: carrying_bindings.clone(),
                            });
                            continue;
                        }

                        let outer_demand = CURRENT_DEMAND
                            .with(|d| d.get())
                            .unwrap_or(crate::backend::eval::cesk::coroutine::Demand::All);

                        continuations.push(Continuation::ProcessIfCondition {
                            then_branch: items[2].clone(), // RAW — not materialized
                            else_branch: items[3].clone(), // RAW — not materialized
                            outer_bindings: Some(bindings),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                            outer_demand,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: Some(ctx.factory().atom("Bool")),
                            demand: Some(crate::backend::eval::cesk::coroutine::Demand::Exactly(1)),
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── Stretch Goal 2: `let*` tight loop via ProcessLetStar ──
                    //
                    // For `(let* ((p1 v1) (p2 v2) ...) body)` with pending bindings B:
                    // Instead of desugaring to N nested `let` S-exprs (N allocations +
                    // 3N trampoline iterations), use ProcessLetStar continuation to
                    // evaluate value expressions sequentially and accumulate bindings.
                    // Reduces to N+2 iterations and 0 nested `let` allocations.
                    if resolved_head_atom == Some("let*") && items.len() == 3 {
                        let bindings_expr = if items[1].as_sexpr().is_some() {
                            items[1].clone()
                        } else {
                            apply_bindings(&items[1], &bindings, ctx.factory())
                        };
                        if let Some(binding_pairs) = bindings_expr.as_sexpr() {
                            if binding_pairs.is_empty() || bindings_expr.is_unit() {
                                // No bindings — evaluate body with outer bindings
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: items[2].clone(),
                                    bindings,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                                continue;
                            }

                            // Extract (pattern, value_expr) pairs
                            let mut pairs: Vec<(MettaValue, MettaValue)> =
                                Vec::with_capacity(binding_pairs.len());
                            for binding in binding_pairs.iter() {
                                if let Some(pair) = binding.as_sexpr() {
                                    if pair.len() == 2 {
                                        pairs.push((pair[0].clone(), pair[1].clone()));
                                    }
                                }
                            }

                            if pairs.is_empty() {
                                // No valid pairs — evaluate body
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: items[2].clone(),
                                    bindings,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                                continue;
                            }

                            // Pop first pair, materialize its value_expr with current bindings
                            let (first_pattern, first_value_expr) = pairs.remove(0);
                            let materialized_value =
                                apply_bindings(&first_value_expr, &bindings, ctx.factory());

                            // I-5: Enter region for let* scope
                            let region_id = crate::backend::eval::cesk::with_region_stack(|s| {
                                s.enter(depth as u32)
                            });
                            continuations.push(Continuation::ProcessLetStar {
                                current_pattern: first_pattern,
                                remaining_pairs: pairs,
                                body: items[2].clone(), // RAW body
                                accumulated_bindings: bindings,
                                env: env.clone(),
                                depth,
                                is_tail_call,
                                region_id,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: materialized_value,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                            continue;
                        }
                        // Fall through to full materialization if bindings_expr is not S-expr
                    }

                    // ── Phase C: `chain` with deferred body ──
                    //
                    // For `(chain expr $var body)` with pending bindings B:
                    // 1. Materialize only `expr` with B (needed for evaluation)
                    // 2. Apply the same kernel-op gate as `StartChain` (spec §06.6.3
                    //    E-CHAIN-SUBST-DONE — only kernel ops auto-evaluate; data binds
                    //    as-is). See StartChain comment block above for HE bisimilarity
                    //    and the runaway-recursion problem the gate prevents.
                    if resolved_head_atom == Some("chain") && items.len() == 4 {
                        let expr = apply_bindings(&items[1], &bindings, ctx.factory());

                        let head_is_kernel = expr
                            .as_sexpr()
                            .and_then(|exp_items| exp_items.first())
                            .and_then(|h| h.as_atom())
                            .map(is_embedded_kernel_op)
                            .unwrap_or(false);

                        if head_is_kernel {
                            continuations.push(Continuation::ProcessChainExpr {
                                var: items[2].clone(),  // $var — doesn't need materialization
                                body: items[3].clone(), // RAW body — not materialized
                                outer_bindings: Some(bindings),
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
                            });

                            work_stack.push(WorkItem::Eval {
                                value: expr,
                                env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            // Data: substitute $var → expr into body (raw),
                            // re-materialize body with the SAME outer bindings B,
                            // then evaluate.
                            let var_name = items[2].as_atom().unwrap_or("");
                            let instantiated = substitute_variable_generic(
                                &items[3],
                                var_name,
                                &expr,
                                ctx.factory(),
                            );
                            let materialized =
                                apply_bindings(&instantiated, &bindings, ctx.factory());
                            work_stack.push(WorkItem::Eval {
                                value: materialized,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                        continue;
                    }

                    // ── Phase C: `case` with deferred bodies ──
                    //
                    // For `(case atom cases)` with pending bindings B:
                    // 1. Materialize only `atom` with B (needed for scrutinee evaluation)
                    // 2. Store `cases` + B as outer_bindings on ProcessCaseAtom
                    // 3. When the matched case template is selected, cases are materialized
                    //    with B (patterns may reference outer variables).
                    if resolved_head_atom == Some("case") && items.len() == 3 {
                        let atom = apply_bindings(&items[1], &bindings, ctx.factory());

                        continuations.push(Continuation::ProcessCaseAtom {
                            cases: items[2].clone(), // RAW — deferred materialization
                            outer_bindings: Some(bindings),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── Stretch Goal 1: Binding-aware deterministic chain ──
                    //
                    // For user-defined deterministic functions, try to chain
                    // through multiple rule applications without returning to
                    // the full trampoline dispatch loop. This saves 2-3 trampoline
                    // iterations per chain step and defers apply_bindings allocation
                    // when the RHS has variables (compose bindings instead).
                    if let Some(chain_result) =
                        try_deferred_deterministic_chain(&template, &bindings, &env, ctx.factory())
                    {
                        match chain_result {
                            DeferredChainResult::Deferred {
                                template: new_template,
                                bindings: new_bindings,
                            } => {
                                // Task #6 Phase 4 (2026-05-18): TCO for
                                // EvalWithBindings recursive RHS. When the
                                // deferred chain re-emits an empty-bindings
                                // step at the same template hash AND we are
                                // already a tail call AND no outer ambient
                                // carrying would be lost, reuse the cached
                                // empty-bindings Arc (refcount bump) instead
                                // of allocating a fresh Arc<Bindings> per
                                // recursion step. Conservative gate per the
                                // plan: any non-trivial binding flow (PLN
                                // Robot's heavy EvalWithBindings traffic)
                                // falls through unchanged.
                                let new_bindings_arc = if new_bindings.is_empty()
                                    && is_tail_call
                                    && carrying_bindings.is_empty()
                                    && new_template.hash_value() == template.hash_value()
                                {
                                    crate::backend::eval::trampoline::types::empty_shared_bindings()
                                } else {
                                    std::sync::Arc::new(new_bindings)
                                };
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: new_template,
                                    bindings: new_bindings_arc,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                            DeferredChainResult::Concrete(value) => {
                                work_stack.push(WorkItem::Eval {
                                    value,
                                    env,
                                    depth,
                                    is_tail_call,
                                    expected_type,
                                    demand: None,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                            DeferredChainResult::Done(value) => {
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![bv(value)], env),
                                });
                            }
                        }
                        continue;
                    }

                    // ── SG1 Phase B: Binding-aware nondeterministic rule matching ──
                    //
                    // Before materializing the full expression, try matching rules
                    // directly against the template with variable resolution through
                    // outer bindings. This avoids O(tree) apply_bindings allocation
                    // when structural matchers can resolve variables on-the-fly.
                    //
                    // Only attempted when:
                    // - Head is known (resolved_head_atom is Some)
                    // - Head is not a special form or grounded op
                    // - All rule candidates have structural matchers
                    //
                    // Falls through to materialization if any candidate lacks a
                    // structural matcher (MORK matching needs concrete expressions).
                    if let Some(head_name) = resolved_head_atom {
                        if !is_reducible_head(head_name) {
                            let arity = items.len() - 1;
                            if let Some(matches) = try_match_rules_with_bindings(
                                &template,
                                &bindings,
                                head_name,
                                arity,
                                &env,
                                ctx.factory(),
                            ) {
                                if !matches.is_empty() {
                                    dispatch_rule_matches(
                                        matches,
                                        SmallVec::new(),
                                        Arc::clone(&env),
                                        depth,
                                        ctx,
                                        &mut work_stack,
                                        &mut continuations,
                                        None,
                                        &*carrying_bindings,
                                    );
                                    continue;
                                }
                                // matches is empty → no rule matched → self-evaluating
                                // Still need to materialize for the result
                            }
                            // None → not all candidates have structural matchers, fall through
                        }
                    }

                    // All other S-expressions: full materialization + Eval
                    let materialized = apply_bindings(&template, &bindings, ctx.factory());
                    work_stack.push(WorkItem::Eval {
                        value: materialized,
                        env,
                        depth,
                        is_tail_call,
                        expected_type,
                        demand: None,
                        carrying_bindings: carrying_bindings.clone(),
                    });
                    continue;
                }

                // Non-S-expression (type, conjunction, etc.): materialize
                let materialized = apply_bindings(&template, &bindings, ctx.factory());
                work_stack.push(WorkItem::Eval {
                    value: materialized,
                    env,
                    depth,
                    is_tail_call,
                    expected_type,
                    demand: None,
                    carrying_bindings: carrying_bindings.clone(),
                });
            }

            WorkItem::Resume { result } => {
                // Take ownership of continuation for processing
                let cont = continuations.pop().expect("non-empty continuation stack");
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?cont, result_values = ?result.0, "resume work item");

                // Process continuation - delegate to continuation handler
                process_continuation(
                    cont,
                    result,
                    &mut work_stack,
                    &mut continuations,
                    &mut final_result,
                    ctx,
                    &mut deferred_shared_drops,
                );
            }
        }
    }

    // Trace: EvalEnd with matching span_id and measured duration
    #[cfg(feature = "trace")]
    {
        if let Some(tc) = ctx.trace_collector() {
            let result_count = final_result.as_ref().map_or(0, |r| r.0.len()) as u32;
            let end_ns = tc.elapsed_ns();
            let duration = end_ns.saturating_sub(_eval_start_ns);
            tc.emit_timed(
                trace_format::TraceTier::TreeWalker,
                0,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::EvalEnd { result_count },
                _eval_start_ns,
                Some(duration),
                Some(_eval_span_id),
            );
        }
    }

    // Clear thread-local trace collector before returning.
    #[cfg(feature = "trace")]
    {
        crate::backend::trace::thread_local_sink::clear_thread_trace_collector();
    }

    // Return final result as EvalOutcome::Complete
    let (results, final_env) = final_result.unwrap_or_else(|| (SmallVec::new(), Arc::new(env)));
    crate::backend::eval::cesk::EvalOutcome::Complete(results, (*final_env).clone())
}

/// Resolve a value to a `SpaceHandle`, auto-binding `&name` atom references
/// to a fresh empty SpaceHandle on first use.
///
/// Mirrors HE's implicit space initialization semantics where `(add-atom &foo
/// ...)` on an unbound `&foo` lazily creates the space. The bind is registered
/// in `env_after` so subsequent references resolve to the same handle.
///
/// Returns:
/// - `Some(handle)` when `value` is already a Space, OR when it's an unbound
///   `&name` atom (auto-creates + binds).
/// - `None` for reserved names (`&`, `&self`, `&kb`, `&stack`), non-atom
///   values, and atoms not starting with `&`.
fn resolve_space_or_autobind<C: EvalContext>(
    value: &MettaValue,
    env_after: &mut SharedEnv,
    ctx: &C,
) -> Option<SpaceHandle> {
    if let Some(handle) = value.as_space() {
        return Some(handle.clone());
    }
    let name = value.as_atom()?;
    if !name.starts_with('&')
        || name == "&"
        || name == "&self"
        || name == "&kb"
        || name == "&stack"
    {
        return None;
    }
    // Auto-bind: lazy SpaceHandle creation and env registration.
    let env_mut = Arc::make_mut(env_after);
    let id = env_mut.create_named_space(name);
    let handle = SpaceHandle::new(id, name.to_string());
    let space_val = ctx.factory().space(handle.clone());
    env_mut.register_token(name, space_val);
    increment_mutation_epoch();
    Some(handle)
}

/// Process a continuation with generic value types.
///
/// This function handles all continuation types, converting at boundaries
/// where necessary to interact with heap-based infrastructure (rules, environment).
fn process_continuation<C: EvalContext>(
    cont: Continuation,
    result: EvalResult,
    work_stack: &mut Vec<WorkItem>,
    continuations: &mut Vec<Continuation>,
    final_result: &mut Option<EvalResult>,
    ctx: &C,
    deferred_shared_drops: &mut Vec<
        std::sync::Arc<crate::backend::environment::GenericEnvironmentShared<MettaValue>>,
    >,
) {
    // Eval-trace binding-flow instrumentation (v5).
    // Capture the Resume boundary's inputs BEFORE the match consumes cont
    // and result. After the match we inspect the last pushed WorkItem:
    //   - If it's a Resume, emit ContinuationEmit (+ BindingsDropped when
    //     the output key-union is a strict subset of the input key-union).
    //   - Otherwise (Done/Eval/EvalWithBindings pushed, or terminal arm),
    //     emit ContinuationExitNoResume.
    // Record-time volume mitigation: skip entirely when inputs are empty
    // (no binding info to flow). All richer filtering is analyzer-side.
    #[cfg(feature = "trace")]
    let trace_ctx: Option<(
        String,
        u64,
        u32,
        Vec<trace_format::BoundValueSnapshot>,
        usize,
    )> = if ctx.trace_collector().is_some()
        && (in_collapse_bind_scope() || result.0.iter().any(|(_, b)| !b.is_empty()))
    {
        let cont_kind = cont.discriminant_name().to_string();
        let flow_id = next_flow_id();
        let cont_depth = continuations.len() as u32;
        let inputs = crate::backend::trace::convert::trace_bound_values(&result.0);
        let tracked_vars: Vec<String> = active_tracked_vars()
            .map(|tv| tv.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        if let Some(tc) = ctx.trace_collector() {
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                cont_depth,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::ContinuationEnter {
                    cont_kind: cont_kind.clone(),
                    flow_id,
                    cont_depth,
                    inputs: inputs.clone(),
                    tracked_vars,
                },
            );
        }
        Some((cont_kind, flow_id, cont_depth, inputs, work_stack.len()))
    } else {
        None
    };

    match cont {
        Continuation::Done => {
            *final_result = Some(result);
        }

        Continuation::CollectSExpr {
            mut remaining,
            mut collected,
            original_env,
            depth,
            outer_carrying,
        } => {
            collected.push(result);

            if remaining.len() == 0 {
                // Phase 2.B: gate on whether any child returned multiple
                // alternatives. If so, route through the binding-preserving
                // slow path, which composes per-combination bindings and
                // drops conflict-failed combinations (HE-bisimilar).
                //
                // Single-alternative children (deterministic) continue to
                // use the existing fast path — zero overhead.
                let nondet_children = collected.iter().any(|(v, _)| v.len() > 1);

                if nondet_children {
                    // Slow path: preserve BoundValue alternatives through the
                    // Cartesian product, compose bindings per combination.
                    let collected_bound: Vec<(SmallVec<[BoundValue; 2]>, MettaEnvironment)> =
                        collected
                            .into_iter()
                            .map(|(vals, shared_env)| (vals, (*shared_env).clone()))
                            .collect();
                    let processed_bound = crate::backend::eval::processing::ops::process_collected_sexpr_bound_generic(
                        collected_bound,
                        (*outer_carrying).clone(),
                        (*original_env).clone(),
                        depth,
                        ctx.factory(),
                    );

                    use crate::backend::eval::processing::ops::GenericProcessedSExprBound;
                    match processed_bound {
                        GenericProcessedSExprBound::Done((results, env)) => {
                            work_stack.push(WorkItem::Resume {
                                result: (results, Arc::new(env)),
                            });
                        }
                        GenericProcessedSExprBound::EvalRuleMatches {
                            matches,
                            env,
                            depth,
                            base_results,
                        } => {
                            if matches.is_empty() {
                                work_stack.push(WorkItem::Resume {
                                    result: (base_results, Arc::new(env)),
                                });
                            } else {
                                // Single-combo rule matches: use its composed
                                // bindings as the dispatch outer_carrying so
                                // RHS eval sees the correct context.
                                let combo_b = base_results
                                    .first()
                                    .map(|(_, b)| b.clone())
                                    .unwrap_or_else(|| (*outer_carrying).clone());
                                dispatch_rule_matches(
                                    matches,
                                    base_results,
                                    Arc::new(env),
                                    depth,
                                    ctx,
                                    work_stack,
                                    continuations,
                                    None,
                                    &combo_b,
                                );
                            }
                        }
                        GenericProcessedSExprBound::EvalCombinations {
                            combinations,
                            env,
                            depth,
                        } => {
                            let env: SharedEnv = Arc::new(env);
                            continuations.push(Continuation::ProcessCombinationsBound {
                                combinations: Box::new(combinations),
                                results: Vec::with_capacity(8),
                                pending_rule_matches: Vec::new(),
                                pending_combo_bindings:
                                    crate::backend::models::GenericBindings::new(),
                                env: env.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                        }
                        GenericProcessedSExprBound::RedispatchSExpr {
                            items,
                            combo_bindings,
                            env,
                            depth: redispatch_depth,
                        } => {
                            let sexpr = ctx.factory().sexpr(items);
                            work_stack.push(WorkItem::EvalWithBindings {
                                template: sexpr,
                                bindings: Arc::new(combo_bindings),
                                env: Arc::new(env),
                                depth: redispatch_depth,
                                is_tail_call: false,
                                expected_type: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    }
                } else {
                    // Fast path (single-alternative children):
                    // Compute the merged bindings from each collected child's
                    // only result. On conflict (HE-bisimilar): emit ZERO results
                    // to silently drop this combination, matching HE's
                    // `BindingsSet::empty()` pruning. The old behavior reset
                    // merged_bindings to empty and continued, producing ghost
                    // tuples downstream with wrong/empty bindings.
                    let mut merged_bindings = crate::backend::models::GenericBindings::new();
                    let mut merge_ok = true;
                    for (vals, _) in collected.iter() {
                        if let Some((_, child_b)) = vals.first() {
                            if !merged_bindings.merge(child_b) {
                                merge_ok = false;
                                break;
                            }
                        }
                    }
                    if !merge_ok {
                        // Phase 2.B fix: drop this combination (HE-bisimilar).
                        // Emits zero results so the caller sees no contribution
                        // from this tuple assembly.
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), original_env.clone()),
                        });
                        return;
                    }

                    // Use generic version - zero conversion needed!
                    // Unwrap SharedEnv → bare MettaEnvironment for process_collected_sexpr_generic
                    let collected_bare: Vec<(SmallVec<[MettaValue; 2]>, MettaEnvironment)> =
                        collected
                            .into_iter()
                            .map(|(vals, shared_env)| (values_of(&vals), (*shared_env).clone()))
                            .collect();
                    let processed = process_collected_sexpr_generic(
                        collected_bare,
                        (*original_env).clone(),
                        depth,
                        ctx.factory(),
                    );

                    match processed {
                        GenericProcessedSExpr::Done((results, env)) => {
                            let mb = merged_bindings.clone();
                            work_stack.push(WorkItem::Resume {
                                result: (
                                    results.into_iter().map(|v| (v, mb.clone())).collect(),
                                    Arc::new(env),
                                ),
                            });
                        }
                        GenericProcessedSExpr::EvalRuleMatches {
                            matches,
                            env,
                            depth,
                            base_results,
                        } => {
                            if matches.is_empty() {
                                let mb = merged_bindings.clone();
                                work_stack.push(WorkItem::Resume {
                                    result: (
                                        base_results.into_iter().map(|v| (v, mb.clone())).collect(),
                                        Arc::new(env),
                                    ),
                                });
                            } else {
                                // Stage 1d-revised: base-results carry merged_bindings
                                // so fallback results (no rule match dispatched) have
                                // correct bindings. For dispatched matches, the
                                // WorkItem::Eval carrying_bindings field (added by
                                // Stage 1d-revised) threads merged_bindings into each
                                // RHS evaluation — see dispatch_rule_matches outer_carrying.
                                let mb = merged_bindings.clone();
                                dispatch_rule_matches(
                                    matches,
                                    base_results.into_iter().map(|v| (v, mb.clone())).collect(),
                                    Arc::new(env),
                                    depth,
                                    ctx,
                                    work_stack,
                                    continuations,
                                    None,
                                    &mb,
                                );
                            }
                        }
                        GenericProcessedSExpr::EvalCombinations {
                            combinations,
                            env,
                            depth,
                        } => {
                            let env: SharedEnv = Arc::new(env);
                            continuations.push(Continuation::ProcessCombinations {
                                combinations: Box::new(combinations),
                                results: Vec::with_capacity(8),
                                pending_rule_matches: Vec::new(),
                                env: env.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                        }
                        GenericProcessedSExpr::RedispatchSExpr {
                            items,
                            env,
                            depth: redispatch_depth,
                        } => {
                            let sexpr = ctx.factory().sexpr(items);
                            work_stack.push(WorkItem::Eval {
                                value: sexpr,
                                env: Arc::new(env),
                                depth: redispatch_depth,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    }
                }
            } else {
                let next = remaining.next().expect("remaining is non-empty");
                let tracked = active_tracked_vars();
                let Some(next_carrying) = project_carrying_for_consumer(
                    &outer_carrying,
                    &next,
                    tracked.as_deref(),
                    ctx.factory(),
                ) else {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), original_env),
                    });
                    return;
                };

                continuations.push(Continuation::CollectSExpr {
                    remaining,
                    collected,
                    original_env: original_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: next,
                    env: original_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: next_carrying,
                });
            }
        }

        Continuation::ProcessRuleMatches {
            mut remaining_matches,
            mut results,
            env,
            depth,
            pre_fork_epoch,
            pre_fork_gen,
            fork_depth,
            mut current_branch_bindings,
            outer_carrying,
            tracked_vars_hint,
            #[cfg(feature = "trace")]
            branch_span_id,
            #[cfg(feature = "trace")]
            branch_start_ns,
            #[cfg(feature = "trace")]
            branch_index,
            #[cfg(feature = "trace")]
            total_branches,
            #[cfg(feature = "trace")]
            is_real_fork,
        } => {
            #[cfg(feature = "trace")]
            let result_count = result.0.len() as u32;

            let result_env = result.1;

            // Stage 1c COMPOSE_MATCH: attach this branch's match bindings to
            // every sub-evaluation result by substitutive composition with
            // the child's bindings, then transitively resolve chains, then
            // project to just the tracked variables of any active
            // collapse-bind. When not inside a collapse-bind scope, the
            // composition short-circuits (Empty outer + empty child = empty).
            let composed: SmallVec<[BoundValue; 2]> = if current_branch_bindings.is_empty()
                && tracked_vars_hint.is_none()
            {
                // Fast path: no collapse-bind scope active; pass through.
                result.0
            } else {
                let factory = ctx.factory();
                result
                    .0
                    .into_iter()
                    .map(|(v, child_b)| {
                        let mut composed =
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*current_branch_bindings,
                                &child_b,
                                factory,
                            );
                        crate::backend::eval::bindings::apply_chain_generic(&mut composed, factory);
                        // Layer A: projection moved to sidecar encoding in
                        // ProcessCollapseEvalResults so user-named bindings (e.g.
                        // $who=a) survive the entire pipeline and get projected
                        // only at the observation point — matching HE's
                        // Bindings::resolve() semantics (interpreter.rs:624).
                        // let projected = match &tracked_vars_hint {
                        //     Some(tv) => crate::backend::eval::bindings::project_bindings_generic(
                        //         &composed,
                        //         tv.as_slice(),
                        //     ),
                        //     None => composed,
                        // };

                        // Removed (2026-05-06): the "Fix 4 mmverify hang plan,
                        // defense-in-depth" trim that called
                        // `transitive_live_vars_generic` and dropped freshened
                        // bindings unreachable from `v`.
                        //
                        // Why removed: the trim was too aggressive when this
                        // handler is dispatched from inside a foldl-atom
                        // iteration. It had no visibility into sibling
                        // iterations on the continuation stack, so it dropped
                        // freshened bindings that ARE referenced by the next
                        // iteration's items. Specifically, for a fold over
                        // `((father b $__fr_182_b) (father $__fr_182_b c))`,
                        // iteration 1's match against `(father b c)` produces
                        // `$__fr_182_b → c`; the trim erased it because
                        // `live_vars((stv 1 0.9))` is empty; iteration 2 then
                        // re-bound `$__fr_182_b → b` against `(father b c)`,
                        // yielding spurious `(grandfather b c)` for PLN's
                        // Direct.metta tests 2/3.
                        //
                        // Why safe to remove: the actual mmverify-hang fix is
                        // Fix 1 at `engine.rs:649-680` (partial-bind rejection
                        // at rule-match source), per Fix 4's own docstring.
                        // The proper iteration-boundary liveness gate is
                        // `filter_fold_propagating_bindings` at
                        // `eval_loop.rs:468-481` (called from ProcessFoldlAtom
                        // at `:7600`, `:7643`, `:7748`). The lazy sibling
                        // handler `ProcessRuleMatchesLazy` at `:5779-5821`
                        // already takes this no-trim path — existence proof
                        // that compose-without-trim is HE-bisimilar.
                        //
                        // HE bisimilarity: HE's `Bindings::merge` uses strict
                        // rejection on inconsistent bindings; HE has no
                        // analogous trim. This restoration matches HE.
                        //
                        // Memory bound: Fix 1 caps freshened-binding count at
                        // per-rule var count (small constant). Debug-build
                        // canary below catches regression; release-build
                        // companion canary (once-per-process, threshold 512)
                        // surfaces the same class in stripped release builds
                        // where the debug_assert is compiled out — caught
                        // the original asymmetry regression that motivated
                        // Phase 7 (see comment at
                        // `parallel_collapse_dispatch` worker, `:2107`).
                        #[cfg(debug_assertions)]
                        {
                            let freshened_count = composed
                                .iter()
                                .filter(|(k, _)| k.starts_with("$__fr_"))
                                .count();
                            debug_assert!(
                                freshened_count < 1024,
                                "ProcessRuleMatches compose produced {} freshened-binding keys; \
                             possible Fix 1 regression. Investigate \
                             enumerate_rules_via_unification. \
                             (See Phase 7 — parallel_collapse_dispatch \
                             CacheRootRefreshGuard asymmetry at eval_loop.rs:2107.)",
                                freshened_count
                            );
                        }
                        #[cfg(not(debug_assertions))]
                        {
                            use std::sync::Once;
                            static WARN_ONCE: Once = Once::new();
                            let freshened_count = composed
                                .iter()
                                .filter(|(k, _)| k.starts_with("$__fr_"))
                                .count();
                            if freshened_count >= 512 {
                                WARN_ONCE.call_once(|| {
                                    tracing::warn!(
                                        freshened_count,
                                        "ProcessRuleMatches compose: high freshened-binding count; \
                                         see Phase 7 (parallel_collapse_dispatch asymmetry, \
                                         eval_loop.rs:2107)"
                                    );
                                });
                            }
                        }
                        (v, composed)
                    })
                    .collect()
            };
            results.extend(composed);

            // Trace: BranchEnd for the branch that just completed.
            // H7 Stage 1 (2026-05-05): only emit when this continuation
            // represents a real fork (paired with BranchStart). Skip the
            // single-match compose-shim path which has sentinel zeros that
            // would produce malformed events (start_ns=0, dur=trace-lifetime).
            #[cfg(feature = "trace")]
            if is_real_fork {
                if let Some(tc) = ctx.trace_collector() {
                    let end_ns = tc.elapsed_ns();
                    let duration = end_ns.saturating_sub(branch_start_ns);
                    tc.emit_timed(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::BranchEnd {
                            branch_index,
                            result_count,
                        },
                        branch_start_ns,
                        Some(duration),
                        Some(branch_span_id),
                    );
                }
            }

            // Check for Prolog-style cut: if (cut) was evaluated during the
            // branch that just completed, commit to this branch's results and
            // discard all remaining alternative matches. The cut signal is
            // depth-targeted so nested dispatches don't accidentally consume it.
            let cut_fired = take_cut_at_depth(fork_depth);

            if remaining_matches.len() == 0 || cut_fired {
                // Stage 1c: a `fork_depth == 0` ProcessRuleMatches is a
                // "single-match compose shim" — pushed by the fast path
                // without calling enter_fork/enter_fork_scope. Skip the
                // matching leave_fork/leave_fork_scope calls to keep the
                // fork bookkeeping consistent.
                if fork_depth != 0 {
                    // All branches consumed (or cut fired) — leave the fork scope.
                    leave_fork();
                    leave_fork_scope(pre_fork_gen);
                }
                // Defer the branch environment's deep drop. Its MettaValues
                // are collected into root_set at the next GC safepoint via
                // collect_roots(), then the Vec is cleared after perform_safepoint.
                deferred_shared_drops.push(std::sync::Arc::clone(&env.shared));
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            } else {
                // remaining_matches is already in generic type (V, GenericBindings<V>)
                let (rhs, raw_bindings) = remaining_matches
                    .next()
                    .expect("remaining_matches is non-empty");

                // Stage 1c: rotate to the next branch's match bindings so the
                // next COMPOSE_MATCH uses them. Each sibling branch has its
                // own independent bindings — no shared-mutable state.
                // Stage 1d-revised: re-compose with outer_carrying so next
                // branch's RHS results inherit the same ambient ancestry.
                current_branch_bindings = std::sync::Arc::new(if outer_carrying.is_empty() {
                    raw_bindings.clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying,
                        &raw_bindings,
                        ctx.factory(),
                    )
                });

                let bindings = std::sync::Arc::new(raw_bindings);

                // Trace: BranchStart for the next branch
                #[cfg(feature = "trace")]
                let (_next_span_id, _next_start_ns, _next_branch_index) = {
                    if let Some(tc) = ctx.trace_collector() {
                        let next_idx = branch_index + 1;
                        let span_id = tc.next_span_id();
                        let start_ns = tc.elapsed_ns();
                        tc.emit_timed(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::BranchStart {
                                branch_index: next_idx,
                                total_branches,
                            },
                            start_ns,
                            None,
                            Some(span_id),
                        );
                        (span_id, start_ns, next_idx)
                    } else {
                        (0u64, 0u64, 0u32)
                    }
                };

                // Restore pre-fork epoch so the next branch sees the same
                // cache validity as the first branch. Without this, side effects
                // from branch N (add-atom incrementing epoch) would invalidate
                // SubgoalTable entries for branch N+1.
                set_mutation_epoch(pre_fork_epoch);
                // Advance the scope generation so cache entries from the
                // previous branch are invisible to this branch. This replaces
                // clearing eval_memo and subgoal_table between branches, since
                // those caches are now scope-generation-aware.
                next_branch_scope();
                // Thunk table must still be cleared between branches because
                // thunks have a multi-state FSM (Suspended -> Blackhole ->
                // Evaluated). Leftover Suspended/Blackhole entries from the
                // previous branch would corrupt cycle detection in the next
                // branch, and the scope_gen check cannot fully isolate the
                // intermediate states without a fundamental redesign of the
                // thunk FSM.
                crate::backend::eval::cesk::clear_thunk_table();

                // Stage 1d-revised: clone current_branch_bindings BEFORE
                // moving it into the continuation, so we can use it below
                // as the RHS WorkItem's carrying_bindings.
                let rot_carrying: crate::backend::models::GenericBindings<MettaValue> =
                    (*current_branch_bindings).clone();
                continuations.push(Continuation::ProcessRuleMatches {
                    remaining_matches,
                    results,
                    env: env.clone(),
                    depth,
                    pre_fork_epoch,
                    pre_fork_gen,
                    fork_depth,
                    current_branch_bindings,
                    outer_carrying,
                    tracked_vars_hint,
                    #[cfg(feature = "trace")]
                    branch_span_id: _next_span_id,
                    #[cfg(feature = "trace")]
                    branch_start_ns: _next_start_ns,
                    #[cfg(feature = "trace")]
                    branch_index: _next_branch_index,
                    #[cfg(feature = "trace")]
                    total_branches,
                    // H7 Stage 1: rotation site is a real fork (paired with
                    // BranchStart for the new branch_index).
                    #[cfg(feature = "trace")]
                    is_real_fork: true,
                });

                // Trace: RuleApplication (tree-walker, subsequent match)
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let bindings_tv: Vec<(String, trace_format::TraceValue)> = bindings
                            .iter()
                            .map(|(k, v)| {
                                (k.to_string(), crate::backend::trace::trace_value_generic(v))
                            })
                            .collect();
                        let trace_rhs = crate::backend::trace::trace_value_generic(&rhs);
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            trace_rhs.clone(),
                            vec![trace_rhs.clone()],
                            None,
                            trace_format::TraceEventKind::RuleApplication {
                                rule_lhs: trace_rhs.clone(),
                                rule_rhs: trace_rhs,
                                bindings: bindings_tv,
                                rule_span: None,
                            },
                        );
                    }
                }

                // Stage 1d-revised: the next branch's RHS inherits the
                // already-composed current_branch_bindings as its carrying
                // (cloned above before moving into the re-pushed continuation).
                // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings
                if rhs.has_variables_fast() {
                    work_stack.push(WorkItem::EvalWithBindings {
                        template: rhs,
                        bindings,
                        env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        carrying_bindings: std::sync::Arc::new(rot_carrying),
                    });
                } else {
                    if is_memoized_normal_form(&rhs) {
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv_with(rhs, rot_carrying)], env),
                        });
                    } else if is_normal_form_bounded(&rhs, &env, 2) {
                        memoize_normal_form(&rhs);
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv_with(rhs, rot_carrying)], env),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: rhs,
                            env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: std::sync::Arc::new(rot_carrying),
                        });
                    }
                }
            }
        }

        // ── I-15: ProcessRuleMatchesLazy — demand-driven branch evaluation ──
        Continuation::ProcessRuleMatchesLazy {
            mut coroutine,
            mut results,
            env: _,
            depth,
            mut current_branch_bindings,
            outer_carrying,
            tracked_vars_hint,
        } => {
            let (eval_results, result_env) = result;

            // Stage 1c: COMPOSE_MATCH composition for each sub-eval result,
            // mirroring ProcessRuleMatches (A.2). Empty bindings fast path
            // preserves zero-overhead for non-collapse-bind evaluations.
            let composed: SmallVec<[BoundValue; 2]> =
                if current_branch_bindings.is_empty() && tracked_vars_hint.is_none() {
                    eval_results
                } else {
                    let factory = ctx.factory();
                    eval_results
                        .into_iter()
                        .map(|(v, child_b)| {
                            let mut c = crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*current_branch_bindings,
                                &child_b,
                                factory,
                            );
                            crate::backend::eval::bindings::apply_chain_generic(&mut c, factory);
                            // Layer A: projection moved to sidecar encoding — see
                            // ProcessCollapseEvalResults. Preserve user-named
                            // bindings through the pipeline; only the observation
                            // point projects to the tracked set (HE Bindings::resolve
                            // semantics).
                            // let projected = match &tracked_vars_hint {
                            //     Some(tv) => crate::backend::eval::bindings::project_bindings_generic(
                            //         &c,
                            //         tv.as_slice(),
                            //     ),
                            //     None => c,
                            // };
                            (v, c)
                        })
                        .collect()
                };

            // Record results from the just-evaluated branch
            for val in composed.iter() {
                results.push(val.clone());
                coroutine.record_result(val.0.clone());
            }

            if coroutine.is_done() {
                // Demand satisfied or branches exhausted
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            } else if let Some((rhs, bindings)) = coroutine.next_branch() {
                // Stage 1c: rotate to the next branch's bindings.
                // Stage 1d-revised: re-compose with outer_carrying.
                current_branch_bindings = std::sync::Arc::new(if outer_carrying.is_empty() {
                    bindings.clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying,
                        &bindings,
                        ctx.factory(),
                    )
                });

                // More branches to evaluate — push continuation and eval next
                continuations.push(Continuation::ProcessRuleMatchesLazy {
                    coroutine,
                    results,
                    env: result_env.clone(),
                    depth,
                    current_branch_bindings,
                    outer_carrying,
                    tracked_vars_hint,
                });

                if rhs.has_variables_fast() {
                    work_stack.push(WorkItem::EvalWithBindings {
                        template: rhs,
                        bindings: std::sync::Arc::new(bindings),
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        carrying_bindings:
                            crate::backend::eval::trampoline::types::empty_shared_bindings(),
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings:
                            crate::backend::eval::trampoline::types::empty_shared_bindings(),
                    });
                }
            } else {
                // Exhausted — return what we have
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            }
        }

        Continuation::ProcessGroundedOp {
            mut state,
            pending_arg_idx,
            env: _,
            depth,
            arg_bindings,
        } => {
            let (result_values, result_env) = result;

            // HE-faithful per-alternative dispatch. When the arg's Eval
            // returned N alternatives, MeTTa HE treats each as an
            // independent plan-vector item: the grounded op is fired once
            // per (substituted, single-valued) alternative, and outputs
            // from all branches accumulate into a flat result list. See
            // `eval_impl` / `execute_bindings` in
            // `hyperon-experimental/lib/src/metta/interpreter.rs:504-549`.
            //
            // 0 alts → empty propagation (branch annihilation).
            // 1 alt  → inline dispatch (fast path, no allocation).
            // N alts → push `ProcessGroundedOpFanout` accumulator, then
            //          dispatch the first alt inline. Remaining alts flow
            //          back through the Fanout continuation which
            //          self-repushes once per alternative.
            let (chosen_value, chosen_bindings) = match result_values.len() {
                0 => {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![], result_env),
                    });
                    return;
                }
                1 => {
                    let mut iter = result_values.into_iter();
                    iter.next().unwrap()
                }
                _ => {
                    let mut alts: Vec<BoundValue> = result_values.into_iter().collect();
                    let first = alts.remove(0);
                    continuations.push(Continuation::ProcessGroundedOpFanout {
                        remaining_alts: alts.into_iter(),
                        results: Vec::new(),
                        template_state: state.clone(),
                        pending_arg_idx,
                        prior_arg_bindings: arg_bindings.clone(),
                        env: result_env.clone(),
                        depth,
                    });
                    first
                }
            };

            // Compose prior arg_bindings with this alt's branch bindings.
            //
            // Phase 2.B Issue #4 fix: use the strict variant. On genuine
            // conflict (non-empty inputs → empty result), this alt is
            // inconsistent — emit zero results rather than firing the
            // grounded op with empty bindings (which produced ghost
            // outputs downstream). HE-bisimilar silent pruning.
            let composed: crate::backend::eval::trampoline::types::SharedBindings =
                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                    &*arg_bindings,
                    &chosen_bindings,
                    ctx.factory(),
                ) {
                    Some(b) => std::sync::Arc::new(b),
                    None => {
                        // Conflicting alt: emit zero and return.
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), result_env),
                        });
                        return;
                    }
                };

            // Install the single-valued arg in state — each branch sees
            // exactly one value at `pending_arg_idx`, matching HE where
            // args are substituted before grounded execution.
            state.set_arg(pending_arg_idx, vec![chosen_value]);

            // Try static dispatch first - works with generic type V (NO conversion)
            let op_name = state.op_name.clone();
            if let Some(work) = execute_grounded_op(&op_name, &mut state, ctx.factory()) {
                match work {
                    GroundedWork::Done(results) => {
                        let values: Vec<MettaValue> = results.into_iter().map(|(v, _)| v).collect();
                        let tag = (*composed).clone();
                        work_stack.push(WorkItem::Resume {
                            result: (
                                values.into_iter().map(|v| (v, tag.clone())).collect(),
                                result_env,
                            ),
                        });
                    }
                    GroundedWork::EvalArg {
                        arg_idx,
                        state: new_state,
                    } => {
                        let arg_bindings_for_eval = composed.clone();
                        continuations.push(Continuation::ProcessGroundedOp {
                            state: Box::new(new_state.clone()),
                            pending_arg_idx: arg_idx,
                            env: result_env.clone(),
                            depth,
                            arg_bindings: composed,
                        });

                        let arg_to_eval = new_state.args[arg_idx].clone();
                        work_stack.push(WorkItem::Eval {
                            value: arg_to_eval,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: arg_bindings_for_eval,
                        });
                    }
                    GroundedWork::Error(e) => {
                        let tag = (*composed).clone();
                        match e {
                            ExecError::NoReduce => {
                                let mut expr_parts = Vec::with_capacity(1 + state.args.len());
                                expr_parts.push(ctx.factory().atom(&state.op_name));
                                for arg in state.args.iter() {
                                    expr_parts.push(arg.clone());
                                }
                                let unreduced = ctx.factory().sexpr(expr_parts);
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![(unreduced, tag)], result_env),
                                });
                            }
                            _ => {
                                // ERR-shape align (2026-05-16): centralized
                                // converter, HE-aligned (Error <call> <detail>).
                                let call_form = state.call_form(ctx.factory());
                                let error_value =
                                    exec_error_to_value(&e, call_form, ctx.factory());
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![(error_value, tag)], result_env),
                                });
                            }
                        }
                    }
                }
            } else {
                let error_value = ctx.factory().error(
                    ctx.factory().atom("OperationNotFoundError"),
                    ctx.factory().string(&format!(
                        "Grounded operation '{}' not found in generic registry",
                        state.op_name
                    )),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(error_value)], result_env),
                });
            }
        }

        Continuation::ProcessGroundedOpFanout {
            mut remaining_alts,
            mut results,
            template_state,
            pending_arg_idx,
            prior_arg_bindings,
            env: _,
            depth,
        } => {
            // On entry: `result` holds the just-finished branch's outputs.
            // Extend the cumulative `results` with them.
            let (branch_outs, result_env) = result;
            results.extend(branch_outs.into_iter());

            // Advance to next alternative, skipping conflict alts.
            //
            // Phase 2.B Issue #4 fix: use strict compose. Conflicting alts
            // are skipped locally (continue the loop) rather than dispatched
            // with empty bindings. HE-bisimilar: no ghost results from
            // inconsistent arg combinations.
            let (v_i, composed) = loop {
                match remaining_alts.next() {
                    Some((v_i, alt_b_i)) => {
                        match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                            &*prior_arg_bindings,
                            &alt_b_i,
                            ctx.factory(),
                        ) {
                            Some(b) => {
                                let composed: crate::backend::eval::trampoline::types::SharedBindings =
                                    std::sync::Arc::new(b);
                                break (v_i, composed);
                            }
                            None => continue, // conflict → skip this alt
                        }
                    }
                    None => {
                        // All alternatives exhausted (or all remaining were
                        // conflicts) — emit accumulated results to the outer
                        // consumer.
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::from_vec(results), result_env),
                        });
                        return;
                    }
                }
            };

            {
                // Fresh state clone for this branch, install single value.
                let mut state_i = (*template_state).clone();
                state_i.set_arg(pending_arg_idx, vec![v_i]);

                // Self-repush Fanout so the next alt's output lands here.
                continuations.push(Continuation::ProcessGroundedOpFanout {
                    remaining_alts,
                    results,
                    template_state,
                    pending_arg_idx,
                    prior_arg_bindings,
                    env: result_env.clone(),
                    depth,
                });

                // Drive this alt through the grounded-op state machine.
                let op_name = state_i.op_name.clone();
                if let Some(work) = execute_grounded_op(&op_name, &mut state_i, ctx.factory()) {
                    match work {
                        GroundedWork::Done(branch_results) => {
                            let values: Vec<MettaValue> =
                                branch_results.into_iter().map(|(v, _)| v).collect();
                            let tag = (*composed).clone();
                            work_stack.push(WorkItem::Resume {
                                result: (
                                    values.into_iter().map(|v| (v, tag.clone())).collect(),
                                    result_env,
                                ),
                            });
                        }
                        GroundedWork::EvalArg {
                            arg_idx,
                            state: new_state,
                        } => {
                            let arg_bindings_for_eval = composed.clone();
                            continuations.push(Continuation::ProcessGroundedOp {
                                state: Box::new(new_state.clone()),
                                pending_arg_idx: arg_idx,
                                env: result_env.clone(),
                                depth,
                                arg_bindings: composed,
                            });

                            let arg_to_eval = new_state.args[arg_idx].clone();
                            work_stack.push(WorkItem::Eval {
                                value: arg_to_eval,
                                env: result_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: arg_bindings_for_eval,
                            });
                        }
                        GroundedWork::Error(e) => {
                            let tag = (*composed).clone();
                            match e {
                                ExecError::NoReduce => {
                                    let mut expr_parts = Vec::with_capacity(1 + state_i.args.len());
                                    expr_parts.push(ctx.factory().atom(&state_i.op_name));
                                    for arg in state_i.args.iter() {
                                        expr_parts.push(arg.clone());
                                    }
                                    let unreduced = ctx.factory().sexpr(expr_parts);
                                    work_stack.push(WorkItem::Resume {
                                        result: (smallvec![(unreduced, tag)], result_env),
                                    });
                                }
                                _ => {
                                    // ERR-shape align (2026-05-16): centralized
                                    // converter, HE-aligned (Error <call>
                                    // <detail>).
                                    let call_form = state_i.call_form(ctx.factory());
                                    let error_value =
                                        exec_error_to_value(&e, call_form, ctx.factory());
                                    work_stack.push(WorkItem::Resume {
                                        result: (smallvec![(error_value, tag)], result_env),
                                    });
                                }
                            }
                        }
                    }
                } else {
                    let error_value = ctx.factory().error(
                        ctx.factory().atom("OperationNotFoundError"),
                        ctx.factory().string(&format!(
                            "Grounded operation '{}' not found in generic registry",
                            state_i.op_name
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(error_value)], result_env),
                    });
                }
                return;
            }
            // (exhaust-all-alts branch is handled in the `None =>` arm of
            // the loop above.)
        }

        Continuation::ProcessCombinations {
            mut combinations,
            mut results,
            mut pending_rule_matches,
            env,
            depth,
            outer_carrying,
        } => {
            let (combo_results, result_env) = result;
            results.extend(combo_results);

            // Process pending rule matches first
            // pending_rule_matches is already in generic type (V, GenericBindings<V>)
            if let Some((rhs, bindings)) = pending_rule_matches.pop() {
                continuations.push(Continuation::ProcessCombinations {
                    combinations,
                    results,
                    pending_rule_matches,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings
                if rhs.has_variables_fast() {
                    work_stack.push(WorkItem::EvalWithBindings {
                        template: rhs,
                        bindings: std::sync::Arc::new(bindings),
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                }
                return;
            }

            // Get next combination
            if let Some(combo) = combinations.next() {
                // Generic iterator yields SmallVec<[V; 8]> - create generic sexpr
                let generic_sexpr = ctx.factory().sexpr(combo.to_vec());

                // Try to match rules using generic version - no conversion needed!
                let all_matches_with_types =
                    try_match_all_rules(&generic_sexpr, &result_env, *ctx.factory());

                if all_matches_with_types.is_empty() {
                    // Phase 2.B HE-bisimilarity fix: distinguish function with
                    // no matching rules (→ empty) from data constructor (→ data).
                    let is_function = generic_sexpr
                        .get_head_symbol()
                        .map(|h| {
                            let arity = generic_sexpr.get_arity();
                            result_env
                                .shared
                                .rule_index
                                .read()
                                .get_candidates(h, arity, None)
                                .next()
                                .is_some()
                        })
                        .unwrap_or(false);

                    if !is_function {
                        // Data constructor: retain as-is.
                        results.push(if outer_carrying.is_empty() {
                            bv(generic_sexpr)
                        } else {
                            bv_with(generic_sexpr, (*outer_carrying).clone())
                        });
                    }
                    // Function with no matching rules → drop this combination
                    // (HE-bisimilar silent pruning).

                    continuations.push(Continuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches,
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else {
                    // Rules matched — strip rhs_type and dispatch via unified gate.
                    // Push ProcessCombinations first (LIFO: it fires after dispatch completes).
                    let matches_deque: Vec<_> = all_matches_with_types
                        .into_iter()
                        .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                        .collect();

                    continuations.push(Continuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches: Vec::new(), // dispatch handles all matches
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    // Dispatch rule matches (parallel or sequential)
                    dispatch_rule_matches(
                        matches_deque,
                        SmallVec::new(),
                        Arc::clone(&result_env),
                        depth,
                        ctx,
                        work_stack,
                        continuations,
                        None,
                        &*outer_carrying,
                    );
                }
            } else {
                // All combinations processed - results already contains generic values
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), env),
                });
            }
        }

        Continuation::ProcessCombinationsBound {
            mut combinations,
            mut results,
            mut pending_rule_matches,
            pending_combo_bindings,
            env,
            depth,
            outer_carrying,
        } => {
            let (combo_results, result_env) = result;
            results.extend(combo_results);

            // Process pending rule matches for the CURRENT combo first.
            if let Some((rhs, bindings)) = pending_rule_matches.pop() {
                let combo_b_arc = Arc::new(pending_combo_bindings.clone());
                continuations.push(Continuation::ProcessCombinationsBound {
                    combinations,
                    results,
                    pending_rule_matches,
                    pending_combo_bindings: pending_combo_bindings.clone(),
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Thread pending_combo_bindings as outer_carrying so the
                // RHS evaluation sees the correct per-combo context.
                if rhs.has_variables_fast() {
                    work_stack.push(WorkItem::EvalWithBindings {
                        template: rhs,
                        bindings: Arc::new(bindings),
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        carrying_bindings: combo_b_arc,
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: combo_b_arc,
                    });
                }
                return;
            }

            // Pull the next combination (iterator already prunes conflicts).
            if let Some((combo, combo_bindings)) = combinations.next() {
                let generic_sexpr = ctx.factory().sexpr(combo.to_vec());

                // Phase 5 (Bug 1): compose outer_carrying with combo_bindings
                // and thread as caller-side outer_carrying so captured caller
                // variables in rule body templates resolve through them
                // instead of being freshened to wildcard names.
                let combo_outer = if outer_carrying.is_empty() {
                    combo_bindings.clone()
                } else if combo_bindings.is_empty() {
                    (*outer_carrying).clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying,
                        &combo_bindings,
                        ctx.factory(),
                    )
                };
                let all_matches_with_types =
                    crate::backend::eval::trampoline::engine::try_match_all_rules_with_outer(
                        &generic_sexpr,
                        &result_env,
                        *ctx.factory(),
                        &combo_outer,
                    );

                if all_matches_with_types.is_empty() {
                    // Phase 2.B HE-bisimilarity: function with no matching
                    // rules → drop combination; data constructor → keep.
                    let is_function = generic_sexpr
                        .get_head_symbol()
                        .map(|h| {
                            let arity = generic_sexpr.get_arity();
                            result_env
                                .shared
                                .rule_index
                                .read()
                                .get_candidates(h, arity, None)
                                .next()
                                .is_some()
                        })
                        .unwrap_or(false);
                    if !is_function {
                        results.push(bv_with(generic_sexpr, combo_bindings));
                    }

                    continuations.push(Continuation::ProcessCombinationsBound {
                        combinations,
                        results,
                        pending_rule_matches,
                        pending_combo_bindings: crate::backend::models::GenericBindings::new(),
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else {
                    // Rules matched: dispatch with combo_bindings as outer_carrying.
                    let matches_deque: Vec<_> = all_matches_with_types
                        .into_iter()
                        .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                        .collect();

                    // Capture combo_bindings for the ProcessCombinationsBound
                    // re-push; pass reference to dispatch_rule_matches.
                    let combo_b_for_continuation = combo_bindings.clone();

                    continuations.push(Continuation::ProcessCombinationsBound {
                        combinations,
                        results,
                        pending_rule_matches: Vec::new(), // dispatch handles all
                        pending_combo_bindings: combo_b_for_continuation,
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    dispatch_rule_matches(
                        matches_deque,
                        SmallVec::new(),
                        Arc::clone(&result_env),
                        depth,
                        ctx,
                        work_stack,
                        continuations,
                        None,
                        &combo_bindings,
                    );
                }
            } else {
                // All combinations processed.
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), env),
                });
            }
        }

        Continuation::ProcessLet {
            pending_values,
            pattern,
            body,
            outer_bindings,
            mut results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (result_values, result_env) = result;

            // Phase 8.5: Extract type constraint once for all values
            let type_constraint = extract_type_constraint(&pattern);
            let shadowed_outer_carrying = if outer_carrying.is_empty()
                || !pattern.has_variables_fast()
            {
                outer_carrying.clone()
            } else {
                std::sync::Arc::new(crate::backend::eval::bindings::prepare_letstar_accumulated(
                    &*outer_carrying,
                    &pattern,
                    ctx.factory(),
                ))
            };

            match pending_values {
                None => {
                    // First resumption: result_values are values to pattern match

                    // Trace: value-result phase
                    #[cfg(feature = "trace")]
                    {
                        if let Some(tc) = ctx.trace_collector() {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                depth as u32,
                                crate::backend::trace::trace_value_generic(&pattern),
                                result_values
                                    .iter()
                                    .map(|(v, _)| crate::backend::trace::trace_value_generic(v))
                                    .collect(),
                                None,
                                trace_format::TraceEventKind::SpecialForm {
                                    form_name: "let".to_string(),
                                    phase: "value-result".to_string(),
                                },
                            );
                        }
                    }

                    // Collect ALL matching values and their bound bodies.
                    // When outer_bindings is present, we compose bindings and
                    // defer body materialization via EvalWithBindings.
                    //
                    // Each entry is either:
                    // - BoundBody::Materialized(value) — body fully instantiated
                    // - BoundBody::Deferred(bindings) — body + composed bindings
                    enum BoundBody {
                        Materialized(MettaValue),
                        Deferred(crate::backend::models::GenericBindings<MettaValue>),
                    }

                    let mut bound_bodies: Vec<BoundBody> = Vec::new();
                    for (value, b) in result_values.iter() {
                        // Phase 8.5: Type pre-check for typed patterns
                        if let Some(ref tc) = type_constraint {
                            if get_ground_type(value).is_some() {
                                let value_type =
                                    infer_type_generic(value, ctx.factory(), &result_env);
                                if !types_match_with_subtypes(&value_type, tc, &result_env) {
                                    continue;
                                }
                            }
                        }
                        if let Some(pm_bindings) = pattern_match(&pattern, value) {
                            // Task #70 gap-fix: compose per-scrutinee-result
                            // bindings (b) with pattern-match bindings so the
                            // body sees variables bound by both. Previously `_b`
                            // was discarded, losing scrutinee-level bindings
                            // like `$y=$b` from `(rule $y) → $y=<ground>`.
                            let pm_with_scrutinee: crate::backend::models::GenericBindings<
                                MettaValue,
                            > = if b.is_empty() {
                                pm_bindings
                            } else {
                                let scrutinee_shadowed =
                                    crate::backend::eval::bindings::prepare_letstar_accumulated(
                                        b,
                                        &pattern,
                                        ctx.factory(),
                                    );
                                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                        &scrutinee_shadowed, &pm_bindings, ctx.factory(),
                                    ) {
                                        Some(composed) => composed,
                                        None => continue, // conflict → drop
                                    }
                            };
                            if let Some(ref ob) = outer_bindings {
                                let outer_shadowed =
                                    crate::backend::eval::bindings::prepare_letstar_accumulated(
                                        ob,
                                        &pattern,
                                        ctx.factory(),
                                    );
                                match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                    &outer_shadowed,
                                    &pm_with_scrutinee,
                                    ctx.factory(),
                                ) {
                                    Some(composed) => {
                                        let tracked = active_tracked_vars();
                                        if let Some(projected) = project_owned_bindings_for_consumer(
                                            &composed,
                                            &body,
                                            tracked.as_deref(),
                                            ctx.factory(),
                                        ) {
                                            bound_bodies.push(BoundBody::Deferred(projected));
                                        }
                                    }
                                    None => continue, // conflict → drop this alt
                                }
                            } else if !pm_with_scrutinee.is_empty() && body.has_variables_fast() {
                                // Defer via EvalWithBindings when body has vars
                                // to resolve via scrutinee bindings.
                                let tracked = active_tracked_vars();
                                if let Some(projected) = project_owned_bindings_for_consumer(
                                    &pm_with_scrutinee,
                                    &body,
                                    tracked.as_deref(),
                                    ctx.factory(),
                                ) {
                                    bound_bodies.push(BoundBody::Deferred(projected));
                                }
                            } else {
                                // No outer, no scrutinee — materialize body as before
                                let instantiated =
                                    apply_bindings(&body, &pm_with_scrutinee, ctx.factory());
                                bound_bodies.push(BoundBody::Materialized(instantiated));
                            }
                        }
                    }

                    if bound_bodies.is_empty() {
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::from_vec(results), result_env),
                        });
                        return;
                    }

                    if bound_bodies.len() == 1 {
                        // Single match - evaluate directly (TCO)
                        let single = bound_bodies.into_iter().next().expect("len == 1");
                        match single {
                            BoundBody::Materialized(val) => {
                                let tracked = active_tracked_vars();
                                let Some(carrying) = project_carrying_for_consumer(
                                    &shadowed_outer_carrying,
                                    &val,
                                    tracked.as_deref(),
                                    ctx.factory(),
                                ) else {
                                    work_stack.push(WorkItem::Resume {
                                        result: (SmallVec::new(), result_env),
                                    });
                                    return;
                                };
                                work_stack.push(WorkItem::Eval {
                                    value: val,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: carrying,
                                });
                            }
                            BoundBody::Deferred(composed_bindings) => {
                                let tracked = active_tracked_vars();
                                let Some(carrying) = project_carrying_for_consumer(
                                    &shadowed_outer_carrying,
                                    &body,
                                    tracked.as_deref(),
                                    ctx.factory(),
                                ) else {
                                    work_stack.push(WorkItem::Resume {
                                        result: (SmallVec::new(), result_env),
                                    });
                                    return;
                                };
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: body.clone(),
                                    bindings: std::sync::Arc::new(composed_bindings),
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    carrying_bindings: carrying,
                                });
                            }
                        }
                        return;
                    }

                    // Multiple matches: materialize all deferred bodies for dispatch
                    let instantiated_bodies: Vec<MettaValue> = bound_bodies
                        .into_iter()
                        .map(|bb| match bb {
                            BoundBody::Materialized(val) => val,
                            BoundBody::Deferred(composed_bindings) => {
                                apply_bindings(&body, &composed_bindings, ctx.factory())
                            }
                        })
                        .collect();

                    // ── Parallel path: evaluate all matched bodies concurrently ──
                    // When multiple values match, their body evaluations are
                    // independent (read-only env, no side effects). Dispatch to
                    // work pool for parallel evaluation.
                    // WFST classification: only parallelize when branches
                    // justify the dispatch overhead.
                    let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
                    let wfst_allows_match = if instantiated_bodies.len() >= 2 {
                        let scheduler = crate::backend::scheduler::global_scheduler();
                        let degree_ok = instantiated_bodies.iter().any(|body| {
                            let (_, action) = scheduler.classify_and_transduce(body);
                            action.parallelism_degree > 1
                        });
                        // H2: branch-purity gate (spec §5.6.1).
                        // Phase 10.D: state-mutation only by default;
                        // opt-in I/O strictness via
                        // METTATRON_STRICT_PRINT_ORDER=1.
                        let all_pure = instantiated_bodies.iter().all(|body| {
                            !crate::backend::scheduler::classification::body_blocks_parallel_dispatch(
                                body, 8,
                            )
                        });
                        degree_ok && all_pure
                    } else {
                        false
                    };

                    let par_budget = if wfst_allows_match
                        && current_depth < max_parallel_depth()
                        && global_eval_pool().active_workers() > 0
                    {
                        try_acquire_budget((instantiated_bodies.len() - 1) as u32, current_depth)
                    } else {
                        0
                    };

                    if par_budget > 0 {
                        let metta_env = (*result_env).clone();
                        // ProcessLet's parallel body dispatch is a fan-out: every
                        // matched-pattern body must produce its result for the
                        // outer let to collect. `Demand::All` is correct here.
                        let tracked = active_tracked_vars();
                        let branches: Vec<ParallelBranch> = instantiated_bodies
                            .into_iter()
                            .filter_map(|body| {
                                project_carrying_for_consumer(
                                    &shadowed_outer_carrying,
                                    &body,
                                    tracked.as_deref(),
                                    ctx.factory(),
                                )
                                .map(|carrying| (body, carrying))
                            })
                            .collect();
                        if branches.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::from_vec(results), result_env),
                            });
                            return;
                        }
                        // **Stack-safety mandate (2026-05-15)**:
                        // trampolinized dispatch via WaitForParallel.
                        // Phase 8: share Arc with RootProvider.
                        let stable_branches_snapshot =
                            std::sync::Arc::new(branches);
                        let handle = parallel_dispatch(
                            std::sync::Arc::clone(&stable_branches_snapshot),
                            metta_env,
                            par_budget,
                            current_depth,
                            crate::backend::eval::cesk::coroutine::Demand::All,
                        );
                        let base_results: SmallVec<[BoundValue; 2]> =
                            SmallVec::from_vec(results);
                        let env_for_resume = result_env.clone();
                        continuations.push(Continuation::WaitForParallel {
                            handle,
                            merge_mode:
                                crate::backend::eval::trampoline::types::ParallelMergeMode::AmbConcat,
                            base_results,
                            outer_carrying: shadowed_outer_carrying.clone(),
                            env: result_env,
                            depth,
                            budget_acquired: par_budget,
                            caller_depth: current_depth,
                            stable_branches_snapshot,
                        });
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env_for_resume),
                        });
                    } else {
                        // ── Sequential path: process bodies one at a time ──
                        // Use ProcessAmb continuation to evaluate instantiated
                        // bodies sequentially and merge their results. Bodies
                        // are already fully instantiated (no per-body bindings
                        // to attach), so wrap each with empty bindings.
                        let mut bodies_iter = instantiated_bodies
                            .into_iter()
                            .map(bv)
                            .collect::<Vec<_>>()
                            .into_iter();
                        let (first_body, _first_b) =
                            bodies_iter.next().expect("bodies is non-empty");

                        continuations.push(Continuation::ProcessAmb {
                            remaining_alts: bodies_iter,
                            results,
                            env: result_env.clone(),
                            depth,
                            outer_carrying: shadowed_outer_carrying.clone(),
                        });

                        let tracked = active_tracked_vars();
                        let Some(carrying) = project_carrying_for_consumer(
                            &shadowed_outer_carrying,
                            &first_body,
                            tracked.as_deref(),
                            ctx.factory(),
                        ) else {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                            return;
                        };
                        work_stack.push(WorkItem::Eval {
                            value: first_body,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying,
                        });
                    }
                    return;
                }
                Some(mut remaining_values) => {
                    // Subsequent resumption: result_values are body evaluation results
                    // Add body results to collected results
                    results.extend(result_values);

                    // Try next value
                    loop {
                        match remaining_values.pop() {
                            Some((value, scrutinee_b)) => {
                                // Phase 8.5: Type pre-check for typed patterns (: $var Type)
                                if let Some(ref tc) = type_constraint {
                                    if get_ground_type(&value).is_some() {
                                        let value_type =
                                            infer_type_generic(&value, ctx.factory(), &result_env);
                                        if !types_match_with_subtypes(&value_type, tc, &result_env)
                                        {
                                            continue; // Type mismatch — skip
                                        }
                                    }
                                }
                                // Task #70 gap-fix: compose scrutinee_b (per-value
                                // bindings, previously `_b` discarded) into the
                                // pattern-match bindings so body eval sees them.
                                let pm_and_scrutinee =
                                    |pm: crate::backend::models::GenericBindings<MettaValue>| -> Option<
                                        crate::backend::models::GenericBindings<MettaValue>,
                                    > {
                                        if scrutinee_b.is_empty() {
                                            Some(pm)
                                        } else {
                                            crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                            &scrutinee_b, &pm, ctx.factory(),
                                        )
                                        }
                                    };
                                if let Some(bindings) = pattern_match(&pattern, &value) {
                                    let bindings = match pm_and_scrutinee(bindings) {
                                        Some(b) => b,
                                        None => continue, // scrutinee/pattern conflict → drop
                                    };
                                    // Trace: pattern-match phase (subsequent resumption)
                                    #[cfg(feature = "trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                crate::backend::trace::trace_value_generic(
                                                    &pattern,
                                                ),
                                                vec![crate::backend::trace::trace_value_generic(
                                                    &value,
                                                )],
                                                None,
                                                trace_format::TraceEventKind::SpecialForm {
                                                    form_name: "let".to_string(),
                                                    phase: "pattern-match".to_string(),
                                                },
                                            );
                                        }
                                    }

                                    // Pattern matches - evaluate body with bindings.
                                    // Phase 2.B Issue #2 fix: strict compose so a
                                    // conflict between outer_bindings and the
                                    // pattern-match bindings drops this alternative
                                    // (continue to next value in remaining_values).
                                    // The old code used `.compose()` which silently
                                    // overwrote conflicts, producing ghost results.
                                    if let Some(ref ob) = outer_bindings {
                                        let outer_shadowed =
                                            crate::backend::eval::bindings::prepare_letstar_accumulated(
                                                ob,
                                                &pattern,
                                                ctx.factory(),
                                            );
                                        let composed = match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                            &outer_shadowed,
                                            &bindings,
                                            ctx.factory(),
                                        ) {
                                            Some(b) => b,
                                            None => continue, // conflict → try next value
                                        };
                                        let tracked = active_tracked_vars();
                                        let composed = match project_owned_bindings_for_consumer(
                                            &composed,
                                            &body,
                                            tracked.as_deref(),
                                            ctx.factory(),
                                        ) {
                                            Some(b) => b,
                                            None => continue,
                                        };
                                        let carrying = match project_carrying_for_consumer(
                                            &shadowed_outer_carrying,
                                            &body,
                                            tracked.as_deref(),
                                            ctx.factory(),
                                        ) {
                                            Some(b) => b,
                                            None => continue,
                                        };
                                        let cont_remaining = std::mem::take(&mut remaining_values);
                                        let cont_results = std::mem::take(&mut results);
                                        continuations.push(Continuation::ProcessLet {
                                            pending_values: Some(cont_remaining),
                                            pattern,
                                            body: body.clone(),
                                            outer_bindings: outer_bindings.clone(),
                                            results: cont_results,
                                            env: result_env.clone(),
                                            depth,
                                            outer_carrying: outer_carrying.clone(),
                                        });
                                        work_stack.push(WorkItem::EvalWithBindings {
                                            template: body,
                                            bindings: std::sync::Arc::new(composed),
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                            carrying_bindings: carrying,
                                        });
                                    } else {
                                        let instantiated_body =
                                            apply_bindings(&body, &bindings, ctx.factory());
                                        let tracked = active_tracked_vars();
                                        let carrying = match project_carrying_for_consumer(
                                            &shadowed_outer_carrying,
                                            &instantiated_body,
                                            tracked.as_deref(),
                                            ctx.factory(),
                                        ) {
                                            Some(b) => b,
                                            None => continue,
                                        };
                                        let cont_remaining = std::mem::take(&mut remaining_values);
                                        let cont_results = std::mem::take(&mut results);
                                        continuations.push(Continuation::ProcessLet {
                                            pending_values: Some(cont_remaining),
                                            pattern,
                                            body: body.clone(),
                                            outer_bindings: outer_bindings.clone(),
                                            results: cont_results,
                                            env: result_env.clone(),
                                            depth,
                                            outer_carrying: outer_carrying.clone(),
                                        });
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_body,
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                            demand: None,
                                            carrying_bindings: carrying,
                                        });
                                    }
                                    return;
                                }
                                // Trace: pattern-no-match phase (subsequent resumption)
                                #[cfg(feature = "trace")]
                                {
                                    if let Some(tc) = ctx.trace_collector() {
                                        tc.emit_converted(
                                            trace_format::TraceTier::TreeWalker,
                                            depth as u32,
                                            crate::backend::trace::trace_value_generic(&pattern),
                                            vec![],
                                            None,
                                            trace_format::TraceEventKind::SpecialForm {
                                                form_name: "let".to_string(),
                                                phase: "pattern-no-match".to_string(),
                                            },
                                        );
                                    }
                                }
                                // Pattern doesn't match - continue to next value
                            }
                            None => {
                                // All values processed - return results to parent
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::from_vec(results), result_env),
                                });
                                return;
                            }
                        }
                    }
                }
            }
        }

        Continuation::CollectGroundedArg {
            items,
            grounded_indices,
            current_idx,
            mut evaluated_results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (result_values, result_env) = result;

            // Store ALL results from evaluation to preserve nondeterminism.
            // A nondeterministic function like (f) → {1, 2, 3} produces 3 results.
            if result_values.is_empty() {
                evaluated_results.push(vec![]);
            } else {
                evaluated_results.push(result_values.into_vec());
            }

            let next_idx = current_idx + 1;
            if next_idx < grounded_indices.len() {
                // More grounded args to evaluate
                let arg_idx = grounded_indices[next_idx];
                let arg_to_eval = items[arg_idx].clone();

                // Phase 9.2: Derive expected_type for next arg
                let arg_expected_type =
                    derive_arg_expected_type::<C>(&items, arg_idx, &result_env, ctx.factory());

                continuations.push(Continuation::CollectGroundedArg {
                    items,
                    grounded_indices,
                    current_idx: next_idx,
                    evaluated_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: arg_to_eval,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: arg_expected_type,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // All grounded args evaluated — compute Cartesian product of
                // nondeterministic results and evaluate each combination.

                // Check if any arg produced empty results
                if evaluated_results.iter().any(|r| r.is_empty()) {
                    // Empty result from any arg → no combinations possible
                    // (Cartesian product of anything × empty = empty)
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else {
                    // Build all Cartesian product combinations as sexpr values.
                    // Each combination substitutes one result per grounded arg into items.
                    let mut combinations: Vec<MettaValue> = Vec::new();

                    // Track whether any grounded arg changed after pre-evaluation.
                    // If no args changed (fixpoint), skip re-evaluation to prevent
                    // infinite loop on bloom filter false positives and data constructors.
                    let mut changed = false;
                    for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                        if evaluated_results[i].len() != 1
                            || evaluated_results[i][0].0 != items[*grounded_idx]
                        {
                            changed = true;
                            break;
                        }
                    }

                    // Compute Cartesian product inline.
                    // Stage 1d-revised: also compute per-combination merged
                    // bindings from each chosen arg's BoundValue.1, so the
                    // combination's evaluation carries its per-branch bindings.
                    // Start with a single empty combination (indices all 0)
                    let mut combo_indices: Vec<usize> = vec![0; evaluated_results.len()];
                    let mut combo_bindings: Vec<
                        crate::backend::models::GenericBindings<MettaValue>,
                    > = Vec::new();
                    loop {
                        // Build this combination's items
                        let mut combo_items = items.clone();
                        // Merge bindings across chosen args.
                        //
                        // Phase 2.B Issue #5 fix: on conflict, DROP the
                        // combination entirely (HE-bisimilar silent pruning).
                        // Previously we emitted the combo with empty bindings,
                        // producing ghost results downstream.
                        let mut merged_b = crate::backend::models::GenericBindings::new();
                        let mut skip_combo = false;
                        for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                            let (ref v, ref b) = evaluated_results[i][combo_indices[i]];
                            combo_items[*grounded_idx] = v.clone();
                            if !merged_b.merge(b) {
                                skip_combo = true;
                                break;
                            }
                        }
                        if !skip_combo {
                            combinations.push(ctx.factory().sexpr(combo_items));
                            combo_bindings.push(merged_b);
                        }
                        // Advance indices (mixed-radix increment)
                        let mut carry = true;
                        for i in (0..combo_indices.len()).rev() {
                            if carry {
                                combo_indices[i] += 1;
                                if combo_indices[i] < evaluated_results[i].len() {
                                    carry = false;
                                } else {
                                    combo_indices[i] = 0;
                                }
                            }
                        }
                        if carry {
                            break; // All combinations exhausted
                        }
                    }

                    // If all combinations conflicted, emit zero.
                    if combinations.is_empty() {
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), result_env),
                        });
                        return;
                    }

                    // Stage 1d-revised: zip combinations with their
                    // per-combo bindings so each combination's evaluation
                    // carries the merged arg bindings.
                    let combos_with_b: Vec<(
                        MettaValue,
                        crate::backend::models::GenericBindings<MettaValue>,
                    )> = combinations
                        .into_iter()
                        .zip(combo_bindings.into_iter())
                        .collect();
                    let mut combinations_iter = combos_with_b.into_iter();

                    if combinations_iter.len() == 1 {
                        let (sexpr, combo_b) =
                            combinations_iter.next().expect("combinations is non-empty");
                        // Compose outer_carrying with the combo's merged arg bindings.
                        let combo_carrying = if outer_carrying.is_empty() {
                            combo_b
                        } else if combo_b.is_empty() {
                            (*outer_carrying).clone()
                        } else {
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &combo_b,
                                ctx.factory(),
                            )
                        };

                        if !changed {
                            // Fixpoint: pre-evaluation didn't change any argument.
                            // Phase 5 (Bug 1): thread combo_carrying as caller-side
                            // outer_carrying so that captured caller variables in
                            // rule body templates resolve through it instead of
                            // being freshened to wildcard names.
                            let all_matches_with_types =
                                crate::backend::eval::trampoline::engine::try_match_all_rules_with_outer(
                                    &sexpr, &result_env, *ctx.factory(), &combo_carrying,
                                );

                            if !all_matches_with_types.is_empty() {
                                let matches_deque: Vec<_> = all_matches_with_types
                                    .into_iter()
                                    .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                    .collect();
                                dispatch_rule_matches(
                                    matches_deque,
                                    SmallVec::new(),
                                    Arc::clone(&result_env),
                                    depth,
                                    ctx,
                                    work_stack,
                                    continuations,
                                    None,
                                    &combo_carrying,
                                );
                            } else {
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![bv_with(sexpr, combo_carrying)], result_env),
                                });
                            }
                        } else {
                            work_stack.push(WorkItem::Eval {
                                value: sexpr,
                                env: result_env,
                                depth,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: std::sync::Arc::new(combo_carrying),
                            });
                        }
                    } else {
                        // Multiple combinations — evaluate each and collect results.
                        let first_pair =
                            combinations_iter.next().expect("combinations is non-empty");
                        let (first_sexpr, first_b) = first_pair;
                        let first_carrying = if outer_carrying.is_empty() {
                            first_b
                        } else if first_b.is_empty() {
                            (*outer_carrying).clone()
                        } else {
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_b,
                                ctx.factory(),
                            )
                        };
                        let app_capacity = combinations_iter.len() + 1;
                        // Store the remaining combos + their bindings for
                        // CollectApplicativeResults to dispatch in order.
                        let remaining_vec: Vec<(
                            MettaValue,
                            crate::backend::models::GenericBindings<MettaValue>,
                        )> = combinations_iter.collect();
                        // Keep `remaining` field type (IntoIter<MettaValue>) —
                        // store the pairs via a side field; but since adding a
                        // new field to CollectApplicativeResults requires type
                        // changes, use a simpler representation: interleave by
                        // passing the bindings separately via a local closure.
                        // For compatibility with the existing field type, we
                        // PRE-COMPUTE each combo's carrying into the value
                        // itself is not possible — so we add a parallel vector
                        // field. However, the simpler path is to convert
                        // `remaining` to hold the pairs via a type change in
                        // the continuation. Done in the next edit.
                        let remaining_pairs_only_values: std::vec::IntoIter<MettaValue> =
                            remaining_vec
                                .iter()
                                .map(|(v, _)| v.clone())
                                .collect::<Vec<_>>()
                                .into_iter();
                        let remaining_pairs_bindings: Vec<
                            crate::backend::models::GenericBindings<MettaValue>,
                        > = remaining_vec.iter().map(|(_, b)| b.clone()).collect();

                        continuations.push(Continuation::CollectApplicativeResults {
                            remaining: remaining_pairs_only_values,
                            results: Vec::with_capacity(app_capacity),
                            env: result_env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                            remaining_bindings: remaining_pairs_bindings,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: first_sexpr,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: std::sync::Arc::new(first_carrying),
                        });
                    }
                }
            }
        }

        Continuation::CollectApplicativeResults {
            mut remaining,
            mut remaining_bindings,
            mut results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (result_values, result_env) = result;
            results.extend(result_values);

            if remaining.len() == 0 {
                // All combinations evaluated — resume parent with collected results
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            } else {
                // Stage 1d-revised: evaluate next combination with its specific
                // per-combo bindings composed with outer_carrying.
                let next = remaining.next().expect("remaining is non-empty");
                let next_b = if remaining_bindings.is_empty() {
                    crate::backend::models::GenericBindings::new()
                } else {
                    remaining_bindings.remove(0)
                };
                let combo_carrying = if outer_carrying.is_empty() {
                    next_b
                } else if next_b.is_empty() {
                    (*outer_carrying).clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying,
                        &next_b,
                        ctx.factory(),
                    )
                };

                continuations.push(Continuation::CollectApplicativeResults {
                    remaining,
                    remaining_bindings,
                    results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: next,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: std::sync::Arc::new(combo_carrying),
                });
            }
        }

        Continuation::ProcessMapAtom {
            mut remaining_elements,
            var_name,
            template,
            mut collected_results,
            env: _,
            depth,
            outer_carrying,
            mut acc_bindings,
        } => {
            let (mut result_values, result_env) = result;

            // H1 full (2026-05-05): propagate_keys-filtered binding composition.
            // Compute the set of names that MAY thread across iterations:
            //   propagate_keys = outer_carrying.keys
            //                  ∪ free_vars(remaining_elements)
            //                  ∪ (free_vars(template) - {var_name} - bound_vars(template))
            // Local binder-introduced names (`let`, `let*`, `sealed`, `function`,
            // `chain`, `unify`) are filtered out so iter-N's let* locals don't
            // collide with iter-(N+1)'s fresh rebindings (see
            // `test_state_mutation_inside_map_atom` which would fail without
            // this filter).
            let propagate_keys: smallvec::SmallVec<[crate::backend::models::BindingName; 16]> = {
                let mut keys: smallvec::SmallVec<[crate::backend::models::BindingName; 16]> =
                    smallvec::SmallVec::new();
                for (name, _) in outer_carrying.iter() {
                    if !keys.iter().any(|key| key.matches(name)) {
                        keys.push(crate::backend::models::BindingName::from(name));
                    }
                }
                for item in remaining_elements.as_slice() {
                    for v in item.free_variables() {
                        if !keys.iter().any(|key| key.matches(v)) {
                            keys.push(crate::backend::models::BindingName::from(v));
                        }
                    }
                }
                let bound = template.bound_variables();
                for v in template.free_variables() {
                    if v != var_name.as_str()
                        && !bound.contains(&v)
                        && !keys.iter().any(|key| key.matches(v))
                    {
                        keys.push(crate::backend::models::BindingName::from(v));
                    }
                }
                keys
            };

            // Add first result from evaluation, threading its bindings into
            // acc_bindings (mirrors HE's `chain` binding-threading semantics).
            if result_values.is_empty() {
                collected_results.push(bv(ctx.factory().unit()));
            } else {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.0.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![first_result], result_env),
                    });
                    return;
                }
                // Filter per-iter bindings to propagate_keys, then compose.
                if !first_result.1.is_empty() {
                    let mut filtered = crate::backend::models::GenericBindings::default();
                    for (name, val) in first_result.1.iter() {
                        if propagate_keys.iter().any(|key| key.matches(name)) {
                            filtered.insert(name, val.clone());
                        }
                    }
                    if !filtered.is_empty() {
                        let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                            &acc_bindings,
                            &filtered,
                            ctx.factory(),
                        );
                        // Genuine ground/ground caller-scope conflict → kill branch.
                        if composed.is_empty() && !acc_bindings.is_empty() && !filtered.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                            return;
                        }
                        acc_bindings = std::sync::Arc::new(composed);
                    }
                }
                collected_results.push(first_result);
            }

            if remaining_elements.len() == 0 {
                // All elements processed - return result list.
                // H1: attach compose(outer_carrying, acc_bindings) so per-iter
                // bindings flow back to the caller (HE-bisimilar). Previously
                // `bv_with(..., outer_carrying)` discarded acc_bindings entirely,
                // breaking apply_subst patterns like
                // `(map-atom $stmt $tok (apply_subst_tok $subst $tok))`.
                let result_list = ctx
                    .factory()
                    .sexpr(collected_results.into_iter().map(|(v, _)| v).collect());
                let final_bindings = if acc_bindings.is_empty() {
                    (*outer_carrying).clone()
                } else if outer_carrying.is_empty() {
                    (*acc_bindings).clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &outer_carrying,
                        &acc_bindings,
                        ctx.factory(),
                    )
                };
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(result_list, final_bindings)], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements
                    .next()
                    .expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated =
                    substitute_variable_generic(&template, &var_name, &next_element, ctx.factory());

                // H1 full (2026-05-05): thread acc_bindings to next iter's
                // carrying via compose(outer_carrying, acc_bindings). The
                // propagate_keys filter (above) ensures only caller-scope
                // names enter acc_bindings, so let*-fresh-scope semantics
                // are preserved.
                let next_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if acc_bindings.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        acc_bindings.clone()
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &outer_carrying,
                                &acc_bindings,
                                ctx.factory(),
                            ),
                        )
                    };

                continuations.push(Continuation::ProcessMapAtom {
                    remaining_elements,
                    var_name,
                    template,
                    collected_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                    acc_bindings,
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: next_carrying,
                });
            }
        }

        Continuation::ProcessFilterAtom {
            current_element,
            mut remaining_elements,
            var_name,
            predicate,
            mut filtered_results,
            env: _,
            depth,
            outer_carrying,
            mut acc_bindings,
        } => {
            let (mut result_values, result_env) = result;

            // H13 (2026-05-05): mirror of map-atom's H1 full —
            // propagate_keys-filtered binding composition.
            //   propagate_keys = outer_carrying.keys
            //                  ∪ free_vars(remaining_elements)
            //                  ∪ (free_vars(predicate) - {var_name}
            //                                          - bound_vars(predicate))
            // Local binder-introduced names (`let`, `let*`, `sealed`, `function`,
            // `chain`, `unify`) are filtered out so iter-N's let* locals don't
            // collide with iter-(N+1)'s fresh rebindings. The accumulator
            // `acc_bindings` flows binding emissions across iterations even
            // when the keep/drop verdict differs — matching HE's
            // `chain (eval (sealed (V) F)) ... (cons-atom ...)` binding flow.
            let propagate_keys: smallvec::SmallVec<[crate::backend::models::BindingName; 16]> = {
                let mut keys: smallvec::SmallVec<[crate::backend::models::BindingName; 16]> =
                    smallvec::SmallVec::new();
                for (name, _) in outer_carrying.iter() {
                    if !keys.iter().any(|key| key.matches(name)) {
                        keys.push(crate::backend::models::BindingName::from(name));
                    }
                }
                for item in remaining_elements.as_slice() {
                    for v in item.free_variables() {
                        if !keys.iter().any(|key| key.matches(v)) {
                            keys.push(crate::backend::models::BindingName::from(v));
                        }
                    }
                }
                let bound = predicate.bound_variables();
                for v in predicate.free_variables() {
                    if v != var_name.as_str()
                        && !bound.contains(&v)
                        && !keys.iter().any(|key| key.matches(v))
                    {
                        keys.push(crate::backend::models::BindingName::from(v));
                    }
                }
                keys
            };

            // Check predicate result and optionally include current element
            if !result_values.is_empty() {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.0.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![first_result], result_env),
                    });
                    return;
                }

                // H13: filter per-iter bindings to propagate_keys, then compose.
                // Composition occurs BEFORE the keep/drop test — bindings flow
                // to the next iteration regardless of whether the predicate
                // returned True or False (matching HE's chain semantics).
                if !first_result.1.is_empty() {
                    let mut filtered = crate::backend::models::GenericBindings::default();
                    for (name, val) in first_result.1.iter() {
                        if propagate_keys.iter().any(|key| key.matches(name)) {
                            filtered.insert(name, val.clone());
                        }
                    }
                    if !filtered.is_empty() {
                        let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                            &acc_bindings,
                            &filtered,
                            ctx.factory(),
                        );
                        // Genuine ground/ground caller-scope conflict → kill branch.
                        if composed.is_empty() && !acc_bindings.is_empty() && !filtered.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                            return;
                        }
                        acc_bindings = std::sync::Arc::new(composed);
                    }
                }

                let should_include = if let Some(b) = first_result.0.as_bool() {
                    b
                } else {
                    !first_result.0.is_unit()
                };

                if should_include {
                    if let Some(elem) = current_element {
                        filtered_results.push(bv(elem));
                    }
                }
            }

            if remaining_elements.len() == 0 {
                // All elements processed - return filtered list.
                // H13: attach compose(outer_carrying, acc_bindings) so per-iter
                // bindings flow back to the caller (mirror map-atom's H1 full).
                let result_list = ctx
                    .factory()
                    .sexpr(filtered_results.into_iter().map(|(v, _)| v).collect());
                let final_bindings = if acc_bindings.is_empty() {
                    (*outer_carrying).clone()
                } else if outer_carrying.is_empty() {
                    (*acc_bindings).clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &outer_carrying,
                        &acc_bindings,
                        ctx.factory(),
                    )
                };
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(result_list, final_bindings)], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements
                    .next()
                    .expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &predicate,
                    &var_name,
                    &next_element,
                    ctx.factory(),
                );

                // H13: thread acc_bindings to next iter via compose.
                let next_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if acc_bindings.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        acc_bindings.clone()
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &outer_carrying,
                                &acc_bindings,
                                ctx.factory(),
                            ),
                        )
                    };

                continuations.push(Continuation::ProcessFilterAtom {
                    current_element: Some(next_element),
                    remaining_elements,
                    var_name,
                    predicate,
                    filtered_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                    acc_bindings,
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: next_carrying,
                });
            }
        }

        Continuation::ProcessFoldlAtom {
            remaining_elements,
            acc_var_name,
            item_var_name,
            operation,
            env: _,
            depth,
            acc_bindings,
        } => {
            let (mut result_values, result_env) = result;

            // Phase 1 (HE-bisimilar foldl-atom binding threading):
            //
            // HE's `foldl-atom` is a recursive user-level MeTTa definition:
            //
            //   (= (foldl-atom $list $init $op)
            //      (if (== $list ())
            //          $init
            //          (foldl-atom (cdr-atom $list) ($op $init (car-atom $list)) $op)))
            //
            // When the op evaluates nondeterministically to N branches at
            // iteration K, HE's interpreter dispatches the outer recursive
            // `foldl-atom` call N TIMES — once per branch — with each
            // branch's own accumulator value and bindings. Subsequent
            // iterations therefore see each branch's per-branch bindings
            // as their ambient context, and a premise that references a
            // variable bound in iteration K (e.g. `(father $b c)` after
            // `(father $a $b)` bound `$b=c`) substitutes → `(father c c)`
            // → no rule match → that branch dies. HE's semantics naturally
            // prune inconsistent branches via this per-branch substitution.
            //
            // MeTTaTron previously took `result_values.swap_remove(0)` —
            // collapsing all N branches into the FIRST by arbitrary order,
            // discarding the other N-1. That broke PLN's `=>` macro (and
            // any foldl-atom over a premise list with shared variables),
            // because spurious branches with inconsistent variable bindings
            // could "win" depending on rule-dispatch enumeration order.
            //
            // Fix: fan out N branches as N independent `(foldl-atom
            // <rest> <branch_acc> <op>)` sub-evaluations, each under its
            // own branch_bindings. ProcessAmb collects their final results,
            // preserving all surviving branches' outputs. This mirrors HE's
            // natural recursive nondeterministic fanout. MeTTa's
            // nondeterministic enumeration order is preserved — no sorts,
            // no determinism-forcing; results are merged in whatever order
            // ProcessAmb dispatches them.

            // Fail-fast: iteration produced no results → fold fails.
            if result_values.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
                return;
            }

            // Prune: Empty literals (failed rule applications) are dead
            // branches; discard them. Keep error branches (they propagate).
            // Use the broader sentinel check to also catch user-level
            // `Atom("Empty")` (HE-bisim §06.4.5; T04/063, T04/082-dir4).
            result_values.retain(|(v, _)| !v.is_empty_sentinel());

            if result_values.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
                return;
            }

            // Error propagation: if any branch produced an error, bubble
            // the first one up (matching HE's error-short-circuit on
            // reserved Error/_erratom).
            if let Some(err_pos) = result_values.iter().position(|(v, _)| v.is_error()) {
                let err_bv = result_values.swap_remove(err_pos);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![err_bv], result_env),
                });
                return;
            }

            // Materialize remaining_elements — we'll reuse the list across
            // branches (each branch's sub-foldl needs its own iterator).
            let remaining_vec: Vec<MettaValue> = remaining_elements.collect();
            let has_more = !remaining_vec.is_empty();

            // Collect free variables of REMAINING items — these are the
            // caller-scope (possibly outer-rule-freshened) vars that
            // must thread across iterations because subsequent items
            // reference them. The op's own per-invocation freshened
            // vars are NOT in this set (they're scoped to the op body).
            //
            // Note: we also include vars from the current iteration's
            // item path via acc_bindings' existing keys — anything
            // previously-bound stays bound.
            let propagate_keys: Vec<crate::backend::models::BindingName> = {
                let mut keys: Vec<crate::backend::models::BindingName> = Vec::new();
                for item in &remaining_vec {
                    for v in item.free_variables() {
                        if !keys.iter().any(|key| key.matches(v)) {
                            keys.push(crate::backend::models::BindingName::from(v));
                        }
                    }
                }
                // Plus anything already in acc_bindings (carried forward).
                for (k, _) in acc_bindings.iter() {
                    if !keys.iter().any(|key| key.matches(k)) {
                        keys.push(crate::backend::models::BindingName::from(k));
                    }
                }
                keys
            };

            if !has_more {
                // Final iteration: every surviving branch is a final
                // accumulator. Compose each branch's bindings with the
                // fold's accumulated bindings (which capture previous
                // iterations' per-branch substitutions) and emit them all.
                let final_results: SmallVec<[BoundValue; 2]> = result_values
                    .into_iter()
                    .filter_map(|(v, child_b)| {
                        let filtered = filter_fold_propagating_bindings(&child_b, &propagate_keys);
                        let mut composed =
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*acc_bindings,
                                &filtered,
                                ctx.factory(),
                            );
                        // Prune branches whose final USER-LEVEL bindings
                        // are inconsistent (compose returned empty from
                        // two non-empty user-bindings inputs — HE's
                        // strict unification rejection).
                        if composed.is_empty() && !acc_bindings.is_empty() && !filtered.is_empty() {
                            return None;
                        }
                        crate::backend::eval::bindings::apply_chain_generic(
                            &mut composed,
                            ctx.factory(),
                        );
                        Some((v, composed))
                    })
                    .collect();
                work_stack.push(WorkItem::Resume {
                    result: (final_results, result_env),
                });
                return;
            }

            if result_values.len() == 1 {
                // Fast path: single surviving branch. Continue the fold
                // linearly — this is the common case (most folds operate
                // on ground-valued expressions that produce one result
                // per iteration).
                let (first_result, child_b) = result_values.swap_remove(0);

                // Filter child_b to retain only bindings that should
                // thread across iterations: user-level vars OR caller-
                // scope freshened vars (those appearing in remaining
                // items' free variables). The op's per-invocation
                // freshened vars are dropped — they'd otherwise cause
                // spurious ground/ground conflicts on repeat invocations
                // of the same op rule (MeTTaTron one-time-freshening
                // artifact with no HE analogue).
                let filtered_child_b = filter_fold_propagating_bindings(&child_b, &propagate_keys);

                // Compose user-level child bindings with accumulator
                // bindings. Genuine user-level conflicts still abort the
                // fold (HE-faithful).
                let mut composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                    &*acc_bindings,
                    &filtered_child_b,
                    ctx.factory(),
                );
                if composed.is_empty() && !acc_bindings.is_empty() && !filtered_child_b.is_empty() {
                    // Genuine user-level binding inconsistency — branch
                    // dies. Matches HE's `Bindings::merge` strict
                    // rejection.
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                    return;
                }
                crate::backend::eval::bindings::apply_chain_generic(&mut composed, ctx.factory());
                let new_acc_bindings = std::sync::Arc::new(composed);

                let mut remaining_iter = remaining_vec.into_iter();
                let next_element = remaining_iter
                    .next()
                    .expect("remaining_vec.is_empty() short-circuited above");

                let instantiated = substitute_variable_generic(
                    &operation,
                    &acc_var_name,
                    &first_result,
                    ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated,
                    &item_var_name,
                    &next_element,
                    ctx.factory(),
                );

                let acc_bindings_for_eval = new_acc_bindings.clone();
                continuations.push(Continuation::ProcessFoldlAtom {
                    remaining_elements: remaining_iter,
                    acc_var_name,
                    item_var_name,
                    operation,
                    env: result_env.clone(),
                    depth,
                    acc_bindings: new_acc_bindings,
                });

                // HE parity: when acc_bindings is non-empty, dispatch via
                // EvalWithBindings so the inner rule-match path consults
                // the accumulated bindings during unification. This lets
                // a variable bound in iteration K influence rule matching
                // in iteration K+1 (the core of the HE-faithful fix).
                if acc_bindings_for_eval.is_empty() {
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: acc_bindings_for_eval,
                    });
                } else {
                    work_stack.push(WorkItem::EvalWithBindings {
                        template: instantiated,
                        bindings: acc_bindings_for_eval.clone(),
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        carrying_bindings: acc_bindings_for_eval,
                    });
                }
                return;
            }

            // Multi-branch: iteration produced N > 1 nondeterministic
            // results. Fan out N independent sub-folds, each continuing
            // the fold over `remaining_vec` with its own accumulator and
            // composed bindings. ProcessAmb sequentially dispatches each
            // alt (preserving enumeration order without forcing it) and
            // collects all survivors at the end.
            //
            // Each alt is the 5-arg foldl-atom form
            //   (foldl-atom <remaining-list> <branch_acc> $acc $item <op>)
            // where `op` is ProcessFoldlAtom's existing `operation` field
            // (an already-wrapped substitution template `(func $acc $item)`).
            // The 5-arg form routes through StartFoldlAtom → ProcessFoldlAtom,
            // seeding acc_bindings from carrying_bindings so each sub-fold
            // starts with its branch's accumulated state intact.
            //
            // We must NOT use the 3-arg form here: it would re-wrap
            // `operation` into `(operation $__fa_acc $__fa_item)`, producing
            // a spurious double-wrap and breaking the fold.
            let remaining_list = ctx.factory().sexpr(remaining_vec);
            let foldl_sym = ctx.factory().atom("foldl-atom");
            let acc_var_atom = ctx.factory().atom(acc_var_name.as_str());
            let item_var_atom = ctx.factory().atom(item_var_name.as_str());

            let alternatives: Vec<BoundValue> = result_values
                .into_iter()
                .filter_map(|(branch_acc, child_b)| {
                    let filtered = filter_fold_propagating_bindings(&child_b, &propagate_keys);
                    let mut composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*acc_bindings,
                        &filtered,
                        ctx.factory(),
                    );
                    // Prune branches with genuinely inconsistent
                    // USER-LEVEL bindings.
                    if composed.is_empty() && !acc_bindings.is_empty() && !filtered.is_empty() {
                        return None;
                    }
                    crate::backend::eval::bindings::apply_chain_generic(
                        &mut composed,
                        ctx.factory(),
                    );
                    let sub_foldl = ctx.factory().sexpr(vec![
                        foldl_sym,
                        remaining_list.clone(),
                        branch_acc,
                        acc_var_atom,
                        item_var_atom,
                        operation.clone(),
                    ]);
                    Some((sub_foldl, composed))
                })
                .collect();

            // If all branches are inconsistent, the fold fails entirely.
            if alternatives.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
                return;
            }

            let mut alts_iter = alternatives.into_iter();
            let (first_val, first_b) = alts_iter
                .next()
                .expect("alternatives.is_empty() short-circuited above");

            // ProcessAmb collects each alt's results into one merged
            // Resume. outer_carrying is empty here because each alt's
            // bindings already subsume acc_bindings via the compose
            // above — no need for ProcessAmb to re-compose.
            continuations.push(Continuation::ProcessAmb {
                remaining_alts: alts_iter.collect::<Vec<_>>().into_iter(),
                results: Vec::new(),
                env: result_env.clone(),
                depth,
                outer_carrying: crate::backend::eval::trampoline::types::empty_shared_bindings(),
            });

            work_stack.push(WorkItem::Eval {
                value: first_val,
                env: result_env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
                demand: None,
                carrying_bindings: std::sync::Arc::new(first_b),
            });
        }

        Continuation::ProcessSortTuple {
            mut sorted,
            mut unsorted,
            current,
            insert_pos,
            var1_name,
            var2_name,
            comparator,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (cmp_results, result_env) = result;

            // Extract boolean comparison result
            let cmp_true = cmp_results
                .first()
                .and_then(|(v, _)| v.as_bool())
                .unwrap_or(false);

            if cmp_true {
                // current < sorted[insert_pos]: insert current here
                sorted.insert(insert_pos, current);
            } else {
                // current >= sorted[insert_pos]: try next position
                let next_pos = insert_pos + 1;
                if next_pos < sorted.len() {
                    // Compare current vs sorted[next_pos]
                    let instantiated = substitute_variable_generic(
                        &comparator,
                        &var1_name,
                        &current,
                        ctx.factory(),
                    );
                    let instantiated = substitute_variable_generic(
                        &instantiated,
                        &var2_name,
                        &sorted[next_pos],
                        ctx.factory(),
                    );

                    continuations.push(Continuation::ProcessSortTuple {
                        sorted,
                        unsorted,
                        current,
                        insert_pos: next_pos,
                        var1_name,
                        var2_name,
                        comparator,
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                    return;
                } else {
                    // current >= all sorted elements: insert at end
                    sorted.push(current);
                }
            }

            // Move to next unsorted element
            if unsorted.is_empty() {
                // Sorting complete
                let result_tuple = ctx.factory().sexpr(sorted);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_tuple)], result_env),
                });
            } else {
                let next_current = unsorted.remove(0);

                // Compare next_current vs sorted[0]
                let instantiated = substitute_variable_generic(
                    &comparator,
                    &var1_name,
                    &next_current,
                    ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated,
                    &var2_name,
                    &sorted[0],
                    ctx.factory(),
                );

                continuations.push(Continuation::ProcessSortTuple {
                    sorted,
                    unsorted,
                    current: next_current,
                    insert_pos: 0,
                    var1_name,
                    var2_name,
                    comparator,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        Continuation::ProcessBestCandidate {
            best,
            best_rank,
            mut remaining,
            current,
            var_name,
            rank_fn,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (rank_results, result_env) = result;

            // Extract numeric rank from evaluation result
            let current_rank = rank_results
                .first()
                .and_then(|(v, _)| v.as_float().or_else(|| v.as_long().map(|l| l as f64)));

            // Determine new best
            let (new_best, new_best_rank) = match (current_rank, best_rank) {
                (Some(cr), Some(br)) if cr > br => (current, Some(cr)),
                (Some(cr), None) => (current, Some(cr)),
                _ => (best.unwrap_or(current), best_rank),
            };

            if remaining.len() == 0 {
                // Done — return best
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(new_best)], result_env),
                });
            } else {
                // Evaluate next element's rank
                let next = remaining.next().expect("remaining is non-empty");

                let instantiated =
                    substitute_variable_generic(&rank_fn, &var_name, &next, ctx.factory());

                continuations.push(Continuation::ProcessBestCandidate {
                    best: Some(new_best),
                    best_rank: new_best_rank,
                    remaining,
                    current: next,
                    var_name,
                    rank_fn,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        Continuation::ProcessIfCondition {
            then_branch,
            else_branch,
            outer_bindings,
            env: _,
            depth,
            outer_carrying,
            outer_demand: _,
        } => {
            let (cond_results, env_after_cond) = result;

            // BUG-T0-009 (spec §06.6, §12): when the condition yields multiple
            // nondeterministic results, fan-out across all alternatives.
            //
            // Approach (per plan T0.B, Plan-agent design 2026-05-11): wrap each
            // condition alternative as a fresh `(if <literal-cond> then else)`
            // expression and dispatch them through a single `ProcessAmb`
            // continuation (same shape used by `superpose` at line 4598+ and
            // `case` at 9990+). Each wrapped if re-enters this handler with
            // `cond_results.len() == 1` and trivially hits the single-result
            // fast path below, which already handles True/False/Error/non-Bool
            // alternatives correctly. ProcessAmb collects all alt results into
            // a single outer `Resume`, preserving the 1:1 Resume↔continuation
            // discipline (avoids the "non-empty continuation stack" panic).
            //
            // PLN cardinality safety: the multi-result path is gated on
            // `cond_results.len() > 1`. The common case (PLN's
            // `Demand::Exactly(1)` condition descent at line 4040-4042) takes
            // the unchanged single-result fast path below, so the Phase 3
            // regression that ballooned Robot.metta peak RSS from 219MB to
            // >900MB (see warning at lines 9148-9158) cannot recur.
            if cond_results.len() > 1 {
                let amb_capacity = cond_results.len();
                let alts: Vec<crate::backend::eval::trampoline::types::BoundValue> = cond_results
                    .iter()
                    .map(|(cond_val, cond_b)| {
                        // Wrap this alternative as a literal-condition if. When
                        // re-entered, the literal short-circuits to its value
                        // and this handler dispatches the appropriate branch.
                        // Cloning `then`/`else` per alt is the unavoidable cost
                        // of fan-out — they are MettaValue (Copy / 8 bytes).
                        let alt_if = ctx.factory().sexpr(vec![
                            ctx.factory().atom("if"),
                            *cond_val,
                            then_branch,
                            else_branch,
                        ]);
                        (alt_if, cond_b.clone())
                    })
                    .collect();

                let mut alts_iter = alts.into_iter();
                let (first_val, first_b) =
                    alts_iter.next().expect("cond_results.len() > 1");

                continuations.push(Continuation::ProcessAmb {
                    remaining_alts: alts_iter,
                    results: Vec::with_capacity(amb_capacity),
                    env: env_after_cond.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Compose outer_carrying with this alt's bindings for the
                // first alt — mirrors the ProcessAmb handler's per-alt
                // composition at line 11885-11898.
                let first_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if first_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_b.clone())
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_b,
                                ctx.factory(),
                            ),
                        )
                    };
                let tracked = active_tracked_vars();
                let Some(first_carrying) = project_carrying_for_consumer(
                    &first_carrying,
                    &first_val,
                    tracked.as_deref(),
                    ctx.factory(),
                ) else {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), env_after_cond),
                    });
                    return;
                };

                work_stack.push(WorkItem::Eval {
                    value: first_val,
                    env: env_after_cond,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: first_carrying,
                });
                return;
            }

            if let Some((first, first_b)) = cond_results.first() {
                // Compose the condition's per-branch bindings with the
                // ambient outer_carrying so the taken branch inherits the
                // bindings that condition evaluation produced (HE-faithful).
                let branch_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if first_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_b.clone())
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                first_b,
                                ctx.factory(),
                            ),
                        )
                    };

                // Trace: condition-result phase
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(first),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "if".to_string(),
                                phase: "condition-result".to_string(),
                            },
                        );
                    }
                }

                // Check for error in condition
                if first.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(first.clone())], env_after_cond),
                    });
                    return;
                }

                // MeTTa HE semantics: if is pure pattern matching on True/False.
                // Bool(true) → then branch, Bool(false) → else branch,
                // Everything else (Unit, atoms, S-exprs) → return unreduced.
                if let Some(is_true) = first.as_bool() {
                    let branch = if is_true {
                        // Trace: then-branch phase
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(first),
                                    vec![crate::backend::trace::trace_value_generic(&then_branch)],
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "if".to_string(),
                                        phase: "then-branch".to_string(),
                                    },
                                );
                            }
                        }
                        then_branch
                    } else {
                        // Trace: else-branch phase
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(first),
                                    vec![crate::backend::trace::trace_value_generic(&else_branch)],
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "if".to_string(),
                                        phase: "else-branch".to_string(),
                                    },
                                );
                            }
                        }
                        else_branch
                    };
                    // Restore the caller demand for the selected branch. The
                    // condition itself is evaluated with `Exactly(1)`, but
                    // that bounded demand must not leak into the branch body.
                    let branch = if let Some(ob) = outer_bindings {
                        if branch.has_variables_fast() {
                            apply_bindings(&branch, &ob, ctx.factory())
                        } else {
                            branch
                        }
                    } else {
                        branch
                    };
                    let tracked = active_tracked_vars();
                    let Some(projected_branch_carrying) = project_carrying_for_consumer(
                        &branch_carrying,
                        &branch,
                        tracked.as_deref(),
                        ctx.factory(),
                    ) else {
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env_after_cond),
                        });
                        return;
                    };
                    work_stack.push(WorkItem::Eval {
                        value: branch,
                        env: env_after_cond,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: Some(crate::backend::eval::cesk::coroutine::Demand::All),
                        carrying_bindings: projected_branch_carrying,
                    });
                } else {
                    // Non-boolean (including Unit) → return unreduced (if cond then else)
                    //
                    // Spec §11.2.1's "non-Bool → NotReducible" rule is satisfied
                    // here as a *residual* normal form: the unreduced
                    // `(if cond then else)` expression matches no equation in
                    // MeTTaTron's space, so any caller that pattern-matches on
                    // it gets the NotReducible-equivalent "no further reduction"
                    // signal. HE bisimilarity: HE arrives at the same observable
                    // outcome via equation-lookup miss → frame-finished with the
                    // residual; MeTTaTron's `Continuation::ProcessIfCondition`
                    // emits the residual directly.
                    //
                    // Replacing this with a synthetic `NotReducible` atom (an
                    // attempted Phase 3 spec-strictness fix on 2026-04-26)
                    // caused PLN inference rules (collapse-bind / superpose-bind
                    // accumulators in lib_pln.metta's recursive Truth_*,
                    // PLN.Derive, LimitSize, and BestCandidate paths) to fan
                    // out cardinality at every collapse boundary because each
                    // NotReducible alternative was treated as a valid result
                    // rather than a stuck residual — Robot.metta peak RSS
                    // jumped from ~219 MB to >900 MB / OOM at 1 GB cap. The
                    // residual-expression form preserves the historical
                    // bounded-memory inference shape.
                    //
                    // Phase C: Materialize branches if outer_bindings present
                    // (needed for the unreduced (if cond then else) output).
                    let (mat_then, mat_else) = if let Some(ref ob) = outer_bindings {
                        (
                            apply_bindings(&then_branch, ob, ctx.factory()),
                            apply_bindings(&else_branch, ob, ctx.factory()),
                        )
                    } else {
                        (then_branch, else_branch)
                    };
                    // Trace: non-boolean phase
                    #[cfg(feature = "trace")]
                    {
                        if let Some(tc) = ctx.trace_collector() {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                depth as u32,
                                crate::backend::trace::trace_value_generic(first),
                                vec![],
                                None,
                                trace_format::TraceEventKind::SpecialForm {
                                    form_name: "if".to_string(),
                                    phase: "non-boolean".to_string(),
                                },
                            );
                        }
                    }
                    let unreduced = ctx.factory().sexpr(vec![
                        ctx.factory().atom("if"),
                        first.clone(),
                        mat_then,
                        mat_else,
                    ]);
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(unreduced)], env_after_cond),
                    });
                }
            } else {
                // MeTTa HE: if-condition produced zero results → entire if produces zero results.
                // This is branch annihilation: an empty condition means the if-expression
                // contributes nothing to the nondeterministic result set.
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), env_after_cond),
                });
            }
        }

        Continuation::ProcessCaseAtom {
            cases,
            outer_bindings,
            env: _,
            depth,
            outer_carrying,
        } => {
            // Phase C: If outer_bindings present, materialize cases (patterns +
            // templates may reference outer-scope variables).
            let cases = if let Some(ref ob) = outer_bindings {
                apply_bindings(&cases, ob, ctx.factory())
            } else {
                cases
            };
            let (atom_results, atom_env) = result;

            // Trace: scrutinee-result phase
            #[cfg(feature = "trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&cases),
                        atom_results
                            .iter()
                            .map(|(v, _)| crate::backend::trace::trace_value_generic(v))
                            .collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "case".to_string(),
                            phase: "scrutinee-result".to_string(),
                        },
                    );
                }
            }

            // Filter out Empty sentinels (both `ValueView::Empty` and the
            // user-visible `Atom("Empty")` symbol — HE-bisim §06.4.5).
            let filtered_results: Vec<_> = atom_results
                .into_iter()
                .filter(|(v, _)| !v.is_empty_sentinel())
                .collect();

            // Handle case when evaluation returns no results
            if filtered_results.is_empty() {
                // Match Empty against cases - NO conversion needed
                let empty_atom = ctx.factory().atom("Empty");
                match eval_switch(&empty_atom, &cases, ctx.factory()) {
                    SwitchResult::Match(template, _bindings) => {
                        let tracked = active_tracked_vars();
                        let Some(carrying) = project_carrying_for_consumer(
                            &outer_carrying,
                            &template,
                            tracked.as_deref(),
                            ctx.factory(),
                        ) else {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), atom_env),
                            });
                            return;
                        };
                        // Template needs evaluation
                        work_stack.push(WorkItem::Eval {
                            value: template,
                            env: atom_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: carrying,
                        });
                    }
                    SwitchResult::Error(err) => {
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(err)], atom_env),
                        });
                    }
                    SwitchResult::NoMatch => {
                        // No case matched - prune branch (MeTTa HE returns Empty)
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), atom_env),
                        });
                    }
                }
                return;
            }

            // MeTTa HE collapse semantics: fully evaluate each scrutinee result
            // before pattern matching. In HE, case is defined as:
            //   (= (case $atom $cases)
            //      (let $c (collapse $atom)
            //        (if (== (noeval $c) ())
            //          (id (switch-minimal Empty $cases))
            //          (chain (eval (superpose $c)) $e (id (switch-minimal $e $cases))))))
            // The `collapse` fully evaluates the scrutinee (including rule application
            // for each nondeterministic result), then `switch-minimal` matches against
            // the already-evaluated results. We mirror this by evaluating each raw
            // scrutinee result before matching.
            let mut remaining_raw = filtered_results.into_iter();
            let (first_raw, first_raw_bindings) =
                remaining_raw.next().expect("filtered_results is non-empty");

            // Task #68 gap-fix: preserve first_raw's bindings as
            // current_raw_bindings so the scrutinee re-eval inherits the
            // scrutinee-evaluation bindings (e.g. $who=a from rule match).
            // Previously this was `empty_shared_bindings()`, silently
            // dropping all scrutinee-level variable bindings.
            let first_raw_carrying = if first_raw_bindings.is_empty() {
                outer_carrying.clone()
            } else if outer_carrying.is_empty() {
                std::sync::Arc::new(first_raw_bindings.clone())
            } else {
                std::sync::Arc::new(crate::backend::eval::bindings::compose_outer_inner_generic(
                    &*outer_carrying,
                    &first_raw_bindings,
                    ctx.factory(),
                ))
            };
            let tracked = active_tracked_vars();
            let Some(first_raw_carrying) = project_carrying_for_consumer(
                &first_raw_carrying,
                &first_raw,
                tracked.as_deref(),
                ctx.factory(),
            ) else {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), atom_env),
                });
                return;
            };
            let first_raw_bindings = match project_owned_bindings_for_consumer(
                &first_raw_bindings,
                &first_raw,
                tracked.as_deref(),
                ctx.factory(),
            ) {
                Some(b) => b,
                None => {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), atom_env),
                    });
                    return;
                }
            };

            continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                remaining_raw,
                evaluated: vec![],
                cases,
                env: atom_env.clone(),
                depth,
                current_raw_bindings: std::sync::Arc::new(first_raw_bindings),
                outer_carrying: outer_carrying.clone(),
            });

            // Re-evaluate with the scrutinee's own bindings threaded through
            // carrying_bindings so rule matches inside the re-eval can resolve
            // variables bound by earlier rule matches in the scrutinee's
            // derivation chain.
            //
            // HE parity (2026-05-17 T04/117 fix): if the scrutinee result is
            // already an Error, skip re-eval. Errors are terminal in HE
            // (verified: `!(eval (Error foo bar))` → `[(eval (Error foo bar))]`
            // — NotReducible). Re-eval'ing an Error like `(Error (function) "msg")`
            // re-fires the inner `(function)` arity check, mangling the shape
            // so case patterns like `(Error $a $c)` no longer match.
            if first_raw.is_error() {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(first_raw)], atom_env),
                });
            } else {
                work_stack.push(WorkItem::Eval {
                    value: first_raw,
                    env: atom_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: first_raw_carrying,
                });
            }
        }

        Continuation::ProcessCaseMultiResults {
            mut remaining_atoms,
            cases,
            mut collected,
            env,
            depth,
            outer_carrying,
        } => {
            let (results, _result_env) = result;
            collected.extend(results);

            if let Some((next_atom, atom_bindings)) = remaining_atoms.next() {
                // Phase 2 Part A fix (task #63): compose per-atom bindings with
                // outer_carrying so scrutinee-bound variables flow into the
                // case body. Strict compose drops the case match on conflict.
                let per_atom_carrying: std::sync::Arc<
                    crate::backend::models::GenericBindings<MettaValue>,
                > = if atom_bindings.is_empty() {
                    outer_carrying.clone()
                } else if outer_carrying.is_empty() {
                    std::sync::Arc::new(atom_bindings.clone())
                } else {
                    match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                        &*outer_carrying,
                        &atom_bindings,
                        ctx.factory(),
                    ) {
                        Some(b) => std::sync::Arc::new(b),
                        None => {
                            // Conflict — skip this atom (HE-bisimilar drop).
                            continuations.push(Continuation::ProcessCaseMultiResults {
                                remaining_atoms,
                                cases,
                                collected,
                                env: env.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env),
                            });
                            return;
                        }
                    }
                };

                // Check if atom is empty - use trait methods, NO conversion
                let is_empty_atom = next_atom.is_empty()
                    || next_atom.as_sexpr().map_or(false, |items| items.is_empty());
                let switch_atom = if is_empty_atom {
                    ctx.factory().atom("Empty")
                } else {
                    next_atom
                };

                // Phase 8.6: Type-driven case pattern skipping.
                // If the scrutinee has a known ground type, filter case patterns
                // to only type-compatible ones. This avoids unnecessary pattern
                // matching against structurally incompatible patterns.
                let effective_cases = if let Some(scrutinee_type) = get_ground_type(&switch_atom) {
                    if let Some(case_pairs) = cases.as_sexpr() {
                        let filtered: Vec<MettaValue> = case_pairs
                            .iter()
                            .filter(|pair| {
                                pair.as_sexpr().map_or(true, |p| {
                                    p.first().map_or(true, |pattern| {
                                        is_pattern_type_compatible(pattern, scrutinee_type)
                                    })
                                })
                            })
                            .cloned()
                            .collect();
                        if filtered.len() < case_pairs.len() {
                            // Some patterns were skipped — use filtered cases
                            ctx.factory().sexpr(filtered)
                        } else {
                            cases.clone() // No change — use original
                        }
                    } else {
                        cases.clone()
                    }
                } else {
                    cases.clone()
                };

                // Use generic switch - NO conversion needed
                match eval_switch(&switch_atom, &effective_cases, ctx.factory()) {
                    SwitchResult::Match(template, _bindings) => {
                        continuations.push(Continuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });

                        // Task #68 gap-fix: WorkItem::Eval does NOT consume
                        // carrying_bindings for variable substitution — it
                        // only threads them as leaf metadata. When the case
                        // body references scrutinee-bound variables (not
                        // captured by the case pattern), they must be
                        // substituted via EvalWithBindings. Mirrors
                        // ProcessLet/ProcessChainBody precedent.
                        if template.has_variables_fast() && !per_atom_carrying.is_empty() {
                            let tracked = active_tracked_vars();
                            let Some(projected_bindings) = project_carrying_for_consumer(
                                &per_atom_carrying,
                                &template,
                                tracked.as_deref(),
                                ctx.factory(),
                            ) else {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), env),
                                });
                                return;
                            };
                            let Some(projected_outer) = project_carrying_for_consumer(
                                &outer_carrying,
                                &template,
                                tracked.as_deref(),
                                ctx.factory(),
                            ) else {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), env),
                                });
                                return;
                            };
                            work_stack.push(WorkItem::EvalWithBindings {
                                template,
                                bindings: projected_bindings,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                carrying_bindings: projected_outer,
                            });
                        } else {
                            let tracked = active_tracked_vars();
                            let Some(projected_carrying) = project_carrying_for_consumer(
                                &per_atom_carrying,
                                &template,
                                tracked.as_deref(),
                                ctx.factory(),
                            ) else {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), env),
                                });
                                return;
                            };
                            work_stack.push(WorkItem::Eval {
                                value: template,
                                env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: projected_carrying,
                            });
                        }
                    }
                    SwitchResult::Error(err) => {
                        // Collect error and continue
                        collected.push(bv(err));

                        continuations.push(Continuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });

                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env),
                        });
                    }
                    SwitchResult::NoMatch => {
                        // No match - continue to next atom
                        continuations.push(Continuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });

                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), env),
                        });
                    }
                }
            } else {
                // All atoms processed
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(collected), env),
                });
            }
        }

        // MeTTa HE collapse semantics: evaluate each raw scrutinee result to
        // normal form before pattern matching. This continuation sequentially
        // evaluates each raw result, collects the evaluated outputs, and then
        // performs the switch/pattern-match phase once all evaluations complete.
        Continuation::ProcessCaseEvalScrutineeResults {
            mut remaining_raw,
            mut evaluated,
            cases,
            env: _,
            depth,
            current_raw_bindings,
            outer_carrying,
        } => {
            let (eval_results, eval_env) = result;

            // Collect non-empty evaluated results (broader sentinel check —
            // catches both `ValueView::Empty` and `Atom("Empty")`)
            evaluated.extend(eval_results.into_iter().filter(|(v, _)| !v.is_empty_sentinel()));

            if let Some((next_raw, next_raw_bindings)) = remaining_raw.next() {
                // Task #68 gap-fix: preserve next_raw's bindings (previously
                // underscored and replaced with empty_shared_bindings()).
                let next_raw_carrying = if next_raw_bindings.is_empty() {
                    outer_carrying.clone()
                } else if outer_carrying.is_empty() {
                    std::sync::Arc::new(next_raw_bindings.clone())
                } else {
                    std::sync::Arc::new(
                        crate::backend::eval::bindings::compose_outer_inner_generic(
                            &*outer_carrying,
                            &next_raw_bindings,
                            ctx.factory(),
                        ),
                    )
                };
                let tracked = active_tracked_vars();
                let Some(next_raw_carrying) = project_carrying_for_consumer(
                    &next_raw_carrying,
                    &next_raw,
                    tracked.as_deref(),
                    ctx.factory(),
                ) else {
                    continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                        remaining_raw,
                        evaluated,
                        cases,
                        env: eval_env.clone(),
                        depth,
                        current_raw_bindings,
                        outer_carrying: outer_carrying.clone(),
                    });
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), eval_env),
                    });
                    return;
                };
                let next_raw_bindings = match project_owned_bindings_for_consumer(
                    &next_raw_bindings,
                    &next_raw,
                    tracked.as_deref(),
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => {
                        continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                            remaining_raw,
                            evaluated,
                            cases,
                            env: eval_env.clone(),
                            depth,
                            current_raw_bindings,
                            outer_carrying: outer_carrying.clone(),
                        });
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), eval_env),
                        });
                        return;
                    }
                };
                // More raw scrutinee results to evaluate — reuse cont slot
                continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                    remaining_raw,
                    evaluated,
                    cases,
                    env: eval_env.clone(),
                    depth,
                    current_raw_bindings: std::sync::Arc::new(next_raw_bindings),
                    outer_carrying: outer_carrying.clone(),
                });

                // HE parity (2026-05-17 T04/117 fix): Errors are terminal —
                // skip re-eval to preserve their structure for pattern matching.
                // See ProcessCaseAtom first_raw comment for full rationale.
                if next_raw.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(next_raw)], eval_env),
                    });
                    return;
                }

                work_stack.push(WorkItem::Eval {
                    value: next_raw,
                    env: eval_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: next_raw_carrying,
                });
            } else {
                // All raw results evaluated — now perform pattern matching
                if evaluated.is_empty() {
                    // All evaluations produced empty — match Empty against cases
                    let empty_atom = ctx.factory().atom("Empty");
                    match eval_switch(&empty_atom, &cases, ctx.factory()) {
                        SwitchResult::Match(template, _bindings) => {
                            let tracked = active_tracked_vars();
                            let Some(carrying) = project_carrying_for_consumer(
                                &outer_carrying,
                                &template,
                                tracked.as_deref(),
                                ctx.factory(),
                            ) else {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), eval_env),
                                });
                                return;
                            };
                            work_stack.push(WorkItem::Eval {
                                value: template,
                                env: eval_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: carrying,
                            });
                        }
                        SwitchResult::Error(err) => {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(err)], eval_env),
                            });
                        }
                        SwitchResult::NoMatch => {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), eval_env),
                            });
                        }
                    }
                    return;
                }

                // Match each evaluated result against cases.
                // Phase 2 Part A fix (task #63): preserve per-atom bindings
                // in the iterator so the case body sees scrutinee bindings.
                let mut eval_atoms = evaluated.into_iter().collect::<Vec<_>>().into_iter();

                if let Some((first_atom, first_atom_bindings)) = eval_atoms.next() {
                    // Compose per-atom bindings with outer_carrying for this
                    // atom's case body evaluation. Strict: drop on conflict.
                    let first_carrying: std::sync::Arc<
                        crate::backend::models::GenericBindings<MettaValue>,
                    > = if first_atom_bindings.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_atom_bindings.clone())
                    } else {
                        match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                            &*outer_carrying,
                            &first_atom_bindings,
                            ctx.factory(),
                        ) {
                            Some(b) => std::sync::Arc::new(b),
                            None => {
                                // Conflict on first atom — move to rest.
                                if eval_atoms.len() == 0 {
                                    work_stack.push(WorkItem::Resume {
                                        result: (SmallVec::new(), eval_env),
                                    });
                                } else {
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms: eval_atoms,
                                        cases,
                                        collected: vec![],
                                        env: eval_env.clone(),
                                        depth,
                                        outer_carrying: outer_carrying.clone(),
                                    });
                                    work_stack.push(WorkItem::Resume {
                                        result: (SmallVec::new(), eval_env),
                                    });
                                }
                                return;
                            }
                        }
                    };
                    let is_empty_atom = first_atom.is_empty()
                        || first_atom
                            .as_sexpr()
                            .map_or(false, |items| items.is_empty());
                    let switch_atom = if is_empty_atom {
                        ctx.factory().atom("Empty")
                    } else {
                        first_atom
                    };

                    match eval_switch(&switch_atom, &cases, ctx.factory()) {
                        SwitchResult::Match(template, _bindings) => {
                            // Trace: case-match phase
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&switch_atom),
                                        vec![crate::backend::trace::trace_value_generic(&template)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "case".to_string(),
                                            phase: "case-match".to_string(),
                                        },
                                    );
                                }
                            }

                            // Task #68 gap-fix: route case body through
                            // EvalWithBindings when template has free vars
                            // and per-atom carrying is non-empty. Mirrors
                            // ProcessCaseMultiResults fix at ~7383.
                            if eval_atoms.len() == 0 {
                                if template.has_variables_fast() && !first_carrying.is_empty() {
                                    let tracked = active_tracked_vars();
                                    let Some(projected_bindings) = project_carrying_for_consumer(
                                        &first_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    let Some(projected_outer) = project_carrying_for_consumer(
                                        &outer_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    work_stack.push(WorkItem::EvalWithBindings {
                                        template,
                                        bindings: projected_bindings,
                                        env: eval_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        carrying_bindings: projected_outer,
                                    });
                                } else {
                                    let tracked = active_tracked_vars();
                                    let Some(projected_carrying) = project_carrying_for_consumer(
                                        &first_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    work_stack.push(WorkItem::Eval {
                                        value: template,
                                        env: eval_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        demand: None,
                                        carrying_bindings: projected_carrying,
                                    });
                                }
                            } else {
                                continuations.push(Continuation::ProcessCaseMultiResults {
                                    remaining_atoms: eval_atoms,
                                    cases,
                                    collected: vec![],
                                    env: eval_env.clone(),
                                    depth,
                                    // Remaining atoms still use outer_carrying as their
                                    // fallback ambient; each atom composes its own bindings
                                    // when it's popped for processing.
                                    outer_carrying: outer_carrying.clone(),
                                });

                                if template.has_variables_fast() && !first_carrying.is_empty() {
                                    let tracked = active_tracked_vars();
                                    let Some(projected_bindings) = project_carrying_for_consumer(
                                        &first_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    let Some(projected_outer) = project_carrying_for_consumer(
                                        &outer_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    work_stack.push(WorkItem::EvalWithBindings {
                                        template,
                                        bindings: projected_bindings,
                                        env: eval_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        carrying_bindings: projected_outer,
                                    });
                                } else {
                                    let tracked = active_tracked_vars();
                                    let Some(projected_carrying) = project_carrying_for_consumer(
                                        &first_carrying,
                                        &template,
                                        tracked.as_deref(),
                                        ctx.factory(),
                                    ) else {
                                        work_stack.push(WorkItem::Resume {
                                            result: (SmallVec::new(), eval_env),
                                        });
                                        return;
                                    };
                                    work_stack.push(WorkItem::Eval {
                                        value: template,
                                        env: eval_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        demand: None,
                                        carrying_bindings: projected_carrying,
                                    });
                                }
                            }
                        }
                        SwitchResult::Error(err) => {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(err)], eval_env),
                            });
                        }
                        SwitchResult::NoMatch => {
                            // Trace: case-no-match phase
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&switch_atom),
                                        vec![],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "case".to_string(),
                                            phase: "case-no-match".to_string(),
                                        },
                                    );
                                }
                            }

                            // No case matched — prune branch (MeTTa HE returns Empty)
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), eval_env),
                            });
                        }
                    }
                }
            }
        }

        Continuation::ProcessEvalEval {
            env: _,
            depth,
            outer_carrying,
            original_eval_expr,
        } => {
            let (eval_results, result_env) = result;

            if eval_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
            } else if eval_results.len() == 1 {
                // Single result - evaluate it (TCO)
                // Unwrap Quoted values: (eval (quote X)) → evaluate X.
                // Quoted is self-evaluating, so without this unwrap we'd loop.
                // Compose outer_carrying with this alt's per-branch bindings
                // so the re-evaluation inherits the alt's binding context.
                let (mut value, alt_b) = eval_results.into_iter().next().unwrap();

                // T04/105 (2026-05-17): HE `metta_call_return` parity for
                // eval/evalc forms (original_eval_expr is Some).
                //   - If result is NotReducible: convert to original (HE
                //     interpreter.rs:1456-1457). The nested-detection in
                //     EvalEvalStep propagates raw NotReducible UP to this
                //     ProcessEvalEval; THIS continuation does the conversion.
                //   - Otherwise: return value as-is (no transitive re-eval —
                //     HE's one-step eval semantics).
                //
                // EvalEval (capture/reduce/progn — original_eval_expr = None)
                // retains the existing full-reduction TCO re-eval below.
                if let Some(ref orig) = original_eval_expr {
                    let final_value = if matches!(
                        value.view(),
                        crate::backend::models::metta_value::ValueView::NotReducible
                    ) {
                        orig.clone()
                    } else {
                        // Unwrap Quoted: (eval (quote X)) → X.
                        if let Some(inner) = value.as_quoted() {
                            inner
                        } else {
                            value
                        }
                    };
                    // Compose alt_b into result bindings so caller-side variable
                    // bindings produced by the inner eval (e.g. via `unify`)
                    // flow upward (mirrors HE Bindings propagation).
                    let result_b = if alt_b.is_empty() {
                        crate::backend::models::GenericBindings::default()
                    } else {
                        alt_b
                    };
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![(final_value, result_b)], result_env),
                    });
                    return;
                }

                if let Some(inner) = value.as_quoted() {
                    value = inner;
                }
                let alt_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if alt_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(alt_b)
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &alt_b,
                                ctx.factory(),
                            ),
                        )
                    };
                work_stack.push(WorkItem::Eval {
                    value,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: alt_carrying,
                });
            } else {
                // Multiple results - fan out via ProcessAmb with per-alt bindings.
                // Each alternative carries its own `b` so downstream
                // evaluation inherits the alt's binding context (HE-faithful).
                // Unwrap Quoted values while preserving bindings.
                //
                // T04/105 (2026-05-17): for `eval`/`evalc` (original_eval_expr
                // is Some), HE one-step semantics: each NotReducible alt
                // converts to original; non-NotReducible alts return as-is
                // (no transitive re-eval).
                let results_vec: Vec<BoundValue> = eval_results
                    .into_iter()
                    .map(|(v, b)| {
                        let resolved = if let Some(ref orig) = original_eval_expr {
                            if matches!(
                                v.view(),
                                crate::backend::models::metta_value::ValueView::NotReducible
                            ) {
                                orig.clone()
                            } else if let Some(inner) = v.as_quoted() {
                                inner
                            } else {
                                v
                            }
                        } else if let Some(inner) = v.as_quoted() {
                            inner
                        } else {
                            v
                        };
                        (resolved, b)
                    })
                    .collect();

                // For eval/evalc (one-step): return all alts directly without re-eval.
                if original_eval_expr.is_some() {
                    work_stack.push(WorkItem::Resume {
                        result: (results_vec.into_iter().collect(), result_env),
                    });
                    return;
                }

                let mut results_iter = results_vec.into_iter();
                let amb_capacity = results_iter.len();
                let (first_val, first_b) = results_iter.next().unwrap();

                continuations.push(Continuation::ProcessAmb {
                    remaining_alts: results_iter,
                    results: Vec::with_capacity(amb_capacity),
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                let first_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if first_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_b)
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_b,
                                ctx.factory(),
                            ),
                        )
                    };

                work_stack.push(WorkItem::Eval {
                    value: first_val,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: first_carrying,
                });
            }
        }

        Continuation::ProcessReturn {
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (arg_results, arg_env) = result;

            // Check for errors first - pass through without wrapping
            if let Some((err, _b)) = arg_results.iter().find(|(r, _)| r.is_error()) {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err.clone())], arg_env),
                });
            } else {
                // Wrap results in return structure: (return value).
                //
                // T04/035 (2026-05-17): PRESERVE per-alt bindings. The chain's
                // unify-bound vars (e.g. `(unify B $a ...)` binding $a=B) flow
                // through the return wrapping so the outer chain's templ-eval
                // sees them. Previously bindings were dropped via `bv()`,
                // losing HE Bindings propagation.
                let return_results: Vec<BoundValue> = arg_results
                    .into_iter()
                    .map(|(r, b)| {
                        let wrapped = ctx.factory().sexpr(vec![ctx.factory().atom("return"), r]);
                        (wrapped, b)
                    })
                    .collect();
                work_stack.push(WorkItem::Resume {
                    result: (return_results.into_iter().collect(), arg_env),
                });
            }
        }

        Continuation::ProcessChainExpr {
            var,
            body,
            outer_bindings,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (expr_results, result_env) = result;

            // Trace: expr-result or expr-empty phase
            #[cfg(feature = "trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if expr_results.is_empty() {
                        "expr-empty"
                    } else {
                        "expr-result"
                    };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&var),
                        expr_results
                            .iter()
                            .map(|(v, _)| crate::backend::trace::trace_value_generic(v))
                            .collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "chain".to_string(),
                            phase: phase.to_string(),
                        },
                    );
                }
            }

            if expr_results.is_empty() {
                // Empty result — produce zero results (branch annihilation, HE-compatible)
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
            } else if expr_results.len() == 1 {
                // Single result - substitute and evaluate body with this
                // alt's bindings composed into both ob (deferred) AND
                // carrying_bindings (ambient for downstream rule matching).
                let (first_val, first_b) = expr_results.into_iter().next().unwrap();
                let var_name = var.as_atom().unwrap_or("");
                let alt_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if first_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_b.clone())
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_b,
                                ctx.factory(),
                            ),
                        )
                    };
                if let Some(mut ob) = outer_bindings {
                    // Merge alt_b into ob so body-eval resolves alt-bound vars.
                    // Layer C: `ob` is `SharedBindings` (Arc). Use `Arc::make_mut`
                    // to gain mutable access — single-owner path is O(1) in-place,
                    // shared path clones once (no worse than the old Box pattern).
                    {
                        let ob_mut = std::sync::Arc::make_mut(&mut ob);
                        for (k, v) in first_b.iter() {
                            ob_mut.insert(k, v.clone());
                        }
                        ob_mut.insert(var_name, first_val.clone());
                    }
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            carrying_bindings: alt_carrying,
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: alt_carrying,
                        });
                    }
                } else {
                    let instantiated =
                        substitute_variable_generic(&body, var_name, &first_val, ctx.factory());
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: alt_carrying,
                    });
                }
            } else {
                // Multiple results - chain evaluates each alt with its own
                // per-branch bindings composed into ob/carrying_bindings.
                let chain_capacity = expr_results.len();
                let mut remaining_values = expr_results.into_iter().collect::<Vec<_>>().into_iter();
                let (first_val, first_b) = remaining_values.next().unwrap();

                continuations.push(Continuation::ProcessChainBody {
                    remaining_values,
                    var: var.clone(),
                    body: body.clone(),
                    outer_bindings: outer_bindings.clone(),
                    results: Vec::with_capacity(chain_capacity),
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                let var_name = var.as_atom().unwrap_or("");
                let alt_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if first_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_b.clone())
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_b,
                                ctx.factory(),
                            ),
                        )
                    };
                if let Some(mut ob) = outer_bindings {
                    {
                        let ob_mut = std::sync::Arc::make_mut(&mut ob);
                        for (k, v) in first_b.iter() {
                            ob_mut.insert(k, v.clone());
                        }
                        ob_mut.insert(var_name, first_val);
                    }
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: alt_carrying,
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: alt_carrying,
                        });
                    }
                } else {
                    let instantiated =
                        substitute_variable_generic(&body, var_name, &first_val, ctx.factory());
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: alt_carrying,
                    });
                }
            }
        }

        Continuation::ProcessChainBody {
            mut remaining_values,
            var,
            body,
            outer_bindings,
            mut results,
            env,
            depth,
            outer_carrying,
        } => {
            let (body_results, _result_env) = result;
            results.extend(body_results);

            if let Some((next_val, next_b)) = remaining_values.next() {
                continuations.push(Continuation::ProcessChainBody {
                    remaining_values,
                    var: var.clone(),
                    body: body.clone(),
                    outer_bindings: outer_bindings.clone(),
                    results,
                    env: env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                let var_name = var.as_atom().unwrap_or("");
                let alt_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if next_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(next_b.clone())
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &next_b,
                                ctx.factory(),
                            ),
                        )
                    };
                if let Some(mut ob) = outer_bindings {
                    {
                        let ob_mut = std::sync::Arc::make_mut(&mut ob);
                        for (k, v) in next_b.iter() {
                            ob_mut.insert(k, v.clone());
                        }
                        ob_mut.insert(var_name, next_val);
                    }
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: alt_carrying,
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: alt_carrying,
                        });
                    }
                } else {
                    let instantiated =
                        substitute_variable_generic(&body, var_name, &next_val, ctx.factory());
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: alt_carrying,
                    });
                }
            } else {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), env),
                });
            }
        }

        Continuation::ProcessFunction {
            iteration_count,
            env: _,
            depth,
            outer_carrying,
        } => {
            const MAX_ITERATIONS: usize = 1000;
            let (eval_results, current_env) = result;

            // Helper: yield a fresh `$__function_result_N` variable when
            // the function body terminates without producing a `(return X)`.
            // HE empirical (T04/023, T04/038): `!(function (just-data))` →
            // `[$result#NN]` where `$result` is a fresh variable. MTT used to
            // forward the body's terminating value (e.g. `[(just-data)]`),
            // which diverged from HE since the function had no explicit
            // return. Use a process-wide atomic counter for the freshening
            // index — α-equivalence drops the name and uses positional
            // identity, so any unique fresh variable suffices.
            fn fresh_function_result<C: EvalContext>(ctx: &C) -> MettaValue {
                static FUNCTION_RESULT_COUNTER: std::sync::atomic::AtomicU64 =
                    std::sync::atomic::AtomicU64::new(0);
                let n = FUNCTION_RESULT_COUNTER
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let name = format!("$__function_result_{}", n);
                let interned: &'static str = crate::backend::models::global_allocator()
                    .alloc_str(&name);
                ctx.factory().atom(interned)
            }

            if eval_results.is_empty() {
                // No body result — yield fresh variable per HE function
                // semantics (`function` of an empty/failed body → fresh).
                let fresh = fresh_function_result(ctx);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(fresh)], current_env),
                });
            } else {
                // Helper: check if a value is a (return ...) expression
                fn is_return_expr(v: &MettaValue) -> bool {
                    if let Some(items) = v.as_sexpr() {
                        if items.len() == 2 {
                            if let Some(s) = items[0].as_atom() {
                                return s == "return";
                            }
                        }
                    }
                    false
                }

                // Partition into return values and continue expressions
                let (final_results, continue_exprs): (Vec<_>, Vec<_>) = eval_results
                    .into_iter()
                    .partition(|(r, _)| is_return_expr(r));

                if !final_results.is_empty() {
                    // Extract return values - unwrap (return value) to just value.
                    //
                    // T04/035 (2026-05-17): PRESERVE bindings from the function
                    // body's evaluation. HE-bisim: when the body's evaluation
                    // produces bindings (e.g. via `(unify B $a ...)` binding
                    // $a → B), those bindings must propagate to the function's
                    // caller (mirrors HE Bindings propagation through
                    // InterpretedAtom). Previously bindings were dropped,
                    // breaking patterns like
                    //   `(chain (function ... (unify B $a ...) ...) $_ $a)`
                    // where the outer chain's templ `$a` should resolve to B.
                    let returns: Vec<BoundValue> = final_results
                        .into_iter()
                        .map(|(r, b)| {
                            let v = if let Some(items) = r.as_sexpr() {
                                items[1].clone()
                            } else {
                                r // shouldn't happen, but be safe
                            };
                            (v, b)
                        })
                        .collect();
                    work_stack.push(WorkItem::Resume {
                        result: (returns.into_iter().collect(), current_env),
                    });
                } else if continue_exprs.is_empty() {
                    // Nothing to continue — body terminated empty.
                    // HE: yield fresh variable (no explicit return).
                    let fresh = fresh_function_result(ctx);
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(fresh)], current_env),
                    });
                } else if iteration_count >= MAX_ITERATIONS {
                    // Hit iteration limit — body never returned. Yield
                    // one fresh variable per continuing branch (HE-bisim).
                    let fresh_branches: SmallVec<[BoundValue; 2]> = continue_exprs
                        .into_iter()
                        .map(|(_v, b)| (fresh_function_result(ctx), b))
                        .collect();
                    work_stack.push(WorkItem::Resume {
                        result: (fresh_branches, current_env),
                    });
                } else if continue_exprs.len() == 1 {
                    // Single continue branch — check if body reached
                    // structural normal form (no rules apply, no rewrite
                    // possible). If so, no `(return X)` will ever fire;
                    // yield a fresh variable now rather than iterating
                    // up to MAX_ITERATIONS uselessly. T04/023, T04/038.
                    let (next_expr, next_b) = continue_exprs.into_iter().next().unwrap();
                    if crate::backend::eval::trampoline::dispatch_hints::is_normal_form_bounded(
                        &next_expr,
                        &current_env,
                        4,
                    ) {
                        let fresh = fresh_function_result(ctx);
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![(fresh, next_b)], current_env),
                        });
                    } else {
                        continuations.push(Continuation::ProcessFunction {
                            iteration_count: iteration_count + 1,
                            env: current_env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });
                        work_stack.push(WorkItem::Eval {
                            value: next_expr,
                            env: current_env,
                            depth, // TCO: reuse depth for iteration
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    // Multiple continue branches — none returned. Yield
                    // one fresh variable per branch (HE-bisim).
                    let fresh_branches: SmallVec<[BoundValue; 2]> = continue_exprs
                        .into_iter()
                        .map(|(_v, b)| (fresh_function_result(ctx), b))
                        .collect();
                    work_stack.push(WorkItem::Resume {
                        result: (fresh_branches, current_env),
                    });
                }
            }
        }

        Continuation::ProcessIsError {
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (expr_results, result_env) = result;

            let is_error = expr_results.iter().any(|(v, _)| v.is_error());
            let result_value = ctx.factory().bool(is_error);

            work_stack.push(WorkItem::Resume {
                result: (smallvec![bv(result_value)], result_env),
            });
        }

        Continuation::ProcessCatch {
            default,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (expr_results, result_env) = result;

            // Check if any result is an error
            let has_error = expr_results.iter().any(|(v, _)| v.is_error());

            if has_error {
                // Evaluate default value
                work_stack.push(WorkItem::Eval {
                    value: default,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // No error - return original results
                work_stack.push(WorkItem::Resume {
                    result: (expr_results, result_env),
                });
            }
        }

        Continuation::ProcessConjunction {
            mut remaining_goals,
            mut accumulated_results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (goal_results, result_env) = result;

            // Check for error or empty result
            if goal_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
                return;
            }

            if goal_results.iter().any(|(v, _)| v.is_error()) {
                let error = goal_results
                    .into_iter()
                    .find(|(v, _)| v.is_error())
                    .unwrap();
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![error], result_env),
                });
                return;
            }

            accumulated_results.extend(goal_results);

            if let Some(next_goal) = remaining_goals.next() {
                continuations.push(Continuation::ProcessConjunction {
                    remaining_goals,
                    accumulated_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: next_goal,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // All goals evaluated - return last result
                let final_result = accumulated_results
                    .pop()
                    .unwrap_or_else(|| bv(ctx.factory().unit()));
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![final_result], result_env),
                });
            }
        }

        Continuation::ProcessUnifyPattern1 {
            pattern2,
            success_body,
            failure_body,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (pattern1_results, result_env) = result;

            if pattern1_results.is_empty() {
                // Empty - evaluate failure body
                work_stack.push(WorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else if pattern1_results.len() == 1 {
                // Single result - check if it's a Space (special handling).
                // Task #68 gap-fix: preserve val1_bindings (previously underscored
                // and discarded). These bindings must compose into success_body
                // when the pattern2 unification produces more bindings.
                let (val1, val1_bindings) = pattern1_results.into_iter().next().unwrap();

                if let Some(handle) = val1.as_space() {
                    // Space unification - match pattern2 against space atoms.
                    //
                    // Fix 2 (mmverify hang resolution): pre-substitute pattern2
                    // with val1_bindings so caller-side variables (e.g. $level
                    // from an outer let* parameter) are concretized BEFORE
                    // unification against kb atoms. Without this, the kb-match
                    // path runs `bidirectional_unify(&raw_pattern, &kb_atom)`
                    // and may produce caller-side var → kb-side value bindings
                    // that don't reflect the caller's intended scoping. Mirrors
                    // the non-Space path (line ~9176) which already substitutes
                    // outer carrying via `EvalWithBindings`.
                    let pattern2 = if pattern2.has_variables_fast() && !val1_bindings.is_empty() {
                        apply_bindings(&pattern2, &val1_bindings, ctx.factory())
                    } else {
                        pattern2
                    };

                    // Check if this is a simple boolean check using generic trait methods
                    // (NO heap conversion needed for this check)
                    let is_boolean_check = is_boolean_check_pattern(&success_body, &failure_body);

                    if is_boolean_check {
                        // Simple existence check - use generic method (no call-site conversion)
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's generic match_space_exists
                            result_env.match_space_exists(&pattern2)
                        } else {
                            // Non-module spaces use SpaceHandle's generic collapse
                            let atoms: Vec<MettaValue> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                crate::backend::eval::trampoline::unification::bidirectional_unify(
                                    &pattern2, atom,
                                )
                                .is_some()
                            })
                        };
                        let result_value = ctx.factory().bool(exists);
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(result_value)], result_env),
                        });
                    } else {
                        // Full space matching with body evaluation
                        if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's match_space
                            let matches: Vec<(MettaValue, usize)> = result_env
                                .match_space(&pattern2, &pattern2)
                                .into_iter()
                                .map(|m| (m.value, m.count))
                                .collect();

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(WorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: outer_carrying.clone(),
                                });
                            } else {
                                // Build bodies to evaluate for each match - values already generic.
                                // Task #68 gap-fix: compose val1_bindings with each match's
                                // unification bindings so variables bound by the pattern1
                                // source expression (e.g. $who from rule match) flow into
                                // the success_body, not just pattern2's unification vars.
                                // S0d.1: use UnifyMode::Unify so var-var-distinct
                                // creates equivalence classes (HE M-VAR-VAR-DISTINCT).
                                let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                                let mut found_match = false;
                                for (generic_value, count) in &matches {
                                    if let Some(uni_bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(&pattern2, generic_value, crate::backend::models::UnifyMode::Unify) {
                                        found_match = true;
                                        // Fast path: no classes formed → legacy compose+apply.
                                        // Class path: apply outer first, then class bindings.
                                        let generic_body = if uni_bindings.is_empty_classes() {
                                            let bindings = uni_bindings.into_entries();
                                            let composed = if val1_bindings.is_empty() {
                                                bindings
                                            } else {
                                                crate::backend::eval::bindings::compose_outer_inner_generic(
                                                    &val1_bindings, &bindings, ctx.factory(),
                                                )
                                            };
                                            apply_bindings(&success_body, &composed, ctx.factory())
                                        } else {
                                            // Apply outer first; class machinery preserves
                                            // value-less class members as ORIGINAL atoms.
                                            let after_outer = if val1_bindings.is_empty() {
                                                success_body
                                            } else {
                                                apply_bindings(&success_body, &val1_bindings, ctx.factory())
                                            };
                                            crate::backend::eval::trampoline::engine::apply_bindings_with_classes(
                                                &after_outer, &uni_bindings, ctx.factory(),
                                            )
                                        };
                                        for _ in 0..*count {
                                            bodies_to_eval.push(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push(failure_body.clone());
                                }

                                let mut bodies_iter = bodies_to_eval.into_iter();
                                if let Some(first_body) = bodies_iter.next() {
                                    if bodies_iter.len() == 0 {
                                        // Single body - tail call directly
                                        work_stack.push(WorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                            demand: None,
                                            carrying_bindings: outer_carrying.clone(),
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        let unify_capacity = bodies_iter.len() + 1;
                                        continuations.push(Continuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_iter,
                                            results: Vec::with_capacity(unify_capacity),
                                            env: result_env.clone(),
                                            depth,
                                            outer_carrying: outer_carrying.clone(),
                                        });
                                        work_stack.push(WorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            is_tail_call: false,
                                            expected_type: None,
                                            demand: None,
                                            carrying_bindings: outer_carrying.clone(),
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(WorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        demand: None,
                                        carrying_bindings: outer_carrying.clone(),
                                    });
                                }
                            }
                        } else {
                            // GENERIC: Non-module spaces - use generic collapse_with_multiplicity

                            let matches: Vec<GenericMultiplicityMatch<MettaValue>> =
                                handle.collapse_with_multiplicity_generic(ctx.factory());

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(WorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: outer_carrying.clone(),
                                });
                            } else {
                                // S0d.1: use UnifyMode::Unify (HE M-VAR-VAR-DISTINCT).
                                let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                                let mut found_match = false;
                                for m in &matches {
                                    if let Some(uni_bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(&pattern2, &m.value, crate::backend::models::UnifyMode::Unify) {
                                        found_match = true;
                                        let generic_body = if uni_bindings.is_empty_classes() {
                                            let bindings = uni_bindings.into_entries();
                                            let composed = if val1_bindings.is_empty() {
                                                bindings
                                            } else {
                                                crate::backend::eval::bindings::compose_outer_inner_generic(
                                                    &val1_bindings, &bindings, ctx.factory(),
                                                )
                                            };
                                            apply_bindings(&success_body, &composed, ctx.factory())
                                        } else {
                                            let after_outer = if val1_bindings.is_empty() {
                                                success_body
                                            } else {
                                                apply_bindings(&success_body, &val1_bindings, ctx.factory())
                                            };
                                            crate::backend::eval::trampoline::engine::apply_bindings_with_classes(
                                                &after_outer, &uni_bindings, ctx.factory(),
                                            )
                                        };
                                        for _ in 0..m.count {
                                            bodies_to_eval.push(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push(failure_body.clone());
                                }

                                let mut bodies_iter = bodies_to_eval.into_iter();
                                if let Some(first_body) = bodies_iter.next() {
                                    if bodies_iter.len() == 0 {
                                        // Single body - tail call directly
                                        work_stack.push(WorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                            demand: None,
                                            carrying_bindings: outer_carrying.clone(),
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        let unify_capacity = bodies_iter.len() + 1;
                                        continuations.push(Continuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_iter,
                                            results: Vec::with_capacity(unify_capacity),
                                            env: result_env.clone(),
                                            depth,
                                            outer_carrying: outer_carrying.clone(),
                                        });
                                        work_stack.push(WorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            is_tail_call: false,
                                            expected_type: None,
                                            demand: None,
                                            carrying_bindings: outer_carrying.clone(),
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(WorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        is_tail_call: true,
                                        expected_type: None,
                                        demand: None,
                                        carrying_bindings: outer_carrying.clone(),
                                    });
                                }
                            }
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2.
                    // Task #68 gap-fix: if val1 produced bindings, those variables
                    // may appear in pattern2 and must be substituted before eval.
                    // Use EvalWithBindings when pattern2 has variables.
                    continuations.push(Continuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    if pattern2.has_variables_fast() && !val1_bindings.is_empty() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: pattern2,
                            bindings: std::sync::Arc::new(val1_bindings),
                            env: result_env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: pattern2,
                            env: result_env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                }
            } else {
                // Multiple results - iterate over them.
                // Phase 2 Part A fix (task #64): preserve per-pattern1-result
                // bindings. Task #68 gap-fix: compose first_b with unification
                // bindings for space-path bodies (symmetric to single-result
                // path above).
                let remaining_vec: Vec<BoundValue> = pattern1_results.into_iter().collect();
                let mut remaining = remaining_vec.into_iter();
                let iter_capacity = remaining.len();
                let (first, first_b) = remaining.next().unwrap();

                continuations.push(Continuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results: remaining,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results: Vec::with_capacity(iter_capacity),
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Check if first is a Space
                if let Some(handle) = first.as_space() {
                    // Space unification for first value
                    if handle.is_module_space() || handle.name == "self" {
                        // Module/self spaces use Environment's match_space
                        let matches: Vec<(MettaValue, usize)> = result_env
                            .match_space(&pattern2, &pattern2)
                            .into_iter()
                            .map(|m| (m.value, m.count))
                            .collect();

                        // S0d.1: UnifyMode::Unify.
                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for (generic_value, count) in &matches {
                            if let Some(uni_bindings) =
                                crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(
                                    &pattern2,
                                    generic_value,
                                    crate::backend::models::UnifyMode::Unify,
                                )
                            {
                                found_match = true;
                                let generic_body = if uni_bindings.is_empty_classes() {
                                    let bindings = uni_bindings.into_entries();
                                    let composed = if first_b.is_empty() {
                                        bindings
                                    } else {
                                        crate::backend::eval::bindings::compose_outer_inner_generic(
                                            &first_b,
                                            &bindings,
                                            ctx.factory(),
                                        )
                                    };
                                    apply_bindings(&success_body, &composed, ctx.factory())
                                } else {
                                    let after_outer = if first_b.is_empty() {
                                        success_body
                                    } else {
                                        apply_bindings(&success_body, &first_b, ctx.factory())
                                    };
                                    crate::backend::eval::trampoline::engine::apply_bindings_with_classes(
                                        &after_outer, &uni_bindings, ctx.factory(),
                                    )
                                };
                                for _ in 0..*count {
                                    bodies_to_eval.push(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push(failure_body.clone());
                        }

                        let mut bodies_iter = bodies_to_eval.into_iter();
                        if let Some(first_body) = bodies_iter.next() {
                            let unify_capacity = bodies_iter.len() + 1;
                            continuations.push(Continuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_iter,
                                results: Vec::with_capacity(unify_capacity),
                                env: result_env.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });
                            work_stack.push(WorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                        }
                    } else {
                        // GENERIC: Non-module spaces - use generic collapse_with_multiplicity

                        let matches: Vec<GenericMultiplicityMatch<MettaValue>> =
                            handle.collapse_with_multiplicity_generic(ctx.factory());

                        // S0d.1: UnifyMode::Unify.
                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(uni_bindings) =
                                crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(
                                    &pattern2, &m.value, crate::backend::models::UnifyMode::Unify,
                                )
                            {
                                found_match = true;
                                let generic_body = if uni_bindings.is_empty_classes() {
                                    let bindings = uni_bindings.into_entries();
                                    let composed = if first_b.is_empty() {
                                        bindings
                                    } else {
                                        crate::backend::eval::bindings::compose_outer_inner_generic(
                                            &first_b,
                                            &bindings,
                                            ctx.factory(),
                                        )
                                    };
                                    apply_bindings(&success_body, &composed, ctx.factory())
                                } else {
                                    let after_outer = if first_b.is_empty() {
                                        success_body
                                    } else {
                                        apply_bindings(&success_body, &first_b, ctx.factory())
                                    };
                                    crate::backend::eval::trampoline::engine::apply_bindings_with_classes(
                                        &after_outer, &uni_bindings, ctx.factory(),
                                    )
                                };
                                for _ in 0..m.count {
                                    bodies_to_eval.push(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push(failure_body.clone());
                        }

                        let mut bodies_iter = bodies_to_eval.into_iter();
                        if let Some(first_body) = bodies_iter.next() {
                            let unify_capacity = bodies_iter.len() + 1;
                            continuations.push(Continuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_iter,
                                results: Vec::with_capacity(unify_capacity),
                                env: result_env.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });
                            work_stack.push(WorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    continuations.push(Continuation::ProcessUnifyPattern2 {
                        val1: first,
                        pattern2: pattern2.clone(),
                        success_body: ctx.factory().atom("__unify_success__"),
                        failure_body: ctx.factory().atom("__unify_failure__"),
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: pattern2,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                }
            }
        }

        Continuation::ProcessUnifyPattern1Iter {
            mut remaining_pattern1_results,
            pattern2,
            success_body,
            failure_body,
            mut all_results,
            env: _iter_env,
            depth,
            outer_carrying,
        } => {
            let (body_results, env_after) = result;

            // Accumulate results from the pattern1 value we just processed
            all_results.extend(body_results);

            // Get next pattern1 value to process.
            // Task #68 gap-fix: preserve val1's bindings and compose with
            // each match's unification bindings so the success_body sees
            // variables bound by the pattern1 source expression.
            if let Some((val1, val1_bindings)) = remaining_pattern1_results.next() {
                // Create new iterator continuation for the REMAINING values
                // (after this one we're about to process)
                continuations.push(Continuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results,
                    env: env_after.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Process this pattern1 value
                if let Some(handle) = val1.as_space() {
                    let pattern = pattern2.clone();
                    // Boolean check optimization: if success is True and failure is False
                    let is_boolean_check = {
                        let success_true = success_body.as_bool() == Some(true)
                            || success_body.as_atom() == Some("True");
                        let failure_false = failure_body.as_bool() == Some(false)
                            || failure_body.as_atom() == Some("False");
                        success_true && failure_false
                    };

                    if is_boolean_check {
                        // Optimized exists check - no need to collect all matches
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            env_after.match_space_exists(&pattern)
                        } else {
                            let atoms: Vec<MettaValue> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                crate::backend::eval::trampoline::unification::bidirectional_unify(
                                    &pattern, atom,
                                )
                                .is_some()
                            })
                        };
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(ctx.factory().bool(exists))], env_after),
                        });
                    } else {
                        // Full match: get matches from appropriate source

                        let matches: Vec<GenericMultiplicityMatch<MettaValue>> =
                            if handle.is_module_space() || handle.name == "self" {
                                // Module/self spaces - use match_space
                                env_after
                                    .match_space(&pattern, &pattern)
                                    .into_iter()
                                    .map(|m| GenericMultiplicityMatch {
                                        value: m.value,
                                        count: m.count,
                                    })
                                    .collect()
                            } else {
                                // Non-module spaces - use collapse_with_multiplicity_generic
                                handle.collapse_with_multiplicity_generic(ctx.factory())
                            };

                        // S0d.1: UnifyMode::Unify.
                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(uni_bindings) =
                                crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(
                                    &pattern, &m.value, crate::backend::models::UnifyMode::Unify,
                                )
                            {
                                found_match = true;
                                let instantiated = if uni_bindings.is_empty_classes() {
                                    let bindings = uni_bindings.into_entries();
                                    let composed = if val1_bindings.is_empty() {
                                        bindings
                                    } else {
                                        crate::backend::eval::bindings::compose_outer_inner_generic(
                                            &val1_bindings,
                                            &bindings,
                                            ctx.factory(),
                                        )
                                    };
                                    apply_bindings(&success_body, &composed, ctx.factory())
                                } else {
                                    let after_outer = if val1_bindings.is_empty() {
                                        success_body
                                    } else {
                                        apply_bindings(&success_body, &val1_bindings, ctx.factory())
                                    };
                                    crate::backend::eval::trampoline::engine::apply_bindings_with_classes(
                                        &after_outer, &uni_bindings, ctx.factory(),
                                    )
                                };
                                for _ in 0..m.count {
                                    bodies_to_eval.push(instantiated.clone());
                                }
                            }
                        }
                        if !found_match {
                            bodies_to_eval.push(failure_body.clone());
                        }

                        let mut bodies_iter = bodies_to_eval.into_iter();
                        if let Some(first_body) = bodies_iter.next() {
                            let unify_capacity = bodies_iter.len() + 1;
                            continuations.push(Continuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_iter,
                                results: Vec::with_capacity(unify_capacity),
                                env: env_after.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });
                            work_stack.push(WorkItem::Eval {
                                value: first_body,
                                env: env_after,
                                depth: depth + 1,
                                is_tail_call: false,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env_after),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    continuations.push(Continuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: env_after.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });
                    // Task #68 gap-fix: substitute val1_bindings into pattern2 via
                    // EvalWithBindings when pattern2 has variables. Plain Eval
                    // would lose val1's bindings.
                    if pattern2.has_variables_fast() && !val1_bindings.is_empty() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: pattern2,
                            bindings: std::sync::Arc::new(val1_bindings),
                            env: env_after,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: pattern2,
                            env: env_after,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                }
            } else {
                // No more pattern1 values - return all accumulated results
                if all_results.is_empty() {
                    work_stack.push(WorkItem::Eval {
                        value: failure_body,
                        env: env_after,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::from_vec(all_results), env_after),
                    });
                }
            }
        }

        Continuation::ProcessUnifyPattern2 {
            val1,
            pattern2: _,
            success_body,
            failure_body,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (pattern2_results, result_env) = result;

            if pattern2_results.is_empty() {
                work_stack.push(WorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // S0d.1: user-facing `(unify ...)` form uses UnifyMode::Unify
                // so var-var-distinct creates an equivalence class. Class-aware
                // apply_bindings preserves the original lookup-key for value-less
                // class members (HE M-VAR-VAR-DISTINCT, spec §4.3.1).
                use crate::backend::eval::trampoline::engine::apply_bindings_with_classes;
                use crate::backend::models::UnifyMode;
                let mut all_bindings: Vec<
                    crate::backend::models::BindingsWithClasses<MettaValue>,
                > = Vec::new();
                for (p2_result, _b) in &pattern2_results {
                    if let Some(bindings) =
                        crate::backend::eval::trampoline::unification::bidirectional_unify_with_mode(
                            &val1, p2_result, UnifyMode::Unify,
                        )
                    {
                        all_bindings.push(bindings);
                    }
                }

                if all_bindings.is_empty() {
                    work_stack.push(WorkItem::Eval {
                        value: failure_body,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else if all_bindings.len() == 1 {
                    let instantiated = apply_bindings_with_classes(
                        &success_body,
                        &all_bindings[0],
                        ctx.factory(),
                    );

                    // T04/035 (2026-05-17): HE Bindings propagation parity.
                    // unify produces new bindings (e.g. `(unify B $a ...)` binds
                    // $a → B). These must flow UPWARD to the caller via the
                    // BoundValue's bindings, so outer forms like `(chain ... $_ $a)`
                    // can resolve `$a` to `B` in the templ post-unify.
                    //
                    // HE source: interpreter.rs:809-841 — unify merges match
                    // bindings with the InterpretedAtom's bindings, which
                    // propagate to the caller via stack return.
                    //
                    // MeTTaTron: compose unify bindings into carrying_bindings
                    // so the body Eval sees them as ambient; the body's
                    // result will tag the unify bindings onto its BoundValue
                    // via the GenericEvalStep::Done propagation point.
                    let unify_b = if all_bindings[0].is_empty_classes() {
                        all_bindings.into_iter().next().unwrap().into_entries()
                    } else {
                        crate::backend::models::GenericBindings::default()
                    };
                    let composed_carrying = if unify_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(unify_b)
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &unify_b,
                                ctx.factory(),
                            ),
                        )
                    };

                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: composed_carrying,
                    });
                } else {
                    // Multi-result: each match produces a body to evaluate.
                    // T04/035 (2026-05-17): the unify bindings for the FIRST
                    // alt are passed via carrying so they flow upward. Other
                    // alts' bindings are lost — multi-result unify is rare
                    // and a full fix requires ProcessUnifyBodies to carry
                    // per-body bindings (out of scope for this targeted fix).
                    let bodies_vec: Vec<MettaValue> = all_bindings
                        .iter()
                        .map(|bindings| {
                            apply_bindings_with_classes(&success_body, bindings, ctx.factory())
                        })
                        .collect();
                    let first_unify_b = if all_bindings[0].is_empty_classes() {
                        all_bindings[0].clone().into_entries()
                    } else {
                        crate::backend::models::GenericBindings::default()
                    };
                    let first_carrying = if first_unify_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(first_unify_b)
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &first_unify_b,
                                ctx.factory(),
                            ),
                        )
                    };
                    let mut bodies_iter = bodies_vec.into_iter();
                    let first_body = bodies_iter.next().unwrap();
                    let unify_capacity = bodies_iter.len() + 1;

                    continuations.push(Continuation::ProcessUnifyBodies {
                        remaining_bodies: bodies_iter,
                        results: Vec::with_capacity(unify_capacity),
                        env: result_env.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: first_body,
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: first_carrying,
                    });
                }
            }
        }

        Continuation::ProcessUnifyBodies {
            mut remaining_bodies,
            mut results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (body_results, env_after_body) = result;
            results.extend(body_results);

            if let Some(next_body) = remaining_bodies.next() {
                continuations.push(Continuation::ProcessUnifyBodies {
                    remaining_bodies,
                    results,
                    env: env_after_body.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });
                work_stack.push(WorkItem::Eval {
                    value: next_body,
                    env: env_after_body,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), env_after_body),
                });
            }
        }

        Continuation::ProcessCollapse {
            env: _,
            depth,
            outer_carrying,
        } => {
            let (expr_results, result_env) = result;

            // Empty results: return empty tuple immediately
            if expr_results.is_empty() {
                let result_list = ctx.factory().sexpr(vec![]);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
                return;
            }

            // ── Parallel path: evaluate all collapse results concurrently ──
            // When there are enough results, dispatch to work pool for parallel
            // evaluation. MeTTa HE collapse results are unordered, so parallel
            // evaluation that changes result order is semantically correct.
            let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
            let n_results = expr_results.len();
            let par_budget = if n_results >= parallel_collapse_threshold()
                && current_depth < max_parallel_depth()
                && global_eval_pool().active_workers() > 0
            {
                try_acquire_budget((n_results - 1) as u32, current_depth)
            } else {
                0
            };

            if par_budget > 0 {
                // **Stack-safety mandate (2026-05-15)**: trampolinized
                // collapse-dispatch (Phase 4 follow-up). Non-blocking
                // `parallel_collapse_dispatch` returns a handle; we push a
                // `WaitForParallelCollapse` continuation and yield to the
                // trampoline outer loop. The merge happens in the
                // `WaitForParallelCollapse` arm of `process_continuation`.
                let metta_items: Vec<crate::backend::eval::trampoline::types::BoundValue> =
                    expr_results.into_iter().collect();
                // Phase 8: share Arc with RootProvider.
                let stable_items_snapshot = std::sync::Arc::new(metta_items);
                let metta_env = (*result_env).clone();
                let handle = parallel_collapse_dispatch(
                    std::sync::Arc::clone(&stable_items_snapshot),
                    metta_env,
                    par_budget,
                    current_depth,
                    depth,
                );
                let env_for_resume = result_env.clone();
                continuations.push(Continuation::WaitForParallelCollapse {
                    handle,
                    merge_mode: crate::backend::eval::trampoline::types::CollapseMergeMode::Plain,
                    stable_items_snapshot,
                    outer_carrying: outer_carrying.clone(),
                    tracked_vars_hint: None,  // Plain mode discards bindings
                    env: result_env,
                    depth,
                    budget_acquired: par_budget,
                    caller_depth: current_depth,
                });
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), env_for_resume),
                });
            } else {
                // ── Sequential path: evaluate one-at-a-time ──
                // MeTTa HE collapse semantics: evaluate each result to normal form.
                let remaining_vec: Vec<BoundValue> = expr_results.into_iter().collect();
                let mut remaining_raw = remaining_vec.into_iter();
                let collapse_capacity = remaining_raw.len(); // total before consuming first
                let (first_raw, first_raw_b) =
                    remaining_raw.next().expect("expr_results is non-empty");

                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated: Vec::with_capacity(collapse_capacity),
                    is_bind: false,
                    current_raw_bindings: std::sync::Arc::new(first_raw_b),
                    env: result_env.clone(),
                    depth,
                    // Plain `collapse` discards bindings — no projection needed.
                    tracked_vars_hint: None,
                    outer_carrying: outer_carrying.clone(),
                });

                // Option C (2026-05-06) — HE-faithful re-eval skip:
                // HE's `collapse_bind_ret` (interpreter.rs:767-792) appends
                // already-driven raws verbatim — it does NOT re-evaluate.
                // The kernel's `Demand::All` path drives every value to a
                // Done step before pushing onto `expr_results`, so re-eval
                // is purely defensive and merely re-confirms fixpoint.
                // For values verifiably in normal form (freeze-tuple
                // outputs, ground sexprs whose head is non-reducible),
                // skip the re-eval and Resume directly with the raw —
                // matching HE.
                if crate::backend::eval::trampoline::is_memoized_normal_form(&first_raw) {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(first_raw)], result_env),
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: first_raw,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                }
            }
        }

        Continuation::ProcessCollapseBind {
            env: _,
            depth,
            outer_carrying,
        } => {
            let (expr_results, result_env) = result;

            // Pop the binding capture frame (may be None if no free vars).
            let captured_frame = pop_binding_capture_frame();

            // Empty results: return empty tuple immediately
            if expr_results.is_empty() {
                let result_list = ctx.factory().sexpr(vec![]);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
                return;
            }

            // Layer A: extract tracked_vars from the just-popped capture frame
            // so the sidecar encoding site can project each result's bindings
            // to just the user-visible set (matching HE's Bindings::resolve()
            // at the observation point). None ⇒ no projection needed.
            let tracked_vars_for_sidecar: Option<std::sync::Arc<SmallVec<[&'static str; 4]>>> =
                captured_frame
                    .as_ref()
                    .map(|f| std::sync::Arc::new(f.tracked_vars.clone()));

            // Stage 1b+: per-result bindings now travel with each BoundValue
            // (expr_results[i].1), so there is nothing to extract from the
            // capture frame — it's now a pure scope marker.
            //
            // Phase 10.A — Stage 1e closure (2026-05-17): the historical
            // `force_sequential = captured_frame.is_some()` gate is no
            // longer needed. Three independent mechanisms now keep
            // HE-bisim correctness intact under parallel dispatch with a
            // popped capture frame:
            //
            //   1. The just-popped frame's tracked_vars are extracted into
            //      `tracked_vars_for_sidecar` (above) and threaded into
            //      `WaitForParallelCollapse.tracked_vars_hint`, where
            //      per-branch binding projection runs at MERGE time on
            //      the parent thread — independent of worker thread state.
            //   2. The popped frame's *outer* scope (if any) is captured
            //      by `parallel_collapse_dispatch`'s
            //      `parent_tracked_vars` snapshot BEFORE worker spawn,
            //      and re-pushed on each worker via
            //      `WorkerCaptureScope::enter`. Any nested collapse-bind
            //      a worker encounters inside an item's eval pushes its
            //      OWN frame on top, preserving HE's lexical scoping.
            //   3. Per-result bindings already travel with each
            //      `BoundValue`, so workers don't depend on the
            //      thread-local capture stack for the result they're
            //      evaluating.
            let _ = captured_frame;

            // ── Parallel path: identical to ProcessCollapse ──
            let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
            let par_budget = if expr_results.len() >= parallel_collapse_threshold()
                && current_depth < max_parallel_depth()
                && global_eval_pool().active_workers() > 0
            {
                try_acquire_budget((expr_results.len() - 1) as u32, current_depth)
            } else {
                0
            };

            if par_budget > 0 {
                // **Stack-safety mandate (2026-05-15)**: trampolinized
                // collapse-bind dispatch (Phase 4 follow-up). The Bind-mode
                // sidecar encoding moves into the `WaitForParallelCollapse`
                // arm of `process_continuation`.
                //
                // **Note**: `force_sequential` (`captured_frame.is_some()`)
                // is checked above; when true, `par_budget == 0` and this
                // branch doesn't fire, so `tracked_vars_for_sidecar` is
                // typically `None` here. We still thread it through for
                // forward-compat once Stage-1e lifts the gate.
                let metta_items: Vec<crate::backend::eval::trampoline::types::BoundValue> =
                    expr_results.into_iter().collect();
                // Phase 8: share Arc with RootProvider.
                let stable_items_snapshot = std::sync::Arc::new(metta_items);
                let metta_env = (*result_env).clone();
                let handle = parallel_collapse_dispatch(
                    std::sync::Arc::clone(&stable_items_snapshot),
                    metta_env,
                    par_budget,
                    current_depth,
                    depth,
                );
                let env_for_resume = result_env.clone();
                continuations.push(Continuation::WaitForParallelCollapse {
                    handle,
                    merge_mode: crate::backend::eval::trampoline::types::CollapseMergeMode::Bind,
                    stable_items_snapshot,
                    outer_carrying: outer_carrying.clone(),
                    tracked_vars_hint: tracked_vars_for_sidecar.clone(),
                    env: result_env,
                    depth,
                    budget_acquired: par_budget,
                    caller_depth: current_depth,
                });
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), env_for_resume),
                });
            } else {
                // ── Sequential path ──
                let remaining_vec: Vec<BoundValue> = expr_results.into_iter().collect();
                let mut remaining_raw = remaining_vec.into_iter();
                let collapse_capacity = remaining_raw.len(); // total before consuming first
                let (first_raw, first_raw_bindings) =
                    remaining_raw.next().expect("expr_results is non-empty");

                // Stage 1e fix: pass the raw's per-branch bindings as
                // `carrying_bindings` for the re-eval. The raw value was
                // produced by an inner evaluation that captured bindings
                // like `$__fr_1_a=$who` (alias) and `$who=a` (concrete);
                // those bindings must travel alongside the value during
                // re-evaluation so downstream handlers can resolve
                // caller-level variables. Without this, re-eval sees an
                // empty binding context and user-named bindings like
                // `$who=a` never reach the sidecar encoding at line
                // ~8407 below. Matches HE's semantics where bindings flow
                // hierarchically through collapse-bind re-interpretation.
                let tracked_slice = tracked_vars_for_sidecar.as_deref().map(|tv| tv.as_slice());
                let raw_bindings_for_eval = match project_owned_bindings_for_consumer(
                    &first_raw_bindings,
                    &first_raw,
                    tracked_slice,
                    ctx.factory(),
                ) {
                    Some(b) => b,
                    None => {
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), result_env),
                        });
                        return;
                    }
                };
                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated: Vec::with_capacity(collapse_capacity),
                    is_bind: true,
                    current_raw_bindings: std::sync::Arc::new(first_raw_bindings),
                    env: result_env.clone(),
                    depth,
                    // Layer A: carry the collapse-bind's tracked vars so the
                    // sidecar encoding projects bindings at the observation
                    // point, not earlier during match composition.
                    tracked_vars_hint: tracked_vars_for_sidecar,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: first_raw,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: std::sync::Arc::new(raw_bindings_for_eval),
                });
            }
        }

        Continuation::ProcessCollapseEvalResults {
            mut remaining_raw,
            mut evaluated,
            is_bind,
            mut current_raw_bindings,
            env: _,
            depth,
            tracked_vars_hint,
            outer_carrying,
        } => {
            let (eval_results, result_env) = result;

            // Stage 1e MERGE: merge the raw's original bindings with each
            // re-eval result's bindings. The re-eval typically produces the
            // same value for ground raws (no new bindings), but in case the
            // raw was a further-evaluable expression, both sources are
            // combined. Filter empty (pruned) branches.
            //
            // Phase 2.B Issue #3 fix: on merge conflict between carrying
            // (raw's match-time bindings) and child_b (re-eval's bindings),
            // DROP the branch — HE-bisimilar silent pruning. The old code
            // used `let _ = merged.merge(&child_b)` which silently masked
            // conflicts, producing ghost pairs whose bindings didn't
            // reflect any consistent evaluation.
            //
            // SPEC NOTE on Empty filtering (§06.11.2). MeTTaTron's collapse-bind
            // is a TWO-STAGE pipeline:
            //   (a) raw-collect at `ProcessCollapseBind` (~line 9265-9282) —
            //       gathers alternatives from the kernel evaluation; does NOT
            //       filter individual Empty raws (they pass through to (b)).
            //   (b) per-raw re-eval-merge (THIS handler) — for each raw,
            //       re-evaluate with carrying bindings and merge results.
            //       Empty filter at the next line is the spec-correct
            //       §06.11.2 "do nothing — discard this alternative" rule,
            //       applied at the only stage where individual Empties are
            //       observable in MeTTaTron's design. HE achieves the same
            //       end-state via its single-stage accumulation
            //       (`collapse_bind_ret` at interpreter.rs:767-778).
            // Removing this filter would let Empty leak into the collapsed
            // tuple — that would be the actual spec violation.
            let carrying = (*current_raw_bindings).clone();
            evaluated.extend(
                eval_results
                    .into_iter()
                    .filter(|(v, _)| !v.is_empty_sentinel())
                    .filter_map(|(v, child_b)| {
                        let mut merged = carrying.clone();
                        if !merged.merge(&child_b) {
                            // Conflict: this branch is inconsistent. Drop.
                            None
                        } else {
                            Some((v, merged))
                        }
                    }),
            );

            if let Some((next_raw, next_raw_bindings)) = remaining_raw.next() {
                // More results to evaluate — preserve state.
                let tracked_slice = tracked_vars_hint.as_deref().map(|tv| tv.as_slice());
                let projected_next_raw_bindings = if is_bind {
                    match project_owned_bindings_for_consumer(
                        &next_raw_bindings,
                        &next_raw,
                        tracked_slice,
                        ctx.factory(),
                    ) {
                        Some(b) => b,
                        None => {
                            continuations.push(Continuation::ProcessCollapseEvalResults {
                                remaining_raw,
                                evaluated,
                                is_bind,
                                current_raw_bindings,
                                env: result_env.clone(),
                                depth,
                                tracked_vars_hint,
                                outer_carrying: outer_carrying.clone(),
                            });
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                            return;
                        }
                    }
                } else {
                    next_raw_bindings
                };
                current_raw_bindings = std::sync::Arc::new(projected_next_raw_bindings);
                let current_raw_bindings_for_eval = current_raw_bindings.clone();
                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated,
                    is_bind,
                    current_raw_bindings,
                    env: result_env.clone(),
                    depth,
                    tracked_vars_hint,
                    outer_carrying: outer_carrying.clone(),
                });

                // Option C (2026-05-06) — same HE-faithful re-eval skip
                // as ProcessCollapse sequential branch. See rationale at
                // the StartCollapse handler.
                if crate::backend::eval::trampoline::is_memoized_normal_form(&next_raw) {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(next_raw)], result_env),
                    });
                } else {
                    let carrying_bindings = if is_bind {
                        current_raw_bindings_for_eval
                    } else {
                        outer_carrying.clone()
                    };
                    work_stack.push(WorkItem::Eval {
                        value: next_raw,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings,
                    });
                }
            } else {
                // All results evaluated — assemble the tuple.
                //
                // HE-bisim §06.11.5: if every alternative was filtered out
                // (Empty / merge-conflict / etc.), produce ZERO results
                // rather than one Unit `()`. T03/065 verifies: when chain
                // ranges over a collapse-bind whose alts all reduce to
                // Empty, the chain body must NOT fire. HE's `[]` semantics
                // for collapse-bind on an all-Empty input.
                if evaluated.is_empty() {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                    return;
                }
                let result_list = if is_bind {
                    // collapse-bind: wrap each result as (result (Bindings ($var val) ...))
                    // Use per-result bindings from each BoundValue.
                    //
                    // Filter freshened rule-body variables (keys starting with
                    // `$__fr_`) before encoding. Freshening generates synthetic
                    // names per rule at load time; they exist only within the
                    // rule's own scope and leaking them as "bindings" to the
                    // caller surprises user code (e.g. PLN's `?` macro which
                    // destructures `(Bindings ($var val) ...)` via `let`).
                    //
                    // CRITICAL: apply_chain_generic FIRST to resolve alias
                    // chains like `$__fr_77_a = $who` + `$__fr_77_a = a` into
                    // `$who = a`. Filtering `$__fr_*` before chain resolution
                    // would drop the alias and lose the user-level binding.
                    // This matches HE's `bindings.resolve(&var)` semantics in
                    // `interpreter.rs:624` where bindings are resolved before
                    // being surfaced to user code.
                    let pairs: Vec<MettaValue> = evaluated
                        .into_iter()
                        .map(|(result_val, bindings)| {
                            let tracked_slice =
                                tracked_vars_hint.as_deref().map(|tv| tv.as_slice());
                            let projected = crate::backend::eval::bindings::project_bindings_for_consumer_generic(
                                &bindings,
                                &[&result_val],
                                tracked_slice,
                                ctx.factory(),
                            )
                            .unwrap_or_default();
                            let filtered = if projected.iter().any(|(k, _)| k.starts_with("$__fr_"))
                            {
                                let mut f = crate::backend::models::GenericBindings::new();
                                for (name, val) in projected.iter() {
                                    if !name.starts_with("$__fr_") {
                                        f.insert_or_replace(name, val.clone());
                                    }
                                }
                                f
                            } else {
                                projected
                            };
                            // HE-bisim §06.11: bindings are rendered with
                            // curly braces. For empty bindings HE prints
                            // `{  }` (two atoms `{` and `}` separated by a
                            // space) inline as siblings of `result_val`,
                            // rather than a nested `(Bindings)` SExpr.
                            // T04/025, T04/026 verify. The harness parser
                            // tokenizes `{` and `}` as separate one-char
                            // words (parser.py:_parse_word breaks only on
                            // whitespace and parens), so to match the
                            // parsed shape we splice them in as siblings.
                            if filtered.is_empty() {
                                ctx.factory().sexpr(vec![
                                    result_val,
                                    ctx.factory().atom("{"),
                                    ctx.factory().atom("}"),
                                ])
                            } else {
                                // Non-empty bindings: use existing
                                // `(Bindings (k v) ...)` SExpr encoding.
                                // HE's exact non-empty format is unverified
                                // in the conformance fixtures we have; this
                                // preserves the prior MTT behavior so that
                                // any consumers reading the bindings out
                                // (e.g. PLN's `?` macro destructuring) still
                                // see the structured form.
                                let bindings_sexpr = encode_bindings_as_sexpr(&filtered, ctx.factory());
                                ctx.factory().sexpr(vec![result_val, bindings_sexpr])
                            }
                        })
                        .collect();
                    ctx.factory().sexpr(pairs)
                } else {
                    ctx.factory()
                        .sexpr(evaluated.into_iter().map(|(v, _)| v).collect())
                };

                // Trace: collapse-result phase
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&result_list),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: if is_bind { "collapse-bind" } else { "collapse" }
                                    .to_string(),
                                phase: "collapse-result".to_string(),
                            },
                        );
                    }
                }

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
            }
        }

        Continuation::ProcessAmb {
            mut remaining_alts,
            mut results,
            env,
            depth,
            outer_carrying,
        } => {
            let (alt_results, result_env) = result;
            results.extend(alt_results);

            if let Some((next_val, alt_b)) = remaining_alts.next() {
                // Use the ORIGINAL env for each alternative (not result_env).
                // Parallel path gives all branches the same pre-fork env;
                // sequential must do the same to preserve semantics.
                continuations.push(Continuation::ProcessAmb {
                    remaining_alts,
                    results,
                    env: env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                // Compose outer_carrying with this alt's per-branch bindings
                // so downstream evaluation inherits the alt's binding context.
                // Matches HE's per-plan-item `(atom, bindings)` dispatch.
                let alt_carrying: crate::backend::eval::trampoline::types::SharedBindings =
                    if alt_b.is_empty() {
                        outer_carrying.clone()
                    } else if outer_carrying.is_empty() {
                        std::sync::Arc::new(alt_b)
                    } else {
                        std::sync::Arc::new(
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying,
                                &alt_b,
                                ctx.factory(),
                            ),
                        )
                    };
                let tracked = active_tracked_vars();
                let Some(alt_carrying) = project_carrying_for_consumer(
                    &alt_carrying,
                    &next_val,
                    tracked.as_deref(),
                    ctx.factory(),
                ) else {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), env),
                    });
                    return;
                };

                work_stack.push(WorkItem::Eval {
                    value: next_val,
                    env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: alt_carrying,
                });
            } else {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            }
        }

        // **Stack-safety mandate (2026-05-15)**: trampolinized wait pump for
        // `parallel_dispatch`. Each invocation does one tick of work; the
        // trampoline outer loop iterates without growing the C stack. See
        // `pump_parallel_wait` for tick semantics.
        Continuation::WaitForParallel {
            handle,
            merge_mode,
            base_results,
            outer_carrying,
            env,
            depth: _,
            budget_acquired,
            caller_depth,
            stable_branches_snapshot,
        } => {
            // (a) done check
            let done_now = handle.remaining.load(Ordering::Acquire) == 0
                || handle.cancel_token.is_satisfied();
            if done_now {
                // Trace: ParallelDispatch done (mirrors enter trace from parallel_dispatch).
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            caller_depth,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::ParallelDispatch {
                                branch_count: handle.num_branches as u32,
                                branch_exprs: vec![],
                                parallel_depth: PARALLEL_BRANCH_DEPTH.with(|d| d.get()),
                                phase: "done".to_string(),
                            },
                        );
                    }
                }

                release_budget(budget_acquired, caller_depth);

                let mut merged = base_results;
                let guard = handle.results.lock().expect("results mutex poisoned");
                for slot in guard.iter() {
                    if let Some(slot_results) = slot.as_ref() {
                        match merge_mode {
                            crate::backend::eval::trampoline::types::ParallelMergeMode::RuleMatch => {
                                if outer_carrying.is_empty() {
                                    merged.extend(slot_results.iter().cloned());
                                } else {
                                    let oc: &crate::backend::models::GenericBindings<MettaValue> =
                                        &*outer_carrying;
                                    merged.extend(slot_results.iter().map(|(v, b)| {
                                        if b.is_empty() {
                                            bv_with(v.clone(), oc.clone())
                                        } else {
                                            let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                                                oc,
                                                b,
                                                ctx.factory(),
                                            );
                                            bv_with(v.clone(), composed)
                                        }
                                    }));
                                }
                            }
                            crate::backend::eval::trampoline::types::ParallelMergeMode::AmbConcat => {
                                merged.extend(slot_results.iter().cloned());
                            }
                        }
                    }
                }
                drop(guard);
                // handle drops here → `_root_guard` pops the frame_chain LIFO
                // entry → `root_frame` Box drops after, releasing its memory.

                work_stack.push(WorkItem::Resume {
                    result: (merged, env),
                });
                return;
            }

            // (b) one-step pump
            pump_parallel_wait(&handle, &stable_branches_snapshot);

            // (c) re-push the continuation (move handle; _root_guard is
            //     not Clone so the variant must transfer ownership).
            let env_clone_for_resume = env.clone();
            continuations.push(Continuation::WaitForParallel {
                handle,
                merge_mode,
                base_results,
                outer_carrying,
                env,
                depth: 0,
                budget_acquired,
                caller_depth,
                stable_branches_snapshot,
            });
            // Push a dummy `Resume` so the trampoline outer loop pops it,
            // calls process_continuation, and re-enters our arm next tick.
            work_stack.push(WorkItem::Resume {
                result: (SmallVec::new(), env_clone_for_resume),
            });
        }

        // **Stack-safety mandate (2026-05-15)**: trampolinized wait pump for
        // `parallel_collapse_dispatch`. Mirrors `WaitForParallel` arm but
        // operates on `ParallelCollapseDispatchHandle` and uses
        // `CollapseMergeMode::{Plain, Bind}` for the result merge.
        Continuation::WaitForParallelCollapse {
            handle,
            merge_mode,
            stable_items_snapshot,
            outer_carrying,
            tracked_vars_hint,
            env,
            depth: _,
            budget_acquired,
            caller_depth,
        } => {
            // (a) done check
            let done_now = handle.remaining.load(Ordering::Acquire) == 0
                || handle.cancel_token.is_satisfied();
            if done_now {
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let form_name = match merge_mode {
                            crate::backend::eval::trampoline::types::CollapseMergeMode::Plain => "collapse",
                            crate::backend::eval::trampoline::types::CollapseMergeMode::Bind => "collapse-bind",
                        };
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            caller_depth,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: form_name.to_string(),
                                phase: "collapse-result-parallel".to_string(),
                            },
                        );
                    }
                }

                release_budget(budget_acquired, caller_depth);

                // Drain results in slot order, filtering out empty values
                // (matches parallel_collapse_eval merge semantics).
                let mut evaluated: Vec<BoundValue> = Vec::new();
                let guard = handle.results.lock().expect("results mutex poisoned");
                for slot in guard.iter() {
                    if let Some(slot_results) = slot.as_ref() {
                        for bv_item in slot_results.iter() {
                            if !bv_item.0.is_empty() {
                                evaluated.push(bv_item.clone());
                            }
                        }
                    }
                }
                drop(guard);

                // Merge per mode.
                let result_list = match merge_mode {
                    crate::backend::eval::trampoline::types::CollapseMergeMode::Plain => {
                        // Plain collapse: emit values only, bindings discarded.
                        ctx.factory().sexpr(
                            evaluated.into_iter().map(|(v, _)| v).collect()
                        )
                    }
                    crate::backend::eval::trampoline::types::CollapseMergeMode::Bind => {
                        // collapse-bind: per-result (value (Bindings ...)) sidecar.
                        let tracked_slice =
                            tracked_vars_hint.as_deref().map(|tv| tv.as_slice());
                        let pairs: Vec<MettaValue> = evaluated
                            .into_iter()
                            .map(|(result_val, bindings)| {
                                let projected =
                                    crate::backend::eval::bindings::project_bindings_for_consumer_generic(
                                        &bindings,
                                        &[&result_val],
                                        tracked_slice,
                                        ctx.factory(),
                                    )
                                    .unwrap_or_default();
                                let filtered = if projected
                                    .iter()
                                    .any(|(k, _)| k.starts_with("$__fr_"))
                                {
                                    let mut f =
                                        crate::backend::models::GenericBindings::new();
                                    for (n, v) in projected.iter() {
                                        if !n.starts_with("$__fr_") {
                                            f.insert_or_replace(n, v.clone());
                                        }
                                    }
                                    f
                                } else {
                                    projected
                                };
                                // HE-bisim §06.11: empty bindings render as
                                // `{  }` (two atoms `{` and `}`) inlined as
                                // siblings of `result_val`. See the parallel
                                // path's branch above for full rationale.
                                if filtered.is_empty() {
                                    ctx.factory().sexpr(vec![
                                        result_val,
                                        ctx.factory().atom("{"),
                                        ctx.factory().atom("}"),
                                    ])
                                } else {
                                    let bindings_sexpr =
                                        encode_bindings_as_sexpr(&filtered, ctx.factory());
                                    ctx.factory().sexpr(vec![result_val, bindings_sexpr])
                                }
                            })
                            .collect();
                        ctx.factory().sexpr(pairs)
                    }
                };

                // handle drops here → _root_guard pops the frame_chain LIFO
                // entry → root_frame Box drops after.
                let _ = outer_carrying;
                let _ = stable_items_snapshot;
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], env),
                });
                return;
            }

            // (b) one-step pump
            pump_parallel_collapse_wait(&handle, &stable_items_snapshot);

            // (c) re-push (move handle; _root_guard is not Clone)
            let env_for_resume = env.clone();
            continuations.push(Continuation::WaitForParallelCollapse {
                handle,
                merge_mode,
                stable_items_snapshot,
                outer_carrying,
                tracked_vars_hint,
                env,
                depth: 0,
                budget_acquired,
                caller_depth,
            });
            work_stack.push(WorkItem::Resume {
                result: (SmallVec::new(), env_for_resume),
            });
        }

        Continuation::ProcessGuard {
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (cond_results, result_env) = result;

            match cond_results.first() {
                Some((v, _)) if v.as_bool() == Some(true) => {
                    // Guard passes - return Unit
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], result_env),
                    });
                }
                Some((v, _)) if v.as_bool() == Some(false) => {
                    // Guard fails - return empty (nondeterministic failure)
                    let _ = v;
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                }
                Some((v, _)) if v.is_error() => {
                    // Error propagates
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(v.clone())], result_env),
                    });
                }
                Some((v, _)) => {
                    // Type error - condition must be Bool
                    let err = ctx.factory().error(
                        v.clone(),
                        ctx.factory().string(&format!(
                            "guard: condition must evaluate to Bool, got {}",
                            v.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], result_env),
                    });
                }
                None => {
                    // Empty results - guard fails
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                }
            }
        }

        Continuation::ProcessGetAtoms {
            space_ref,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (space_results, mut result_env) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    space_ref,
                    ctx.factory().string("get-atoms: space evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], result_env),
                });
            } else {
                let (first, _) = &space_results[0];
                // Auto-bind `&name` atoms; transparent for already-resolved Space values.
                let resolved_handle = resolve_space_or_autobind(first, &mut result_env, ctx);
                if let Some(handle) = resolved_handle.as_ref() {
                    // X.6 followup: for `&self` / module spaces, query the
                    // environment's atom space (where add-atom &self routes
                    // its writes per ProcessAddAtomSpace handler). The
                    // SpaceHandle itself is a stub for these — its own
                    // storage is empty. For external named spaces
                    // (`(new-space)` results), use handle.collapse_generic.
                    let atoms: Vec<MettaValue> =
                        if handle.is_module_space() || handle.name == "self" {
                            result_env.get_all_atoms()
                        } else {
                            handle.collapse_generic(ctx.factory())
                        };
                    if atoms.is_empty() {
                        // Empty space returns empty results
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::new(), result_env),
                        });
                    } else {
                        // Return all atoms as separate results (superposition)
                        work_stack.push(WorkItem::Resume {
                            result: (atoms.into_iter().map(bv).collect(), result_env),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "get-atoms: first argument must be a space, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], result_env),
                    });
                }
            }
        }

        Continuation::ProcessMatchSpace {
            space_arg,
            pattern,
            template,
            env,
            depth,
            outer_carrying,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    space_arg,
                    ctx.factory().string("match: space evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                // Auto-bind `&name` atoms; transparent for already-resolved Space values.
                let resolved_handle = resolve_space_or_autobind(first, &mut env_after, ctx);
                if let Some(handle) = resolved_handle.as_ref() {
                    if handle.is_module_space() || handle.name == "self" {
                        // Phase 8.4: Type-aware match optimization.
                        // If pattern is (: $var TypeName), use the types HashMap as a
                        // reverse index instead of scanning the entire MORK space.
                        let type_filtered = if let Some(pat_items) = pattern.as_sexpr() {
                            if pat_items.len() == 3 {
                                if let (Some(":"), Some(var), Some(type_name)) = (
                                    pat_items[0].as_atom(),
                                    pat_items[1].as_atom(),
                                    pat_items[2].as_atom(),
                                ) {
                                    if var.starts_with('$') {
                                        // Use type index: O(k) where k = atoms of matching type
                                        let matching_atoms = env.get_atoms_of_type(type_name);
                                        let results: Vec<MettaValue> = matching_atoms
                                            .iter()
                                            .map(|name| {
                                                let mut bindings =
                                                    crate::backend::models::GenericBindings::new();
                                                bindings.insert(var, ctx.factory().atom(name));
                                                apply_bindings(&template, &bindings, ctx.factory())
                                            })
                                            .collect();
                                        Some(results)
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let generic_results: Vec<MettaValue> = if let Some(filtered) = type_filtered
                        {
                            filtered
                        } else {
                            // Standard path: match_space which handles serialization internally
                            let matches = env.match_space(&pattern, &template);
                            // Expand multiplicities into flat list
                            matches
                                .into_iter()
                                .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                                .collect()
                        };

                        // Trace: space-result phase (&self / module space)
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    generic_results
                                        .iter()
                                        .map(|v| crate::backend::trace::trace_value_generic(v))
                                        .collect(),
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "match".to_string(),
                                        phase: "space-result".to_string(),
                                    },
                                );
                            }
                        }

                        // Schedule each instantiated template through WorkItem::Eval —
                        // identical to the owned-space branch below. Without this
                        // re-eval pass, instantiated templates that contain reducible
                        // sub-expressions (grounded ops, user rules) propagate
                        // unreduced, breaking arithmetic on values from match results
                        // and any nested-template chains. Literal-value results
                        // (e.g. floats, atoms) still self-evaluate to themselves with
                        // negligible overhead.
                        if generic_results.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env_after),
                            });
                        } else if generic_results.len() == 1 {
                            work_stack.push(WorkItem::Eval {
                                value: generic_results.into_iter().next().unwrap(),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            // Multiple matches — queue template evaluations.
                            let mut generic_templates = generic_results.into_iter();
                            let tmpl_capacity = generic_templates.len();
                            let first_template = generic_templates.next().unwrap();

                            continuations.push(Continuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: Vec::with_capacity(tmpl_capacity),
                                env: env_after.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(WorkItem::Eval {
                                value: first_template,
                                env: Arc::new(forked_env),
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    } else {
                        // Owned space - match against atoms in SpaceHandle via unified match_pattern_generic
                        let instantiated_templates: Vec<MettaValue> =
                            handle.match_pattern_generic(&pattern, &template, ctx.factory());

                        // Trace: space-result phase (owned space)
                        #[cfg(feature = "trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    instantiated_templates
                                        .iter()
                                        .map(|v| crate::backend::trace::trace_value_generic(v))
                                        .collect(),
                                    None,
                                    trace_format::TraceEventKind::SpecialForm {
                                        form_name: "match".to_string(),
                                        phase: "space-result".to_string(),
                                    },
                                );
                            }
                        }

                        if instantiated_templates.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), env_after),
                            });
                        } else if instantiated_templates.len() == 1 {
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_templates.into_iter().next().unwrap(),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            // Multiple matches - queue template evaluations
                            let mut generic_templates = instantiated_templates.into_iter();
                            let tmpl_capacity = generic_templates.len(); // total before consuming first
                            let first_template = generic_templates.next().unwrap();

                            continuations.push(Continuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: Vec::with_capacity(tmpl_capacity),
                                env: env_after.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(WorkItem::Eval {
                                value: first_template,
                                env: Arc::new(forked_env),
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    }
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessMatchTemplates {
            mut remaining_templates,
            mut results,
            env,
            depth,
            outer_carrying,
        } => {
            let (template_results, _env_after) = result;
            results.extend(template_results);

            if remaining_templates.len() == 0 {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), env),
                });
            } else {
                let next_template = remaining_templates.next().unwrap();

                continuations.push(Continuation::ProcessMatchTemplates {
                    remaining_templates,
                    results,
                    env: env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: next_template,
                    env: Arc::new(env.fork_for_nondeterminism()),
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        Continuation::ProcessAddAtomSpace {
            space_ref,
            atom,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    space_ref,
                    ctx.factory().string("add-atom: space evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                // Auto-bind `&name` atoms; transparent for already-resolved Space values.
                let resolved_handle = resolve_space_or_autobind(first, &mut env_after, ctx);
                if let Some(handle) = resolved_handle.as_ref() {
                    // MeTTa HE semantics: add the UNEVALUATED atom to the space.
                    // The atom is NOT evaluated — per HE docs: "Adds atom into the
                    // atomspace without reducing it".
                    let is_self_space = handle.is_module_space() || handle.name == "self";

                    if is_self_space {
                        // &self space: add directly to environment's PathMap/RuleIndex.
                        // match &self and get-atoms query env.match_space() / env.get_all_atoms(),
                        // NOT the SpaceHandle, so atoms must live in the environment.
                        // add_to_space() handles routing: rules → add_rule() (PathMap + RuleIndex),
                        // type assertions → types HashMap, all atoms → PathMap.
                        Arc::make_mut(&mut env_after).add_to_space(&atom);
                    } else {
                        // Named space: add to SpaceHandle (match queries SpaceHandle
                        // for non-&self spaces via handle.collapse_generic()).
                        handle.add_atom_generic(&atom);
                    }
                    // Phase 3.2: Bump mutation_epoch alone. EVAL_MEMO and
                    // MATCH_RESULT_CACHE both gate lookups on mutation_epoch, so
                    // this makes all prior entries stale without a bulk clear.
                    // LRU reclaims stale entries lazily as new entries evict them.
                    increment_mutation_epoch();

                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        // Disabled: ProcessAddAtomAtom is no longer constructed. The atom evaluation
        // step has been eliminated — add-atom now takes unevaluated atoms per MeTTa HE
        // semantics. See ProcessAddAtomSpace above.
        // Continuation::ProcessAddAtomAtom {
        //     space_handle,
        //     atom,
        //     env: _,
        //     depth: _,
        //     parent_cont,
        // } => {
        //     let (atom_results, env_after) = result;
        //
        //     if atom_results.is_empty() {
        //         let err = ctx.factory().error(
        //             "add-atom: atom evaluated to empty",
        //             atom,
        //         );
        //         work_stack.push(WorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (smallvec![err], env_after),
        //         });
        //     } else {
        //         // GENERIC: Use add_atom_generic to avoid heap conversion
        //         space_handle.add_atom_generic(&atom_results[0]);
        //         work_stack.push(WorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (smallvec![ctx.factory().unit()], env_after),
        //         });
        //     }
        // }
        Continuation::ProcessRemoveAtomSpace {
            space_ref,
            atom,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    space_ref,
                    ctx.factory().string("remove-atom: space evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                // Auto-bind `&name` atoms; transparent for already-resolved Space values.
                let resolved_handle = resolve_space_or_autobind(first, &mut env_after, ctx);
                if let Some(handle) = resolved_handle.as_ref() {
                    // MeTTa HE semantics: remove the UNEVALUATED atom from the space.
                    // The atom is NOT evaluated — mirrors add-atom behavior.
                    let is_self_space = handle.is_module_space() || handle.name == "self";

                    if is_self_space {
                        // &self space: remove from environment's PathMap/RuleIndex.
                        // Mirrors the add-atom routing: match &self queries the
                        // environment, so removals must target the environment.
                        // remove_from_space() handles routing: rules → De Bruijn removal
                        // + RuleIndex sync, type assertions → types HashMap, all atoms → PathMap.
                        Arc::make_mut(&mut env_after).remove_from_space(&atom);
                    } else {
                        // Named space: remove from SpaceHandle
                        handle.remove_atom_generic(&atom);
                    }
                    // Phase 3.2: Bump mutation_epoch alone; EVAL_MEMO and
                    // MATCH_RESULT_CACHE gate lookups on mutation_epoch.
                    increment_mutation_epoch();

                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        // Disabled: ProcessRemoveAtomAtom is no longer constructed. The atom evaluation
        // step has been eliminated — remove-atom now takes unevaluated atoms per MeTTa HE
        // semantics. See ProcessRemoveAtomSpace above.
        // Continuation::ProcessRemoveAtomAtom {
        //     space_handle,
        //     atom,
        //     env: _,
        //     depth: _,
        //     parent_cont,
        // } => {
        //     let (atom_results, env_after) = result;
        //
        //     if atom_results.is_empty() {
        //         let err = ctx.factory().error(
        //             "remove-atom: atom evaluated to empty",
        //             atom,
        //         );
        //         work_stack.push(WorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (smallvec![err], env_after),
        //         });
        //     } else {
        //         // Remove the atom from the space
        //         // NOTE: Arena engine returns Unit() regardless of whether removal succeeded
        //         // GENERIC: Use remove_atom_generic to avoid heap conversion
        //         space_handle.remove_atom_generic(&atom_results[0]);
        //         work_stack.push(WorkItem::Resume {
        //             cont_id: parent_cont,
        //             result: (smallvec![ctx.factory().unit()], env_after),
        //         });
        //     }
        // }
        Continuation::ProcessNewState {
            initial_value,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (init_results, mut env_after) = result;

            if init_results.is_empty() {
                let err = ctx.factory().error(
                    initial_value,
                    ctx.factory().string("new-state: initial value evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                // Use create_state directly - values are already V
                let state_id = Arc::make_mut(&mut env_after).create_state(&init_results[0].0);
                increment_mutation_epoch();
                let state_value = ctx.factory().state(state_id);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(state_value)], env_after),
                });
            }
        }

        Continuation::ProcessGetState {
            state_ref,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    state_ref,
                    ctx.factory().string("get-state: state reference evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &state_results[0];
                if let Some(state_id) = first.as_state() {
                    // Use get_state directly - returns V
                    if let Some(generic_value) = env_after.get_state(state_id) {
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(generic_value)], env_after),
                        });
                    } else {
                        let err = ctx.factory().error(
                            first.clone(),
                            ctx.factory().string(&format!(
                                "get-state: state {} not found",
                                state_id
                            )),
                        );
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(err)], env_after),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "get-state: argument must be a state reference, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessChangeStateRef {
            state_ref,
            new_value,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    state_ref,
                    ctx.factory().string("change-state!: state reference evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &state_results[0];
                if first.as_state().is_some() {
                    continuations.push(Continuation::ProcessChangeStateValue {
                        state_value: first.clone(),
                        new_value: new_value.clone(),
                        env: env_after.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: new_value,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "change-state!: first argument must be a state reference, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessChangeStateValue {
            state_value,
            new_value,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (value_results, mut env_after) = result;

            if value_results.is_empty() {
                let err = ctx.factory().error(
                    new_value,
                    ctx.factory().string("change-state!: new value evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                // Get the state ID from state_value
                if let Some(state_id) = state_value.as_state() {
                    // Use change_state directly - values are already V
                    Arc::make_mut(&mut env_after).change_state(state_id, &value_results[0].0);
                    increment_mutation_epoch();
                    let result_state = ctx.factory().state(state_id);
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(result_state)], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        state_value,
                        ctx.factory().string("change-state!: expected state value"),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessRepr {
            atom: _,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().string(""))], env_after),
                });
            } else {
                let repr = atom_results[0].0.friendly_repr();
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().string(&repr))], env_after),
                });
            }
        }

        Continuation::ProcessFormatArgsString {
            format_arg,
            args_arg,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (format_results, env_after) = result;

            if format_results.is_empty() {
                let err = ctx.factory().error(
                    format_arg,
                    ctx.factory().string("format-args: format string evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &format_results[0];
                if let Some(format_str) = first.as_string() {
                    continuations.push(Continuation::ProcessFormatArgsArgs {
                        format_str: format_str.to_string(),
                        args_arg: args_arg.clone(),
                        env: env_after.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: args_arg,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "format-args: first argument must be a string, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessFormatArgsArgs {
            format_str,
            args_arg,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (args_results, env_after) = result;

            if args_results.is_empty() {
                let err = ctx.factory().error(
                    args_arg,
                    ctx.factory().string("format-args: args evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                // Get args as a list - use native generic values directly
                let args_list: Vec<&MettaValue> = if let Some(items) = args_results[0].0.as_sexpr()
                {
                    items.iter().collect()
                } else {
                    args_results.iter().map(|(v, _)| v).collect()
                };

                // S14b (2026-05-14): HE-compatible format-args via dyn-fmt semantics.
                // `{}` consumed sequentially (also `{N}` indexed for back-compat);
                // `{{` / `}}` escape literal braces; strings stripped of surrounding
                // quotes via to_display_string (matches HE's `atom_to_string`).
                let result_str = format_args_he(&format_str, &args_list);

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().string(&result_str))], env_after),
                });
            }
        }

        Continuation::ProcessPrintln {
            atom: _,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (atom_results, env_after) = result;

            for (atom_result, _b) in &atom_results {
                // Use to_display_string() - prints strings without quotes
                println!("{}", atom_result.to_display_string());
            }
            // println! does NOT increment mutation_epoch — it's an IO effect,
            // not a state mutation. Caching of println!-containing expressions
            // is prevented by the IO type system: println! has return type
            // (IO Unit), which propagates through user-defined functions via
            // Phase 10 inference. should_memoize_with_env checks the inferred
            // IO type and refuses to memoize.

            work_stack.push(WorkItem::Resume {
                result: (smallvec![bv(ctx.factory().unit())], env_after),
            });
        }

        Continuation::ProcessTraceMessage {
            message: _,
            value_expr,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (msg_results, env_after) = result;

            // HE semantics: print message on its own line, no prefix
            if let Some((first, _)) = msg_results.first() {
                eprintln!("{}", first.friendly_repr());
            }

            // Now evaluate the value
            continuations.push(Continuation::ProcessTraceValue {
                value_expr: value_expr.clone(),
                env: env_after.clone(),
                depth,
                outer_carrying: outer_carrying.clone(),
            });

            work_stack.push(WorkItem::Eval {
                value: value_expr,
                env: env_after,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
                demand: None,
                carrying_bindings: outer_carrying.clone(),
            });
        }

        Continuation::ProcessTraceValue {
            value_expr: _,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (value_results, env_after) = result;

            // HE semantics: return evaluated value(s) as-is.
            // If empty, propagate empty (valid nondeterministic dead-end).
            // HE trace! does not print the value — only the message.
            work_stack.push(WorkItem::Resume {
                result: (value_results, env_after),
            });
        }

        Continuation::ProcessGetMetatype {
            atom: _,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().atom("Undefined"))], env_after),
                });
            } else {
                let (first, _) = &atom_results[0];
                // Delegate to shared ValueView::metatype() — single source of
                // truth across T0/T1/T2-T3, HE-aligned. view() strips Spanned.
                let metatype = first.view().metatype();
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().atom(metatype))], env_after),
                });
            }
        }

        Continuation::ProcessBind {
            token,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (atom_results, mut env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    ctx.factory().atom(&token),
                    ctx.factory().string("bind!: atom evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                Arc::make_mut(&mut env_after).register_token(&token, atom_results[0].0.clone());
                increment_mutation_epoch();
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().unit())], env_after),
                });
            }
        }

        // if-reducible: expr has been evaluated, compare to original
        Continuation::ProcessIfReducible {
            original_expr,
            then_branch,
            else_branch,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (eval_results, env_after) = result;

            // Determine if the expression reduced:
            // - Empty results → irreducible (nothing produced)
            // - Single result equal to original → irreducible
            // - Otherwise → reduced (result changed or multiple results)
            let is_irreducible = if eval_results.is_empty() {
                true
            } else if eval_results.len() == 1 {
                eval_results[0].0 == original_expr
            } else {
                // Multiple results means the expression nondeterministically reduced
                false
            };

            // Trace: reduced / irreducible phase
            #[cfg(feature = "trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if is_irreducible {
                        "irreducible"
                    } else {
                        "reduced"
                    };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&original_expr),
                        eval_results
                            .iter()
                            .map(|(v, _)| crate::backend::trace::trace_value_generic(v))
                            .collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "if-reducible".to_string(),
                            phase: phase.to_string(),
                        },
                    );
                }
            }

            if is_irreducible {
                // Expression didn't change — evaluate else branch
                work_stack.push(WorkItem::Eval {
                    value: else_branch,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // Expression reduced — evaluate then branch
                work_stack.push(WorkItem::Eval {
                    value: then_branch,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        // match-or: space has been evaluated, now perform match with default fallback
        Continuation::ProcessMatchOrSpace {
            space_arg: _,
            pattern,
            default,
            template,
            env,
            depth,
            outer_carrying,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                // Trace: default-branch phase (space empty)
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&pattern),
                            vec![crate::backend::trace::trace_value_generic(&default)],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "match-or".to_string(),
                                phase: "default-branch".to_string(),
                            },
                        );
                    }
                }

                // Space evaluated to empty — use default
                work_stack.push(WorkItem::Eval {
                    value: default,
                    env: env_after,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                let (first, _) = &space_results[0];
                if let Some(handle) = first.as_space() {
                    if handle.is_module_space() || handle.name == "self" {
                        // &self or module space — use env.match_space
                        let matches = env.match_space(&pattern, &template);
                        let generic_results: Vec<MettaValue> = matches
                            .into_iter()
                            .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                            .collect();

                        if generic_results.is_empty() {
                            // Trace: default-branch phase (&self no match)
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&pattern),
                                        vec![crate::backend::trace::trace_value_generic(&default)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "match-or".to_string(),
                                            phase: "default-branch".to_string(),
                                        },
                                    );
                                }
                            }

                            // No matches — evaluate default
                            work_stack.push(WorkItem::Eval {
                                value: default,
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else if generic_results.len() == 1 {
                            // Single match — evaluate template result
                            work_stack.push(WorkItem::Eval {
                                value: generic_results.into_iter().next().expect("non-empty"),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            // Multiple matches — queue template evaluations
                            let mut templates = generic_results.into_iter();
                            let tmpl_capacity = templates.len(); // total before consuming first
                            let first_template = templates.next().expect("non-empty");

                            continuations.push(Continuation::ProcessMatchTemplates {
                                remaining_templates: templates,
                                results: Vec::with_capacity(tmpl_capacity),
                                env: env_after.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(WorkItem::Eval {
                                value: first_template,
                                env: Arc::new(forked_env),
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    } else {
                        // Owned space — match against SpaceHandle
                        let instantiated_templates: Vec<MettaValue> =
                            handle.match_pattern_generic(&pattern, &template, ctx.factory());

                        if instantiated_templates.is_empty() {
                            // Trace: default-branch phase (owned space no match)
                            #[cfg(feature = "trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&pattern),
                                        vec![crate::backend::trace::trace_value_generic(&default)],
                                        None,
                                        trace_format::TraceEventKind::SpecialForm {
                                            form_name: "match-or".to_string(),
                                            phase: "default-branch".to_string(),
                                        },
                                    );
                                }
                            }

                            // No matches — evaluate default
                            work_stack.push(WorkItem::Eval {
                                value: default,
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else if instantiated_templates.len() == 1 {
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_templates
                                    .into_iter()
                                    .next()
                                    .expect("non-empty"),
                                env: env_after,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            let mut generic_templates = instantiated_templates.into_iter();
                            let tmpl_capacity = generic_templates.len(); // total before consuming first
                            let first_template = generic_templates.next().expect("non-empty");

                            continuations.push(Continuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: Vec::with_capacity(tmpl_capacity),
                                env: env_after.clone(),
                                depth,
                                outer_carrying: outer_carrying.clone(),
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(WorkItem::Eval {
                                value: first_template,
                                env: Arc::new(forked_env),
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    }
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "match-or: first argument must be a space, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        // Memo-related continuations - delegate to heap conversion for now
        Continuation::ProcessMemoTable {
            memo_ref,
            expr,
            first_only,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let err = ctx.factory().error(
                    memo_ref,
                    ctx.factory().string("memo/memo!: memo reference evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    // Check if already cached - use generic lookup
                    if let Some(cached) = memo_handle.lookup_generic(&expr, ctx.factory()) {
                        work_stack.push(WorkItem::Resume {
                            result: (cached.into_iter().map(bv).collect(), env_after),
                        });
                    } else {
                        // Not cached - evaluate and cache result
                        continuations.push(Continuation::ProcessMemoExpr {
                            memo_handle: memo_handle.clone(),
                            expr: expr.clone(),
                            first_only,
                            env: env_after.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env: env_after,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "memo/memo!: first argument must be a memo table, got {}",
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessMemoExpr {
            memo_handle,
            expr,
            first_only,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (expr_results, env_after) = result;

            // Cache the result using generic store
            if first_only && !expr_results.is_empty() {
                let slice = &expr_results[..1];
                let _vals: Vec<MettaValue> = slice.iter().map(|(v, _)| v.clone()).collect();
                memo_handle.store_generic(&expr, &_vals);
            } else {
                {
                    let _vals: Vec<MettaValue> =
                        expr_results.iter().map(|(v, _)| v.clone()).collect();
                    memo_handle.store_generic(&expr, &_vals);
                };
            }

            work_stack.push(WorkItem::Resume {
                result: (expr_results, env_after),
            });
        }

        Continuation::ProcessNewMemoName {
            name_arg,
            size_arg,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (name_results, env_after) = result;

            if name_results.is_empty() {
                let err = ctx.factory().error(
                    name_arg,
                    ctx.factory().string("new-memo: name evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &name_results[0];
                let name = if let Some(s) = first.as_string() {
                    s.to_string()
                } else if let Some(a) = first.as_atom() {
                    a.to_string()
                } else {
                    first.friendly_repr()
                };

                if let Some(size_value) = size_arg {
                    continuations.push(Continuation::ProcessNewMemoSize {
                        name,
                        size_arg: size_value.clone(),
                        env: env_after.clone(),
                        depth,
                        outer_carrying: outer_carrying.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: size_value,
                        env: env_after,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    // No size argument - create memo with default size (no limit)
                    let memo_handle = crate::backend::models::MemoHandle::new(name);
                    let memo_value = ctx.factory().memo(memo_handle);
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(memo_value)], env_after),
                    });
                }
            }
        }

        Continuation::ProcessNewMemoSize {
            name,
            size_arg,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (size_results, env_after) = result;

            if size_results.is_empty() {
                let err = ctx.factory().error(
                    size_arg,
                    ctx.factory().string("new-memo: size evaluated to empty"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let size = size_results[0].0.as_long().unwrap_or(1000) as usize;
                let memo_handle = crate::backend::models::MemoHandle::with_max_size(name, size);
                let memo_value = ctx.factory().memo(memo_handle);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(memo_value)], env_after),
                });
            }
        }

        Continuation::ProcessMemoOp {
            memo_ref,
            is_clear,
            env: _,
            depth: _,
            outer_carrying: _,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let op_name = if is_clear {
                    "clear-memo!"
                } else {
                    "memo-stats"
                };
                let err = ctx.factory().error(
                    memo_ref,
                    ctx.factory().string(&format!(
                        "{}: memo reference evaluated to empty",
                        op_name
                    )),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    if is_clear {
                        memo_handle.clear();
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(ctx.factory().unit())], env_after),
                        });
                    } else {
                        #[cfg(feature = "track-stats")]
                        {
                            let stats = memo_handle.stats();
                            let stats_sexpr = ctx.factory().sexpr(vec![
                                ctx.factory().atom("hits"),
                                ctx.factory().long(stats.0 as i64),
                                ctx.factory().atom("misses"),
                                ctx.factory().long(stats.1 as i64),
                                ctx.factory().atom("size"),
                                ctx.factory().long(stats.2 as i64),
                            ]);
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(stats_sexpr)], env_after),
                            });
                        }
                        #[cfg(not(feature = "track-stats"))]
                        {
                            let detail_atom = ctx
                                .factory()
                                .atom("Rebuild with: cargo build --features track-stats");
                            let err = ctx.factory().error(
                                detail_atom,
                                ctx.factory().string("memo-stats requires track-stats feature"),
                            );
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(err)], env_after),
                            });
                        }
                    }
                } else {
                    let op_name = if is_clear {
                        "clear-memo!"
                    } else {
                        "memo-stats"
                    };
                    let err = ctx.factory().error(
                        first.clone(),
                        ctx.factory().string(&format!(
                            "{}: argument must be a memo table, got {}",
                            op_name,
                            first.friendly_repr()
                        )),
                    );
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(err)], env_after),
                    });
                }
            }
        }

        Continuation::MemoizeResult {
            expr_hash,
            mutation_epoch: saved_epoch,
            env: _,
            depth: _,
        } => {
            let (result_values, result_env) = result;

            // Only cache results if no mutations occurred during evaluation.
            // If the epoch advanced, a side effect happened transitively,
            // so the result may depend on mutable state and must not be cached.
            //
            // Values-only cache contract (intentional discard of bindings):
            // `eval_memo_put` stores values independent of caller context.
            // On cache hit (line ~2241), consumer re-tags with the retrieving
            // caller's carrying_bindings. Storing bindings here would leak
            // cross-caller within the same query. The `should_memoize` gate
            // restricts caching to ground-input expressions, so cached
            // values are themselves ground.
            if mutation_epoch() == saved_epoch {
                eval_memo_put(
                    expr_hash,
                    &result_values
                        .iter()
                        .map(|(v, _)| v.clone())
                        .collect::<Vec<_>>(),
                );
            }

            work_stack.push(WorkItem::Resume {
                result: (result_values, result_env),
            });
        }

        // ── Stretch Goal 2: ProcessLetStar tight loop ──
        //
        // Sequential binding evaluation for `let*` without desugaring to
        // nested `let` forms. Each resumption pattern-matches the result
        // against the current pair's pattern, accumulates bindings, and
        // evaluates the next pair's value expression.
        //
        // Flow: EvalWithBindings detects `let*`, pops first (pattern, value_expr),
        // pushes ProcessLetStar(current_pattern, remaining_pairs, body, bindings),
        // pushes Eval(materialized_value_expr). On Resume:
        // 1. Pattern-match result against current_pattern
        // 2. Compose new bindings with accumulated_bindings
        // 3. If more pairs: materialize next value_expr, push ProcessLetStar, push Eval
        // 4. If no more pairs: push EvalWithBindings(body, final_bindings)
        Continuation::ProcessLetStar {
            current_pattern,
            mut remaining_pairs,
            body,
            mut accumulated_bindings,
            env: _,
            depth,
            is_tail_call,
            region_id,
        } => {
            let (result_values, result_env) = result;

            // Deterministic fast path: single result → pattern match + accumulate
            if result_values.len() == 1 {
                // HE parity: compose the per-branch bindings from the value
                // expression's rule-match into `accumulated_bindings` so
                // subsequent pairs and the body see variables bound by the
                // value expression (e.g. `$b` in `(father b $b)` ↔ `(father b c)`
                // binds `$b=c`, which must be visible to later `(father $b c)`).
                //
                // Phase 2.B Issue #1 fix: use `compose_outer_inner_strict_generic`
                // so conflicts between `accumulated_bindings` and per-branch /
                // pattern-match bindings cause the branch to die (HE-bisimilar
                // silent pruning). The previous `.compose()` was a silent
                // union that masked conflicts and produced ghost outputs.
                //
                // 2026-04-23: the earlier "scope barrier" strip of `$__fr_*`
                // from `accumulated_bindings` was removed — it dropped
                // freshened vars from the CURRENT rule invocation (legitimate
                // let*-pair bindings produced by e.g. PLN's `BestCandidate`
                // recursive body), causing downstream `if` conditions to see
                // unbound freshened vars and return unreduced forms.
                let (value, per_branch_bindings) = &result_values[0];
                let composed_outer =
                    match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                        &*accumulated_bindings,
                        per_branch_bindings,
                        ctx.factory(),
                    ) {
                        Some(b) => b,
                        None => {
                            // Binding conflict: per-branch bindings contradict accumulated.
                            crate::backend::eval::cesk::with_region_stack(|s| {
                                s.exit();
                            });
                            work_stack.push(WorkItem::Resume {
                                result: (SmallVec::new(), result_env),
                            });
                            return;
                        }
                    };
                accumulated_bindings = std::sync::Arc::new(composed_outer);

                if let Some(pm_bindings) = pattern_match(&current_pattern, value) {
                    // PLN-fix 2026-04 (pattern-keyed shadow + scope barrier):
                    // before strict-composing pm into accumulated, apply both
                    // strips via `prepare_letstar_accumulated`:
                    //   (1) drop `$__fr_*` scope leaks from prior iterations;
                    //   (2) drop keys that the current pattern is about to
                    //       bind, so the new pair's pm wins (HE let* shadow
                    //       semantics: `(let* (($x 1) ($x 2)) $x) → 2`).
                    // Strict-compose still fires for any NON-shadow conflict
                    // (e.g., rule-match inner bindings on variables outside
                    // the current pattern), preserving ghost-branch pruning.
                    let accumulated_prep =
                        crate::backend::eval::bindings::prepare_letstar_accumulated(
                            &*accumulated_bindings,
                            &current_pattern,
                            ctx.factory(),
                        );
                    let composed_pm =
                        match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                            &accumulated_prep,
                            &pm_bindings,
                            ctx.factory(),
                        ) {
                            Some(b) => b,
                            None => {
                                // PM bindings conflict with accumulated
                                // (e.g. let* re-binds $x to a different value).
                                crate::backend::eval::cesk::with_region_stack(|s| {
                                    s.exit();
                                });
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), result_env),
                                });
                                return;
                            }
                        };
                    accumulated_bindings = std::sync::Arc::new(composed_pm);

                    if remaining_pairs.is_empty() {
                        // I-5: Exit region — let* scope complete
                        crate::backend::eval::cesk::with_region_stack(|s| {
                            s.exit();
                        });
                        // All bindings resolved — evaluate body with composed bindings.
                        // Also carry them as ambient `carrying_bindings` so nested
                        // chain/let/rule-match handlers can resolve variables bound
                        // in this let* chain.
                        let tracked = active_tracked_vars();
                        let projected_accumulated = match project_owned_bindings_for_consumer(
                            &*accumulated_bindings,
                            &body,
                            tracked.as_deref(),
                            ctx.factory(),
                        ) {
                            Some(b) => std::sync::Arc::new(b),
                            None => {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), result_env),
                                });
                                return;
                            }
                        };
                        let ambient = projected_accumulated.clone();
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: projected_accumulated,
                            env: result_env,
                            depth,
                            is_tail_call,
                            expected_type: None,
                            carrying_bindings: ambient,
                        });
                    } else {
                        // More pairs to process — pop next pair
                        let (next_pattern, next_value_expr) = remaining_pairs.remove(0);
                        let materialized_value =
                            apply_bindings(&next_value_expr, &accumulated_bindings, ctx.factory());

                        // 2026-04-23: the earlier "scope barrier at iteration
                        // handoff" strip of `$__fr_*` was removed — it dropped
                        // freshened bindings legitimately produced by the
                        // CURRENT rule invocation's recursion body, breaking
                        // PLN's `BestCandidate` and similar recursive rules
                        // whose let* body references caller-scope freshened
                        // names via the accumulated context.
                        let tracked = active_tracked_vars();
                        let ambient = match project_carrying_for_consumer(
                            &accumulated_bindings,
                            &materialized_value,
                            tracked.as_deref(),
                            ctx.factory(),
                        ) {
                            Some(b) => b,
                            None => {
                                work_stack.push(WorkItem::Resume {
                                    result: (SmallVec::new(), result_env),
                                });
                                return;
                            }
                        };
                        continuations.push(Continuation::ProcessLetStar {
                            current_pattern: next_pattern,
                            remaining_pairs,
                            body,
                            accumulated_bindings,
                            env: result_env.clone(),
                            depth,
                            is_tail_call,
                            region_id, // I-5: propagate region through let* chain
                        });

                        // Carry accumulated bindings into the next value expr's
                        // evaluation so rules that reference let*-bound variables
                        // (e.g. `(father $b c)` where $b was bound earlier)
                        // unify against the already-bound variable, not a free one.
                        work_stack.push(WorkItem::Eval {
                            value: materialized_value,
                            env: result_env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: ambient,
                        });
                    }
                } else {
                    // Pattern match failed — let* produces empty (MeTTa HE semantics)
                    crate::backend::eval::cesk::with_region_stack(|s| {
                        s.exit();
                    }); // I-5
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                }
            } else if result_values.is_empty() {
                // Zero results — let* produces empty
                crate::backend::eval::cesk::with_region_stack(|s| {
                    s.exit();
                }); // I-5
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
            } else {
                // Multiple results — nondeterministic value expression.
                // I-5: Exit region before fallback (region doesn't span materialized let forms)
                crate::backend::eval::cesk::with_region_stack(|s| {
                    s.exit();
                });
                // Fall back to standard `let` machinery for each result.
                // Build nested let form for remaining pairs + body, then
                // use ProcessAmb to handle each result.
                let mut let_body = body;
                for (pat, val_expr) in remaining_pairs.into_iter().rev() {
                    let_body = ctx.factory().sexpr(vec![
                        ctx.factory().atom("let"),
                        pat,
                        val_expr,
                        let_body,
                    ]);
                }

                // For each result value, pattern-match and evaluate the rest.
                // HE parity: compose the per-branch rule-match bindings into
                // the pattern-match bindings so variables bound by the value
                // expression propagate through into subsequent iterations and
                // the body (e.g. `$b=c` from `(father b $b) ↔ (father b c)`).
                // Keep both the materialized body AND its composed bindings
                // (as `carrying_bindings`) so downstream rule-matches can see
                // the bound variable, not just its textual substitution.
                // Phase 2.B Issue #1 fix: use strict compose in fallback
                // fold so conflicting alternatives are silently dropped.
                // 2026-04-23: the "scope barrier" strip of `$__fr_*` was
                // removed here as well — see matching change in the fast path.
                // Pattern-keyed shadow is still applied via
                // `prepare_letstar_accumulated` below so user-level var
                // rebinding across let* pairs follows HE semantics.
                let mut bound_bodies: Vec<(
                    MettaValue,
                    crate::backend::eval::trampoline::types::SharedBindings,
                )> = Vec::new();
                for (value, per_branch) in result_values.iter() {
                    if let Some(pm_bindings) = pattern_match(&current_pattern, value) {
                        let with_branch =
                            match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                &*accumulated_bindings,
                                per_branch,
                                ctx.factory(),
                            ) {
                                Some(b) => b,
                                None => continue, // conflict → drop this alternative
                            };
                        // Pattern-keyed shadow: drop keys the pattern is about
                        // to bind so the pm wins. Matches HE's `let*` shadow
                        // semantics for user-level variable rebinding across
                        // pairs. Applied on `with_branch` (not on the source
                        // accumulated) because we want it to affect the pm
                        // merge, not the per-branch merge.
                        let with_branch_shadow =
                            crate::backend::eval::bindings::prepare_letstar_accumulated(
                                &with_branch,
                                &current_pattern,
                                ctx.factory(),
                            );
                        let composed =
                            match crate::backend::eval::bindings::compose_outer_inner_strict_generic(
                                &with_branch_shadow,
                                &pm_bindings,
                                ctx.factory(),
                            ) {
                                Some(b) => b,
                                None => continue,
                            };
                        let materialized = apply_bindings(&let_body, &composed, ctx.factory());
                        let tracked = active_tracked_vars();
                        if let Some(projected) = project_owned_bindings_for_consumer(
                            &composed,
                            &materialized,
                            tracked.as_deref(),
                            ctx.factory(),
                        ) {
                            bound_bodies.push((materialized, std::sync::Arc::new(projected)));
                        }
                    }
                }

                if bound_bodies.is_empty() {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else if bound_bodies.len() == 1 {
                    let (materialized, ambient) =
                        bound_bodies.into_iter().next().expect("len == 1");
                    work_stack.push(WorkItem::Eval {
                        value: materialized,
                        env: result_env,
                        depth,
                        is_tail_call,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: ambient,
                    });
                } else {
                    // Nondeterministic: fan out via ProcessAmb. Each alt carries
                    // its own composed (accumulated ∘ per_branch ∘ pm) bindings
                    // so the body's rule-matching sees the bound variables.
                    let mut iter = bound_bodies.into_iter();
                    let (first_val, first_ambient) = iter.next().expect("bodies non-empty");
                    let rest: Vec<BoundValue> = iter.map(|(v, b)| (v, (*b).clone())).collect();

                    continuations.push(Continuation::ProcessAmb {
                        remaining_alts: rest.into_iter(),
                        results: Vec::new(),
                        env: result_env.clone(),
                        depth,
                        outer_carrying: accumulated_bindings.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: first_val,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: first_ambient,
                    });
                }
            }
        }

        // ── I-4: CompleteSubgoal — cache tabling results ──
        Continuation::CompleteSubgoal {
            expr_hash,
            env: _,
            depth: _depth,
            start_epoch,
        } => {
            let (result_values, result_env) = result;

            // Unmark from active evaluation set — this expression is
            // no longer on the call stack.
            crate::backend::eval::cesk::unmark_eval_active(expr_hash);

            // Only cache if no mutations occurred during evaluation.
            // If the epoch changed, the expression (or something it
            // transitively called) performed a side effect — caching
            // would suppress re-execution on future calls.
            //
            // Cache values only; cross-branch isolation is enforced by
            // scope_gen (per-branch watermark) + query_generation
            // (per-`!` watermark). Sibling-branch hits are prevented by
            // is_scope_visible, so re-tagging with the retrieving
            // branch's carrying_bindings is safe.
            if start_epoch == mutation_epoch() {
                // Values-only cache contract (intentional discard of `_b`):
                // the cache stores results independent of caller context.
                // On cache hit (line ~2193), consumers re-tag with THEIR OWN
                // `carrying_bindings`. Storing `_b` would leak Caller A's
                // bindings to Caller B's hit — a within-query ghost that
                // `query_generation` cannot prevent. See
                // tests/ghost_branch_regression.rs::
                // within_query_cache_isolation_contract for enforcement.
                let cached: smallvec::SmallVec<[MettaValue; 2]> =
                    result_values.iter().map(|(v, _b)| v.clone()).collect();
                crate::backend::eval::cesk::with_subgoal_table(|t| {
                    t.complete(expr_hash, cached);
                });
            }

            #[cfg(feature = "trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        _depth as u32,
                        crate::backend::trace::trace_value_generic(
                            &result_values
                                .first()
                                .map(|(v, _)| v.clone())
                                .unwrap_or_else(|| ctx.factory().unit()),
                        ),
                        result_values
                            .iter()
                            .map(|(v, _)| crate::backend::trace::trace_value_generic(v))
                            .collect(),
                        None,
                        trace_format::TraceEventKind::TablingDecision {
                            expr_hash,
                            decision: trace_format::TablingDecisionKind::CompleteStore,
                            result_count: Some(result_values.len() as u32),
                        },
                    );
                }
            }

            work_stack.push(WorkItem::Resume {
                result: (result_values, result_env),
            });
        }

        // ── I-6: CompleteThunk — cache thunk results ──
        Continuation::CompleteThunk {
            thunk_hash,
            env: _,
            depth: _,
            start_epoch,
        } => {
            let (result_values, result_env) = result;

            // Only cache if no mutations occurred during evaluation.
            // Values-only cache contract: thunk hash ALREADY includes the
            // template's bindings (line ~3966), so each (template, bindings)
            // combination gets its own cache entry. Values alone suffice —
            // the caller's carrying_bindings is reconstituted at cache hit
            // (line ~3977) via bv_with(v, cb.clone()). Storing bindings here
            // would leak cross-caller within the same query.
            if start_epoch == mutation_epoch() {
                let cached: smallvec::SmallVec<[MettaValue; 2]> =
                    result_values.iter().map(|(v, _)| v.clone()).collect();
                crate::backend::eval::cesk::with_thunk_table(|t| {
                    t.update(thunk_hash, cached);
                });
            }

            work_stack.push(WorkItem::Resume {
                result: (result_values, result_env),
            });
        }

        Continuation::CollectFreezeArgs {
            mut args,
            reducible_indices,
            current_idx,
            mut evaluated_results,
            env: _,
            depth,
            outer_carrying,
        } => {
            let (result_values, result_env) = result;

            evaluated_results.push(if result_values.is_empty() {
                vec![]
            } else {
                result_values.into_vec()
            });

            let next_idx = current_idx + 1;
            if next_idx < reducible_indices.len() {
                let arg_idx = reducible_indices[next_idx];
                let arg_to_eval = args[arg_idx].clone();

                continuations.push(Continuation::CollectFreezeArgs {
                    args,
                    reducible_indices,
                    current_idx: next_idx,
                    evaluated_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: arg_to_eval,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // All args evaluated. Build frozen tuples.
                if evaluated_results.iter().any(|r| r.is_empty()) {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else {
                    // Install single-result args directly.
                    for (i, idx) in reducible_indices.iter().enumerate() {
                        if evaluated_results[i].len() == 1 {
                            args[*idx] = evaluated_results[i][0].0.clone();
                        }
                    }

                    let all_single = evaluated_results.iter().all(|r| r.len() == 1);
                    if all_single {
                        let tuple = ctx.factory().sexpr(args);
                        memoize_normal_form(&tuple);
                        let cb = &*outer_carrying;
                        work_stack.push(WorkItem::Resume {
                            result: (
                                smallvec![if cb.is_empty() {
                                    bv(tuple)
                                } else {
                                    bv_with(tuple, cb.clone())
                                }],
                                result_env,
                            ),
                        });
                    } else {
                        // Cartesian product for nondeterministic args.
                        let mut sealed_results: SmallVec<[BoundValue; 2]> = SmallVec::new();
                        let mut combo_indices = vec![0usize; evaluated_results.len()];
                        loop {
                            let mut combo_args = args.clone();
                            for (i, idx) in reducible_indices.iter().enumerate() {
                                combo_args[*idx] = evaluated_results[i][combo_indices[i]].0.clone();
                            }
                            let tuple = ctx.factory().sexpr(combo_args);
                            memoize_normal_form(&tuple);
                            let cb = &*outer_carrying;
                            sealed_results.push(if cb.is_empty() {
                                bv(tuple)
                            } else {
                                bv_with(tuple, cb.clone())
                            });

                            let mut carry = true;
                            for i in (0..combo_indices.len()).rev() {
                                if carry {
                                    combo_indices[i] += 1;
                                    if combo_indices[i] < evaluated_results[i].len() {
                                        carry = false;
                                    } else {
                                        combo_indices[i] = 0;
                                    }
                                }
                            }
                            if carry {
                                break;
                            }
                        }
                        work_stack.push(WorkItem::Resume {
                            result: (sealed_results, result_env),
                        });
                    }
                }
            }
        }
    }

    // Eval-trace binding-flow instrumentation (v5) — post-match emission.
    // Inspect the work item pushed by the handler:
    //   - WorkItem::Resume → emit ContinuationEmit (+ BindingsDropped on
    //     strict-subset key-union loss).
    //   - Anything else    → emit ContinuationExitNoResume with exit_kind.
    //   - Nothing pushed   → terminal arm (e.g., Done). Emit
    //     ContinuationExitNoResume with exit_kind="done".
    #[cfg(feature = "trace")]
    if let Some((cont_kind, flow_id, cont_depth, inputs, before_len)) = trace_ctx {
        if let Some(tc) = ctx.trace_collector() {
            let pushed = if work_stack.len() > before_len {
                work_stack.last()
            } else {
                None
            };
            match pushed {
                Some(WorkItem::Resume {
                    result: (outputs, _),
                }) => {
                    let site = format!("eval_loop:{}", line!());
                    let output_snaps = crate::backend::trace::convert::trace_bound_values(outputs);

                    // Compute key-union diff.
                    let mut input_keys = std::collections::BTreeSet::<String>::new();
                    for bv in inputs.iter() {
                        for (k, _) in bv.bindings.iter() {
                            input_keys.insert(k.clone());
                        }
                    }
                    let mut output_keys = std::collections::BTreeSet::<String>::new();
                    for bv in output_snaps.iter() {
                        for (k, _) in bv.bindings.iter() {
                            output_keys.insert(k.clone());
                        }
                    }
                    let dropped_keys: Vec<String> =
                        input_keys.difference(&output_keys).cloned().collect();

                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        cont_depth,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::ContinuationEmit {
                            cont_kind: cont_kind.clone(),
                            flow_id,
                            site: site.clone(),
                            outputs: output_snaps,
                        },
                    );

                    if !dropped_keys.is_empty() {
                        // Collect sample (key,val) pairs for debugging.
                        let mut sample: Vec<(String, trace_format::TraceValue)> =
                            Vec::with_capacity(dropped_keys.len());
                        for bv in inputs.iter() {
                            for (k, v) in bv.bindings.iter() {
                                if dropped_keys.contains(k) && !sample.iter().any(|(sk, _)| sk == k)
                                {
                                    sample.push((k.clone(), v.clone()));
                                }
                            }
                        }
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            cont_depth,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::BindingsDropped {
                                cont_kind,
                                flow_id,
                                site,
                                dropped_keys,
                                sample,
                            },
                        );
                    }
                }
                Some(WorkItem::Eval { .. }) => {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        cont_depth,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::ContinuationExitNoResume {
                            cont_kind,
                            flow_id,
                            exit_kind: "eval".to_string(),
                        },
                    );
                }
                Some(WorkItem::EvalWithBindings { .. }) => {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        cont_depth,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::ContinuationExitNoResume {
                            cont_kind,
                            flow_id,
                            exit_kind: "eval-with-bindings".to_string(),
                        },
                    );
                }
                _ => {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        cont_depth,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::ContinuationExitNoResume {
                            cont_kind,
                            flow_id,
                            exit_kind: "done".to_string(),
                        },
                    );
                }
            }
        }
    }
}
