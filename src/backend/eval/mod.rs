// Eval module: Arena-based lazy evaluation with pattern matching and built-in dispatch
//
// The evaluation pipeline uses arena-allocated MettaValue exclusively.
// The eval() entry point handles bytecode/JIT tiering with
// tree-walker fallback via eval_trampoline().

pub(crate) mod alpha_equiv;
pub(crate) mod bindings;
pub mod cesk;
pub(crate) mod expr_vec_frame;
pub(crate) mod frame_label;
pub(crate) mod freshening;
pub(crate) mod git_import;
pub(crate) mod helpers;
mod list_ops;
pub(crate) mod modules;
pub mod monad_registry;
pub(crate) mod mork_forms;
mod pattern;
pub mod priority;
mod processing;
pub(crate) mod set_ops;
pub(crate) mod space_match;
pub(crate) mod step;
pub(crate) mod testing_ops;
pub mod tier_forced;
pub mod trampoline;
pub(crate) mod type_fixpoint;
pub(crate) mod types;

#[cfg(test)]
mod arena_tests;

// Re-export from pattern module
pub use pattern::pattern_match;

// Re-export from helpers module
pub use helpers::apply_bindings;
// Re-export helpers for step/grounded module access
pub(crate) use helpers::{is_eager_special_form, is_grounded_op};

// Re-export from trampoline module
pub use trampoline::eval_trampoline;
// Re-export arena context types for external use
pub use trampoline::{MettaEnvironment, StaticEvalContext};

// Type system re-exports for benchmarking
pub use step::{
    extract_arg_types, extract_return_type, find_grounded_arg_indices_generic,
    find_typed_arg_indices_generic, is_arrow_type, is_declared_value_type, is_meta_type_value,
    validate_grounded_arg_types,
};
pub use type_fixpoint::run_type_fixpoint;
pub use types::{
    apply_type_bindings, eval_check_type_generic, eval_get_type_generic,
    eval_get_type_space_generic, eval_type_cast_generic, eval_validate_atom_generic,
    extract_type_constraint, freshen_type_variables, get_ground_type, infer_arrow_type_from_rule,
    infer_type_generic, infer_types_generic, is_meta_type, is_pattern_type_compatible,
    match_types_with_bindings, types_match_generic, types_match_with_subtypes,
};

// =============================================================================
// Arena-based Evaluation with Bytecode/JIT Tiering
// =============================================================================

use std::cell::RefCell;

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait, SafepointRootHandle};

/// Type alias for arena evaluation result.
/// Uses SmallVec<[MettaValue; 2]> to inline up to 2 elements, avoiding heap
/// allocation for the common single-result case.
pub type EvalResult = (SmallVec<[MettaValue; 2]>, MettaEnvironment);

/// The return type of the public [`eval`] entry point.
///
/// In the **legacy slab opt-out** build this is EXACTLY [`EvalResult`] (the
/// `(results, env)` 2-tuple) — the slab signature is UNCHANGED, so existing slab
/// callers compile verbatim.
///
/// In the **default `index-gc`** build it gains a third element: an
/// `Option<SafepointRootHandle>` carrying the E1-FLIP Path B V4 **B3
/// directive-exit leaving-park** handle. B3 registers
/// the leaving root set (`reach(E₀) ∪ results`) into the gen-unconditional `SAFEPOINT_ROOTS`
/// channel just before `eval()`'s `EvalGuard` drops, then stamps the witness
/// (`note_reified_park`). The handle MUST RIDE to the caller (the F1 ride-to-caller pattern,
/// design C-0c) and drop only AFTER the caller consumes `results` — a held-in-`eval()`-scope
/// handle leaves a sliver `[eval() returns, caller re-registers]` during which a concurrent
/// dedicated-GC cycle that snapshots the witness (this thread's slot is RELEASED at the
/// outermost `EvalGuard::drop`, so it is NOT waited for) would sweep `results`. `None`
/// whenever B3 did not fire (the default: DEDICATED off, or no concurrent cycle in flight).
///
/// Production callers therefore bind the third element to a NAMED local (NOT `_`) whose
/// scope outlives result consumption; test callers (no concurrent GC under DEDICATED-off)
/// use the `(results, env, ..)` rest-pattern, which is valid for BOTH the
/// 2-tuple (legacy slab) and the 3-tuple (default index-gc) and harmlessly drops
/// the always-`None` handle.
/// See [`EvalReturn`] (slab variant). The third element rides the B3 leaving-park handle.
pub type EvalReturn = (
    SmallVec<[MettaValue; 2]>,
    MettaEnvironment,
    Option<SafepointRootHandle>,
);

// =============================================================================
// Thread-Local Cache Root Snapshot
// =============================================================================
//
// Thread-local caches (EVAL_MEMO, MATCH_RESULT_CACHE, subgoal table, thunk
// table) hold MettaValue references that persist across eval() calls. During
// intra-eval safepoints, these are collected via collect_eval_memo_roots() etc.
// and included in the temporary root set.
//
// However, BETWEEN eval() calls (when ACTIVE_EVALUATORS == 0), the session
// release GC runs on a background worker thread and cannot access thread-local
// caches. Its surviving set (from trace_surviving_set → collect_all_roots_readonly)
// only includes environment roots and safepoint roots — NOT cache roots.
//
// If a previous session's values exist in thread-local caches and the session
// release frees them, any subsequent eval that hits the cache returns dangling
// MettaValues → use-after-poison in format_result.
//
// Fix: Before the EvalGuard drops (transitioning to quiescent state), snapshot
// all thread-local cache roots into the safepoint root registry. The
// SafepointRootHandle is stored in a thread-local so it persists until the
// next eval() call replaces it. This ensures cache roots are visible to the
// session release GC via trace_safepoint_live_set().

thread_local! {
    /// Holds the SafepointRootHandle for thread-local cache roots.
    /// Replaced at the end of each eval() call. The handle keeps cache roots
    /// registered as safepoint roots until the next eval() replaces it.
    static CACHE_ROOT_HANDLE: RefCell<Option<SafepointRootHandle>> = const { RefCell::new(None) };
}

/// Snapshot all thread-local cache roots into the safepoint root registry.
///
/// Called from eval() just before the EvalGuard drops. Returns a
/// SafepointRootHandle that keeps the roots registered. The caller stores
/// this handle in CACHE_ROOT_HANDLE (replacing the previous one).
fn snapshot_cache_roots() -> Option<SafepointRootHandle> {
    use crate::backend::models::register_temporary_roots;

    let mut roots = Vec::with_capacity(256);
    trampoline::dispatch_hints::collect_eval_memo_roots(&mut roots);
    trampoline::dispatch_hints::collect_match_result_roots(&mut roots);
    cesk::tabling::collect_subgoal_roots(&mut roots);
    cesk::thunk::collect_thunk_roots(&mut roots);

    if roots.is_empty() {
        None
    } else {
        Some(register_temporary_roots(roots))
    }
}

/// Refresh this thread's persistent cache-root handle.
///
/// Main eval threads and parallel worker threads both hold thread-local
/// evaluation caches. The handle keeps those cached values visible to
/// quiescent/session GC while the thread is idle between eval calls.
pub(crate) fn refresh_thread_local_cache_roots() {
    let new_handle = snapshot_cache_roots();
    CACHE_ROOT_HANDLE.with(|h| {
        *h.borrow_mut() = new_handle;
    });
}

/// Drop guard used by worker closures so cache roots are refreshed while their
/// EvalGuard is still active, including cancellation unwind paths.
pub(crate) struct CacheRootRefreshGuard;

impl CacheRootRefreshGuard {
    #[inline]
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Drop for CacheRootRefreshGuard {
    fn drop(&mut self) {
        refresh_thread_local_cache_roots();
    }
}

/// Evaluate an MettaValue with bytecode/JIT tiering.
///
/// This function provides zero-conversion evaluation for MettaValue expressions
/// using session-scoped dual-arena allocation. It uses the arena tiered cache
/// to track executions and trigger background bytecode compilation, then
/// executes via the highest available tier:
///
/// 1. JIT Stage 2 (native code, 500+ executions)
/// 2. JIT Stage 1 (native code, 100+ executions)
/// 3. Bytecode VM (2+ executions)
/// 4. Tree-walker interpreter (fallback)
///
/// The `EvalGuard` is explicitly scoped so it is dropped before attempting
/// GC lifecycle work. This ensures library and test code that calls `eval()`
/// directly (without the `main.rs` between-expression loop) still reaches
/// quiescent points where GC can trigger and backpressure can be released.
///
/// # Arguments
/// * `value` - The MettaValue expression to evaluate
/// * `env` - The arena-based environment
/// * `state` - The MettaState owning the storage arena
///
/// # Returns
/// A tuple of (results, updated_environment) where results are `Vec<MettaValue>`.
///
/// # Example
/// ```ignore
/// use mettatron::backend::compile::compile;
/// use mettatron::backend::eval::eval;
/// use mettatron::backend::eval::trampoline::new_env;
///
/// let state = compile("!(+ 1 2)").unwrap();
/// let env = new_env();
///
/// for &expr in state.source() {
///     let (results, env) = eval(expr, env, &state);
///     println!("{:?}", results);
/// }
/// ```
pub fn eval(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
) -> EvalReturn {
    use crate::backend::models::EvalGuard;

    // S1 TOPLEVEL (2026-05-13): the programmatic `eval()` API is HE
    // INTERPRET mode by contract. Callers (Rust tests, REPL programmatic
    // entry, Rholang integration) explicitly ASK for reduction and expect
    // results back. The ADD/INTERPRET distinction is a *source-level*
    // concept (bare top-level S-expr vs `(! ...)` directive) handled by
    // the per-directive runner loop in `main.rs` / `mtt_conformance.rs`;
    // the library API bypasses it. Without this flag set, programmatic
    // `eval(query, env)` calls would return `[]` under the new ADD-mode
    // gate in `process_single_combination_generic`.
    //
    // The runner is responsible for selectively clearing interpret_mode
    // before non-bang directives — see `main.rs` source-eval loop.
    //
    // S2 BANG-WORD (2026-05-13): reset `bang_body` to false at the start
    // of each `eval()` call. The flag is set only by the `!` arm and is
    // strictly per-directive; without this reset it would leak across
    // directives via the propagated env (e.g., a `!` directive followed
    // by a bare `(= foo bar)` would see bang_body=true left over and
    // skip rule registration). The runner threads the env between
    // directives, so the env reaches eval() with whatever bang_body it
    // had at the end of the previous directive — that value is stale.
    let mut env = env;
    env.set_interpret_mode(true);
    env.set_bang_body(false);

    // Driver-C publication: `state.source/output` are control held by the caller
    // above the CESK machine. Midloop/rendezvous collections cannot read `state`
    // directly, so publish those values to the narrow safepoint channel for this
    // eval interval; quiescence still also reads `state` structurally by name.
    let _driver_c_handle = {
        let mut driver_roots = Vec::new();
        state.collect_driver_program_roots(&mut driver_roots);
        crate::backend::models::register_temporary_roots(driver_roots)
    };

    // E1-FLIP Path B V4 — B3 directive-exit leaving-park handle (the F1 ride-to-caller).
    // Built INSIDE the `_guard` scope (after `eval_inner` returns, before the guard drops);
    // declared here so it ESCAPES that scope AND the post-processing below, riding out in
    // the returned `EvalReturn` so the caller drops it only AFTER consuming the results.
    // `None` whenever B3 does not fire. Index-gc only (slab return is the unchanged 2-tuple).
    let mut b3_root_handle: Option<SafepointRootHandle> = None;

    // Scope the EvalGuard so it drops after eval completes.
    let mut result = {
        let _guard = EvalGuard::enter();

        // ── E1-FLIP Path B V4 — B2′: register E₀'s env struct in the global
        // live-env registry so the dedicated GC thread can walk it EVERY cycle,
        // participant-independently (the env struct is per-`GenericEnvironmentShared`
        // CoW-cloned at fork, NOT one process-global Arc — so the GC thread cannot
        // reach it from a global handle without this registry). Registered BEFORE
        // `env` is moved into `eval_inner`; the RAII handle is held for the whole
        // `_guard` scope (so E₀ is covered for the directive's lifetime). Covers the
        // CoW-forked child bindings via the branch-spawn registration too (granularity
        // (a)). BYTE-IDENTICAL WHEN DORMANT: #[cfg(index-gc)] wall + dedicated-first.
        let _live_env_handle = {
            if crate::backend::models::gc_allocator::dedicated_gc_enabled() {
                let dyn_env: std::sync::Arc<dyn crate::backend::models::gc_allocator::EnvRoots> =
                    env.shared.clone();
                Some(crate::backend::models::gc_allocator::register_live_env(
                    &dyn_env,
                ))
            } else {
                None
            }
        };

        let r = eval_inner(value, env, state);

        // ── E1-FLIP Path B V4 — B3: the directive-exit LEAVING-PARK ──
        // This thread is about to drop its outermost `EvalGuard` (→ N_THREADS--, and the
        // V4 witness slot RELEASES at the outermost drop) while it still carries live
        // values: `r.0` (the about-to-return results) and `reach(E₀)` (the env it leaves
        // behind). A concurrent dedicated-GC cycle that snapshots the witness AFTER this
        // thread's slot releases will NOT wait for this thread — so those values MUST
        // already be in the gen-unconditional `SAFEPOINT_ROOTS` channel (drained EVERY
        // cycle, Pin 2) for that cycle to mark them, AND this thread must STAMP the
        // witness so a cycle in flight RIGHT NOW (whose snapshot still sees this slot
        // occupied) does not proceed until this publish is visible.
        //
        // Gated `dedicated_gc_enabled() && is_gc_requested()`: B3 fires ONLY when a
        // dedicated cycle is actually in flight (the only time the leaving-park matters).
        // BYTE-IDENTICAL when dormant (DEDICATED off ⇒ `request_concurrent_collection`
        // early-returns ⇒ `is_gc_requested()` is never true ⇒ this block is inert; in
        // slab the whole thing is `#[cfg]`'d out). The `leaving` set uses the SAME reader
        // the quiescence hook below uses (`collect_persistent_roots(E₀) ∪ r.0`), NOT a
        // bare TierLeaf — so E₀ is fully covered (B2′ makes E₀ global, but the leaving
        // thread also routes it through SAFEPOINT_ROOTS for the snapshot-after-release
        // window). Registering a SUPERSET of the final result (pre-error-filter `r.0`) is
        // SOUND: extra dead roots defer one cycle, never a UAF.
        {
            use crate::backend::models::gc_allocator::{
                current_cycle_gen, dedicated_gc_enabled, is_gc_requested, note_reified_park,
            };
            use crate::backend::models::register_temporary_roots;
            if dedicated_gc_enabled() && is_gc_requested() {
                let mut leaving: Vec<MettaValue> = Vec::with_capacity(r.0.len() + 64);
                crate::backend::eval::cesk::roots::collect_persistent_roots(
                    &mut leaving,
                    r.1.shared.as_ref(),
                );
                leaving.extend(r.0.iter().copied());
                // Register into SAFEPOINT_ROOTS BEFORE the stamp+fence so the roots are
                // visible to the driver's drain the instant the witness says "published".
                b3_root_handle = Some(register_temporary_roots(leaving));
                // Release fence: publish the SAFEPOINT_ROOTS registration to any GC thread
                // that subsequently observes our witness stamp (Acquire). The stamp is the
                // synchronizing write; the fence orders the registration before it.
                core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
                // The empty-machine LEAVING stamp: mark the witness published for the
                // current cycle so an in-flight snapshot that still sees this slot occupied
                // proceeds only after `leaving ⊆ SAFEPOINT_ROOTS` is visible. SOUND because
                // E₀ ∈ B2′ + r.0 ∈ the persistent channel + the thread is leaving;
                // gen-unconditional (no per-cycle-buffer dependency).
                if is_gc_requested() {
                    note_reified_park(current_cycle_gen());
                }
            }
        }

        // Snapshot thread-local cache roots into the safepoint root registry
        // BEFORE the EvalGuard drops (while ACTIVE_EVALUATORS > 0).
        //
        // This ensures cache roots are visible to session release GC via
        // trace_safepoint_live_set(). Without this, cached MettaValues from
        // previous sessions are invisible to the session release worker (which
        // runs on a different thread and cannot access thread-locals), causing
        // use-after-poison when the cache returns freed values.
        //
        // The handle replaces the previous one — old roots are unregistered.
        refresh_thread_local_cache_roots();

        r
    };
    // _guard dropped here — ACTIVE_EVALUATORS decremented.
    // CACHE_ROOT_HANDLE holds the cache roots in the safepoint registry,
    // so they're visible to session release GC during quiescence.

    // ── Inc 6: single-threaded store-centric GC (TRUE-quiescence reclaim) ──
    // The FIRST WORKING index collector. The `EvalGuard` has just dropped, so
    // `active_evaluator_count() == 0`: no trampoline loop and no bytecode VM is
    // live on the Rust stack — exactly the slab GC's session-release reclaim
    // point and exactly the proven `QuiescenceInvariant` (activeEvaluators
    // empty). The collector fires ONLY in index mode and ONLY when no evaluator
    // thread is active (`index_gc::gate_open`), making it safe even when fanout
    // is configured. The complete root set is the structural persistent reader
    // UNIONED with the about-to-be-returned result values (held here in a Rust
    // local, not yet in any RootProvider).
    //
    // Dead in the legacy slab opt-out build: `gc_mode_is_index()` const-folds
    // to `false` when `index-gc` is off, so the slab path is byte-identical.
    // Cheap pre-check (gate + watermark) avoids the `collect_all_roots()` walk on
    // every eval; only build the root set when a collection will actually fire.
    if crate::backend::eval::cesk::index_heap::index_gc::should_collect() {
        // ── A4.4 quiescence machine-equivalence oracle (debug-only; index-gc only) ──
        // Asserts the structural-persistent feed below covers the discovered set the
        // BEFORE feed used (collect_all_roots ∪ result). Gated on gc_mode_is_index()
        // (slab has no structural mirror for frame-chain roots). PERMANENT CI invariant.
        #[cfg(debug_assertions)]
        if crate::backend::models::metta_value::gc_mode_is_index() {
            crate::backend::eval::cesk::roots::assert_quiescence_superset(
                &result.0,
                result.1.shared.as_ref(),
                state,
            );
        }
        // A4.4 FLIP (quiescence): feed the collector from the PERSISTENT structural reader —
        //   collect_persistent_roots(E₀)        — reach(E₀-env) ∪ global anchors ∪ K-spine
        //   ∪ result.0                          — the about-to-return values (a Rust local)
        //   ∪ state.collect_driver_program_roots — the driver's program control (C).
        // C∪K are empty post-EvalGuard ⇒ NO current WorkItem ⇒ collect_persistent_roots
        // (not collect_machine_roots). collect_all_roots() fed the oracle's OLD until
        // A5/F4 deleted it. Use `result.1.shared` (the consumed `env` was moved into eval_inner).
        let mut roots: Vec<MettaValue> = Vec::with_capacity(result.0.len() + 64);
        crate::backend::eval::cesk::roots::collect_persistent_roots(
            &mut roots,
            result.1.shared.as_ref(),
        );
        roots.extend(result.0.iter().copied());
        state.collect_driver_program_roots(&mut roots);
        // KEPT narrow driver-transport channel: SAFEPOINT_ROOTS (the driver's
        // cross-directive result accumulator via register_temporary_roots + the
        // thread-local cache snapshot). NOT replaced by the structural reader —
        // dropping it would free the driver's accumulated results → UAF. (A5.4 narrows.)
        crate::backend::models::collect_safepoint_roots(&mut roots);
        // E1-a.3: route the SAME roots through the dedicated index-GC driver.
        // The EvalGuard dropped above ⇒ n_threads()==0 here ⇒ true-quiescence
        // collection is reachable even when FANOUT>0 is configured; midloop
        // non-rendezvous collection is the path that backs off under FANOUT>0.
        crate::backend::eval::cesk::gc_driver::collect_quiescence(roots);
    }

    // Phase 10.5: Run type fixpoint if rules were added during this eval.
    // O(1) atomic check; no-op when no new types were registered.
    // Must be OUTSIDE EvalGuard scope: run_type_fixpoint() acquires rule_index.read(),
    // and add_rule() (which completed during eval) holds rule_index.write().
    // Calling after eval() returns ensures all locks are released (no deadlock).
    result.1.maybe_run_type_fixpoint();

    // HE-bisim check_alternatives (interpreter.rs:1079-1108): drop Error
    // alternatives from the top-level eval result bag whenever ≥1 non-error
    // result exists. Mirrors the trampoline's `EvalOutcome::Complete` filter
    // (trampoline/eval_loop.rs) and the in-collapse-bind filter
    // (eval_loop.rs:12989). Required to bisimulate T07/003-factorial-style
    // fixtures whose recursive rule produces an unproductive `(* -N ...)`
    // branch that hits the `max-stack-depth` cap; HE drops the Error,
    // returning `[120]`. Applied at this top level (not inside `eval_inner`)
    // so the filter fires regardless of which tier produced the result
    // (bytecode VM / JIT / tree-walker trampoline / builtin-chunk path).
    let any_success = result.0.iter().any(|v| !v.is_error_sentinel());
    if any_success {
        result.0.retain(|v| !v.is_error_sentinel());
    }

    // Session-based GC: reclamation is triggered by SessionGuard::drop() between
    // top-level expressions. No post-eval GC lifecycle needed here — the caller
    // (main.rs, rholang_integration.rs) manages SessionGuard around eval+format.

    // E1-FLIP Path B V4 — B3: ride the leaving-park handle out so the caller drops it
    // only AFTER consuming the results (C-0c). Slab return is the unchanged 2-tuple.
    {
        (result.0, result.1, b3_root_handle)
    }
}

/// Evaluate an MettaValue with bytecode/JIT tiering and trace collection.
///
/// Identical to [`eval`] but threads a `TraceCollector` through the evaluation
/// pipeline so that all tree-walker events are recorded. The bytecode/JIT tiers
/// also fall back to the trace-enabled tree-walker on bailout.
///
/// Only available when the `eval-trace` feature is enabled.
#[cfg(feature = "trace")]
pub fn eval_with_trace(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
    collector: &std::sync::Arc<crate::backend::trace::TraceCollector>,
) -> EvalResult {
    use crate::backend::models::EvalGuard;

    // S1 TOPLEVEL (2026-05-13): programmatic API is HE INTERPRET mode.
    // See `eval()` for rationale.
    //
    // S2 BANG-WORD (2026-05-13): reset bang_body per directive (see `eval()`).
    let mut env = env;
    env.set_interpret_mode(true);
    env.set_bang_body(false);

    // Wire the trace collector into the work pool so worker threads can emit
    // trace events (WorkPoolTaskEnqueued, WorkPoolScaleEvent, etc.).
    // OnceLock inside — only the first call has effect; subsequent are no-ops.
    crate::backend::models::work_pool::set_work_pool_trace_collector(collector);

    let result = {
        let _guard = EvalGuard::enter();
        let r = eval_inner_with_trace(value, env, state, collector);

        // Snapshot cache roots before EvalGuard drops (same rationale as eval()).
        refresh_thread_local_cache_roots();

        r
    };

    result.1.maybe_run_type_fixpoint();
    result
}

/// Inner eval body with trace — bytecode/JIT tiered execution with trace-enabled
/// tree-walker fallback.
///
/// Sets the thread-local trace collector before each bytecode/JIT dispatch so that
/// opcode handlers and JIT runtime helpers can emit trace events via
/// `with_thread_trace_collector()`. Emits `TierDispatch` events at each tier
/// selection point for tier-transition visibility.
#[cfg(feature = "trace")]
fn eval_inner_with_trace(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
    collector: &std::sync::Arc<crate::backend::trace::TraceCollector>,
) -> EvalResult {
    #[cfg(feature = "track-stats")]
    use crate::backend::bytecode::ExecutionTier;
    use crate::backend::bytecode::{
        can_compile, can_compile_with_env, eval_bytecode_arena_with_env, execute_arena,
        global_tiered_cache, TierStatusKind,
    };
    use crate::backend::trace::thread_local_sink::{
        clear_thread_trace_collector, set_thread_trace_collector,
    };

    let compilation_state = global_tiered_cache().record_execution(&value);
    let execution_count = compilation_state
        .execution_count
        .load(std::sync::atomic::Ordering::Relaxed);
    let expr_hash = compilation_state.expr_hash;

    // Set thread-local trace collector for bytecode/JIT instrumentation.
    set_thread_trace_collector(collector);

    // Gate: if any sub-expression's head is an overridable grounded op with
    // a user rule currently installed, bypass bytecode/JIT tiers and route
    // to the trampoline so user rules take precedence (HE-bisimilar behavior
    // for names HE defines in stdlib.metta as overridable rules).
    let has_overridden_grounded = expression_has_overridden_grounded_op(&value, &env);

    if can_compile(&value) && !has_overridden_grounded {
        // JIT Stage 2
        if compilation_state.jit2_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit2_code() {
                // Emit TierDispatch event
                collector.emit_converted(
                    trace_format::TraceTier::JitStage2,
                    0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![],
                    None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::JitStage2,
                        execution_count,
                    },
                );

                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache().record_tier_execution(ExecutionTier::JitStage2);
                        clear_thread_trace_collector();
                        return (SmallVec::from_vec(results), new_env);
                    }
                    Err(_) => {}
                }
            }
        }

        // JIT Stage 1
        if compilation_state.jit1_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit1_code() {
                collector.emit_converted(
                    trace_format::TraceTier::JitStage1,
                    0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![],
                    None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::JitStage1,
                        execution_count,
                    },
                );

                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache().record_tier_execution(ExecutionTier::JitStage1);
                        clear_thread_trace_collector();
                        return (SmallVec::from_vec(results), new_env);
                    }
                    Err(_) => {}
                }
            }
        }

        // Bytecode VM
        if compilation_state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = compilation_state.bytecode_chunk() {
                collector.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    crate::backend::trace::trace_value_generic(&value),
                    vec![],
                    None,
                    trace_format::TraceEventKind::TierDispatch {
                        expression_hash: expr_hash,
                        selected_tier: trace_format::TraceTier::BytecodeVM,
                        execution_count,
                    },
                );

                match execute_arena(chunk, env.clone()) {
                    Ok((results, new_env, unreduced)) => {
                        if unreduced {
                            // Bytecode couldn't reduce — fall through
                        } else {
                            #[cfg(feature = "track-stats")]
                            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
                            clear_thread_trace_collector();
                            return (SmallVec::from_vec(results), new_env);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
    }

    // Check pre-compiled built-in registry — keep this aligned with eval_inner
    // so enabling trace does not change tier selection for common operations.
    if !has_overridden_grounded {
        if let Some(items) = value.as_sexpr() {
            if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                let arity = (items.len() - 1) as u8;
                if let Some(builtin_chunk) =
                    crate::backend::bytecode::builtin_chunks::get_builtin_chunk(head_atom, arity)
                {
                    let mut arg_values = SmallVec::<[MettaValue; 4]>::with_capacity(arity as usize);
                    let mut current_env = env.clone();
                    let mut all_single = true;
                    for arg in &items[1..] {
                        clear_thread_trace_collector();
                        let (results, new_env) = trampoline::eval_trampoline_with_trace(
                            *arg,
                            current_env,
                            state,
                            collector,
                        );
                        current_env = std::sync::Arc::try_unwrap(new_env)
                            .unwrap_or_else(|arc| (*arc).clone());
                        if results.len() == 1 {
                            arg_values.push(results.into_iter().next().expect("len checked").0);
                        } else {
                            all_single = false;
                            break;
                        }
                    }
                    if all_single {
                        set_thread_trace_collector(collector);
                        let factory = current_env.factory().clone();
                        let mut vm =
                            crate::backend::bytecode::GenericBytecodeVM::with_env_and_factory(
                                builtin_chunk,
                                current_env.clone(),
                                factory,
                            );
                        for arg in arg_values {
                            vm.value_stack.push(arg);
                        }
                        if let Ok((results, _env_opt)) = vm.run_with_env() {
                            if !results.is_empty() {
                                clear_thread_trace_collector();
                                return (SmallVec::from_vec(results), current_env);
                            }
                        }
                    }
                }
            }
        }
    }

    // Environment-aware bytecode is only valid for pure, closed rule-backed
    // calls. Keep this gate aligned with trampoline sub-expression dispatch:
    // meta-typed args, overridden grounded heads, and impure/cut rule bodies
    // require the tree-walker.
    let compilable_with_env = compilation_state
        .cached_compilable_with_env()
        .unwrap_or_else(|| {
            let result = can_compile_with_env(&value);
            compilation_state.set_compilable_with_env(result);
            result
        });
    let has_meta_typed = expression_has_declared_meta_typed_params(&value, &env);
    let has_impure_rules = expression_involves_impure_rules(&value, &env);
    if compilable_with_env && !has_meta_typed && !has_overridden_grounded && !has_impure_rules {
        collector.emit_converted(
            trace_format::TraceTier::BytecodeVM,
            0,
            crate::backend::trace::trace_value_generic(&value),
            vec![],
            None,
            trace_format::TraceEventKind::TierDispatch {
                expression_hash: expr_hash,
                selected_tier: trace_format::TraceTier::BytecodeVM,
                execution_count,
            },
        );

        let vm_result = if let Some(chunk) = compilation_state.bytecode_chunk() {
            let factory = env.factory().clone();
            let mut vm = crate::backend::bytecode::GenericBytecodeVM::with_env_and_factory(
                chunk,
                env.clone(),
                factory.clone(),
            );
            vm.yield_on_top_return = true;
            vm.run()
                .map(|results| {
                    let unreduced = vm.unreduced || vm.had_unreduced_result;
                    let has_choices = vm.choice_points_len() > 0;
                    let final_env = vm.env.take().unwrap_or_else(|| {
                        crate::backend::environment::core::MettaEnvironment::new(factory)
                    });
                    (results, final_env, unreduced, has_choices)
                })
                .ok()
        } else {
            match eval_bytecode_arena_with_env(&value, env.clone()) {
                Ok((results, new_env, unreduced, has_choices)) => {
                    if compilation_state.try_start_bytecode_compile() {
                        if let Ok(chunk) =
                            crate::backend::bytecode::compile_bytecode_arc("cached_env", &value)
                        {
                            compilation_state.set_bytecode_ready(chunk);
                        }
                    }
                    Some((results, new_env, unreduced, has_choices))
                }
                Err(_) => None,
            }
        };

        if let Some((results, new_env, unreduced, has_choices)) = vm_result {
            if !unreduced && !has_choices {
                #[cfg(feature = "track-stats")]
                global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);

                // S2 BANG-WORD (2026-05-13): set bang_body=true on the
                // post-VM trampoline env when the original expression was a
                // `(! ...)` directive — so decl-atom results from the VM
                // (e.g. `(= foo bar)`) flow through T0 as data, not as
                // registration. See `eval_inner` for the detailed rationale.
                let is_bang_directive = value
                    .as_sexpr()
                    .and_then(|items| items.first())
                    .and_then(|h| h.as_atom())
                    .is_some_and(|s| s == "!");

                // Complete evaluation via trampoline (see eval_inner for rationale).
                let mut final_results = SmallVec::with_capacity(results.len());
                let mut final_env = new_env;
                if is_bang_directive {
                    final_env.set_bang_body(true);
                }
                for result in results {
                    clear_thread_trace_collector();
                    let (sub_results, sub_env) =
                        trampoline::eval_trampoline_with_trace(result, final_env, state, collector);
                    // Strip per-branch bindings at the external API boundary.
                    final_results.extend(sub_results.into_iter().map(|(v, _)| v));
                    final_env =
                        std::sync::Arc::try_unwrap(sub_env).unwrap_or_else(|arc| (*arc).clone());
                    if is_bang_directive {
                        final_env.set_bang_body(true);
                    }
                }
                if is_bang_directive {
                    final_env.set_bang_body(false);
                }
                clear_thread_trace_collector();
                return (final_results, final_env);
            }
        }
    }

    // Tier 0: Tree-walker with trace collection
    collector.emit_converted(
        trace_format::TraceTier::TreeWalker,
        0,
        crate::backend::trace::trace_value_generic(&value),
        vec![],
        None,
        trace_format::TraceEventKind::TierDispatch {
            expression_hash: expr_hash,
            selected_tier: trace_format::TraceTier::TreeWalker,
            execution_count,
        },
    );

    clear_thread_trace_collector();
    #[cfg(feature = "track-stats")]
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    let (results, env_arc) = trampoline::eval_trampoline_with_trace(value, env, state, collector);
    let env = std::sync::Arc::try_unwrap(env_arc).unwrap_or_else(|arc| (*arc).clone());
    // Strip per-branch bindings at the external API boundary.
    (results.into_iter().map(|(v, _)| v).collect(), env)
}

/// Inner eval body — bytecode/JIT tiered execution with tree-walker fallback.
///
/// Called from `eval()` while an `EvalGuard` is held (ACTIVE_EVALUATORS > 0).
/// This function must NOT create its own `EvalGuard` — the caller manages the
/// guard's lifetime to ensure proper quiescent-state transitions.
fn eval_inner(
    value: MettaValue,
    env: MettaEnvironment,
    state: &crate::backend::models::MettaState,
) -> EvalResult {
    #[cfg(feature = "track-stats")]
    use crate::backend::bytecode::ExecutionTier;
    use crate::backend::bytecode::{
        can_compile, can_compile_with_env, eval_bytecode_arena_with_env, execute_arena,
        global_tiered_cache, TierStatusKind,
    };

    // Record execution in arena tiered cache
    // This triggers background bytecode and JIT compilation at thresholds
    let compilation_state = global_tiered_cache().record_execution(&value);

    // Gate: if any sub-expression's head is an overridable grounded op with
    // a user rule currently installed, bypass bytecode/JIT tiers and route
    // to the trampoline so user rules take precedence.
    let has_overridden_grounded = expression_has_overridden_grounded_op(&value, &env);

    // Check if this expression can be compiled to bytecode (pure expressions)
    if can_compile(&value) && !has_overridden_grounded {
        // Check for JIT execution first (highest tier)
        // JIT Stage 2 (very hot code, 500+ executions)
        if compilation_state.jit2_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit2_code() {
                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache().record_tier_execution(ExecutionTier::JitStage2);
                        return (SmallVec::from_vec(results), new_env);
                    }
                    Err(_) => {
                        // JIT execution failed, fall through
                    }
                }
            }
        }

        // JIT Stage 1 (hot code, 100+ executions)
        if compilation_state.jit1_status() == TierStatusKind::Ready {
            if let Some(code) = compilation_state.jit1_code() {
                match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
                    Ok((results, new_env)) => {
                        #[cfg(feature = "track-stats")]
                        global_tiered_cache().record_tier_execution(ExecutionTier::JitStage1);
                        return (SmallVec::from_vec(results), new_env);
                    }
                    Err(_) => {
                        // JIT execution failed, fall through
                    }
                }
            }
        }

        // Bytecode VM (warm code, 2+ executions)
        if compilation_state.bytecode_status() == TierStatusKind::Ready {
            if let Some(chunk) = compilation_state.bytecode_chunk() {
                // Execute via generic bytecode VM (zero-conversion)
                match execute_arena(chunk, env.clone()) {
                    Ok((results, new_env, unreduced)) => {
                        if unreduced {
                            // Bytecode couldn't reduce — fall through
                        } else {
                            #[cfg(feature = "track-stats")]
                            global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);
                            return (SmallVec::from_vec(results), new_env);
                        }
                    }
                    Err(_) => {
                        // Bytecode execution failed, fall through to tree-walker
                    }
                }
            }
        }
    }

    // Check pre-compiled built-in registry — zero compilation overhead for
    // common operations like (+, -, *, /, car-atom, etc.)
    if !has_overridden_grounded {
        if let Some(items) = value.as_sexpr() {
            if let Some(head_atom) = items.first().and_then(|v| v.as_atom()) {
                let arity = (items.len() - 1) as u8;
                if let Some(builtin_chunk) =
                    crate::backend::bytecode::builtin_chunks::get_builtin_chunk(head_atom, arity)
                {
                    // Evaluate arguments first (they may need reduction)
                    let mut arg_values = SmallVec::<[MettaValue; 4]>::with_capacity(arity as usize);
                    let mut current_env = env.clone();
                    let mut all_single = true;
                    for arg in &items[1..] {
                        let (results, new_env) = eval_trampoline(arg.clone(), current_env, state);
                        current_env = (*new_env).clone();
                        if results.len() == 1 {
                            arg_values.push(results.into_iter().next().expect("len checked").0);
                        } else {
                            all_single = false;
                            break;
                        }
                    }
                    if all_single {
                        let factory = current_env.factory().clone();
                        let mut vm =
                            crate::backend::bytecode::GenericBytecodeVM::with_env_and_factory(
                                builtin_chunk,
                                current_env.clone(),
                                factory.clone(),
                            );
                        for arg in arg_values {
                            vm.value_stack.push(arg);
                        }
                        if let Ok((results, _env_opt)) = vm.run_with_env() {
                            if !results.is_empty() {
                                return (SmallVec::from_vec(results), current_env);
                            }
                        }
                    }
                }
            }
        }
    }

    // Try environment-aware bytecode for expressions that need rule dispatch.
    // Cache compiled chunks in TieredCache to avoid recompilation on every call.
    // Use cached compilability check to avoid redundant recursive tree walks.
    let compilable_with_env = compilation_state
        .cached_compilable_with_env()
        .unwrap_or_else(|| {
            let result = can_compile_with_env(&value);
            compilation_state.set_compilable_with_env(result);
            result
        });
    // Environment-aware bytecode is only valid for pure, closed rule-backed
    // calls. Keep this gate aligned with trampoline sub-expression dispatch.
    let has_impure_rules = expression_involves_impure_rules(&value, &env);
    let has_meta_typed = expression_has_declared_meta_typed_params(&value, &env);
    if compilable_with_env && !has_impure_rules && !has_meta_typed && !has_overridden_grounded {
        // Reuse compilation_state from the record_execution at line 371 —
        // same expression hash, avoids redundant DashMap lookup + hash computation.
        let compilation_state_env = &compilation_state;
        let cached_chunk = compilation_state_env.bytecode_chunk();

        let vm_result = if let Some(chunk) = cached_chunk {
            // Cache hit — execute cached chunk directly (no recompilation).
            // yield_on_top_return exhausts all nondeterministic alternatives
            // within a single run() call, eliminating tree-walker fallback.
            let factory = env.factory().clone();
            let mut vm = crate::backend::bytecode::GenericBytecodeVM::with_env_and_factory(
                chunk,
                env.clone(),
                factory.clone(),
            );
            vm.yield_on_top_return = true;
            vm.run()
                .map(|results| {
                    let unreduced = vm.unreduced || vm.had_unreduced_result;
                    let has_choices = vm.choice_points_len() > 0;
                    let final_env = vm.env.take().unwrap_or_else(|| {
                        crate::backend::environment::core::MettaEnvironment::new(factory)
                    });
                    (results, final_env, unreduced, has_choices)
                })
                .ok()
        } else {
            // Cache miss — compile, cache, and execute
            match eval_bytecode_arena_with_env(&value, env.clone()) {
                Ok((results, new_env, unreduced, has_choices)) => {
                    // Cache the compiled chunk for future reuse
                    if compilation_state_env.try_start_bytecode_compile() {
                        if let Ok(chunk) =
                            crate::backend::bytecode::compile_bytecode_arc("cached_env", &value)
                        {
                            compilation_state_env.set_bytecode_ready(chunk);
                        }
                    }
                    Some((results, new_env, unreduced, has_choices))
                }
                Err(_) => None,
            }
        };

        if let Some((results, new_env, unreduced, has_choices)) = vm_result {
            if !unreduced && !has_choices {
                #[cfg(feature = "track-stats")]
                global_tiered_cache().record_tier_execution(ExecutionTier::Bytecode);

                // S2 BANG-WORD (2026-05-13): when the original expression is
                // a `(! ...)` directive, the bytecode VM has already done a
                // full INTERPRET-mode evaluation (EnterInterpretMode +
                // ExitInterpretMode bracket the body). The post-VM trampoline
                // still runs to finish reducing non-normal-form results
                // (e.g., `(first-from-pair (a b))` returned unreduced by the
                // bytecode tier needs T0 to dispatch the special-form arm),
                // BUT it must run with `bang_body=true` set on the env so
                // the T0 `=` / `:` arms recognize they're still inside the
                // `!` body and treat decl-atoms as data instead of
                // registering them. Otherwise `(! (= foo bar))` → VM
                // returns `(= foo bar)` → post-VM T0 runs with bang_body=
                // false (cleared by ExitInterpretMode) → `=` arm registers
                // the rule and emits `[]`.
                let is_bang_directive = value
                    .as_sexpr()
                    .and_then(|items| items.first())
                    .and_then(|h| h.as_atom())
                    .is_some_and(|s| s == "!");

                // Complete evaluation via trampoline (returns immediately for
                // normal forms via O(1) bloom filter check).
                let mut final_results = SmallVec::with_capacity(results.len());
                let mut final_env = new_env;
                if is_bang_directive {
                    final_env.set_bang_body(true);
                }
                for result in results {
                    let (sub_results, sub_env) = eval_trampoline(result, final_env, state);
                    final_results.extend(sub_results.into_iter().map(|(v, _)| v));
                    final_env = (*sub_env).clone();
                    // Re-assert bang_body across trampoline iterations — the
                    // trampoline may propagate a cleared flag back via env
                    // cloning paths that pre-date S2.
                    if is_bang_directive {
                        final_env.set_bang_body(true);
                    }
                }
                if is_bang_directive {
                    // Clear bang_body before returning so the runner's next
                    // directive starts with a clean slate (mirrors `eval()`'s
                    // per-directive reset).
                    final_env.set_bang_body(false);
                }
                return (final_results, final_env);
            }
        }
    }

    // Tier 0: Tree-walker interpreter (cold code or fallback)
    #[cfg(feature = "track-stats")]
    global_tiered_cache().record_tier_execution(ExecutionTier::Interpreter);
    let (results, shared_env) = eval_trampoline(value, env, state);
    (
        results.into_iter().map(|(v, _)| v).collect(),
        (*shared_env).clone(),
    )
}

/// Recursively check if any sub-expression's head has rules that use `(cut)`.
/// Used to gate the bytecode path: the bytecode VM doesn't implement cut.
pub(crate) fn expression_involves_cut_rules(value: &MettaValue, env: &MettaEnvironment) -> bool {
    expression_involves_rule_rhs_atom(value, env, &["cut"])
}

pub(crate) fn expression_involves_impure_rules(value: &MettaValue, env: &MettaEnvironment) -> bool {
    expression_involves_cut_rules(value, env)
        || expression_involves_rule_rhs_atom(
            value,
            env,
            &[
                "println!",
                "trace!",
                "add-atom",
                "remove-atom",
                "change-state!",
                "bind!",
                "import!",
                "include",
                "new-state",
                "new-space",
            ],
        )
}

/// REJECTED-alternative note (experiment #16, 2026-06-11): an index-native
/// copy-free walk (IndexHeap-level, child Addrs in place, verdict-identical —
/// conformance 483/0 + an 11-case differential oracle green) replaced this
/// function's materializing accessors and measured Robot FANOUT=0 at
/// p=0.416, d=-0.04 (n=51/arm, interleaved) — PURE NULL. That refuted the
/// frame-pointer profile's claim that ~18% of Robot wall was memmove called
/// from this gate: FP unwinds through the FRAMELESS AVX memmove leaf walk a
/// stale rbp chain and mis-attribute the caller. The gate's shadow accesses
/// are evidently warm cache hits (cf. exp14: the epoch CHECK, not
/// materialization, was that path's cost). Patch archived at
/// docs/cesk-gc/rejected-patches/exp16-copy-free-gate-walk.patch; do not
/// re-attempt from sampled-attribution data — use exact call graphs
/// (callgrind) or an intervention experiment.
fn expression_involves_rule_rhs_atom(
    value: &MettaValue,
    env: &MettaEnvironment,
    needles: &[&str],
) -> bool {
    if let Some(head) = value.get_head_symbol() {
        if needles
            .iter()
            .any(|needle| env.rule_rhs_contains_atom(head, needle))
        {
            return true;
        }
    }
    if let Some(items) = value.as_sexpr() {
        return items
            .iter()
            .any(|item| expression_involves_rule_rhs_atom(item, env, needles));
    }
    false
}

/// Gate: skip the bytecode VM path when any sub-expression's head has a
/// declared arrow type whose parameter positions include MeTTa meta-types
/// (`Atom`, `Expression`, `Symbol`, etc.). The bytecode VM compiles user
/// calls with eager applicative arg evaluation (fast Cartesian-preserving
/// path), which violates HE's `interpret_function` semantics when the head
/// declares a meta-typed parameter — HE passes meta-typed args unevaluated.
///
/// The tree-walker trampoline (`find_typed_arg_indices_generic` in
/// `eval/step/sexpr.rs`) correctly honors per-arg meta-type declarations.
/// Routing meta-typed calls through the trampoline preserves both
/// HE bisimilarity for declared meta-types AND the existing applicative
/// Cartesian fanout for value-typed args.
///
/// Returns `true` if any call head (anywhere in the expression tree) has
/// an explicitly declared arrow type with at least one meta-typed formal
/// parameter. Inferred types are ignored — only user-declared `(: f (-> ...))`
/// assertions trigger this gate, matching HE's "declared types win" rule.
pub(crate) fn expression_has_declared_meta_typed_params(
    value: &MettaValue,
    env: &MettaEnvironment,
) -> bool {
    // Hot-path fast exit: if the env has ANY atoms with declared type
    // assertions (type_bloom non-empty for any key), walk the tree. Otherwise
    // return false immediately in O(1). mmverify declares zero type
    // assertions, so this shortcuts every eval_inner invocation to O(1).
    if !env.has_any_declared_types() {
        return false;
    }
    expression_has_declared_meta_typed_params_recursive(value, env)
}

#[inline]
fn expression_has_declared_meta_typed_params_recursive(
    value: &MettaValue,
    env: &MettaEnvironment,
) -> bool {
    use crate::backend::eval::step::{extract_arg_types, is_meta_type};

    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if env.may_have_type(head) {
                let op_types = env.get_types_generic(head);
                for t in &op_types {
                    if let Some(arg_types) = extract_arg_types(t) {
                        if arg_types.iter().any(is_meta_type) {
                            return true;
                        }
                    }
                }
            }
        }
        return items
            .iter()
            .any(|item| expression_has_declared_meta_typed_params_recursive(item, env));
    }
    false
}

/// Gate: skip the bytecode/JIT tiers when any sub-expression's head is an
/// overridable grounded op that has a user rule currently installed. The
/// bytecode compiler emits direct grounded opcodes (e.g. `StructuralHead` for
/// `car-atom`, dedicated handlers for `map-atom`/`filter-atom`/`foldl-atom`)
/// that bypass user-rule dispatch; the tree-walker trampoline honors the
/// override via the dispatch arm at `sexpr.rs:1393-1412`. To keep MeTTaTron
/// bisimilar with HE (where stdlib.metta defines these names as overridable
/// rules), we route such expressions through the trampoline so user rules
/// take precedence.
///
/// Fast exit: if no override bit is set on the env's `DispatchOverrides`,
/// return `false` in O(1) without walking the expression tree.
pub(crate) fn expression_has_overridden_grounded_op(
    value: &MettaValue,
    env: &MettaEnvironment,
) -> bool {
    let overrides = env.dispatch_overrides();
    if !overrides.any_overridden() {
        return false;
    }
    expression_has_overridden_grounded_op_recursive(value, overrides)
}

#[inline]
fn expression_has_overridden_grounded_op_recursive(
    value: &MettaValue,
    overrides: &crate::backend::environment::dispatch_overrides::DispatchOverrides,
) -> bool {
    use crate::backend::environment::dispatch_overrides::overridable_op_id;

    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            // Data-treating heads: their args are patterns/data, not evaluated
            // calls. A `car-atom` appearing inside `(= ...)`, `(quote ...)`,
            // `(add-atom &s ...)`, or `(remove-atom &s ...)` is a syntactic
            // occurrence — the bytecode/VM routes such forms through
            // non-grounded opcodes (DefineRule, SpaceAdd, SpaceRemove, etc.)
            // that never materialize the grounded `car-atom` call path, so
            // the override bit shouldn't gate them.
            match head {
                "=" | "quote" | "add-atom" | "remove-atom" => return false,
                _ => {}
            }
            if let Some(id) = overridable_op_id(head) {
                if overrides.is_overridden(id) {
                    return true;
                }
            }
        }
        return items
            .iter()
            .any(|item| expression_has_overridden_grounded_op_recursive(item, overrides));
    }
    false
}

/// Gate: skip the bytecode VM path when the expression tree contains any
/// PT-canonical translator form (`prog1`, `forall`, `foldall`, `|->`,
/// `translatePredicate`) or any side-effecting top-level grounded op
/// (`println!`) whose T1 lowering does not match T0.
///
/// The T0 trampoline (`eval/step/sexpr.rs`) implements these forms via
/// rewrite-to-let or rewrite-to-eval. T1's bytecode compiler has no
/// matching opcodes, so the VM falls through to the data-constructor
/// path and returns the raw S-expression. To preserve T0↔T1 bisimilarity
/// without duplicating logic across tiers, route any expression that
/// mentions these forms through T0.
///
/// Mandate compliance: this is compile-time tier selection, not a
/// runtime gate or env-var/feature/CLI switch.
pub(crate) fn expression_has_t0_only_form(value: &MettaValue) -> bool {
    if let Some(items) = value.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            match head {
                "prog1" | "forall" | "foldall" | "|->" | "translatePredicate"
                // PHE-012: top-level (println! ...) must return Unit ((); T0's
                // dispatch arm at `step/sexpr.rs` handles this) — T1's
                // compile_call emits an (println! ...) SExpr that the VM
                // never reduces to Unit.
                | "println!"
                // PHE-008 fixture 045: get-type on a declared symbol should
                // return just the declared type, not also the declaration
                // itself. T0's special-form arm at `step/sexpr.rs` handles
                // this correctly via `infer_type_generic`. T1's GetType
                // opcode + the directive-loop's auto-add behavior emits
                // both the `(: foo Number)` declaration AND the inferred
                // `Number` — duplicating the surface output.
                | "get-type" => return true,
                // PHE-007 fixture 044: `(case (no-rule) ((1 first) (Empty fallback)))`
                // — T0 implements PT-canonical case-Empty-default fallback
                // (`trampoline/eval_loop.rs:10801-10817`): when no scrutinee atom
                // matches any case arm AND the cases include an `(Empty default)`
                // arm, fire the default via negation-as-failure. T1's compile_case
                // emits Pop+Fail on no-match, never consulting the `(Empty …)`
                // arm. Route case-with-Empty-arm expressions through T0 to
                // preserve PT-canonical fallback semantics.
                "case" => {
                    if items.len() >= 3 {
                        if let Some(arms) = items[2].as_sexpr() {
                            for arm in arms {
                                if let Some(arm_items) = arm.as_sexpr() {
                                    if arm_items.len() == 2
                                        && arm_items[0].as_atom() == Some("Empty")
                                    {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        return items.iter().any(expression_has_t0_only_form);
    }
    false
}

/// Execute JIT-compiled code for arena expression with environment threading.
pub(crate) fn execute_jit_arena_with_env(
    state: &std::sync::Arc<crate::backend::bytecode::ExprCompilationState>,
    native_ptr: *const (),
    env: MettaEnvironment,
) -> Result<(Vec<MettaValue>, MettaEnvironment), ()> {
    use crate::backend::bytecode::jit::HybridExecutor;
    use crate::backend::models::{active_factory, global_allocator};

    // Get allocator and factory from global singleton (factory via the
    // GC-migration seam `active_factory()`).
    let allocator = global_allocator();
    let factory = active_factory();

    // Get the bytecode chunk (needed for constants)
    let chunk = state.bytecode_chunk().ok_or(())?;

    // Create hybrid executor and run in arena mode with environment
    let mut executor = HybridExecutor::new();
    executor
        .execute_jit_arena_with_env(&chunk, native_ptr, allocator, &factory, env)
        .map_err(|_| ())
}
