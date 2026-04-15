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
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

// ── Background Drop Worker ─────────────────────────────────────────────
//
// A dedicated thread that receives batches of deferred environment drops
// and destructs them off the hot evaluation path. The PathMap/MettaTrie
// trie cascade drops (33% inclusive CPU) now happen here instead of on
// the eval thread. Spawned lazily on first use.

type SharedEnvArc = std::sync::Arc<crate::backend::environment::GenericEnvironmentShared<MettaValue>>;

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

use smallvec::{SmallVec, smallvec};
use tracing::trace;

use super::context::{EvalContext, MettaEnvironment, SharedEnv};
use crate::backend::models::gc_allocator::RootProvider;
use super::engine::{
    apply_bindings, eval_switch, is_boolean_check_pattern, pattern_match,
    try_match_all_rules, try_deferred_deterministic_chain, DeferredChainResult,
    SwitchResult,
};
use super::types::{bv, bv_with, bvs_from_values, values_of, BoundValue, Continuation, EvalResult, WorkItem};
use super::super::list_ops::substitute_variable_generic;
use super::super::processing::{
    process_collected_sexpr_generic, GenericProcessedSExpr,
};
use super::super::step::{eval_step_generic, GenericEvalStep};

use crate::backend::eval::types::{
    extract_type_constraint, get_ground_type, is_pattern_type_compatible,
    infer_type_generic, types_match_generic, types_match_with_subtypes,
};
use crate::backend::grounded::{execute_grounded_op, ExecError, GroundedWork};
use crate::backend::models::{
    EvalGuard, GcFactory, GenericMultiplicityMatch, MettaValue, MettaValueFactory, MettaValueInner,
    MettaValueTrait,
};
use crate::backend::models::metta_value::is_variable_str;
use crate::backend::models::work_pool::global_eval_pool;
use crate::backend::priority_scheduler::{priority_levels, TaskTypeId};

/// Cached check for the `METTA_DEBUG_EVAL` environment variable.
/// Uses `OnceLock` so the syscall happens at most once per process.
static METTA_DEBUG_EVAL_CACHED: OnceLock<bool> = OnceLock::new();

fn is_debug_eval() -> bool {
    *METTA_DEBUG_EVAL_CACHED.get_or_init(|| std::env::var("METTA_DEBUG_EVAL").is_ok())
}

// Evaluation memoization and type-driven dispatch helpers extracted to `dispatch_hints`
// module for icache locality. Re-import the functions used in this file.
use super::dispatch_hints::{
    invalidate_normal_form_memo, is_memoized_normal_form, memoize_normal_form,
    is_normal_form_bounded,
    derive_arg_expected_type,
    should_memoize, should_memoize_with_env, eval_memo_get, eval_memo_put,
    clear_eval_memo, clear_match_result_cache,
    collect_eval_memo_roots, collect_match_result_roots,
    mutation_epoch, increment_mutation_epoch, set_mutation_epoch,
    enter_fork_scope, next_branch_scope, leave_fork_scope,
};
use super::engine::{try_deterministic_chain, try_match_rules_with_bindings};
use super::dispatch_hints::is_reducible_head;

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
        Continuation::ProcessRuleMatches { .. }
        | Continuation::ProcessRuleMatchesLazy { .. } => {
            SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 0,
            }
        }
        Continuation::ProcessGroundedOp { .. } => SchedulerStackSymbol::GroundedOp,
        Continuation::ProcessCombinations { .. } => SchedulerStackSymbol::Combinations,
        Continuation::ProcessLet { .. }
        | Continuation::ProcessLetStar { .. } => {
            SchedulerStackSymbol::LetChain { depth: 0 }
        }
        Continuation::CollectSExpr { .. }
        | Continuation::CollectGroundedArg { .. } => {
            SchedulerStackSymbol::ArgEval { position: 0 }
        }
        Continuation::ProcessIfCondition { .. } => {
            SchedulerStackSymbol::Conditional { branch: 0 }
        }
        Continuation::ProcessCaseAtom { .. }
        | Continuation::ProcessCaseEvalScrutineeResults { .. } => {
            SchedulerStackSymbol::CaseSwitch
        }
        Continuation::ProcessCollapseEvalResults { .. } => {
            SchedulerStackSymbol::Collapse
        }
        Continuation::MemoizeResult { .. }
        | Continuation::CompleteSubgoal { .. }
        | Continuation::CompleteThunk { .. } => {
            SchedulerStackSymbol::Memoize
        }
        _ => SchedulerStackSymbol::Root,
    }
}

/// Hash the top-3 continuation frames for WPDS context weight lookup.
///
/// Returns a 64-bit context hash suitable for the SchedulerAutomaton's
/// context_weight() method.
fn hash_continuation_context(
    continuations: &[Continuation],
) -> u64 {
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
/// Depth 0: 50%, Depth 1: 30%, Depth 2: 15%, Depth 3+: 5%.
/// This replaces the exponential `4^(-depth)` decay with configurable quotas,
/// allowing inner forks to exploit more parallelism when outer levels are idle.
const DEPTH_QUOTA_PERCENTS: [u32; MAX_DEPTH_LEVELS] = [50, 30, 15, 5, 0, 0, 0, 0];

/// Per-depth parallel branch budget quotas (Phase 3.6).
///
/// Each depth level has its own atomic budget counter, independently acquired
/// and released. This prevents shallow forks from exhausting all budget and
/// starving deeper levels.
struct DepthBudgets {
    /// Budget counters per depth level. Index = min(depth, MAX_DEPTH_LEVELS-1).
    quotas: [AtomicU32; MAX_DEPTH_LEVELS],
    /// Total budget across all levels (for diagnostics).
    total: u32,
}

static DEPTH_BUDGETS: OnceLock<DepthBudgets> = OnceLock::new();

/// Maximum parallel nesting depth, cached from `METTATRON_MAX_PARALLEL_DEPTH`.
///
/// Default: 3. Set to 0 to disable parallel branching entirely.
static MAX_PARALLEL_DEPTH: OnceLock<u32> = OnceLock::new();

fn max_parallel_depth() -> u32 {
    *MAX_PARALLEL_DEPTH.get_or_init(|| {
        std::env::var("METTATRON_MAX_PARALLEL_DEPTH")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(3)
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
            .unwrap_or(4)
    })
}

fn depth_budgets() -> &'static DepthBudgets {
    DEPTH_BUDGETS.get_or_init(|| {
        let cpus = num_cpus::get() as u32;
        let total = cpus.saturating_mul(2).min(128);

        // Initialize per-depth quotas. Can't use array init with AtomicU32
        // directly, so initialize each element.
        let quotas = std::array::from_fn(|i| {
            let pct = DEPTH_QUOTA_PERCENTS[i];
            let quota = (total * pct) / 100;
            // Ensure at least 1 slot for active depth levels (depths 0-3)
            AtomicU32::new(if pct > 0 { quota.max(1) } else { 0 })
        });

        DepthBudgets { quotas, total }
    })
}

/// Try to acquire N budget slots at the given nesting depth.
///
/// Phase 3.6: Uses per-depth quota system instead of exponential `4^(-depth)` decay.
/// Each depth level has its own budget pool, preventing shallow forks from
/// starving deeper levels.
///
/// Budget gate: when the work pool queue is saturated
/// (`queue_depth > active_workers * 2`), no budget is granted.
///
/// Returns actual slots acquired (0..=N).
fn try_acquire_budget(n: u32, depth: u32) -> u32 {
    let budgets = depth_budgets();

    // Dynamic budget gate: check queue pressure.
    let pool = global_eval_pool();
    let queue_depth = pool.queue_len();
    let active = pool.active_workers();
    if active > 0 && queue_depth > active * 2 {
        return 0;
    }

    let level = (depth as usize).min(MAX_DEPTH_LEVELS - 1);
    let quota = &budgets.quotas[level];

    let mut current = quota.load(Ordering::Relaxed);
    loop {
        let granted = n.min(current);
        if granted == 0 {
            return 0;
        }
        match quota.compare_exchange_weak(
            current,
            current - granted,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return granted,
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
    mut matches: Vec<(MettaValue, crate::backend::models::GenericBindings<MettaValue>)>,
    base_results: SmallVec<[BoundValue; 2]>,
    env: MettaEnvironment,
    depth: usize,
    ctx: &C,
    work_stack: &mut Vec<WorkItem>,
    continuations: &mut Vec<Continuation>,
    demand: Option<crate::backend::eval::cesk::coroutine::Demand>,
    outer_carrying: &crate::backend::models::GenericBindings<MettaValue>,
) {
    // Wrap bare env in Arc for O(1) sharing across WorkItem/Continuation fields.
    let env: SharedEnv = Arc::new(env);

    debug_assert!(!matches.is_empty(), "dispatch_rule_matches called with empty matches");

    // ── Single-match fast path ──
    // 93.3% of rule matches produce exactly 1 result. When there's exactly 1
    // match and no accumulated base_results, skip the ProcessRuleMatches
    // continuation entirely: no env clone, no VecDeque, no trace overhead.
    if matches.len() == 1 && base_results.is_empty() {
        let (rhs, bindings) = matches.pop().expect("matches has exactly 1 element");

        // Trace: RuleApplication (single match — no fork)
        #[cfg(feature = "eval-trace")]
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
            let composed = if outer_carrying.is_empty() {
                bindings.clone()
            } else {
                crate::backend::eval::bindings::compose_outer_inner_generic(
                    outer_carrying,
                    &bindings,
                    ctx.factory(),
                )
            };
            let current_branch_bindings = Box::new(composed);
            continuations.push(Continuation::ProcessRuleMatches {
                remaining_matches: Vec::new().into_iter(),
                results: Vec::new(),
                env: env.clone(),
                depth,
                pre_fork_epoch: mutation_epoch(),
                pre_fork_gen: 0, // not entering a fork scope — no CP to restore
                fork_depth: 0,   // no fork — cut targeting irrelevant
                current_branch_bindings,
                outer_carrying: Box::new(outer_carrying.clone()),
                tracked_vars_hint,
                #[cfg(feature = "eval-trace")]
                branch_span_id: 0,
                #[cfg(feature = "eval-trace")]
                branch_start_ns: 0,
                #[cfg(feature = "eval-trace")]
                branch_index: 0,
                #[cfg(feature = "eval-trace")]
                total_branches: 1,
            });
        }

        // Stage 1d-revised: compute RHS carrying = compose(outer_carrying, match_bindings).
        // This is HE-faithful: each in-flight alternative's Bindings travel
        // with its evaluation. Only non-empty when in a collapse-bind scope
        // or ambient is passed (zero overhead otherwise).
        let rhs_carrying: crate::backend::models::GenericBindings<MettaValue> =
            if outer_carrying.is_empty() && !in_collapse_bind_scope() {
                crate::backend::models::GenericBindings::new()
            } else {
                crate::backend::eval::bindings::compose_outer_inner_generic(
                    outer_carrying, &bindings, ctx.factory(),
                )
            };
        // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings.
        // When RHS has no variables, push as Eval directly (O(1) pointer copy).
        if rhs.has_variables_fast() {
            work_stack.push(WorkItem::EvalWithBindings {
                template: rhs,
                bindings: Box::new(bindings),
                env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
                carrying_bindings: Box::new(rhs_carrying),
            });
        } else {
            // Normal-form short-circuit for ground RHS
            if is_memoized_normal_form(&rhs) {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(rhs, rhs_carrying)], env),
                });
            } else if is_normal_form_bounded(&rhs, &*env, 2) {
                memoize_normal_form(&rhs);
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv_with(rhs, rhs_carrying)], env),
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
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: Box::new(rhs_carrying),
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: Box::new(rhs_carrying),
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
            let mut coroutine = crate::backend::eval::cesk::coroutine::BranchCoroutine::new(
                matches, effective_demand,
            );
            // BranchCoroutine with non-empty matches always has at least one branch.
            let (rhs, bindings) = coroutine.next_branch()
                .expect("BranchCoroutine::new with non-empty matches must have first branch");
            // Push the lazy continuation to collect results incrementally.
            // Stage 1c: stash this branch's match bindings so incoming sub-eval
            // results get composed with them (per-branch provenance).
            // Stage 1d-revised: compose with outer_carrying (caller's ambient).
            let current_branch_bindings = Box::new(if outer_carrying.is_empty() {
                bindings.clone()
            } else {
                crate::backend::eval::bindings::compose_outer_inner_generic(
                    outer_carrying, &bindings, ctx.factory(),
                )
            });
            let tracked_vars_hint = active_tracked_vars().map(std::sync::Arc::new);
            continuations.push(Continuation::ProcessRuleMatchesLazy {
                coroutine: Box::new(coroutine),
                results: base_results.into_vec(),
                env: env.clone(),
                depth,
                current_branch_bindings,
                outer_carrying: Box::new(outer_carrying.clone()),
                tracked_vars_hint,
            });
            // Stage 1d-revised: Lazy first branch RHS carrying = compose(outer, match).
            let lazy_carrying: crate::backend::models::GenericBindings<MettaValue> =
                if outer_carrying.is_empty() && !in_collapse_bind_scope() {
                    crate::backend::models::GenericBindings::new()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        outer_carrying, &bindings, ctx.factory(),
                    )
                };
            // Evaluate the first branch
            if rhs.has_variables_fast() {
                work_stack.push(WorkItem::EvalWithBindings {
                    template: rhs,
                    bindings: Box::new(bindings),
                    env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    carrying_bindings: Box::new(lazy_carrying),
                });
            } else {
                work_stack.push(WorkItem::Eval {
                    value: apply_bindings(&rhs, &bindings, ctx.factory()),
                    env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: Box::new(lazy_carrying),
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
        matches.iter().any(|(rhs, _)| {
            let (_, action) = scheduler.classify_and_transduce(rhs);
            action.parallelism_degree > 1
        })
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
        let branches: Vec<MettaValue> = matches
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
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: branches.len() as u32,
                    },
                );
            }
        }

        // Phase 4c: Nondeterministic fork → pre-seed tiered cache for each branch.
        // Fork branches are "hot by definition" — pre-seeding eliminates warmup delay.
        {
            let cache = crate::backend::bytecode::tiered_cache::global_tiered_cache();
            for branch in &branches {
                cache.preseed_for_immediate_compile(branch.hash_value());
            }
        }

        // Budget was acquired from try_acquire_budget — pass it to parallel_branch_eval
        // for release on completion.
        let actual_budget_acquired = budget;

        // WPDS Layer 3: compute continuation context hash for context-aware scheduling
        let ctx_hash = hash_continuation_context(continuations);
        CONTINUATION_CONTEXT_HASH.with(|h| h.set(ctx_hash));

        let results = parallel_branch_eval(branches, metta_env, actual_budget_acquired, current_depth);

        // Merge with base_results from prior branches (e.g., from EvalRuleMatches)
        let mut merged = base_results;
        merged.extend(results.into_iter().map(bv));

        work_stack.push(WorkItem::Resume {
            result: (merged, env),
        });
    } else {
        // ── Sequential path: consume first match, push ProcessRuleMatches for rest ──
        let mut remaining_iter = matches.into_iter();
        let _total_branches = (remaining_iter.len() + 1) as u32; // +1 matches existing trace convention
        let (rhs, bindings) = remaining_iter.next().expect("matches is non-empty");

        // collapse-bind: capture tracked variable bindings from first match.
        capture_bindings_if_active(&bindings);

        // Trace: NondeterministicFork + BranchStart for first branch
        #[cfg(feature = "eval-trace")]
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
        let current_branch_bindings = Box::new(if outer_carrying.is_empty() {
            bindings.clone()
        } else {
            crate::backend::eval::bindings::compose_outer_inner_generic(
                outer_carrying, &bindings, ctx.factory(),
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
            outer_carrying: Box::new(outer_carrying.clone()),
            tracked_vars_hint,
            #[cfg(feature = "eval-trace")]
            branch_span_id: _branch_span_id,
            #[cfg(feature = "eval-trace")]
            branch_start_ns: {
                ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
            },
            #[cfg(feature = "eval-trace")]
            branch_index: 0,
            #[cfg(feature = "eval-trace")]
            total_branches: _total_branches,
        });

        // Trace: RuleApplication (first match)
        #[cfg(feature = "eval-trace")]
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
        let seq_carrying: crate::backend::models::GenericBindings<MettaValue> =
            if outer_carrying.is_empty() && !in_collapse_bind_scope() {
                crate::backend::models::GenericBindings::new()
            } else {
                crate::backend::eval::bindings::compose_outer_inner_generic(
                    outer_carrying, &bindings, ctx.factory(),
                )
            };
        // Phase 1: Lazy binding — defer apply_bindings via EvalWithBindings
        if rhs.has_variables_fast() {
            work_stack.push(WorkItem::EvalWithBindings {
                template: rhs,
                bindings: Box::new(bindings),
                env,
                depth,
                is_tail_call: true,
                expected_type: None,
                carrying_bindings: Box::new(seq_carrying),
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
                    carrying_bindings: Box::new(seq_carrying),
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
    /// Fork depth at which the collapse-bind was entered.
    collapse_fork_depth: u32,
}

thread_local! {
    /// Stack of scope markers for nested `collapse-bind` calls.
    /// Empty when no `collapse-bind` is active.
    static BINDING_CAPTURE_STACK: RefCell<Vec<BindingCaptureFrame>> = const { RefCell::new(Vec::new()) };
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
fn capture_bindings_if_active(_match_bindings: &crate::backend::models::GenericBindings<MettaValue>) {}

/// Stage 1b no-op: snapshots were replaced by BoundValue.1 propagation.
#[inline]
fn snapshot_bindings_for_results(_count: usize, _at_fork_depth: u32) {}

/// Stage 1b no-op: branch-switch clearing was replaced by BoundValue.1
/// propagation (each branch's bindings travel with its own result).
#[inline]
fn clear_current_bindings_for_new_branch() {}

/// Encode bindings as an S-expression: `(Bindings ($var val) ...)`.
/// Used by collapse-bind to pair each result with its captured bindings.
fn encode_bindings_as_sexpr(
    bindings: &crate::backend::models::GenericBindings<MettaValue>,
    factory: &crate::backend::models::gc_allocator::GcFactory,
) -> MettaValue {
    let mut items: Vec<MettaValue> = Vec::with_capacity(bindings.len() + 1);
    items.push(factory.atom("Bindings"));
    for (name, value) in bindings.iter() {
        items.push(factory.sexpr(vec![factory.atom(name), value.clone()]));
    }
    factory.sexpr(items)
}

/// Decode bindings from an S-expression `(Bindings ($var val) ...)` back to
/// `GenericBindings<MettaValue>`. Used by `ground-with-bindings` and (future)
/// `superpose-bind` to reconstruct bindings from their serialized form.
pub fn decode_bindings_from_sexpr(
    sexpr: &MettaValue,
    factory: &crate::backend::models::gc_allocator::GcFactory,
) -> crate::backend::models::GenericBindings<MettaValue> {
    let mut bindings = crate::backend::models::GenericBindings::new();
    if let Some(items) = sexpr.as_sexpr() {
        // Skip head "Bindings" atom
        for pair in items.iter().skip(1) {
            if let Some(pair_items) = pair.as_sexpr() {
                if pair_items.len() == 2 {
                    if let Some(name) = pair_items[0].as_atom() {
                        bindings.insert(name, pair_items[1]);
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
        if d > 0 { c.set(d - 1); }
    });
}

/// Evaluate nondeterministic branches in parallel via the work pool.
///
/// Uses scatter-gather: evaluate branch 0 locally, spawn branches 1..N
/// to the eval pool, block-wait via condvar, merge results.
///
/// # Arguments
/// - `branches`: Pre-instantiated RHS values (bindings already applied)
/// - `env`: The evaluation environment (cloned per branch)
/// - `budget_acquired`: Number of budget slots to release on completion
///
/// # Returns
/// Flat vector of all results from all branches, concatenated in branch order.
fn parallel_branch_eval(
    branches: Vec<crate::backend::models::MettaValue>,
    env: crate::backend::environment::core::MettaEnvironment,
    budget_acquired: u32,
    caller_depth: u32,
) -> Vec<crate::backend::models::MettaValue> {
    use std::sync::{Arc, Condvar, Mutex};

    use super::context::ParallelBranchContext;

    type MettaValue = crate::backend::models::MettaValue;

    let num_branches = branches.len();
    debug_assert!(num_branches >= 2, "parallel_branch_eval requires at least 2 branches");

    // Trace: ParallelDispatch enter
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::with_trace_collector_ref(|tc| {
            let branch_tvs: Vec<trace_format::TraceValue> = branches.iter()
                .take(4)
                .map(|b| crate::backend::trace::trace_value_generic(b))
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

    // Pre-allocate result slots: Vec<Option<Vec<MettaValue>>>
    let results: Arc<Mutex<Vec<Option<Vec<MettaValue>>>>> =
        Arc::new(Mutex::new(vec![None; num_branches]));
    let remaining = Arc::new(AtomicU32::new((num_branches - 1) as u32));
    let done_pair = Arc::new((Mutex::new(false), Condvar::new()));

    let pool = global_eval_pool();
    let child_depth = caller_depth + 1;

    // Spawn branches 1..N to the work pool via PriorityQueue.
    // PriorityQueue provides instant condvar wakeup, priority levels, and
    // adaptive worker scaling — purpose-built for MeTTaTron's scheduling.
    for (slot, branch_expr) in branches.iter().enumerate().skip(1) {
        let branch_expr = branch_expr.clone();
        let env = env.clone();
        let results = Arc::clone(&results);
        let remaining = Arc::clone(&remaining);
        let done_pair = Arc::clone(&done_pair);

        // WFST classification: classify the branch expression for
        // automata-based scheduling priority and weight tracking.
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
        let descriptor = crate::backend::scheduler::TaskDescriptor::pack(
            head_hash, arity, depth_bucket, 0,
        );

        // WPDS Layer 3: compute effective priority using continuation context
        let ctx_hash = CONTINUATION_CONTEXT_HASH.with(|h| h.get());
        let effective_pri = scheduler.effective_priority(cost_class, ctx_hash);

        let closure = move || {
            // Set depth for nested parallel branching. At depth >= MAX_PARALLEL_DEPTH,
            // the gate falls through to sequential. Budget is scaled by 4^(-depth).
            PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));

            // I-8: Enter thread-local allocation region for contention-free allocation
            let _region_guard = crate::backend::eval::cesk::RegionGuard::enter();

            // Track this parallel eval as active (prevents GC during evaluation)
            let _guard = EvalGuard::enter();
            let ctx = ParallelBranchContext::get();
            let (eval_results, _new_env) =
                eval_trampoline(branch_expr, env, &ctx);

            // Store result in pre-allocated slot (no contention per slot)
            {
                let mut guard = results.lock().expect("results mutex poisoned");
                guard[slot] = Some(eval_results.into_iter().map(|(v, _)| v).collect());
            }

            // Decrement barrier; if last task, notify waiter
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

    // Evaluate branch 0 locally (avoids pool overhead for 1 task).
    // Increment depth so any recursive MatchRules in this branch goes sequential.
    let branch0_results = {
        PARALLEL_BRANCH_DEPTH.with(|d| d.set(d.get() + 1));
        let ctx = ParallelBranchContext::get();
        let (eval_results, _new_env) =
            eval_trampoline(branches[0].clone(), env.clone(), &ctx);
        PARALLEL_BRANCH_DEPTH.with(|d| d.set(d.get() - 1));
        eval_results
    };

    // Store branch 0 results
    {
        let mut guard = results.lock().expect("results mutex poisoned");
        guard[0] = Some(branch0_results.into_iter().map(|(v, _)| v).collect());
    }

    // Trace: ParallelDispatch branch0-done
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::with_trace_collector_ref(|tc| {
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                caller_depth,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::ParallelDispatch {
                    branch_count: num_branches as u32,
                    branch_exprs: vec![],
                    parallel_depth: PARALLEL_BRANCH_DEPTH.with(|d| d.get()),
                    phase: "branch0-done".to_string(),
                },
            );
        });
    }

    // Wait for all spawned tasks to complete, with work-stealing.
    //
    // Instead of purely blocking on the condvar, the main thread alternates
    // between:
    // 1. A short condvar wait (1ms) — instant wakeup if workers finish
    // 2. Stealing tasks from the pool queue — keeps the main thread productive
    //
    // This eliminates the idle gap where the main thread sits blocked while
    // workers evaluate branches. The main thread effectively becomes a
    // temporary worker, draining the queue alongside the pool workers.
    //
    // Falls back to stall detection + overflow if no progress is made.
    {
        let mut prev_remaining = remaining.load(Ordering::Acquire);
        let mut stall_count = 0u32;
        let mut overflow_requested = false;
        let queue = pool.queue();

        let (lock, cvar) = &*done_pair;
        let mut done = lock.lock().expect("done mutex poisoned");
        while !*done {
            // Short condvar wait: check for completion frequently
            let result = cvar
                .wait_timeout(done, std::time::Duration::from_millis(1))
                .expect("done condvar wait failed");
            done = result.0;
            if *done {
                break;
            }

            // Work-stealing: try to pop and execute a task from the pool queue.
            // This keeps the main thread productive while waiting for branches.
            // Execute up to 4 stolen tasks per wake cycle to amortize lock overhead.
            for _ in 0..4 {
                if remaining.load(Ordering::Acquire) == 0 {
                    break; // All branches done, stop stealing
                }
                if let Some(task) = queue.try_pop() {
                    // Drop the condvar lock before executing the stolen task
                    drop(done);
                    task.execute();
                    // Re-acquire the condvar lock
                    done = lock.lock().expect("done mutex poisoned");
                    if *done {
                        break;
                    }
                } else {
                    break; // Queue empty, nothing to steal
                }
            }

            if !*done {
                let curr_remaining = remaining.load(Ordering::Acquire);
                if curr_remaining > 0 && curr_remaining == prev_remaining {
                    stall_count += 1;
                    // After 20 consecutive stalls (~20ms with no progress), spawn overflow
                    // workers. Threshold is higher than before (was 2) because the 1ms
                    // condvar timeout makes stall detection more granular.
                    if stall_count >= 20 && !overflow_requested {
                        pool.spawn_overflow(curr_remaining as usize);
                        overflow_requested = true;
                        tracing::warn!(
                            remaining = curr_remaining,
                            active_workers = pool.active_workers(),
                            overflow = pool.overflow_count(),
                            "parallel_branch_eval: stall detected, spawned overflow workers"
                        );
                    }
                } else {
                    stall_count = 0;
                }
                prev_remaining = curr_remaining;
            }
        }
    }

    // Release budget slots back to the depth level they were acquired from
    release_budget(budget_acquired, caller_depth);

    // Merge results in branch order
    let mut merged = Vec::new();
    let guard = results.lock().expect("results mutex poisoned");
    for slot_result in guard.iter() {
        if let Some(ref branch_results) = slot_result {
            merged.extend_from_slice(branch_results);
        }
    }

    merged
}

/// Minimum number of collapse results to trigger parallel evaluation.
/// Below this threshold, the sequential `ProcessCollapseEvalResults` path
/// is cheaper due to lower overhead (no Arc, no Mutex, no condvar).
const PARALLEL_COLLAPSE_THRESHOLD: usize = 16;


/// Evaluate collapse results in parallel via the work pool.
///
/// Structurally identical to `parallel_branch_eval`, but evaluates each item
/// to normal form at depth+1 (matching `ProcessCollapseEvalResults` semantics)
/// and filters out empty results.
///
/// # Arguments
/// - `items`: Nondeterministic results from the inner expression of `collapse`
/// - `env`: The evaluation environment (cloned per item)
/// - `budget_acquired`: Number of budget slots to release on completion
/// - `caller_depth`: Parallel nesting depth of the caller
/// - `eval_depth`: The MeTTa evaluation depth (used as depth+1 for each item)
///
/// # Returns
/// Vec of all evaluated results (empty values filtered out), in item order.
fn parallel_collapse_eval(
    items: Vec<crate::backend::models::MettaValue>,
    env: crate::backend::environment::core::MettaEnvironment,
    budget_acquired: u32,
    caller_depth: u32,
    eval_depth: usize,
) -> Vec<crate::backend::models::MettaValue> {
    use std::sync::{Arc, Condvar, Mutex};

    use super::context::ParallelBranchContext;

    type MettaValue = crate::backend::models::MettaValue;

    let num_items = items.len();
    debug_assert!(num_items >= 2, "parallel_collapse_eval requires at least 2 items");

    // Pre-allocate result slots: Vec<Option<Vec<MettaValue>>>
    let results: Arc<Mutex<Vec<Option<Vec<MettaValue>>>>> =
        Arc::new(Mutex::new(vec![None; num_items]));
    let remaining = Arc::new(AtomicU32::new((num_items - 1) as u32));
    let done_pair = Arc::new((Mutex::new(false), Condvar::new()));

    let pool = global_eval_pool();
    let child_depth = caller_depth + 1;

    // Spawn items 1..N to the work pool
    for (slot, item_expr) in items.iter().enumerate().skip(1) {
        let item_expr = item_expr.clone();
        let env = env.clone();
        let results = Arc::clone(&results);
        let remaining = Arc::clone(&remaining);
        let done_pair = Arc::clone(&done_pair);

        // WFST classification for collapse items
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
        let descriptor = crate::backend::scheduler::TaskDescriptor::pack(
            head_hash, arity, depth_bucket, 0,
        );

        let closure = move || {
            PARALLEL_BRANCH_DEPTH.with(|d| d.set(child_depth));

            // I-8: Enter thread-local allocation region
            let _region_guard = crate::backend::eval::cesk::RegionGuard::enter();

            let _guard = EvalGuard::enter();
            let ctx = ParallelBranchContext::get();
            let (eval_results, _new_env) =
                eval_trampoline(item_expr, env, &ctx);

            // Store result in pre-allocated slot
            {
                let mut guard = results.lock().expect("results mutex poisoned");
                guard[slot] = Some(eval_results.into_iter().map(|(v, _)| v).collect());
            }

            // Decrement barrier; if last task, notify waiter
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

    // Evaluate item 0 locally
    let item0_results = {
        PARALLEL_BRANCH_DEPTH.with(|d| d.set(d.get() + 1));
        let ctx = ParallelBranchContext::get();
        let (eval_results, _new_env) =
            eval_trampoline(items[0].clone(), env.clone(), &ctx);
        PARALLEL_BRANCH_DEPTH.with(|d| d.set(d.get() - 1));
        eval_results
    };

    // Store item 0 results
    {
        let mut guard = results.lock().expect("results mutex poisoned");
        guard[0] = Some(item0_results.into_iter().map(|(v, _)| v).collect());
    }

    // Wait for all spawned tasks to complete, with work-stealing
    {
        let mut prev_remaining = remaining.load(Ordering::Acquire);
        let mut stall_count = 0u32;
        let mut overflow_requested = false;
        let queue = pool.queue();

        let (lock, cvar) = &*done_pair;
        let mut done = lock.lock().expect("done mutex poisoned");
        while !*done {
            let result = cvar
                .wait_timeout(done, std::time::Duration::from_millis(1))
                .expect("done condvar wait failed");
            done = result.0;
            if *done {
                break;
            }

            // Work-stealing: up to 4 stolen tasks per wake cycle
            for _ in 0..4 {
                if remaining.load(Ordering::Acquire) == 0 {
                    break;
                }
                if let Some(task) = queue.try_pop() {
                    drop(done);
                    task.execute();
                    done = lock.lock().expect("done mutex poisoned");
                    if *done {
                        break;
                    }
                } else {
                    break;
                }
            }

            if !*done {
                let curr_remaining = remaining.load(Ordering::Acquire);
                if curr_remaining > 0 && curr_remaining == prev_remaining {
                    stall_count += 1;
                    if stall_count >= 20 && !overflow_requested {
                        pool.spawn_overflow(curr_remaining as usize);
                        overflow_requested = true;
                        tracing::warn!(
                            remaining = curr_remaining,
                            active_workers = pool.active_workers(),
                            overflow = pool.overflow_count(),
                            "parallel_collapse_eval: stall detected, spawned overflow workers"
                        );
                    }
                } else {
                    stall_count = 0;
                }
                prev_remaining = curr_remaining;
            }
        }
    }

    // Release budget slots back to the depth level they were acquired from
    release_budget(budget_acquired, caller_depth);

    // Merge results in item order, filtering empty values
    let _ = eval_depth; // depth used by caller for trace; items already eval'd at depth+1
    let mut merged = Vec::new();
    let guard = results.lock().expect("results mutex poisoned");
    for slot_result in guard.iter() {
        if let Some(ref item_results) = slot_result {
            merged.extend(item_results.iter().filter(|v| !v.is_empty()).cloned());
        }
    }

    merged
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
    // Isolate fork/cut thread-local state so nested trampoline calls
    // (from `test`, `assertEqual`, `collapse` within synchronous ops,
    // etc.) don't corrupt the outer trampoline's cut signaling.
    // Without this, a `(cut)` inside a `test` body could consume the
    // outer dispatch's cut target or vice versa.
    let saved_fork = FORK_DEPTH.with(|c| c.replace(0));
    let saved_cut = CUT_TARGET_DEPTH.with(|c| c.replace(0));

    let mut outcome = eval_trampoline_inner(value, env, ctx, None, None, 0);
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
) -> crate::backend::eval::cesk::EvalOutcome {
    // Debug tracing controlled by environment variable (cached — one syscall per process)
    let debug_eval = is_debug_eval();
    let mut eval_count: u64 = 0;

    // Trace: EvalStart with span correlation + start timestamp.
    // These variables carry the start timestamp and span ID to the EvalEnd site.
    #[cfg(feature = "eval-trace")]
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
    #[cfg(feature = "eval-trace")]
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
    let mut work_stack: Vec<WorkItem> =
        if let Some(ws) = resume_work_stack {
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
                carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
            });
            ws
        };

    let mut continuations: Vec<Continuation> =
        if let Some(cs) = resume_continuations {
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
    let mut root_set = crate::backend::eval::cesk::RootSet::<MettaValue>::with_estimated_capacity(
        32, 64, 0,
    );

    // I-4/I-6: Clear subgoal and thunk tables between top-level evaluations
    // to prevent stale cached results from previous evaluations.
    // Skip when resuming — tables were already cleared on the initial call.
    if !is_resuming {
        crate::backend::eval::cesk::clear_subgoal_table();
        crate::backend::eval::cesk::clear_thunk_table();
    }

    // Deferred environment drops: hold Arc clones to dying environments' shared
    // state, deferring the expensive cascading Arc::drop_slow out of the hot path.
    // At each GC safepoint, we call collect_roots() on each deferred env to add
    // their MettaValues to the root_set (so the GC doesn't sweep them), then
    // clear the Vec AFTER the safepoint completes.
    let mut deferred_shared_drops: Vec<std::sync::Arc<
        crate::backend::environment::GenericEnvironmentShared<MettaValue>,
    >> = Vec::new();

    // Main trampoline loop
    #[cfg(feature = "eval-trace")]
    let mut _trampoline_iter: u64 = 0;
    while let Some(work) = work_stack.pop() {
        // Trace: TrampolineStep (gated by METTA_TRACE_TRAMPOLINE=1)
        #[cfg(feature = "eval-trace")]
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
                        WorkItem::EvalWithBindings { template, depth, .. } => (
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

        // Periodic GC safepoint check (every 4096 trampoline iterations)
        if gc_counter & 0xFFF == 0 && ctx.should_safepoint() {
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

            // Phase 2.2: Incremental nursery collection (thread-local, no quiescence needed).
            // Runs BEFORE cache clearing and old-gen safepoint. Uses the algebraic
            // root set to determine which nursery values are live.
            {
                crate::backend::eval::cesk::with_nursery_collector(|collector| {
                    if collector.should_collect() {
                        // Build live pointer set from root set
                        let concrete_roots = root_set.as_mut_vec();
                        let live_ptrs: std::collections::HashSet<usize> = concrete_roots
                            .iter()
                            .map(|v| v.inner_ptr() as usize)
                            .collect();
                        collector.collect(&live_ptrs);
                    }
                });
            }

            // Clear thread-local MORK serialization caches before GC runs.
            // After GC, slab slots may be reused (ABA), so cached pointer keys
            // would alias different values. Clear BEFORE perform_safepoint.
            crate::backend::environment::rule_management::clear_mork_bytes_cache();
            crate::backend::mork_convert::clear_ground_fragment_cache();

            // Clear value hash cache — pointer-keyed, same ABA concern.
            crate::backend::models::metta_value::clear_value_hash_cache();

            // Clear hash-consing table — entries reference slab pointers, same ABA concern.
            crate::backend::models::gc_allocator::clear_hash_cons_table();

            // Clear normal-form bloom filter before GC runs.
            // After GC, slab slots may be reused (ABA), so stale bloom entries
            // keyed by inner_ptr would falsely report new values at the same
            // address as "normal form" — skipping evaluation incorrectly.
            invalidate_normal_form_memo();

            #[cfg(feature = "eval-trace")]
            let _root_count = root_set.len() as u32;
            #[cfg(feature = "eval-trace")]
            let _safepoint_start = {
                ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
            };
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
                let batch: Vec<SharedEnvArc> = deferred_shared_drops.drain(batch_start..).collect();
                // Send concrete Arc<GenericEnvironmentShared<MettaValue>> to background
                // drop worker. No type erasure needed — concrete dispatch.
                let _ = get_drop_sender().send(batch);
            }

            // Trace: GcSafepoint with measured pause duration
            #[cfg(feature = "eval-trace")]
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
        }

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
            let depth_hint = continuations.last()
                .map(|c| c.depth_hint())
                .unwrap_or(0) as u32;
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

                // Debug trace (zero-conversion: uses Debug trait)
                if debug_eval {
                    eval_count += 1;
                    if eval_count % 1000 == 0 || eval_count < 100 {
                        eprintln!(
                            "[EVAL#{}] depth={} work_stack={} conts={} value={:?}",
                            eval_count,
                            depth,
                            work_stack.len(),
                            continuations.len(),
                            value
                        );
                    }
                }

                // Phase 9.5: Normal-form memoization check.
                // If this S-expression has been previously evaluated and reached
                // fixpoint (evaluated to itself), skip evaluation entirely.
                let is_sexpr = value.as_sexpr().is_some();
                if is_sexpr && is_memoized_normal_form(&value) {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(value)], env),
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
                if is_sexpr && depth >= 2 && !value.has_variables_fast() && should_memoize_with_env(&value, &*env) {
                    let tabling_hash = value.hash_value();

                    // Step 1: Cycle detection via active evaluation set.
                    // True cycle = expression is on its own call stack.
                    if crate::backend::eval::cesk::is_actively_evaluating(tabling_hash) {
                        #[cfg(feature = "eval-trace")]
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
                    let lookup = crate::backend::eval::cesk::with_subgoal_table(|t| {
                        t.lookup(tabling_hash)
                    });
                    match lookup {
                        crate::backend::eval::cesk::TableLookup::Complete(cached) => {
                            #[cfg(feature = "eval-trace")]
                            {
                                if let Some(tc) = ctx.trace_collector() {
                                    tc.emit_converted(
                                        trace_format::TraceTier::TreeWalker,
                                        depth as u32,
                                        crate::backend::trace::trace_value_generic(&value),
                                        cached.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
                                        None,
                                        trace_format::TraceEventKind::TablingDecision {
                                            expr_hash: tabling_hash,
                                            decision: trace_format::TablingDecisionKind::CacheHit,
                                            result_count: Some(cached.len() as u32),
                                        },
                                    );
                                }
                            }
                            work_stack.push(WorkItem::Resume {
                                result: (cached.into_iter().map(bv).collect(), env),
                            });
                            continue;
                        }
                        crate::backend::eval::cesk::TableLookup::Absent => {
                            #[cfg(feature = "eval-trace")]
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

                // Expression-level memoization: check if we've evaluated this
                // exact expression before (by content hash). Only for MettaValue
                // (compile-time constant after monomorphization) and pure expressions.
                let memo_hash = if is_sexpr && should_memoize_with_env(&value, &*env) {
                    let h = value.hash_value();
                    if let Some(cached_results) = eval_memo_get(h) {
                        // Cache hit — skip evaluation entirely.
                        work_stack.push(WorkItem::Resume {
                            result: (cached_results.into_iter().map(bv).collect(), env),
                        });
                        continue;
                    }
                    Some(h)
                } else {
                    None
                };

                // Sub-expression tiered dispatch: increment per-slot execution counter
                // and attempt dispatch to compiled bytecode/JIT.
                //
                // For compilable S-expressions (~3% of all in PLN), this:
                // 1. Increments the per-slot atomic counter (~10-15 cycles)
                // 2. Reads the cached compilation hash from the slot (~3 cycles)
                // 3. If hash is non-zero (cron has flushed): DashMap lookup (~15 cycles)
                // 4. If compiled code is ready: dispatch to highest tier (JIT2 > JIT1 > Bytecode)
                // 5. On dispatch success: push Resume and skip eval_step_generic
                //
                // Net overhead for cold expressions (no compiled code): ~25-30 cycles
                // Net benefit for hot expressions: tree-walker step replaced by bytecode/JIT
                //
                if is_sexpr {
                    let has_compilable_head = if let Some(items) = value.as_sexpr() {
                        if let Some(head) = items.first() {
                            if let Some(name) = head.as_atom() {
                                matches!(name,
                                    "!" | "eval"
                                    | "+" | "-" | "*" | "/" | "%" | "abs" | "pow"
                                    | "<" | "<=" | ">" | ">=" | "==" | "!="
                                    | "and" | "or" | "not" | "xor"
                                    | "if" | "case" | "chain"
                                    | "let" | "let*"
                                    | "superpose"
                                    | "quote" | "unquote"
                                    | "car-atom" | "cdr-atom" | "cons-atom" | "size-atom"
                                    | "decons-atom" | "empty"
                                    | "map-atom" | "filter-atom" | "foldl-atom"
                                    | "get-type" | "get-metatype"
                                    | "error" | "is-error" | "catch"
                                    | "repr"
                                )
                            } else { false }
                        } else { false }
                    } else { false };
                    // Skip tiered dispatch if any argument is a grounded sub-expression
                    // that needs pre-evaluation (e.g., (+ 1 1)). The bytecode VM
                    // would dispatch the rule with unevaluated arguments, binding
                    // $var = (+ 1 1) instead of $var = 2. The tree-walker's Step 2
                    // correctly pre-evaluates these before rule matching.
                    let has_grounded_args = if has_compilable_head {
                        if let Some(items) = value.as_sexpr() {
                            items.iter().skip(1).any(|arg| {
                                super::engine::binding_value_needs_eval(arg)
                            })
                        } else { false }
                    } else { false };

                    if has_compilable_head && !has_grounded_args {
                        // Merged: increment per-slot exec counter AND read cached compilation hash
                        // in a single thread-local + generation check (vs 2× for separate calls).
                        let compilation_hash = crate::backend::bytecode::tiered_cache::increment_and_get_hash(value.inner_ptr());

                        // Try dispatching to compiled bytecode/JIT.
                        // hash != 0 guard short-circuits before any trait dispatch / DashMap lookup for cold code.
                        if compilation_hash != 0 {
                        if let Some((results, new_env)) = ctx.try_compiled_dispatch(&value, &env, compilation_hash) {
                            #[cfg(feature = "eval-trace")]
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
                                            expression_hash: 0,
                                            selected_tier: trace_format::TraceTier::BytecodeVM,
                                            execution_count: 0,
                                        },
                                    );
                                }
                            }
                            work_stack.push(WorkItem::Resume {
                                result: (results.into_iter().map(bv).collect(), Arc::new(new_env)),

                            });
                            continue; // Skip eval_step_generic — compiled code handled it
                        }
                        // Dispatch returned None — fall through to tree-walker
                        }
                    }
                }

                // Push MemoizeResult continuation if we got a cache miss on a
                // memoizable expression. When the evaluation resolves, this
                // continuation caches the results for future lookups.
                if let Some(h) = memo_hash {
                    continuations.push(Continuation::MemoizeResult {
                        expr_hash: h,
                        mutation_epoch: mutation_epoch(),
                        env: env.clone(),
                        depth,
                    });
                }

                // Save input pointer for fixpoint detection (Phase 9.5)
                let input_ptr = if is_sexpr { value.inner_ptr() } else { std::ptr::null() };

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
                                values.into_iter()
                                    .map(|v| bv_with(v, cb.clone()))
                                    .collect(),
                                Arc::new(step_env),
                            )
                        };
                        work_stack.push(WorkItem::Resume { result });
                    }

                    // Need to evaluate S-expression sub-items
                    GenericEvalStep::EvalSExpr { items, env: step_env, depth } => {
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
                    GenericEvalStep::StartGroundedOp { state, env: step_env, depth } => {
                        let env: SharedEnv = Arc::new(step_env);
                        let mut state = state;
                        // Use static dispatch - monomorphized for each value type
                        // Clone op_name to avoid borrow conflict with mutable state
                        let op_name = state.op_name.clone();
                        #[cfg(feature = "eval-trace")]
                        let _grounded_start_ns = {
                            ctx.trace_collector().map(|tc| tc.elapsed_ns()).unwrap_or(0)
                        };
                        if let Some(work) = execute_grounded_op(&op_name, &mut state, ctx.factory()) {
                            match work {
                                GroundedWork::Done(results) => {
                                    // Results are already in correct type V - NO conversion
                                    let values: Vec<MettaValue> = results
                                        .into_iter()
                                        .map(|(v, _)| v)
                                        .collect();
                                    // Trace: GroundedOp success with duration
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let end_ns = tc.elapsed_ns();
                                            let duration = end_ns.saturating_sub(_grounded_start_ns);
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts = Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() { parts.push(arg.clone()); }
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
                                    work_stack.push(WorkItem::Resume {
                                        result: (values.into_iter().map(bv).collect(), env),

                                    });
                                }
                                GroundedWork::EvalArg { arg_idx, state: new_state } => {
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
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            let (error_kind, message) = match &e {
                                                ExecError::NoReduce => ("NoReduce", String::new()),
                                                ExecError::Runtime(msg) => ("Runtime", msg.clone()),
                                                ExecError::Arithmetic(msg) => ("Arithmetic", msg.clone()),
                                                ExecError::IncorrectArgument(msg) => ("IncorrectArgument", msg.clone()),
                                            };
                                            let input = crate::backend::trace::trace_value_generic(
                                                &ctx.factory().sexpr({
                                                    let mut parts = Vec::with_capacity(1 + state.args.len());
                                                    parts.push(ctx.factory().atom(&op_name));
                                                    for arg in state.args.iter() { parts.push(arg.clone()); }
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
                                            let mut expr_parts = Vec::with_capacity(1 + state.args.len());
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
                                            let error_value = match e {
                                                ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                                                ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                                                ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                                                ExecError::NoReduce => unreachable!(),
                                            };
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
                                &format!("Grounded operation '{}' not found in generic registry", op_name),
                                ctx.factory().atom("OperationNotFoundError"),
                            );
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(error_value)], env),
                            });
                        }
                    }

                    // Start let binding
                    GenericEvalStep::StartLetBinding { pattern, value_expr, body, env: step_env, depth } => {
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
                    GenericEvalStep::EvalIfBranch { branch, env: step_env, depth } => {
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
                    GenericEvalStep::EvalRuleMatchesLazy { mut matches, env: step_env, depth } => {
                        let env: SharedEnv = Arc::new(step_env);
                        // 8.7: Branch pruning — filter out matches whose rhs_type
                        // is known to be incompatible with the expected_type
                        if let Some(ref expected) = expected_type {
                            let before_count = matches.len();

                            #[cfg(feature = "eval-trace")]
                            let mut pruned_types: Vec<Option<trace_format::TraceValue>> = Vec::new();

                            matches.retain(|(_rhs, _bindings, rhs_type)| {
                                let keep = match rhs_type {
                                    Some(rt) => types_match_generic(rt, expected),
                                    None => true, // Unknown type — don't prune (conservative)
                                };
                                #[cfg(feature = "eval-trace")]
                                if !keep {
                                    pruned_types.push(
                                        rhs_type.as_ref().map(crate::backend::trace::trace_value_generic)
                                    );
                                }
                                keep
                            });

                            // Emit BranchPrune trace event when pruning occurred
                            #[cfg(feature = "eval-trace")]
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
                                                expected_type: crate::backend::trace::trace_value_generic(expected),
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
                            let matches_deque: Vec<_> = matches.into_iter()
                                .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                .collect();
                            dispatch_rule_matches(matches_deque, SmallVec::new(), (*env).clone(), depth, ctx, &mut work_stack, &mut continuations, demand, &crate::backend::models::GenericBindings::new());
                        }
                    }

                    // Evaluate grounded arguments
                    GenericEvalStep::EvalGroundedArgs { items, grounded_indices, env: step_env, depth } => {
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
                                &items, first_idx, &env, ctx.factory(),
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
                    GenericEvalStep::StartMapAtom { elements, var_name, template, env: step_env, depth } => {
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
                            });

                            // Substitute variable and evaluate - NO CONVERSION NEEDED
                            let instantiated = substitute_variable_generic(
                                &template, &var_name, &first, ctx.factory(),
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
                    GenericEvalStep::StartFilterAtom { elements, var_name, predicate, env: step_env, depth } => {
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
                            });

                            // NO CONVERSION NEEDED - use generic substitute
                            let instantiated = substitute_variable_generic(
                                &predicate, &var_name, &first, ctx.factory(),
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
                    GenericEvalStep::StartFoldlAtom { elements, init, acc_var_name, item_var_name, operation, env: step_env, depth } => {
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
                                &operation, &acc_var_name, &init, ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated, &item_var_name, &first, ctx.factory(),
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
                    GenericEvalStep::StartSortTuple { elements, var1_name, var2_name, comparator, env: step_env, depth } => {
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
                                &comparator, &var1_name, &current, ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated, &var2_name, &first, ctx.factory(),
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
                    GenericEvalStep::StartBestCandidate { elements, var_name, rank_fn, env: step_env, depth } => {
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
                                &rank_fn, &var_name, &first, ctx.factory(),
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
                    GenericEvalStep::EvalIfCondition { condition, then_branch, else_branch, env: step_env, depth } => {
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
                                demand: None,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        } else {
                            continuations.push(Continuation::ProcessIfCondition {
                                then_branch,
                                else_branch,
                                outer_bindings: None,
                                env: env.clone(),
                                depth,
                                outer_carrying: carrying_bindings.clone(),
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
                                demand: Some(crate::backend::eval::cesk::coroutine::Demand::Exactly(1)),
                                carrying_bindings: carrying_bindings.clone(),
                            });
                        }
                    }

                    // Evaluate case atom
                    GenericEvalStep::EvalCaseAtom { atom, cases, env: step_env, depth } => {
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
                    GenericEvalStep::SwitchAtom { atom, cases, env: step_env, depth } => {
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

                    // Evaluate eval
                    GenericEvalStep::EvalEval { arg, env: step_env, depth } => {
                        let env: SharedEnv = Arc::new(step_env);
                        continuations.push(Continuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
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

                    // Evaluate return
                    GenericEvalStep::EvalReturn { value, env: step_env, depth } => {
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
                    GenericEvalStep::StartChain { expr, var, body, env: step_env, depth } => {
                        let env: SharedEnv = Arc::new(step_env);
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
                    }

                    // Start function
                    GenericEvalStep::StartFunction { expr, env: step_env, depth } => {
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
                    GenericEvalStep::EvalIsError { expr, env: step_env, depth } => {
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
                    GenericEvalStep::StartCatch { expr, default, env: step_env, depth } => {
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

                    // Start conjunction
                    GenericEvalStep::StartConjunction { goals, env: step_env, depth } => {
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
                    GenericEvalStep::StartUnify { pattern1, pattern2, success_body, failure_body, env: step_env, depth } => {
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
                    GenericEvalStep::StartCollapse { expr, env: step_env, depth } => {
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
                    GenericEvalStep::StartCollapseBind { expr, env: step_env, depth } => {
                        let env: SharedEnv = Arc::new(step_env);

                        // Push a binding capture frame if the expression has free variables.
                        // These tracked variables will be captured at dispatch_rule_matches
                        // sites during the inner expression's evaluation.
                        if expr.has_variables_fast() {
                            let free_vars = expr.free_variables();
                            if !free_vars.is_empty() {
                                let tracked: SmallVec<[&'static str; 4]> = free_vars
                                    .into_iter()
                                    .collect();
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

                    // Start amb
                    GenericEvalStep::StartAmb { alternatives, env: step_env, depth } => {
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
                                alternatives.iter().any(|alt| {
                                    let (_, action) = scheduler.classify_and_transduce(alt);
                                    action.parallelism_degree > 1
                                })
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
                                #[cfg(feature = "eval-trace")]
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
                                let results = parallel_branch_eval(
                                    alternatives, metta_env, par_budget, current_depth,
                                );

                                work_stack.push(WorkItem::Resume {
                                    result: (results.into_iter().map(bv).collect(), env),
                                });
                            } else {
                                // ── Sequential path (original) ──
                                let mut alts_iter = alternatives.into_iter();
                                let amb_capacity = alts_iter.len(); // total before consuming first
                                let first = alts_iter.next().expect("alternatives is non-empty");

                                continuations.push(Continuation::ProcessAmb {
                                    remaining_alts: alts_iter,
                                    results: Vec::with_capacity(amb_capacity),
                                    env: env.clone(),
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
                    }

                    // Start guard
                    GenericEvalStep::StartGuard { condition, env: step_env, depth } => {
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
                    GenericEvalStep::StartGetAtoms { space_ref, env: step_env, depth } => {
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
                    GenericEvalStep::StartMemo { memo_ref, expr, first_only, env: step_env, depth } => {
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
                    GenericEvalStep::StartNewMemo { name_arg, size_arg, env: step_env, depth } => {
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
                    GenericEvalStep::StartMemoOp { memo_ref, op_type, env: step_env, depth } => {
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
                    GenericEvalStep::StartMatch { space_arg, pattern, template, env: step_env, depth } => {
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
                    GenericEvalStep::StartAddAtom { space_ref, atom, env: step_env, depth } => {
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
                    GenericEvalStep::StartRemoveAtom { space_ref, atom, env: step_env, depth } => {
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
                    GenericEvalStep::StartNewState { initial_value, env: step_env, depth } => {
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
                    GenericEvalStep::StartGetState { state_ref, env: step_env, depth } => {
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
                    GenericEvalStep::StartChangeState { state_ref, new_value, env: step_env, depth } => {
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
                    GenericEvalStep::StartRepr { atom, env: step_env, depth } => {
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
                    GenericEvalStep::StartFormatArgs { format_arg, args_arg, env: step_env, depth } => {
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
                    GenericEvalStep::StartPrintln { atom, env: step_env, depth } => {
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
                    GenericEvalStep::StartTrace { message, value_expr, env: step_env, depth } => {
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
                    GenericEvalStep::StartGetMetatype { atom, env: step_env, depth } => {
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
                    GenericEvalStep::StartBind { token, atom_expr, env: step_env, depth } => {
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
                    GenericEvalStep::EvalIfReducible { expr, then_branch, else_branch, env: step_env, depth } => {
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
                    GenericEvalStep::StartMatchOr { space_arg, pattern, default, template, env: step_env, depth } => {
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

                // Phase 2.4: Environment trimming — remove bindings not referenced
                // by the template. This reduces root set size at GC safepoints and
                // avoids carrying dead bindings through nested let* chains.
                //
                // CRITICAL: must compute the **transitive closure** of needed
                // variables. If the template uses $B, and $B is bound to
                // `(Inheritance $1 ...)`, then $1 must also be kept — otherwise
                // it will be lost when $B is later substituted (e.g., for
                // bidirectional unification cases where a rule variable is
                // bound to an expression containing a free input variable that
                // was bound by unification of a sibling occurrence).
                //
                // Algorithm: fixpoint expansion. Start with `template.free_variables()`,
                // then for each kept binding, add its value's free variables to
                // the needed set. Repeat until stable. Bounded by `bindings.len()`.
                let bindings = if bindings.len() > 1 {
                    let mut needed = template.free_variables();
                    // Fixpoint: expand needed set with transitively-referenced vars.
                    // Each iteration adds at least one new variable, so the loop
                    // terminates after at most `bindings.len()` iterations.
                    loop {
                        let prev_len = needed.len();
                        for (name, val) in bindings.iter() {
                            if needed.contains(&name) && val.has_variables_fast() {
                                let val_vars = val.free_variables();
                                for v in val_vars {
                                    if !needed.contains(&v) {
                                        needed.push(v);
                                    }
                                }
                            }
                        }
                        if needed.len() == prev_len {
                            break;
                        }
                    }
                    if needed.len() < bindings.len() {
                        let mut trimmed = crate::backend::models::GenericBindings::new();
                        for (name, val) in bindings.iter() {
                            if needed.contains(&name) {
                                trimmed.insert(name, val.clone());
                            }
                        }
                        Box::new(trimmed)
                    } else {
                        bindings
                    }
                } else {
                    bindings
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
                    let lookup = crate::backend::eval::cesk::with_thunk_table(|t| t.lookup(thunk_hash));
                    match lookup {
                        crate::backend::eval::cesk::ThunkLookup::Evaluated(cached) => {
                            work_stack.push(WorkItem::Resume {
                                result: (cached.into_iter().map(bv).collect(), env),
                            });
                            continue;
                        }
                        crate::backend::eval::cesk::ThunkLookup::Blackhole => {
                            // Infinite recursion detected — return error
                            let error_val = ctx.factory().error("blackhole", ctx.factory().atom("infinite recursion in EvalWithBindings"));
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
                                value: template, env, depth, is_tail_call, expected_type, demand: None,
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
                        || bindings.iter().any(|(_, val)| super::engine::binding_value_needs_eval(val))
                    {
                        let materialized = apply_bindings(&template, &bindings, ctx.factory());
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&template),
                                    vec![crate::backend::trace::trace_value_generic(&materialized)],
                                    None,
                                    trace_format::TraceEventKind::BindingsApplied {
                                        template: crate::backend::trace::trace_value_generic(&template),
                                        bindings: bindings.iter()
                                            .map(|(k, v)| (k.to_string(), crate::backend::trace::trace_value_generic(v)))
                                            .collect(),
                                        result: crate::backend::trace::trace_value_generic(&materialized),
                                    },
                                );
                            }
                        }
                        work_stack.push(WorkItem::Eval {
                            value: materialized, env, depth, is_tail_call, expected_type, demand: None,
                            carrying_bindings: carrying_bindings.clone(),
                        });
                        continue;
                    }

                    // ── `let` with lazy body: the key optimization ──
                    //
                    // For `(let pattern value_expr body)` with pending bindings B:
                    // 1. Materialize pattern and value_expr with B (needed immediately)
                    // 2. Keep body RAW + store B as outer_bindings on ProcessLet
                    // 3. When ProcessLet produces pattern-match bindings B2:
                    //    compose(B, B2) and push EvalWithBindings{body, compose(B, B2)}
                    //
                    // For nested let* of depth N, the body is never materialized
                    // until the innermost level, giving O(N) instead of O(N^2).
                    if resolved_head_atom == Some("let") && items.len() == 4 {
                        let pattern = apply_bindings(&items[1], &bindings, ctx.factory());
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
                            work_stack.push(WorkItem::EvalWithBindings {
                                template: branch_raw.clone(),
                                bindings,
                                env,
                                depth,
                                is_tail_call,
                                expected_type,
                                carrying_bindings: carrying_bindings.clone(),
                            });
                            continue;
                        }

                        continuations.push(Continuation::ProcessIfCondition {
                            then_branch: items[2].clone(), // RAW — not materialized
                            else_branch: items[3].clone(), // RAW — not materialized
                            outer_bindings: Some(bindings),
                            env: env.clone(),
                            depth,
                            outer_carrying: carrying_bindings.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: Some(ctx.factory().atom("Bool")),
                            demand: None,
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
                        let bindings_expr = apply_bindings(&items[1], &bindings, ctx.factory());
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
                            let mut pairs: Vec<(MettaValue, MettaValue)> = Vec::with_capacity(binding_pairs.len());
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
                            let materialized_value = apply_bindings(
                                &first_value_expr, &bindings, ctx.factory(),
                            );

                            // I-5: Enter region for let* scope
                            let region_id = crate::backend::eval::cesk::with_region_stack(|s| s.enter(depth as u32));
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
                    // 2. Keep `body` RAW + store B as outer_bindings on ProcessChainExpr
                    // 3. When expr resolves, compose {$var → result} into B and evaluate
                    //    body via EvalWithBindings{body, B ∪ {$var → result}}.
                    if resolved_head_atom == Some("chain") && items.len() == 4 {
                        let expr = apply_bindings(&items[1], &bindings, ctx.factory());

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
                    if let Some(chain_result) = try_deferred_deterministic_chain(
                        &template, &bindings, &env, ctx.factory(),
                    ) {
                        match chain_result {
                            DeferredChainResult::Deferred { template: new_template, bindings: new_bindings } => {
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: new_template,
                                    bindings: Box::new(new_bindings),
                                    env, depth, is_tail_call, expected_type,
                                    carrying_bindings: carrying_bindings.clone(),
                                });
                            }
                            DeferredChainResult::Concrete(value) => {
                                work_stack.push(WorkItem::Eval {
                                    value, env, depth, is_tail_call, expected_type, demand: None,
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
                                &template, &bindings, head_name, arity, &env, ctx.factory(),
                            ) {
                                if !matches.is_empty() {
                                    dispatch_rule_matches(matches, SmallVec::new(), (*env).clone(), depth, ctx, &mut work_stack, &mut continuations, None, &crate::backend::models::GenericBindings::new());
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
                        value: materialized, env, depth, is_tail_call, expected_type, demand: None,
                        carrying_bindings: carrying_bindings.clone(),
                    });
                    continue;
                }

                // Non-S-expression (type, conjunction, etc.): materialize
                let materialized = apply_bindings(&template, &bindings, ctx.factory());
                work_stack.push(WorkItem::Eval {
                    value: materialized, env, depth, is_tail_call, expected_type, demand: None,
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
    #[cfg(feature = "eval-trace")]
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
    #[cfg(feature = "eval-trace")]
    {
        crate::backend::trace::thread_local_sink::clear_thread_trace_collector();
    }

    // Return final result as EvalOutcome::Complete
    let (results, final_env) = final_result.unwrap_or_else(|| (SmallVec::new(), Arc::new(env)));
    crate::backend::eval::cesk::EvalOutcome::Complete(results, (*final_env).clone())
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
    deferred_shared_drops: &mut Vec<std::sync::Arc<
        crate::backend::environment::GenericEnvironmentShared<MettaValue>,
    >>,
) {
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
                // Stage 1d MERGE: compute the merged bindings from each
                // collected child's first result. The constructed sexpr's
                // bindings = merge of children's bindings. On conflict, the
                // sexpr has empty bindings (branch is inconsistent but we
                // preserve the value itself so the evaluator can proceed).
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
                    merged_bindings = crate::backend::models::GenericBindings::new();
                }

                // Use generic version - zero conversion needed!
                // Unwrap SharedEnv → bare MettaEnvironment for process_collected_sexpr_generic
                let collected_bare: Vec<(SmallVec<[MettaValue; 2]>, MettaEnvironment)> = collected
                    .into_iter()
                    .map(|(vals, shared_env)| (values_of(&vals), (*shared_env).clone()))
                    .collect();
                let processed = process_collected_sexpr_generic(collected_bare, (*original_env).clone(), depth, ctx.factory());

                match processed {
                    GenericProcessedSExpr::Done((results, env)) => {
                        let mb = merged_bindings.clone();
                        work_stack.push(WorkItem::Resume {
                            result: (
                                results.into_iter()
                                    .map(|v| (v, mb.clone()))
                                    .collect(),
                                Arc::new(env),
                            ),
                        });
                    }
                    GenericProcessedSExpr::EvalRuleMatches { matches, env, depth, base_results } => {
                        if matches.is_empty() {
                            let mb = merged_bindings.clone();
                            work_stack.push(WorkItem::Resume {
                                result: (
                                    base_results.into_iter()
                                        .map(|v| (v, mb.clone()))
                                        .collect(),
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
                                base_results.into_iter()
                                    .map(|v| (v, mb.clone()))
                                    .collect(),
                                env, depth, ctx, work_stack, continuations, None,
                                &mb,
                            );
                        }
                    }
                    GenericProcessedSExpr::EvalCombinations { combinations, env, depth } => {
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
                    GenericProcessedSExpr::RedispatchSExpr { items, env, depth: redispatch_depth } => {
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
            } else {
                let next = remaining.next().expect("remaining is non-empty");

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
                    carrying_bindings: outer_carrying.clone(),
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
            #[cfg(feature = "eval-trace")]
            branch_span_id,
            #[cfg(feature = "eval-trace")]
            branch_start_ns,
            #[cfg(feature = "eval-trace")]
            branch_index,
            #[cfg(feature = "eval-trace")]
            total_branches,
        } => {
            #[cfg(feature = "eval-trace")]
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
                result.0.into_iter().map(|(v, child_b)| {
                    let mut composed =
                        crate::backend::eval::bindings::compose_outer_inner_generic(
                            &*current_branch_bindings,
                            &child_b,
                            factory,
                        );
                    crate::backend::eval::bindings::apply_chain_generic(&mut composed, factory);
                    let projected = match &tracked_vars_hint {
                        Some(tv) => crate::backend::eval::bindings::project_bindings_generic(
                            &composed,
                            tv.as_slice(),
                        ),
                        None => composed,
                    };
                    (v, projected)
                }).collect()
            };
            results.extend(composed);

            // Trace: BranchEnd for the branch that just completed
            #[cfg(feature = "eval-trace")]
            {
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
                let (rhs, raw_bindings) = remaining_matches.next().expect("remaining_matches is non-empty");

                // Stage 1c: rotate to the next branch's match bindings so the
                // next COMPOSE_MATCH uses them. Each sibling branch has its
                // own independent bindings — no shared-mutable state.
                // Stage 1d-revised: re-compose with outer_carrying so next
                // branch's RHS results inherit the same ambient ancestry.
                *current_branch_bindings = if outer_carrying.is_empty() {
                    raw_bindings.clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying, &raw_bindings, ctx.factory(),
                    )
                };

                let bindings = Box::new(raw_bindings);

                // Trace: BranchStart for the next branch
                #[cfg(feature = "eval-trace")]
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
                    #[cfg(feature = "eval-trace")]
                    branch_span_id: _next_span_id,
                    #[cfg(feature = "eval-trace")]
                    branch_start_ns: _next_start_ns,
                    #[cfg(feature = "eval-trace")]
                    branch_index: _next_branch_index,
                    #[cfg(feature = "eval-trace")]
                    total_branches,
                });

                // Trace: RuleApplication (tree-walker, subsequent match)
                #[cfg(feature = "eval-trace")]
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
                        carrying_bindings: Box::new(rot_carrying),
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
                            carrying_bindings: Box::new(rot_carrying),
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
            let composed: SmallVec<[BoundValue; 2]> = if current_branch_bindings.is_empty()
                && tracked_vars_hint.is_none()
            {
                eval_results
            } else {
                let factory = ctx.factory();
                eval_results.into_iter().map(|(v, child_b)| {
                    let mut c =
                        crate::backend::eval::bindings::compose_outer_inner_generic(
                            &*current_branch_bindings,
                            &child_b,
                            factory,
                        );
                    crate::backend::eval::bindings::apply_chain_generic(&mut c, factory);
                    let projected = match &tracked_vars_hint {
                        Some(tv) => crate::backend::eval::bindings::project_bindings_generic(
                            &c,
                            tv.as_slice(),
                        ),
                        None => c,
                    };
                    (v, projected)
                }).collect()
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
                *current_branch_bindings = if outer_carrying.is_empty() {
                    bindings.clone()
                } else {
                    crate::backend::eval::bindings::compose_outer_inner_generic(
                        &*outer_carrying, &bindings, ctx.factory(),
                    )
                };

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
                        bindings: Box::new(bindings),
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                    });
                } else {
                    work_stack.push(WorkItem::Eval {
                        value: rhs,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
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
            mut arg_bindings,
        } => {
            let (result_values, result_env) = result;

            // Stage 1d-revised: collect per-branch bindings for the arg's
            // nondet results into a parallel `arg_branch_bindings` vector
            // (same order as the values). When the grounded op eventually
            // computes outputs via Cartesian product of per-arg results
            // (e.g. AddOp step 2 iterates `a_results × b_results`), we
            // re-tag each output with the MERGE of the specific (a_b, b_b)
            // pair's bindings. This matches HE's per-alternative bindings
            // model where each result is a `(value, bindings)` pair.
            //
            // For now, merge all branch bindings into arg_bindings (union of
            // compatible pieces; conflicts ignored). Per-combination tagging
            // requires grounded ops to return BoundValue — a separate
            // refactor (GroundedOp trait signature change) tracked in the
            // Stage 2 follow-up.
            for (_, child_b) in result_values.iter() {
                let _ = arg_bindings.merge(child_b);
            }

            // Set evaluated arg using the stored arg_idx from the EvalArg return
            state.set_arg(pending_arg_idx, result_values.into_iter().map(|(v, _)| v).collect());

            // Try static dispatch first - works with generic type V (NO conversion)
            let op_name = state.op_name.clone();
            if let Some(work) = execute_grounded_op(&op_name, &mut state, ctx.factory()) {
                match work {
                    GroundedWork::Done(results) => {
                        // Results are already in correct type V - NO conversion
                        let values: Vec<MettaValue> = results
                            .into_iter()
                            .map(|(v, _)| v)
                            .collect();
                        // Tag each grounded-op output with the merged arg bindings.
                        let merged = (*arg_bindings).clone();
                        work_stack.push(WorkItem::Resume {
                            result: (
                                values.into_iter()
                                    .map(|v| (v, merged.clone()))
                                    .collect(),
                                result_env,
                            ),
                        });
                    }
                    GroundedWork::EvalArg { arg_idx, state: new_state } => {
                        let arg_bindings_for_eval = arg_bindings.clone();
                        continuations.push(Continuation::ProcessGroundedOp {
                            state: Box::new(new_state.clone()),
                            pending_arg_idx: arg_idx,
                            env: result_env.clone(),
                            depth,
                            arg_bindings,
                        });

                        // Arg already in correct type V - NO conversion
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
                        let merged = (*arg_bindings).clone();
                        match e {
                            ExecError::NoReduce => {
                                // MeTTa HE semantics: return the original expression unreduced
                                let mut expr_parts = Vec::with_capacity(1 + state.args.len());
                                expr_parts.push(ctx.factory().atom(&state.op_name));
                                for arg in state.args.iter() {
                                    expr_parts.push(arg.clone());
                                }
                                let unreduced = ctx.factory().sexpr(expr_parts);
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![(unreduced, merged)], result_env),
                                });
                            }
                            _ => {
                                let error_value = match e {
                                    ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                                    ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                                    ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                                    ExecError::NoReduce => unreachable!(),
                                };
                                work_stack.push(WorkItem::Resume {
                                    result: (smallvec![(error_value, merged)], result_env),
                                });
                            }
                        }
                    }
                }
            } else {
                // Operation not in generic registry - report error
                // All 14 standard TCO operations are in the generic registry.
                let error_value = ctx.factory().error(
                    &format!("Grounded operation '{}' not found in generic registry", state.op_name),
                    ctx.factory().atom("OperationNotFoundError"),
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(error_value)], result_env),
                });
            }
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
                        bindings: Box::new(bindings),
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
                let all_matches_with_types = try_match_all_rules(&generic_sexpr, &result_env, *ctx.factory());

                if all_matches_with_types.is_empty() {
                    // No rule matches - expression is data
                    results.push(bv(generic_sexpr));

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
                    dispatch_rule_matches(matches_deque, SmallVec::new(), (*result_env).clone(), depth, ctx, work_stack, continuations, None, &crate::backend::models::GenericBindings::new());
                }
            } else {
                // All combinations processed - results already contains generic values
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

            match pending_values {
                None => {
                    // First resumption: result_values are values to pattern match

                    // Trace: value-result phase
                    #[cfg(feature = "eval-trace")]
                    {
                        if let Some(tc) = ctx.trace_collector() {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                depth as u32,
                                crate::backend::trace::trace_value_generic(&pattern),
                                result_values.iter().map(|(v, _)| crate::backend::trace::trace_value_generic(v)).collect(),
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
                    for (value, _b) in result_values.iter() {
                        // Phase 8.5: Type pre-check for typed patterns
                        if let Some(ref tc) = type_constraint {
                            if get_ground_type(value).is_some() {
                                let value_type = infer_type_generic(value, ctx.factory(), &result_env);
                                if !types_match_with_subtypes(&value_type, tc, &result_env) {
                                    continue;
                                }
                            }
                        }
                        if let Some(pm_bindings) = pattern_match(&pattern, value) {
                            if let Some(ref ob) = outer_bindings {
                                // Compose outer + pattern-match bindings, defer body
                                let composed = ob.compose(&pm_bindings);
                                bound_bodies.push(BoundBody::Deferred(composed));
                            } else {
                                // No outer bindings — materialize body as before
                                let instantiated = apply_bindings(&body, &pm_bindings, ctx.factory());
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
                                work_stack.push(WorkItem::Eval {
                                    value: val,
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: outer_carrying.clone(),
                                });
                            }
                            BoundBody::Deferred(composed_bindings) => {
                                work_stack.push(WorkItem::EvalWithBindings {
                                    template: body.clone(),
                                    bindings: Box::new(composed_bindings),
                                    env: result_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    carrying_bindings: outer_carrying.clone(),
                                });
                            }
                        }
                        return;
                    }

                    // Multiple matches: materialize all deferred bodies for dispatch
                    let instantiated_bodies: Vec<MettaValue> = bound_bodies.into_iter().map(|bb| {
                        match bb {
                            BoundBody::Materialized(val) => val,
                            BoundBody::Deferred(composed_bindings) => {
                                apply_bindings(&body, &composed_bindings, ctx.factory())
                            }
                        }
                    }).collect();

                    // ── Parallel path: evaluate all matched bodies concurrently ──
                    // When multiple values match, their body evaluations are
                    // independent (read-only env, no side effects). Dispatch to
                    // work pool for parallel evaluation.
                    // WFST classification: only parallelize when branches
                    // justify the dispatch overhead.
                    let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
                    let wfst_allows_match = if instantiated_bodies.len() >= 2 {
                        let scheduler = crate::backend::scheduler::global_scheduler();
                        instantiated_bodies.iter().any(|body| {
                            let (_, action) = scheduler.classify_and_transduce(body);
                            action.parallelism_degree > 1
                        })
                    } else {
                        false
                    };

                    let par_budget = if wfst_allows_match
                        && current_depth < max_parallel_depth()
                        && global_eval_pool().active_workers() > 0
                    {
                        try_acquire_budget(
                            (instantiated_bodies.len() - 1) as u32,
                            current_depth,
                        )
                    } else {
                        0
                    };

                    if par_budget > 0 {
                        let metta_env = (*result_env).clone();
                        let par_results = parallel_branch_eval(
                            instantiated_bodies, metta_env, par_budget, current_depth,
                        );

                        // Merge with accumulated results
                        let mut merged = results;
                        merged.extend(par_results.into_iter().map(bv));

                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::from_vec(merged), result_env),

                        });
                    } else {
                        // ── Sequential path: process bodies one at a time ──
                        // Use ProcessAmb continuation to evaluate instantiated bodies
                        // sequentially and merge their results. This avoids re-pattern-
                        // matching since bodies are already instantiated.
                        let mut bodies_iter = instantiated_bodies.into_iter();
                        let first_body = bodies_iter.next().expect("bodies is non-empty");

                        continuations.push(Continuation::ProcessAmb {
                            remaining_alts: bodies_iter,
                            results,
                            env: result_env.clone(),
                            depth,
                            outer_carrying: outer_carrying.clone(),
                        });

                        work_stack.push(WorkItem::Eval {
                            value: first_body,
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
                Some(mut remaining_values) => {
                    // Subsequent resumption: result_values are body evaluation results
                    // Add body results to collected results
                    results.extend(result_values);

                    // Try next value
                    loop {
                        match remaining_values.pop() {
                            Some((value, _b)) => {
                                // Phase 8.5: Type pre-check for typed patterns (: $var Type)
                                // Only apply to ground-type values (Number/Bool/String) where
                                // type inference is definitive. S-expressions and atoms may
                                // structurally match the pattern even if type inference says otherwise.
                                if let Some(ref tc) = type_constraint {
                                    if get_ground_type(&value).is_some() {
                                        let value_type = infer_type_generic(&value, ctx.factory(), &result_env);
                                        if !types_match_with_subtypes(&value_type, tc, &result_env) {
                                            continue; // Type mismatch — skip
                                        }
                                    }
                                }
                                if let Some(bindings) = pattern_match(&pattern, &value) {
                                    // Trace: pattern-match phase (subsequent resumption)
                                    #[cfg(feature = "eval-trace")]
                                    {
                                        if let Some(tc) = ctx.trace_collector() {
                                            tc.emit_converted(
                                                trace_format::TraceTier::TreeWalker,
                                                depth as u32,
                                                crate::backend::trace::trace_value_generic(&pattern),
                                                vec![crate::backend::trace::trace_value_generic(&value)],
                                                None,
                                                trace_format::TraceEventKind::SpecialForm {
                                                    form_name: "let".to_string(),
                                                    phase: "pattern-match".to_string(),
                                                },
                                            );
                                        }
                                    }

                                    // Restore continuation for collecting more results
                                    continuations.push(Continuation::ProcessLet {
                                        pending_values: Some(remaining_values),
                                        pattern,
                                        body: body.clone(),
                                        outer_bindings: outer_bindings.clone(),
                                        results,
                                        env: result_env.clone(),
                                        depth,
                                        outer_carrying: outer_carrying.clone(),
                                    });

                                    // Pattern matches - evaluate body with bindings
                                    if let Some(ref ob) = outer_bindings {
                                        let composed = ob.compose(&bindings);
                                        work_stack.push(WorkItem::EvalWithBindings {
                                            template: body,
                                            bindings: Box::new(composed),
                                            env: result_env,
                                            depth,
                                            is_tail_call: true,
                                            expected_type: None,
                                            carrying_bindings: outer_carrying.clone(),
                                        });
                                    } else {
                                        let instantiated_body = apply_bindings(&body, &bindings, ctx.factory());
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_body,
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
                                // Trace: pattern-no-match phase (subsequent resumption)
                                #[cfg(feature = "eval-trace")]
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
                let arg_expected_type = derive_arg_expected_type::<C>(
                    &items, arg_idx, &result_env, ctx.factory(),
                );

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
                    let mut combo_bindings: Vec<crate::backend::models::GenericBindings<MettaValue>> = Vec::new();
                    loop {
                        // Build this combination's items
                        let mut combo_items = items.clone();
                        // Merge bindings across chosen args. On conflict, skip
                        // this combination (empty bindings — combo still valid
                        // for value semantics, just no binding provenance).
                        let mut merged_b = crate::backend::models::GenericBindings::new();
                        let mut skip_combo = false;
                        for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                            let (ref v, ref b) = evaluated_results[i][combo_indices[i]];
                            combo_items[*grounded_idx] = v.clone();
                            if !merged_b.merge(b) {
                                // Conflicting bindings across args — this combo
                                // won't contribute consistent bindings, but the
                                // value is still evaluable.
                                skip_combo = true;
                                break;
                            }
                        }
                        combinations.push(ctx.factory().sexpr(combo_items));
                        combo_bindings.push(if skip_combo {
                            crate::backend::models::GenericBindings::new()
                        } else {
                            merged_b
                        });

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

                    // Stage 1d-revised: zip combinations with their
                    // per-combo bindings so each combination's evaluation
                    // carries the merged arg bindings.
                    let combos_with_b: Vec<(MettaValue, crate::backend::models::GenericBindings<MettaValue>)> =
                        combinations.into_iter().zip(combo_bindings.into_iter()).collect();
                    let mut combinations_iter = combos_with_b.into_iter();

                    if combinations_iter.len() == 1 {
                        let (sexpr, combo_b) = combinations_iter.next().expect("combinations is non-empty");
                        // Compose outer_carrying with the combo's merged arg bindings.
                        let combo_carrying = if outer_carrying.is_empty() {
                            combo_b
                        } else if combo_b.is_empty() {
                            (*outer_carrying).clone()
                        } else {
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying, &combo_b, ctx.factory(),
                            )
                        };

                        if !changed {
                            // Fixpoint: pre-evaluation didn't change any argument.
                            let all_matches_with_types = try_match_all_rules(
                                &sexpr, &result_env, *ctx.factory()
                            );

                            if !all_matches_with_types.is_empty() {
                                let matches_deque: Vec<_> =
                                    all_matches_with_types.into_iter()
                                        .map(|(rhs, bindings, _rhs_type)| (rhs, bindings))
                                        .collect();
                                dispatch_rule_matches(matches_deque, SmallVec::new(), (*result_env).clone(), depth, ctx, work_stack, continuations, None, &combo_carrying);
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
                                carrying_bindings: Box::new(combo_carrying),
                            });
                        }
                    } else {
                        // Multiple combinations — evaluate each and collect results.
                        let first_pair = combinations_iter.next().expect("combinations is non-empty");
                        let (first_sexpr, first_b) = first_pair;
                        let first_carrying = if outer_carrying.is_empty() {
                            first_b
                        } else if first_b.is_empty() {
                            (*outer_carrying).clone()
                        } else {
                            crate::backend::eval::bindings::compose_outer_inner_generic(
                                &*outer_carrying, &first_b, ctx.factory(),
                            )
                        };
                        let app_capacity = combinations_iter.len() + 1;
                        // Store the remaining combos + their bindings for
                        // CollectApplicativeResults to dispatch in order.
                        let remaining_vec: Vec<(MettaValue, crate::backend::models::GenericBindings<MettaValue>)> =
                            combinations_iter.collect();
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
                            remaining_vec.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>().into_iter();
                        let remaining_pairs_bindings: Vec<crate::backend::models::GenericBindings<MettaValue>> =
                            remaining_vec.iter().map(|(_, b)| b.clone()).collect();

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
                            carrying_bindings: Box::new(first_carrying),
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
            if std::env::var("MTN_DEBUG_CAR").is_ok() {
                eprintln!("[CAR] received {} results, remaining={}, accumulated results={}",
                    result_values.len(), remaining.len(), results.len());
            }
            results.extend(result_values);

            if remaining.len() == 0 {
                // All combinations evaluated — resume parent with collected results
                if std::env::var("MTN_DEBUG_CAR").is_ok() {
                    eprintln!("[CAR] final: {} results", results.len());
                }
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
                        &*outer_carrying, &next_b, ctx.factory(),
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
                    carrying_bindings: Box::new(combo_carrying),
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
        } => {
            let (mut result_values, result_env) = result;

            // Add first result from evaluation
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
                collected_results.push(first_result);
            }

            if remaining_elements.len() == 0 {
                // All elements processed - return result list
                let result_list = ctx.factory().sexpr(
                    collected_results.into_iter().map(|(v, _)| v).collect()
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.next().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &template, &var_name, &next_element, ctx.factory(),
                );

                continuations.push(Continuation::ProcessMapAtom {
                    remaining_elements,
                    var_name,
                    template,
                    collected_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
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
        } => {
            let (mut result_values, result_env) = result;

            // Check predicate result and optionally include current element
            if !result_values.is_empty() {
                let (first_result, _b) = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(first_result)], result_env),
                    });
                    return;
                }

                let should_include = if let Some(b) = first_result.as_bool() {
                    b
                } else {
                    !first_result.is_unit()
                };

                if should_include {
                    if let Some(elem) = current_element {
                        filtered_results.push(bv(elem));
                    }
                }
            }

            if remaining_elements.len() == 0 {
                // All elements processed - return filtered list
                let result_list = ctx.factory().sexpr(
                    filtered_results.into_iter().map(|(v, _)| v).collect()
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.next().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &predicate, &var_name, &next_element, ctx.factory(),
                );

                continuations.push(Continuation::ProcessFilterAtom {
                    current_element: Some(next_element),
                    remaining_elements,
                    var_name,
                    predicate,
                    filtered_results,
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        Continuation::ProcessFoldlAtom {
            mut remaining_elements,
            acc_var_name,
            item_var_name,
            operation,
            env: _,
            depth,
            mut acc_bindings,
        } => {
            let (mut result_values, result_env) = result;

            // Get the new accumulator value + bindings from the result.
            // Stage 1d MERGE: the child's bindings (from inner rule dispatches)
            // are merged into acc_bindings; on conflict, emit zero results
            // (the fold branch is inconsistent).
            let (accumulator, new_acc_bindings) = if result_values.is_empty() {
                (ctx.factory().unit(), (*acc_bindings).clone())
            } else {
                let (first_result, child_b) = result_values.swap_remove(0);

                // Check for error propagation (bindings come along too)
                if first_result.is_error() {
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![(first_result, child_b)], result_env),
                    });
                    return;
                }

                // MERGE child bindings into accumulator bindings.
                let mut merged = (*acc_bindings).clone();
                if !merged.merge(&child_b) {
                    // Conflict — drop the fold branch (zero results).
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                    return;
                }
                (first_result, merged)
            };

            if remaining_elements.len() == 0 {
                // All elements processed - return final accumulator with the
                // merged bindings accumulated across every iteration.
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![(accumulator, new_acc_bindings)], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.next().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &operation, &acc_var_name, &accumulator, ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated, &item_var_name, &next_element, ctx.factory(),
                );

                // Update acc_bindings for the next iteration.
                *acc_bindings = new_acc_bindings;
                let acc_bindings_for_eval = acc_bindings.clone();
                continuations.push(Continuation::ProcessFoldlAtom {
                    remaining_elements,
                    acc_var_name,
                    item_var_name,
                    operation,
                    env: result_env.clone(),
                    depth,
                    acc_bindings,
                });

                work_stack.push(WorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: acc_bindings_for_eval,
                });
            }
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
            let cmp_true = cmp_results.first()
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
                        &comparator, &var1_name, &current, ctx.factory(),
                    );
                    let instantiated = substitute_variable_generic(
                        &instantiated, &var2_name, &sorted[next_pos], ctx.factory(),
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
                    &comparator, &var1_name, &next_current, ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated, &var2_name, &sorted[0], ctx.factory(),
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
            let current_rank = rank_results.first().and_then(|(v, _)| {
                v.as_float().or_else(|| v.as_long().map(|l| l as f64))
            });

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

                let instantiated = substitute_variable_generic(
                    &rank_fn, &var_name, &next, ctx.factory(),
                );

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
        } => {
            let (cond_results, env_after_cond) = result;

            if let Some((first, _b)) = cond_results.first() {
                // Trace: condition-result phase
                #[cfg(feature = "eval-trace")]
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
                        #[cfg(feature = "eval-trace")]
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
                        #[cfg(feature = "eval-trace")]
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
                    // Phase C: If outer_bindings present, defer materialization
                    // of the taken branch via EvalWithBindings.
                    if let Some(ob) = outer_bindings {
                        if branch.has_variables_fast() {
                            work_stack.push(WorkItem::EvalWithBindings {
                                template: branch,
                                bindings: ob,
                                env: env_after_cond,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        } else {
                            work_stack.push(WorkItem::Eval {
                                value: branch,
                                env: env_after_cond,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
                            });
                        }
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: branch,
                            env: env_after_cond,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    // Non-boolean (including Unit) → return unreduced (if cond then else)
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
                    #[cfg(feature = "eval-trace")]
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
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&cases),
                        atom_results.iter().map(|(v, _)| crate::backend::trace::trace_value_generic(v)).collect(),
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: "case".to_string(),
                            phase: "scrutinee-result".to_string(),
                        },
                    );
                }
            }

            // Filter out Empty sentinels
            let filtered_results: Vec<_> = atom_results
                .into_iter()
                .filter(|(v, _)| !v.is_empty())
                .collect();

            // Handle case when evaluation returns no results
            if filtered_results.is_empty() {
                // Match Empty against cases - NO conversion needed
                let empty_atom = ctx.factory().atom("Empty");
                match eval_switch(&empty_atom, &cases, ctx.factory()) {
                    SwitchResult::Match(template, _bindings) => {
                        // Template needs evaluation
                        work_stack.push(WorkItem::Eval {
                            value: template,
                            env: atom_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
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
            let (first_raw, _b) = remaining_raw.next().expect("filtered_results is non-empty");

            continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                remaining_raw,
                evaluated: vec![],
                cases,
                env: atom_env.clone(),
                depth,
                current_raw_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                outer_carrying: outer_carrying.clone(),
            });

            work_stack.push(WorkItem::Eval {
                value: first_raw,
                env: atom_env,
                depth: depth + 1,
                is_tail_call: false,
                expected_type: None,
                demand: None,
                carrying_bindings: outer_carrying.clone(),
            });
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

            if let Some(next_atom) = remaining_atoms.next() {
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
                        let filtered: Vec<MettaValue> = case_pairs.iter().filter(|pair| {
                            pair.as_sexpr().map_or(true, |p| {
                                p.first().map_or(true, |pattern| {
                                    is_pattern_type_compatible(pattern, scrutinee_type)
                                })
                            })
                        }).cloned().collect();
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

                        work_stack.push(WorkItem::Eval {
                            value: template,
                            env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
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

            // Collect non-empty evaluated results
            evaluated.extend(eval_results.into_iter().filter(|(v, _)| !v.is_empty()));

            if let Some((next_raw, _b)) = remaining_raw.next() {
                // More raw scrutinee results to evaluate — reuse cont slot
                continuations.push(Continuation::ProcessCaseEvalScrutineeResults {
                    remaining_raw,
                    evaluated,
                    cases,
                    env: eval_env.clone(),
                    depth,
                    current_raw_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: next_raw,
                    env: eval_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // All raw results evaluated — now perform pattern matching
                if evaluated.is_empty() {
                    // All evaluations produced empty — match Empty against cases
                    let empty_atom = ctx.factory().atom("Empty");
                    match eval_switch(&empty_atom, &cases, ctx.factory()) {
                        SwitchResult::Match(template, _bindings) => {
                            work_stack.push(WorkItem::Eval {
                                value: template,
                                env: eval_env,
                                depth,
                                is_tail_call: true,
                                expected_type: None,
                                demand: None,
                                carrying_bindings: outer_carrying.clone(),
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

                // Match each evaluated result against cases
                let mut eval_atoms = evaluated.into_iter().map(|(v, _)| v).collect::<Vec<_>>().into_iter();

                if let Some(first_atom) = eval_atoms.next() {
                    let is_empty_atom = first_atom.is_empty()
                        || first_atom.as_sexpr().map_or(false, |items| items.is_empty());
                    let switch_atom = if is_empty_atom {
                        ctx.factory().atom("Empty")
                    } else {
                        first_atom
                    };

                    match eval_switch(&switch_atom, &cases, ctx.factory()) {
                        SwitchResult::Match(template, _bindings) => {
                            // Trace: case-match phase
                            #[cfg(feature = "eval-trace")]
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

                            if eval_atoms.len() == 0 {
                                work_stack.push(WorkItem::Eval {
                                    value: template,
                                    env: eval_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: outer_carrying.clone(),
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

                                work_stack.push(WorkItem::Eval {
                                    value: template,
                                    env: eval_env,
                                    depth,
                                    is_tail_call: true,
                                    expected_type: None,
                                    demand: None,
                                    carrying_bindings: outer_carrying.clone(),
                                });
                            }
                        }
                        SwitchResult::Error(err) => {
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(err)], eval_env),
                            });
                        }
                        SwitchResult::NoMatch => {
                            // Trace: case-no-match phase
                            #[cfg(feature = "eval-trace")]
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
                let (mut value, _b) = eval_results.into_iter().next().unwrap();
                if let Some(inner) = value.as_quoted() {
                    value = inner;
                }
                work_stack.push(WorkItem::Eval {
                    value,
                    env: result_env,
                    depth,
                    is_tail_call: true,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                // Multiple results - evaluate each (unwrap Quoted values)
                let results_vec: Vec<_> = eval_results.into_iter().map(|(v, _)| {
                    if let Some(inner) = v.as_quoted() { inner } else { v }
                }).collect();
                let mut results_iter = results_vec.into_iter();
                let amb_capacity = results_iter.len(); // total before consuming first
                let first = results_iter.next().unwrap();

                continuations.push(Continuation::ProcessAmb {
                    remaining_alts: results_iter,
                    results: Vec::with_capacity(amb_capacity),
                    env: result_env.clone(),
                    depth,
                    outer_carrying: outer_carrying.clone(),
                });

                work_stack.push(WorkItem::Eval {
                    value: first,
                    env: result_env,
                    depth,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            }
        }

        Continuation::ProcessReturn {
            env: _,
            depth: _,
            outer_carrying,
        } => {
            let (arg_results, arg_env) = result;

            // Check for errors first - pass through without wrapping
            if let Some((err, _b)) = arg_results.iter().find(|(r, _)| r.is_error()) {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err.clone())], arg_env),
                });
            } else {
                // Wrap results in return structure: (return value)
                let return_results: Vec<MettaValue> = arg_results
                    .into_iter()
                    .map(|(r, _)| {
                        ctx.factory().sexpr(vec![
                            ctx.factory().atom("return"),
                            r,
                        ])
                    })
                    .collect();
                work_stack.push(WorkItem::Resume {
                    result: (return_results.into_iter().map(bv).collect(), arg_env),

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
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if expr_results.is_empty() { "expr-empty" } else { "expr-result" };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&var),
                        expr_results.iter().map(|(v, _)| crate::backend::trace::trace_value_generic(v)).collect(),
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
                // Single result - substitute and evaluate body
                let var_name = var.as_atom().unwrap_or("");
                // Phase C: Compose chain variable binding with outer_bindings
                // and defer materialization via EvalWithBindings.
                if let Some(mut ob) = outer_bindings {
                    ob.insert(var_name, expr_results[0].0.clone());
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env: result_env,
                            depth,
                            is_tail_call: true,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    let instantiated = substitute_variable_generic(
                        &body,
                        var_name,
                        &expr_results[0].0,
                        ctx.factory(),
                    );
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                }
            } else {
                // Multiple results - chain evaluates each
                let mut remaining_values = expr_results.into_iter().map(|(v, _)| v).collect::<Vec<_>>().into_iter();
                let chain_capacity = remaining_values.len(); // total before consuming first
                let first = remaining_values.next().unwrap();

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

                // Phase C: Compose chain variable binding with outer_bindings
                let var_name = var.as_atom().unwrap_or("");
                if let Some(mut ob) = outer_bindings {
                    ob.insert(var_name, first);
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env: result_env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    let instantiated = substitute_variable_generic(
                        &body,
                        var_name,
                        &first,
                        ctx.factory(),
                    );
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
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

            if let Some(next_value) = remaining_values.next() {
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

                // Phase C: Compose chain variable binding with outer_bindings
                let var_name = var.as_atom().unwrap_or("");
                if let Some(mut ob) = outer_bindings {
                    ob.insert(var_name, next_value);
                    if body.has_variables_fast() {
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: ob,
                            env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    } else {
                        work_stack.push(WorkItem::Eval {
                            value: body,
                            env,
                            depth,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: outer_carrying.clone(),
                        });
                    }
                } else {
                    let instantiated = substitute_variable_generic(
                        &body,
                        var_name,
                        &next_value,
                        ctx.factory(),
                    );
                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env,
                        depth,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
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

            if eval_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().unit())], current_env),
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
                let (final_results, continue_exprs): (Vec<_>, Vec<_>) =
                    eval_results.into_iter().partition(|(r, _)| is_return_expr(r));

                if !final_results.is_empty() {
                    // Extract return values - unwrap (return value) to just value
                    let returns: Vec<MettaValue> = final_results
                        .into_iter()
                        .map(|(r, _)| {
                            if let Some(items) = r.as_sexpr() {
                                items[1].clone()
                            } else {
                                r // shouldn't happen, but be safe
                            }
                        })
                        .collect();
                    work_stack.push(WorkItem::Resume {
                        result: (returns.into_iter().map(bv).collect(), current_env),

                    });
                } else if continue_exprs.is_empty() {
                    // Nothing to continue
                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], current_env),
                    });
                } else if iteration_count >= MAX_ITERATIONS {
                    // Hit iteration limit
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::from_vec(continue_exprs), current_env),

                    });
                } else {
                    // Continue evaluating
                    if continue_exprs.len() == 1 {
                        let (next_expr, _b) = continue_exprs.into_iter().next().unwrap();
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
                    } else {
                        // Multiple continue expressions - just return them
                        // (more complex handling would evaluate each, but this matches heap engine)
                        work_stack.push(WorkItem::Resume {
                            result: (SmallVec::from_vec(continue_exprs), current_env),

                        });
                    }
                }
            }
        }

        Continuation::ProcessIsError {
            env: _,
            depth: _,
            outer_carrying,
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
                let error = goal_results.into_iter().find(|(v, _)| v.is_error()).unwrap();
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
                let final_result = accumulated_results.pop().unwrap_or_else(|| bv(ctx.factory().unit()));
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
                // Single result - check if it's a Space (special handling)
                let (val1, _b) = pattern1_results.into_iter().next().unwrap();

                if let Some(handle) = val1.as_space() {
                    // Space unification - match pattern2 against space atoms
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
                                crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern2, atom).is_some()
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
                            let matches: Vec<(MettaValue, usize)> =
                                result_env.match_space(&pattern2, &pattern2)
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
                                // Build bodies to evaluate for each match - values already generic
                                let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                                let mut found_match = false;
                                for (generic_value, count) in &matches {
                                    if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern2, generic_value) {
                                        found_match = true;
                                        let generic_body = apply_bindings(&success_body, &bindings, ctx.factory());
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
                                // Build bodies to evaluate for each match - NO conversion needed
                                let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                                let mut found_match = false;
                                for m in &matches {
                                    if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern2, &m.value) {
                                        found_match = true;
                                        let generic_body = apply_bindings(&success_body, &bindings, ctx.factory());
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
                    // Non-space: evaluate pattern2
                    continuations.push(Continuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
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
            } else {
                // Multiple results - iterate over them
                let remaining_vec: Vec<_> = pattern1_results.into_iter().map(|(v, _)| v).collect();
                let mut remaining = remaining_vec.into_iter();
                let iter_capacity = remaining.len(); // total before consuming first
                let first = remaining.next().unwrap();

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
                        let matches: Vec<(MettaValue, usize)> =
                            result_env.match_space(&pattern2, &pattern2)
                                .into_iter()
                                .map(|m| (m.value, m.count))
                                .collect();

                        // Build bodies for matches - values already generic
                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for (generic_value, count) in &matches {
                            if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern2, generic_value) {
                                found_match = true;
                                let generic_body = apply_bindings(&success_body, &bindings, ctx.factory());
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

                        // Build bodies for matches - NO conversion needed
                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern2, &m.value) {
                                found_match = true;
                                let generic_body = apply_bindings(&success_body, &bindings, ctx.factory());
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

            // Get next pattern1 value to process
            if let Some(val1) = remaining_pattern1_results.next() {
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
                                crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern, atom).is_some()
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
                                    .map(|m| GenericMultiplicityMatch { value: m.value, count: m.count })
                                    .collect()
                            } else {
                                // Non-module spaces - use collapse_with_multiplicity_generic
                                handle.collapse_with_multiplicity_generic(ctx.factory())
                            };

                        let mut bodies_to_eval: Vec<MettaValue> = Vec::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&pattern, &m.value) {
                                found_match = true;
                                let instantiated =
                                    apply_bindings(&success_body, &bindings, ctx.factory());
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
                // WAM union-find bidirectional unification: handles variables
                // on both sides, occurs check, and conflict detection.
                let mut all_bindings = Vec::new();
                for (p2_result, _b) in &pattern2_results {
                    if let Some(bindings) = crate::backend::eval::trampoline::unification::bidirectional_unify(&val1, p2_result) {
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
                    // Apply bindings generically - NO conversion needed
                    let instantiated = apply_bindings(&success_body, &all_bindings[0], ctx.factory());

                    work_stack.push(WorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        is_tail_call: true,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: outer_carrying.clone(),
                    });
                } else {
                    // Multiple bindings - pre-instantiate all bodies generically
                    let bodies_vec: Vec<MettaValue> = all_bindings.iter()
                        .map(|bindings| {
                            apply_bindings(&success_body, bindings, ctx.factory())
                        })
                        .collect();
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
                        carrying_bindings: outer_carrying.clone(),
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
            let par_budget = if n_results >= PARALLEL_COLLAPSE_THRESHOLD
                && current_depth < max_parallel_depth()
                && global_eval_pool().active_workers() > 0
            {
                try_acquire_budget((n_results - 1) as u32, current_depth)
            } else {
                0
            };

            if par_budget > 0 {
                let metta_items: Vec<MettaValue> = expr_results.into_iter().map(|(v, _)| v).collect();
                let metta_env = (*result_env).clone();

                let evaluated = parallel_collapse_eval(
                    metta_items, metta_env, par_budget, current_depth, depth,
                );

                // Assemble the tuple
                let result_list = ctx.factory().sexpr(evaluated);

                // Trace: collapse-result phase
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&result_list),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "collapse".to_string(),
                                phase: "collapse-result-parallel".to_string(),
                            },
                        );
                    }
                }

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
            } else {
                // ── Sequential path: evaluate one-at-a-time ──
                // MeTTa HE collapse semantics: evaluate each result to normal form.
                let remaining_vec: Vec<BoundValue> = expr_results.into_iter().collect();
                let mut remaining_raw = remaining_vec.into_iter();
                let collapse_capacity = remaining_raw.len(); // total before consuming first
                let (first_raw, first_raw_b) = remaining_raw.next().expect("expr_results is non-empty");

                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated: Vec::with_capacity(collapse_capacity),
                    is_bind: false,
                    current_raw_bindings: Box::new(first_raw_b),
                    env: result_env.clone(),
                    depth,
                });

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

            // Stage 1b+: per-result bindings now travel with each BoundValue
            // (expr_results[i].1), so there is nothing to extract from the
            // capture frame — it's now a pure scope marker.
            //
            // Force sequential flag retained until Stage 1e: parallel paths
            // don't yet thread tracked_vars_hint into workers, so we stay
            // sequential whenever a scope marker was active.
            let force_sequential = captured_frame.is_some();
            let _ = captured_frame;

            // ── Parallel path: identical to ProcessCollapse ──
            let current_depth = PARALLEL_BRANCH_DEPTH.with(|d| d.get());
            let par_budget = if !force_sequential
                && expr_results.len() >= PARALLEL_COLLAPSE_THRESHOLD
                && current_depth < max_parallel_depth()
                && global_eval_pool().active_workers() > 0
            {
                try_acquire_budget((expr_results.len() - 1) as u32, current_depth)
            } else {
                0
            };

            if par_budget > 0 {
                let metta_items: Vec<MettaValue> = expr_results.into_iter().map(|(v, _)| v).collect();
                let metta_env = (*result_env).clone();

                let evaluated = parallel_collapse_eval(
                    metta_items, metta_env, par_budget, current_depth, depth,
                );

                let result_list = ctx.factory().sexpr(evaluated);

                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&result_list),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: "collapse-bind".to_string(),
                                phase: "collapse-result-parallel".to_string(),
                            },
                        );
                    }
                }

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(result_list)], result_env),
                });
            } else {
                // ── Sequential path ──
                let remaining_vec: Vec<BoundValue> = expr_results.into_iter().collect();
                let mut remaining_raw = remaining_vec.into_iter();
                let collapse_capacity = remaining_raw.len(); // total before consuming first
                let (first_raw, first_raw_bindings) =
                    remaining_raw.next().expect("expr_results is non-empty");

                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated: Vec::with_capacity(collapse_capacity),
                    is_bind: true,
                    current_raw_bindings: Box::new(first_raw_bindings),
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(WorkItem::Eval {
                    value: first_raw,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
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
        } => {
            let (eval_results, result_env) = result;

            // Stage 1e MERGE: merge the raw's original bindings with each
            // re-eval result's bindings. The re-eval typically produces the
            // same value for ground raws (no new bindings), but in case the
            // raw was a further-evaluable expression, both sources are
            // combined. Filter empty (pruned) branches.
            let carrying = (*current_raw_bindings).clone();
            evaluated.extend(
                eval_results.into_iter()
                    .filter(|(v, _)| !v.is_empty())
                    .map(|(v, child_b)| {
                        let mut merged = carrying.clone();
                        // If merge conflicts, keep the carrying bindings as
                        // the branch's identity (raw was the producer).
                        let _ = merged.merge(&child_b);
                        (v, merged)
                    })
            );

            if let Some((next_raw, next_raw_bindings)) = remaining_raw.next() {
                // More results to evaluate — preserve state.
                *current_raw_bindings = next_raw_bindings;
                let current_raw_bindings_for_eval = current_raw_bindings.clone();
                continuations.push(Continuation::ProcessCollapseEvalResults {
                    remaining_raw,
                    evaluated,
                    is_bind,
                    current_raw_bindings,
                    env: result_env.clone(),
                    depth,
                });

                work_stack.push(WorkItem::Eval {
                    value: next_raw,
                    env: result_env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: current_raw_bindings_for_eval,
                });
            } else {
                // All results evaluated — assemble the tuple
                let result_list = if is_bind {
                    // collapse-bind: wrap each result as (result (Bindings ($var val) ...))
                    // Use per-result bindings from each BoundValue.
                    let pairs: Vec<MettaValue> = evaluated.into_iter().map(|(result_val, bindings)| {
                        let bindings_sexpr = encode_bindings_as_sexpr(&bindings, ctx.factory());
                        ctx.factory().sexpr(vec![result_val, bindings_sexpr])
                    }).collect();
                    ctx.factory().sexpr(pairs)
                } else {
                    ctx.factory().sexpr(evaluated.into_iter().map(|(v, _)| v).collect())
                };

                // Trace: collapse-result phase
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&result_list),
                            vec![],
                            None,
                            trace_format::TraceEventKind::SpecialForm {
                                form_name: if is_bind { "collapse-bind" } else { "collapse" }.to_string(),
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

            if let Some(next_alt) = remaining_alts.next() {
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

                work_stack.push(WorkItem::Eval {
                    value: next_alt,
                    env,
                    depth: depth + 1,
                    is_tail_call: false,
                    expected_type: None,
                    demand: None,
                    carrying_bindings: outer_carrying.clone(),
                });
            } else {
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::from_vec(results), result_env),
                });
            }
        }

        Continuation::ProcessGuard {
            env: _,
            depth: _,
            outer_carrying,
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
                        &format!(
                            "guard: condition must evaluate to Bool, got {}",
                            v.friendly_repr()
                        ),
                        v.clone(),
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
            outer_carrying,
        } => {
            let (space_results, result_env) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "get-atoms: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], result_env),
                });
            } else {
                let (first, _) = &space_results[0];
                if let Some(handle) = first.as_space() {
                    // GENERIC: Use collapse_generic to avoid heap conversion
                    let atoms: Vec<MettaValue> = handle.collapse_generic(ctx.factory());
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
                        &format!("get-atoms: first argument must be a space, got {}", first.friendly_repr()),
                        first.clone(),
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
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "match: space evaluated to empty",
                    space_arg,
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                if let Some(handle) = first.as_space() {
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
                                        let results: Vec<MettaValue> = matching_atoms.iter()
                                            .map(|name| {
                                                let mut bindings = crate::backend::models::GenericBindings::new();
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

                        let generic_results: Vec<MettaValue> = if let Some(filtered) = type_filtered {
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
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    generic_results.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
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
                        #[cfg(feature = "eval-trace")]
                        {
                            if let Some(tc) = ctx.trace_collector() {
                                tc.emit_converted(
                                    trace_format::TraceTier::TreeWalker,
                                    depth as u32,
                                    crate::backend::trace::trace_value_generic(&pattern),
                                    instantiated_templates.iter().map(|v| crate::backend::trace::trace_value_generic(v)).collect(),
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
                        &format!(
                            "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "add-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                if let Some(handle) = first.as_space() {
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
                        // Invalidate match_result_cache and eval_memo — cached results
                        // may reference patterns that now have new matches.
                        clear_match_result_cache();
                        clear_eval_memo();
                    }
                    increment_mutation_epoch();

                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (space_results, mut env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "remove-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                let (first, _) = &space_results[0];
                if let Some(handle) = first.as_space() {
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
                        clear_match_result_cache();
                        clear_eval_memo();
                    }
                    increment_mutation_epoch();

                    work_stack.push(WorkItem::Resume {
                        result: (smallvec![bv(ctx.factory().unit())], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (init_results, mut env_after) = result;

            if init_results.is_empty() {
                let err = ctx.factory().error(
                    "new-state: initial value evaluated to empty",
                    initial_value,
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
            outer_carrying,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    "get-state: state reference evaluated to empty",
                    state_ref,
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
                            &format!("get-state: state {} not found", state_id),
                            first.clone(),
                        );
                        work_stack.push(WorkItem::Resume {
                            result: (smallvec![bv(err)], env_after),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "get-state: argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
                    "change-state!: state reference evaluated to empty",
                    state_ref,
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
                        &format!(
                            "change-state!: first argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (value_results, mut env_after) = result;

            if value_results.is_empty() {
                let err = ctx.factory().error(
                    "change-state!: new value evaluated to empty",
                    new_value,
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
                        "change-state!: expected state value",
                        state_value,
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
            outer_carrying,
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
                    "format-args: format string evaluated to empty",
                    format_arg,
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
                        &format!(
                            "format-args: first argument must be a string, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (args_results, env_after) = result;

            if args_results.is_empty() {
                let err = ctx.factory().error(
                    "format-args: args evaluated to empty",
                    args_arg,
                );
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(err)], env_after),
                });
            } else {
                // Get args as a list - use native generic values directly
                let args_list: Vec<&MettaValue> = if let Some(items) = args_results[0].0.as_sexpr() {
                    items.iter().collect()
                } else {
                    args_results.iter().map(|(v, _)| v).collect()
                };

                // Simple format string substitution using friendly_repr
                let mut result_str = format_str.clone();
                for (i, arg) in args_list.iter().enumerate() {
                    let placeholder = format!("{{{}}}", i);
                    let repr = arg.friendly_repr();
                    result_str = result_str.replace(&placeholder, &repr);
                }

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().string(&result_str))], env_after),
                });
            }
        }

        Continuation::ProcessPrintln {
            atom: _,
            env: _,
            depth: _,
            outer_carrying,
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
            outer_carrying,
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
            outer_carrying,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().atom("Undefined"))], env_after),
                });
            } else {
                let (first, _) = &atom_results[0];
                let metatype = match first.inner_raw() {
                    MettaValueInner::Quoted(_) | MettaValueInner::SExpr(_) => "Expression",
                    MettaValueInner::Atom(s) if is_variable_str(s) => "Variable",
                    MettaValueInner::Atom(_) => "Symbol",
                    MettaValueInner::Bool(_) | MettaValueInner::Long(_)
                    | MettaValueInner::Float(_) | MettaValueInner::String(_) => "Grounded",
                    MettaValueInner::Error(..) => "Error",
                    MettaValueInner::Spanned(..) => {
                        let stripped = first.strip_one_span();
                        // Re-dispatch on the stripped value
                        match stripped.inner_raw() {
                            MettaValueInner::Quoted(_) | MettaValueInner::SExpr(_) => "Expression",
                            MettaValueInner::Atom(s) if is_variable_str(s) => "Variable",
                            MettaValueInner::Atom(_) => "Symbol",
                            MettaValueInner::Bool(_) | MettaValueInner::Long(_)
                            | MettaValueInner::Float(_) | MettaValueInner::String(_) => "Grounded",
                            MettaValueInner::Error(..) => "Error",
                            _ => "Undefined",
                        }
                    }
                    _ => "Undefined",
                };

                work_stack.push(WorkItem::Resume {
                    result: (smallvec![bv(ctx.factory().atom(metatype))], env_after),
                });
            }
        }

        Continuation::ProcessBind {
            token,
            env: _,
            depth: _,
            outer_carrying,
        } => {
            let (atom_results, mut env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    "bind!: atom evaluated to empty",
                    ctx.factory().atom(&token),
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
            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let phase = if is_irreducible { "irreducible" } else { "reduced" };
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&original_expr),
                        eval_results.iter().map(|(v, _)| crate::backend::trace::trace_value_generic(v)).collect(),
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
                #[cfg(feature = "eval-trace")]
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
                            #[cfg(feature = "eval-trace")]
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
                            #[cfg(feature = "eval-trace")]
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
                                value: instantiated_templates.into_iter().next().expect("non-empty"),
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
                        &format!(
                            "match-or: first argument must be a space, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
                    "memo/memo!: memo reference evaluated to empty",
                    memo_ref,
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
                        &format!(
                            "memo/memo!: first argument must be a memo table, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            outer_carrying,
        } => {
            let (expr_results, env_after) = result;

            // Cache the result using generic store
            if first_only && !expr_results.is_empty() {
                let slice = &expr_results[..1]; let _vals: Vec<MettaValue> = slice.iter().map(|(v, _)| v.clone()).collect(); memo_handle.store_generic(&expr, &_vals);
            } else {
                { let _vals: Vec<MettaValue> = expr_results.iter().map(|(v, _)| v.clone()).collect(); memo_handle.store_generic(&expr, &_vals); };
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
                    "new-memo: name evaluated to empty",
                    name_arg,
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
            outer_carrying,
        } => {
            let (size_results, env_after) = result;

            if size_results.is_empty() {
                let err = ctx.factory().error(
                    "new-memo: size evaluated to empty",
                    size_arg,
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
            outer_carrying,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                let err = ctx.factory().error(
                    &format!("{}: memo reference evaluated to empty", op_name),
                    memo_ref,
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
                            let detail = ctx.factory().atom("Rebuild with: cargo build --features track-stats");
                            let err = ctx.factory().error(
                                "memo-stats requires track-stats feature",
                                detail,
                            );
                            work_stack.push(WorkItem::Resume {
                                result: (smallvec![bv(err)], env_after),
                            });
                        }
                    }
                } else {
                    let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                    let err = ctx.factory().error(
                        &format!(
                            "{}: argument must be a memo table, got {}",
                            op_name,
                            first.friendly_repr()
                        ),
                        first.clone(),
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
            if mutation_epoch() == saved_epoch {
                eval_memo_put(expr_hash, &result_values.iter().map(|(v, _)| v.clone()).collect::<Vec<_>>());
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
                let (value, _) = &result_values[0];

                if let Some(pm_bindings) = pattern_match(&current_pattern, value) {
                    // Compose pattern-match bindings into accumulated
                    accumulated_bindings = Box::new(accumulated_bindings.compose(&pm_bindings));

                    if remaining_pairs.is_empty() {
                        // I-5: Exit region — let* scope complete
                        crate::backend::eval::cesk::with_region_stack(|s| { s.exit(); });
                        // All bindings resolved — evaluate body with composed bindings
                        work_stack.push(WorkItem::EvalWithBindings {
                            template: body,
                            bindings: accumulated_bindings,
                            env: result_env,
                            depth,
                            is_tail_call,
                            expected_type: None,
                            carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                        });
                    } else {
                        // More pairs to process — pop next pair
                        let (next_pattern, next_value_expr) = remaining_pairs.remove(0);
                        let materialized_value = apply_bindings(
                            &next_value_expr, &accumulated_bindings, ctx.factory(),
                        );

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

                        work_stack.push(WorkItem::Eval {
                            value: materialized_value,
                            env: result_env,
                            depth: depth + 1,
                            is_tail_call: false,
                            expected_type: None,
                            demand: None,
                            carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                        });
                    }
                } else {
                    // Pattern match failed — let* produces empty (MeTTa HE semantics)
                    crate::backend::eval::cesk::with_region_stack(|s| { s.exit(); }); // I-5
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                }
            } else if result_values.is_empty() {
                // Zero results — let* produces empty
                crate::backend::eval::cesk::with_region_stack(|s| { s.exit(); }); // I-5
                work_stack.push(WorkItem::Resume {
                    result: (SmallVec::new(), result_env),
                });
            } else {
                // Multiple results — nondeterministic value expression.
                // I-5: Exit region before fallback (region doesn't span materialized let forms)
                crate::backend::eval::cesk::with_region_stack(|s| { s.exit(); });
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

                // For each result value, pattern-match and evaluate the rest
                let mut bound_bodies: Vec<MettaValue> = Vec::new();
                for (value, _) in result_values.iter() {
                    if let Some(pm_bindings) = pattern_match(&current_pattern, value) {
                        let composed = accumulated_bindings.compose(&pm_bindings);
                        let materialized = apply_bindings(&let_body, &composed, ctx.factory());
                        bound_bodies.push(materialized);
                    }
                }

                if bound_bodies.is_empty() {
                    work_stack.push(WorkItem::Resume {
                        result: (SmallVec::new(), result_env),
                    });
                } else if bound_bodies.len() == 1 {
                    work_stack.push(WorkItem::Eval {
                        value: bound_bodies.into_iter().next().expect("len == 1"),
                        env: result_env,
                        depth,
                        is_tail_call,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                    });
                } else {
                    let mut bodies_iter = bound_bodies.into_iter();
                    let first = bodies_iter.next().expect("bodies non-empty");

                    continuations.push(Continuation::ProcessAmb {
                        remaining_alts: bodies_iter,
                        results: Vec::new(),
                        env: result_env.clone(),
                        depth,
                        outer_carrying: accumulated_bindings.clone(),
                    });

                    work_stack.push(WorkItem::Eval {
                        value: first,
                        env: result_env,
                        depth: depth + 1,
                        is_tail_call: false,
                        expected_type: None,
                        demand: None,
                        carrying_bindings: Box::new(crate::backend::models::GenericBindings::new()),
                    });
                }
            }
        }

        // ── I-4: CompleteSubgoal — cache tabling results ──
        Continuation::CompleteSubgoal {
            expr_hash,
            env: _,
            depth,
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
            if start_epoch == mutation_epoch() {
                let cached: smallvec::SmallVec<[MettaValue; 2]> =
                    result_values.iter().map(|(v, _)| v.clone()).collect();
                crate::backend::eval::cesk::with_subgoal_table(|t| {
                    t.complete(expr_hash, cached);
                });
            }

            #[cfg(feature = "eval-trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(
                            &result_values.first().map(|(v, _)| v.clone()).unwrap_or_else(|| ctx.factory().unit())
                        ),
                        result_values.iter().map(|(v, _)| crate::backend::trace::trace_value_generic(v)).collect(),
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
    }
}
