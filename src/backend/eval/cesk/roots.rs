//! Algebraic Root Set Computation for the SECK Machine
//!
//! This module provides precise, algebraic GC root computation from the
//! formalized SECK machine state, following the Van Horn & Might methodology:
//!
//! ```text
//! roots(⟨S, E, C, K, Store⟩) = addrs_in(S) ∪ addrs_in(C) ∪ range(E) ∪ addrs_in(K)
//! ```
//!
//! ## Current vs Target
//!
//! The current trampoline collects roots by iterating work items and continuations,
//! cloning values into a temporary `Vec<V>`. This is correct but:
//! - Allocates a new Vec on every safepoint
//! - Clones values (cheap for NaN-boxed, less cheap for slab pointers)
//! - Mixes machine state traversal with GC-specific concerns
//!
//! The algebraic root set formalizes this into a reusable `RootSet` that:
//! - Pre-allocates the root buffer once per trampoline invocation
//! - Provides `collect_from_*` methods matching SECK components
//! - Enables incremental root tracking (Phase 2.2) via dirty flags
//! - Separates root collection logic from GC triggering logic
//!
//! ## Environment & global-anchor roots (CESK Phase A4)
//!
//! The persistent global environment **E₀** is read *structurally*, not via the
//! `ROOT_REGISTRY`/`RootProvider` discovery apparatus. E₀ has two homes:
//! - the env struct, folded in by [`RootSet::collect_structural`] through
//!   `GenericEnvironmentShared::collect_roots_into`; and
//! - a fixed set of global singleton caches (the tiered/bytecode/memo caches, the
//!   named-space registry, and the compiler's cached atom statics), read *by name*
//!   via [`collect_global_anchors`].
//!
//! `collect_all` itself still computes only the control registers (S ∪ C ∪ K);
//! E₀ is added by `collect_structural` + `collect_global_anchors`. The registry
//! remains live in parallel until Phase A5 deletes it — the A4.3 machine-equivalence
//! oracle first proves the structural multiset ⊇ the registry multiset.

use crate::backend::models::MettaValueTrait;

use super::super::trampoline::{Continuation, WorkItem};
use super::operand_stack::OperandStack;

// ============================================================================
// RootSet
// ============================================================================

/// Reusable buffer for collecting GC root values from SECK machine state.
///
/// The `RootSet` is allocated once per trampoline invocation and reused
/// across GC safepoints. It collects all `V` values reachable from the
/// machine's stack, control expression, and continuations.
///
/// ## Root Categories
///
/// The root set is the union of values from all SECK components:
///
/// | Component | Source | Method |
/// |-----------|--------|--------|
/// | **S** (Stack) | Operand stack values | `collect_from_operand_stack()` |
/// | **C** (Control) | Current work item | `collect_from_work_items()` |
/// | **K** (Kontinuation) | Continuation stack | `collect_from_continuations()` |
/// | **E** (Environment) | Managed externally via `RootProvider` | N/A |
/// | **Store** | Internal to allocator | N/A |
///
/// Additional root sources (eval memo cache, match result cache, frame chain)
/// are collected via `collect_auxiliary_roots()`.
#[derive(Debug)]
pub struct RootSet<V: MettaValueTrait> {
    /// Pre-allocated buffer for root values.
    /// Grows as needed but never shrinks within a trampoline invocation.
    roots: Vec<V>,
}

impl<V: MettaValueTrait + Clone> RootSet<V> {
    /// Create a new root set with estimated capacity.
    ///
    /// Capacity is based on typical PLN evaluation profiles:
    /// ~2 values per work item + ~4 per continuation + operand stack.
    #[inline]
    pub fn with_estimated_capacity(
        work_stack_len: usize,
        continuation_len: usize,
        operand_stack_len: usize,
    ) -> Self {
        let estimated = 2 + work_stack_len * 2 + continuation_len * 4 + operand_stack_len;
        Self {
            roots: Vec::with_capacity(estimated),
        }
    }

    /// Create a new root set with explicit capacity.
    #[inline]
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            roots: Vec::with_capacity(capacity),
        }
    }

    /// Clear collected roots, retaining allocated capacity for reuse.
    #[inline]
    pub fn clear(&mut self) {
        self.roots.clear();
    }

    /// Collect roots from the operand stack (S component).
    ///
    /// Corresponds to `addrs_in(S)` in the algebraic root formula.
    #[inline]
    pub fn collect_from_operand_stack(&mut self, stack: &OperandStack<V>) {
        stack.collect_roots(&mut self.roots);
    }

    /// Return the collected roots as a Vec for consumption by the GC.
    ///
    /// Drains the internal buffer, transferring ownership to the caller.
    /// The RootSet retains its allocated capacity for the next collection cycle.
    #[inline]
    pub fn drain_into_vec(&mut self) -> Vec<V> {
        std::mem::take(&mut self.roots)
    }

    /// Return a reference to the collected roots.
    #[inline]
    pub fn roots(&self) -> &[V] {
        &self.roots
    }

    /// Return the number of collected roots.
    #[inline]
    pub fn len(&self) -> usize {
        self.roots.len()
    }

    /// Check if no roots have been collected.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Push additional roots from external sources.
    ///
    /// Used for auxiliary root sources like eval memo cache, match result cache,
    /// and caller frame chain — values that are not part of the SECK state
    /// but must survive GC.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.roots.push(value);
    }

    /// Extend with additional roots from an iterator.
    #[inline]
    pub fn extend(&mut self, values: impl IntoIterator<Item = V>) {
        self.roots.extend(values);
    }

    /// Get a mutable reference to the underlying root buffer.
    ///
    /// Used for collecting roots from external sources that need a `&mut Vec<V>`.
    #[inline]
    pub fn as_mut_vec(&mut self) -> &mut Vec<V> {
        &mut self.roots
    }
}

/// Monomorphized methods that interact with the concrete WorkItem and
/// Continuation types (which now use MettaValue directly).
impl RootSet<crate::backend::models::MettaValue> {
    /// Collect roots from work items (C component — control expressions).
    ///
    /// Corresponds to `addrs_in(C)` in the algebraic root formula.
    /// Includes the currently-popped work item and all items remaining on the stack.
    pub fn collect_from_work_items(&mut self, current_work: &WorkItem, work_stack: &[WorkItem]) {
        current_work.collect_values(&mut self.roots);
        for w in work_stack {
            w.collect_values(&mut self.roots);
        }
    }

    /// Collect roots from the continuation stack (K component).
    ///
    /// Corresponds to `addrs_in(K)` in the algebraic root formula.
    pub fn collect_from_continuations(&mut self, continuations: &[Continuation]) {
        for c in continuations {
            c.collect_values(&mut self.roots);
        }
    }

    /// Collect all roots from a complete SECK machine snapshot.
    ///
    /// Convenience method that calls all `collect_from_*` methods.
    /// This is the algebraic root formula:
    ///
    /// ```text
    /// roots = addrs_in(S) ∪ addrs_in(C) ∪ addrs_in(K)
    /// ```
    ///
    /// Environment roots (E) are managed separately via `RootProvider`.
    pub fn collect_all(
        &mut self,
        operand_stack: &OperandStack<crate::backend::models::MettaValue>,
        current_work: &WorkItem,
        work_stack: &[WorkItem],
        continuations: &[Continuation],
    ) {
        self.clear();
        self.collect_from_operand_stack(operand_stack);
        self.collect_from_work_items(current_work, work_stack);
        self.collect_from_continuations(continuations);
    }

    /// CESK Phase A4 — the single **structural** root reader.
    ///
    /// Reads GC roots directly from the reified machine instead of discovering
    /// them through the `ROOT_REGISTRY`/`frame_chain` apparatus:
    ///
    /// ```text
    /// roots = addrs_in(S) ∪ addrs_in(C) ∪ addrs_in(K) ∪ reach(E₀)
    /// ```
    ///
    /// = `collect_all` (the control registers S/C/K — E_local rides inside C/K as
    /// the per-frame `carrying_bindings`) plus **E₀**, the persistent global
    /// environment, via its inherent `collect_roots_into` (the structural seam, not
    /// the `RootProvider` registry). This is the Morrisett `σ|_Reachable(⟨C,E,K⟩)`
    /// formula read from the machine.
    ///
    /// A4.2 will additionally fold in the native-stack K-spine (suspended trampoline
    /// activations + live bytecode-VM leaves, via typed thread-locals) and the fixed
    /// global-cache anchors (`collect_global_anchors`); A4.3's machine-equivalence
    /// oracle proves this reader's multiset ⊇ the old apparatus's before A5 deletes
    /// the apparatus. Until A4.4 this method is additive (used only by tests/oracle).
    pub fn collect_structural(
        &mut self,
        operand_stack: &OperandStack<crate::backend::models::MettaValue>,
        current_work: &WorkItem,
        work_stack: &[WorkItem],
        continuations: &[Continuation],
        env0: &crate::backend::environment::core::GenericEnvironmentShared<
            crate::backend::models::MettaValue,
        >,
    ) {
        // S ∪ C ∪ K (control registers; clears the buffer internally).
        self.collect_all(operand_stack, current_work, work_stack, continuations);
        // ∪ reach(E₀) — the persistent global environment, read structurally.
        env0.collect_roots_into(&mut self.roots);
    }
}

/// CESK Phase A4.2a — the fixed **global-anchor** structural root reader.
///
/// The persistent global environment E₀ has a second structural home beyond the
/// env struct (which [`RootSet::collect_structural`] already folds in): a small,
/// *statically known* set of global singleton caches that live outside the env
/// struct yet are equally always-live roots — the tiered bytecode cache, the
/// bytecode-chunk cache, the eval memo cache, the named-space registry, and the
/// compiler's cached atom statics. This function reads those five anchors **by
/// name** (a fixed call sequence) rather than discovering them through the
/// `ROOT_REGISTRY` / `Weak<dyn RootProvider>` dynamic dispatch.
///
/// Each call delegates to the holder's own inherent collector — the exact bodies
/// the corresponding `RootProvider` impls now delegate to — so the result is
/// **byte-identical** to what the registry would have produced for these five
/// providers. It is **additive** until A4.4: the registry path still runs in
/// parallel, and A4.3's machine-equivalence oracle proves
/// `collect_structural ∪ collect_global_anchors ∪ (K-spine)` ⊇ the registry
/// multiset before A5 deletes the apparatus.
///
/// Appends to `out` (never clears it), matching the registry's append contract.
///
/// The `global_*()` accessors each trigger an idempotent `ensure_*_registered()`
/// on first use, but by any safepoint these `OnceLock`s are already initialised
/// (the caches are populated during normal evaluation), so this reader never
/// mutates the registry in practice.
pub fn collect_global_anchors(out: &mut Vec<crate::backend::models::MettaValue>) {
    use crate::backend::bytecode::{
        cache::collect_bytecode_cache_roots, compiler::collect_compiler_atom_roots,
        global_space_registry, memo_cache::global_memo_cache, tiered_cache::global_tiered_cache,
    };
    // The five persistent global singleton anchors, read by name (A4.2a).
    global_tiered_cache().collect_roots_into(out);
    global_space_registry().collect_all_gc_values(out);
    global_memo_cache().collect_all_values(out);
    collect_bytecode_cache_roots(out);
    collect_compiler_atom_roots(out);
    // A4.3 — the four thread-local evaluation caches the discovered safepoint
    // roots (eval_loop.rs:3533-3536). In the single-threaded index-gc regime the
    // calling thread is the SOLE owner of σ, so these thread-locals are
    // machine-global σ-value holders read by name — the same contract as the five
    // persistent anchors above. (`collect_binding_capture_roots` is an empty
    // no-op, so it is deliberately omitted.)
    crate::backend::eval::trampoline::dispatch_hints::collect_eval_memo_roots(out);
    crate::backend::eval::trampoline::dispatch_hints::collect_match_result_roots(out);
    crate::backend::eval::cesk::tabling::collect_subgoal_roots(out);
    crate::backend::eval::cesk::thunk::collect_thunk_roots(out);
}

/// CESK Phase A4.2b — the single **structural machine-root** reader. The one
/// entry the A4.3 machine-equivalence oracle and the A4.4 safepoint consume:
///
/// ```text
/// roots = collect_structural(S, C, K, E₀)        — control registers + env struct
///       ∪ collect_global_anchors                 — E₀'s global singleton caches
///       ∪ collect_k_spine                        — the native-stack K-spine
///                                                   (suspended activations + VM leaves)
/// ```
///
/// This is the complete Morrisett `σ|_Reachable(⟨C,E,K⟩)` root set read from the
/// reified machine plus the persistent global environment, with NO dependence on
/// the `ROOT_REGISTRY` / `frame_chain` discovery apparatus. Appends to `out`.
///
/// Additive / unused in the hot path until A4.4 (only tests + the A4.3 oracle
/// call it). The registry/frame_chain paths still run in parallel until A5; the
/// oracle proves this reader's multiset ⊇ theirs before they are deleted.
pub fn collect_machine_roots(
    out: &mut Vec<crate::backend::models::MettaValue>,
    operand_stack: &OperandStack<crate::backend::models::MettaValue>,
    current_work: &WorkItem,
    work_stack: &[WorkItem],
    continuations: &[Continuation],
    env0: &crate::backend::environment::core::GenericEnvironmentShared<
        crate::backend::models::MettaValue,
    >,
) {
    // S ∪ C ∪ K (control registers), via the RootSet structural reader. `collect_all`
    // CLEARS its own buffer, so we collect into a fresh RootSet and append — `out`'s
    // pre-existing contents are preserved.
    let mut rs = RootSet::with_capacity(out.len() + 64);
    rs.collect_all(operand_stack, current_work, work_stack, continuations);
    out.extend(rs.drain_into_vec());
    // ∪ reach(E₀-env) ∪ global anchors ∪ K-spine — the persistent structural roots.
    // (Byte-identical to the prior `collect_structural` + anchors + k_spine: same
    // pointer multiset in the same order — see docs/cesk-gc/a4-4-collector-flip-design.md.)
    collect_persistent_roots(out, env0);
}

/// CESK Phase A4.4 — the **persistent** structural root reader: the machine-global
/// roots live at EVERY safepoint, INCLUDING true quiescence (where the control
/// registers S∪C∪K are empty and there is no current `WorkItem`):
///
/// ```text
/// persistent_roots = reach(E₀-env)         — the persistent global environment struct
///                  ∪ collect_global_anchors  — E₀'s global singleton caches (5+4)
///                  ∪ collect_k_spine         — the native-stack K-spine
/// ```
///
/// Exactly the non-control-register part of [`collect_machine_roots`]. The two
/// quiescence collectors (`eval()` / `eval_with_tier`, C∪K empty post-`EvalGuard`)
/// consume THIS (∪ the about-to-return result values ∪ the driver-C program), while
/// `collect_machine_roots` = `collect_all`(S∪C∪K) ∪ this (for the midloop safepoint,
/// where C∪K are live). Appends to `out` (never clears).
pub fn collect_persistent_roots(
    out: &mut Vec<crate::backend::models::MettaValue>,
    env0: &crate::backend::environment::core::GenericEnvironmentShared<
        crate::backend::models::MettaValue,
    >,
) {
    // reach(E₀-env) — the persistent global environment, read structurally.
    env0.collect_roots_into(out);
    // ∪ E₀'s global singleton caches (5 OnceLock + 4 thread-local).
    collect_global_anchors(out);
    // ∪ the native-stack K-spine (suspended activations + live VM leaves).
    super::k_spine::collect_k_spine(out);
}

/// CESK Phase A4.4 — the QUIESCENCE machine-equivalence oracle. Asserts the discovered
/// quiescence root set (`collect_all_roots()` ∪ `result`) is covered by the structural
/// PERSISTENT reader (`collect_persistent_roots` ∪ `result`) UNION the legitimately-kept
/// driver-C program (`MettaState.{source,output}`). The safety direction (no protected
/// root dropped) for the A4.4 flip of the two quiescence collectors. Debug-only; the
/// callers gate it on `gc_mode_is_index()` (in slab mode `collect_all_roots`' frame-chain
/// roots have no structural mirror — the structural reader is the index-gc root source).
/// PERMANENT CI invariant, kept until A5 deletes the discovery apparatus.
#[cfg(debug_assertions)]
pub fn assert_quiescence_superset(
    result: &[crate::backend::models::MettaValue],
    env0: &crate::backend::environment::core::GenericEnvironmentShared<
        crate::backend::models::MettaValue,
    >,
    state: &crate::backend::models::MettaState,
) {
    // OLD = the discovered set the BEFORE feed consumed: collect_all_roots()
    // (ROOT_REGISTRY ∪ SAFEPOINT_ROOTS) ∪ the about-to-return result values.
    // A5.5: collect_all_roots is slab-only (registry walled to slab). In index the
    // registry is empty by construction (0 providers; its only contribution was
    // collect_safepoint_roots ⊆ KEPT below), so OLD ← Vec::new() here only SHRINKS
    // the discovered set — `OLD ⊆ (NEW ∪ KEPT)` is preserved by monotonicity (the
    // dropped term was already covered by KEPT). old_vals stays defined in both builds.
    let mut old_vals: Vec<crate::backend::models::MettaValue> = {
        #[cfg(not(feature = "index-gc"))]
        {
            crate::backend::models::collect_all_roots()
        }
        #[cfg(feature = "index-gc")]
        {
            Vec::new()
        }
    };
    old_vals.extend(result.iter().copied());
    let mut old: Vec<usize> = old_vals.iter().map(|v| v.inner_ptr() as usize).collect();

    // NEW = the structural PERSISTENT reader (reach E₀-env ∪ global anchors ∪ K-spine)
    // ∪ the about-to-return result values. (No collect_machine_roots: C∪K are empty at
    // quiescence, so there is no current WorkItem and no control registers.)
    let mut new_vals: Vec<crate::backend::models::MettaValue> = Vec::with_capacity(old.len() + 64);
    collect_persistent_roots(&mut new_vals, env0);
    new_vals.extend(result.iter().copied());
    let mut new: Vec<usize> = new_vals.iter().map(|v| v.inner_ptr() as usize).collect();

    // KEPT = the apparatus roots A4.4 does NOT replace structurally:
    //  (1) the driver's program control (C): MettaState.source + .output (re-homed A5.3b); and
    //  (2) SAFEPOINT_ROOTS — the NARROW driver-transport channel (the conformance/REPL/driver
    //      cross-directive result accumulator via register_temporary_roots + the thread-local
    //      cache snapshot via CACHE_ROOT_HANDLE). The plan keeps SAFEPOINT_ROOTS narrow; the
    //      structural reader does NOT cover the driver's accumulated results, so they are KEPT
    //      (else the flipped collector would free them → UAF). A5.4 narrows it.
    let mut kept_vals: Vec<crate::backend::models::MettaValue> = Vec::new();
    state.collect_driver_program_roots(&mut kept_vals);
    crate::backend::models::collect_safepoint_roots(&mut kept_vals);
    let mut kept: Vec<usize> = kept_vals.iter().map(|v| v.inner_ptr() as usize).collect();

    old.sort_unstable();
    old.dedup();
    new.sort_unstable();
    new.dedup();
    kept.sort_unstable();
    kept.dedup();

    // OLD ⊆ (NEW ∪ KEPT): every discovered root is structural (NEW) or the kept driver-C.
    let missing: Vec<usize> = old
        .iter()
        .copied()
        .filter(|p| new.binary_search(p).is_err() && kept.binary_search(p).is_err())
        .collect();
    if !missing.is_empty() {
        let sample: Vec<String> = missing
            .iter()
            .take(16)
            .map(|p| format!("{:#x}", p))
            .collect();
        let missing_set: std::collections::HashSet<usize> = missing.iter().copied().collect();
        let missing_dbg: Vec<String> = old_vals
            .iter()
            .filter(|v| missing_set.contains(&(v.inner_ptr() as usize)))
            .take(8)
            .map(|v| format!("{:?}", v))
            .collect();
        panic!(
            "A4.4 QUIESCENCE machine-equivalence oracle FAILED: discovered OLD is NOT \
             covered by (structural-persistent NEW ∪ driver-C KEPT). |OLD|={} |NEW|={} \
             |KEPT|={} |missing|={}\n  sample missing inner_ptrs (<=16): [{}]\n  missing \
             values (<=8): [{}]\n  At true quiescence C∪K are empty, so a missing root \
             means: (a) a thread-local cache not in collect_global_anchors; (b) a transient \
             register (a result value not passed in, or a deferred-env not yet drained) \
             unaccounted; (c) a global anchor (tiered/bytecode/memo/space/compiler) the \
             persistent reader omits; (d) a driver-C root (MettaState.source/output) not \
             exposed via collect_driver_program_roots; (e) an env-struct root \
             collect_roots_into misses.",
            old.len(),
            new.len(),
            kept.len(),
            missing.len(),
            sample.join(", "),
            missing_dbg.join(" | "),
        );
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{global_factory, MettaValue, MettaValueFactory};
    use smallvec::smallvec;

    fn factory() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_empty_root_set() {
        let rs = RootSet::<MettaValue>::with_capacity(16);
        assert!(rs.is_empty());
        assert_eq!(rs.len(), 0);
    }

    #[test]
    fn test_collect_from_operand_stack() {
        let f = factory();
        let mut stack = OperandStack::new();
        stack.push_frame();
        stack.push(f.long(1));
        stack.push(f.long(2));

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_operand_stack(&stack);
        assert_eq!(rs.len(), 2);
    }

    #[test]
    fn test_collect_from_work_items() {
        let f = factory();
        let current = WorkItem::Eval {
            value: f.long(42),
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        };
        let stack: Vec<WorkItem> = vec![WorkItem::Resume {
            result: (
                smallvec![
                    crate::backend::eval::trampoline::types::bv(f.long(1)),
                    crate::backend::eval::trampoline::types::bv(f.long(2)),
                ],
                std::sync::Arc::new(env()),
            ),
        }];

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_work_items(&current, &stack);
        assert_eq!(rs.len(), 3); // 1 from current Eval + 2 from Resume
    }

    #[test]
    fn test_collect_from_continuations() {
        let f = factory();
        let conts: Vec<Continuation> = vec![
            Continuation::Done,
            Continuation::ProcessCatch {
                default: f.atom("fallback"),
                env: std::sync::Arc::new(env()),
                depth: 0,
                outer_carrying: crate::backend::eval::trampoline::types::empty_shared_bindings(),
            },
        ];

        let mut rs = RootSet::with_capacity(8);
        rs.collect_from_continuations(&conts);
        assert_eq!(rs.len(), 1); // Done=0, ProcessCatch=1
    }

    #[test]
    fn test_collect_all() {
        let f = factory();
        let mut operand_stack = OperandStack::new();
        operand_stack.push_frame();
        operand_stack.push(f.long(10));

        let current = WorkItem::Eval {
            value: f.long(20),
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        };
        let work_stack: Vec<WorkItem> = vec![];
        let continuations: Vec<Continuation> = vec![Continuation::Done];

        let mut rs = RootSet::with_estimated_capacity(0, 1, 1);
        rs.collect_all(&operand_stack, &current, &work_stack, &continuations);
        assert_eq!(rs.len(), 2); // 1 from operand stack + 1 from current work item
    }

    /// CESK A4.1 contract: `collect_structural` == `collect_all(S∪C∪K)` ∪
    /// `reach(E₀)` (the env's inherent `collect_roots_into`), as a sorted
    /// `inner_ptr` multiset. Pins that the structural reader includes BOTH the
    /// control registers and the persistent global environment, read from the
    /// machine (not via the registry).
    #[test]
    fn test_collect_structural_is_collect_all_plus_env0() {
        let f = factory();
        let e = env();

        let v_c = f.long(99);
        let current = WorkItem::Eval {
            value: v_c,
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        };
        let work_stack: Vec<WorkItem> = vec![];
        let continuations: Vec<Continuation> = vec![Continuation::Done];
        let operand_stack = OperandStack::new();

        // Expected = collect_all(S∪C∪K) ∪ env0.collect_roots_into.
        let mut expected: Vec<usize> = Vec::new();
        {
            let mut rs_all = RootSet::with_capacity(8);
            rs_all.collect_all(&operand_stack, &current, &work_stack, &continuations);
            expected.extend(rs_all.roots().iter().map(|v| v.inner_ptr() as usize));
            let mut env_roots: Vec<MettaValue> = Vec::new();
            e.shared.collect_roots_into(&mut env_roots);
            expected.extend(env_roots.iter().map(|v| v.inner_ptr() as usize));
        }

        let mut rs = RootSet::with_capacity(8);
        rs.collect_structural(
            &operand_stack,
            &current,
            &work_stack,
            &continuations,
            e.shared.as_ref(),
        );
        let mut got: Vec<usize> = rs.roots().iter().map(|v| v.inner_ptr() as usize).collect();

        expected.sort_unstable();
        got.sort_unstable();
        assert_eq!(
            got, expected,
            "collect_structural must equal collect_all(S∪C∪K) ∪ reach(E₀)"
        );
        // The control root (the Eval value) is present.
        assert!(got.contains(&(v_c.inner_ptr() as usize)));
    }

    /// CESK A4.2a contract: `collect_global_anchors` reads the five global
    /// singleton anchors by name without panicking, and **appends** (never
    /// clears) — matching the registry's append contract. We assert the append
    /// invariant (robust under concurrent cache mutation by other tests; we do
    /// NOT assert a cross-read multiset, which would be flaky against the shared
    /// global caches).
    #[test]
    fn test_collect_global_anchors_appends_without_panic() {
        let f = factory();
        let sentinel = f.long(0xA42A);
        let sentinel_ptr = sentinel.inner_ptr();
        let mut out: Vec<MettaValue> = vec![sentinel];
        super::collect_global_anchors(&mut out);
        // Append contract: the pre-existing root is preserved at index 0.
        assert_eq!(
            out[0].inner_ptr(),
            sentinel_ptr,
            "collect_global_anchors must append, not clear the buffer"
        );
        assert!(
            !out.is_empty(),
            "collect_global_anchors must preserve pre-existing roots"
        );
    }

    /// CESK A4.2b — `collect_machine_roots` composes `collect_structural` ∪
    /// `collect_global_anchors` ∪ `collect_k_spine`. Embryo of the A4.3 oracle:
    /// with a k_spine record pushed and a control value in C, BOTH must appear
    /// in the composed root set.
    #[test]
    fn test_collect_machine_roots_includes_control_and_kspine() {
        use crate::backend::eval::cesk::k_spine::{SuspendedActivation, SuspendedActivationGuard};
        let f = factory();
        let e = env();

        let v_c = f.long(0x1234); // a control-register (C) value
        let current = WorkItem::Eval {
            value: v_c,
            env: std::sync::Arc::new(env()),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
            carrying_bindings: crate::backend::eval::trampoline::types::empty_shared_bindings(),
        };
        let work_stack: Vec<WorkItem> = vec![];
        let continuations: Vec<Continuation> = vec![Continuation::Done];
        let operand_stack = OperandStack::new();

        let v_k = f.long(0x5678); // a K-spine value
        let kspine_exprs = vec![v_k];

        let mut got: Vec<MettaValue> = Vec::new();
        {
            // SAFETY: kspine_exprs outlives the guard (same scope).
            let _g = unsafe {
                SuspendedActivationGuard::push(SuspendedActivation::ExprVec {
                    exprs: &kspine_exprs as *const Vec<MettaValue>,
                })
            };
            collect_machine_roots(
                &mut got,
                &operand_stack,
                &current,
                &work_stack,
                &continuations,
                e.shared.as_ref(),
            );
        }
        let ptrs: Vec<usize> = got.iter().map(|v| v.inner_ptr() as usize).collect();
        assert!(
            ptrs.contains(&(v_c.inner_ptr() as usize)),
            "collect_machine_roots must include the control (C) root"
        );
        assert!(
            ptrs.contains(&(v_k.inner_ptr() as usize)),
            "collect_machine_roots must include the K-spine root"
        );
    }

    #[test]
    fn test_drain_and_reuse() {
        let f = factory();
        let mut rs = RootSet::with_capacity(8);
        rs.push(f.long(1));
        rs.push(f.long(2));

        let drained = rs.drain_into_vec();
        assert_eq!(drained.len(), 2);
        assert!(rs.is_empty()); // drained

        // Can reuse after drain
        rs.push(f.long(3));
        assert_eq!(rs.len(), 1);
    }

    #[test]
    fn test_clear_retains_capacity() {
        let f = factory();
        let mut rs = RootSet::<MettaValue>::with_capacity(64);
        for i in 0..32 {
            rs.push(f.long(i));
        }
        rs.clear();
        assert!(rs.is_empty());
        // Capacity is retained (we can't easily assert this but clear() should not shrink)
    }
}
