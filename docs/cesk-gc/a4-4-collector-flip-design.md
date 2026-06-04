# A4.4 — Collector flip: feed the index-gc collector from the STRUCTURAL reader

CESK GC migration, Phase A4.4. **Load-bearing** (behavior-changing in index mode):
this flips the three live `index-gc` collector call sites to assemble their root set
from the **structural machine reader** (`cesk::roots::collect_machine_roots` +
`cesk::roots::collect_persistent_roots`) ∪ the legitimately-kept driver-C program
(`MettaState.{source,output}`), replacing the discovered-apparatus assembly
(`collect_all_roots()` + the in-loop `RootSet`). The slab build is byte-identical
(every flipped call is `gc_mode_is_index()`-gated and const-folds dead when the
feature is off).

Designed by a Plan agent 2026-05-29 over the verified A4.1/A4.2/A4.3 state
(`roots.rs`, `k_spine.rs`, the live A4.3 midloop oracle at `eval_loop.rs:3549`, and the
driver-C seam from `a4-3-driver-c-resolution.md`). All file:line anchors below were
re-verified against source this session.

Prereqs in place (verified):
- `cesk::roots::collect_machine_roots(out, S, current_work, work_stack, K, env0)` =
  `collect_structural`(S∪C∪K∪reach E₀-env) ∪ `collect_global_anchors`(5 OnceLock + 4
  thread-local caches) ∪ `cesk::k_spine::collect_k_spine` (`roots.rs`, A4.2b).
- `collect_global_anchors(out)` and `k_spine::collect_k_spine(out)` are `pub`.
- `GenericEnvironmentShared::collect_roots_into(&self, out)` is `pub(crate)` (`core.rs:2168`).
- `MettaState::collect_driver_program_roots(&self, out)` reads `gc_roots.{source,output}`
  (`metta_state.rs:231`). `EvalContext::collect_driver_roots(&self, _out)` defaulted no-op
  (`context.rs:97`); `SessionContext` override → `state.collect_driver_program_roots`
  (`session_context.rs:177`).
- `MettaEnvironment.shared: Arc<GenericEnvironmentShared<MettaValue>>` is `pub(crate)`
  (`core.rs:489`); `env.shared.as_ref()` is already used by the A4.3 oracle.
- index-gc API: `index_gc::should_collect()` / `run_collection_if_triggered(&[MettaValue])`
  (index_heap.rs:788/923); `should_collect_midloop()` /
  `run_collection_if_triggered_midloop(&[MettaValue])` (index_heap.rs:877/948).
- `gc_mode_is_index()` = `crate::backend::models::metta_value::gc_mode_is_index()`
  (`metta_value.rs:520`, `pub(crate)`).

---

## Q-A — Shared helper vs inline: ADD `collect_persistent_roots`, REFACTOR `collect_machine_roots`

**Decision: add `cesk::roots::collect_persistent_roots(out, env0)` and refactor
`collect_machine_roots` to call it (pure factoring, byte-identical output).**

The quiescence sites (`eval()`, `eval_with_tier`) have **no `current_work`** — at
true quiescence C∪K are empty (post-`EvalGuard`) and there is no single `WorkItem` to
pass `collect_machine_roots`. They need exactly the *persistent* structural roots:
E₀-env ∪ global anchors ∪ K-spine, WITHOUT the S∪C∪K control registers. Three options:

1. **Inline the 3 calls at each quiescence site** (`env0.collect_roots_into`,
   `collect_global_anchors`, `collect_k_spine`). Rejected: duplicates the persistent-root
   recipe at 2 sites + the refactored `collect_machine_roots`; a future anchor addition
   must be made in 3 places; the quiescence oracle (Q-B) would have to re-list them too
   (5 places). Drift risk.
2. **Pass a synthetic empty `WorkItem`** to `collect_machine_roots`. Rejected: there is no
   canonical "empty" `WorkItem` (the nearest is `Eval{ value: <something> }`, which would
   inject a spurious root); contrived and fragile.
3. **Factor out `collect_persistent_roots`; `collect_machine_roots` = control-registers ∪
   `collect_persistent_roots`.** Chosen: one source of truth for the persistent recipe;
   `collect_machine_roots` stays byte-identical (proof below); the quiescence sites + their
   oracle call the same helper the midloop reader uses transitively.

### New function (additive, `roots.rs`)

```rust
/// CESK Phase A4.4 — the **persistent** structural root reader: the machine-global
/// roots that are live at EVERY safepoint, INCLUDING true quiescence (where the
/// control registers S∪C∪K are empty and there is no current `WorkItem`):
///
/// ```text
/// persistent_roots = reach(E₀-env)        — the persistent global environment struct
///                  ∪ collect_global_anchors — E₀'s global singleton caches (5+4)
///                  ∪ collect_k_spine        — the native-stack K-spine
/// ```
///
/// This is exactly the non-control-register part of `collect_machine_roots`. The two
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
```

### Refactored `collect_machine_roots` (byte-identical output)

The CURRENT body (verified `roots.rs`):

```rust
pub fn collect_machine_roots(out, operand_stack, current_work, work_stack, continuations, env0) {
    // S ∪ C ∪ K ∪ reach(E₀-env), via the RootSet structural reader.
    let mut rs = RootSet::with_capacity(out.len() + 64);
    rs.collect_structural(operand_stack, current_work, work_stack, continuations, env0);
    out.extend(rs.drain_into_vec());
    // ∪ E₀'s global singleton caches.
    collect_global_anchors(out);
    // ∪ the native-stack K-spine (suspended activations + live VM leaves).
    super::k_spine::collect_k_spine(out);
}
```

The NEW body — `collect_all`(S∪C∪K) then `collect_persistent_roots`:

```rust
pub fn collect_machine_roots(out, operand_stack, current_work, work_stack, continuations, env0) {
    // S ∪ C ∪ K (control registers), via the RootSet structural reader. NOTE:
    // `collect_all` (called inside `RootSet`) CLEARS its own buffer, so we collect
    // into a fresh RootSet and append — out's pre-existing contents are preserved.
    let mut rs = RootSet::with_capacity(out.len() + 64);
    rs.collect_all(operand_stack, current_work, work_stack, continuations);
    out.extend(rs.drain_into_vec());
    // ∪ reach(E₀-env) ∪ global anchors ∪ K-spine — the persistent structural roots.
    collect_persistent_roots(out, env0);
}
```

**Byte-identical proof.** The old body appends, in order:
`[ rs.collect_structural(...) ] ++ collect_global_anchors(out) ++ collect_k_spine(out)`.
`collect_structural` = `collect_all`(S∪C∪K) **then** `env0.collect_roots_into(&mut self.roots)`
(verified `roots.rs`), so `rs.drain_into_vec()` = `collect_all_seq ++ env0_seq`. The new body
appends `[ rs.collect_all(...) ] ++ collect_persistent_roots(out)`
= `collect_all_seq ++ ( env0_seq ++ global_anchors_seq ++ k_spine_seq )`. Both equal
`collect_all_seq ++ env0_seq ++ global_anchors_seq ++ k_spine_seq` — **the same multiset in
the same order**. The only mechanical change: in the old body `env0.collect_roots_into` ran
against `rs.roots` (the RootSet buffer) before drain; in the new body it runs against `out`
directly. Both append the identical pointers; the RootSet wrapper carried no transform. The
existing `roots.rs` tests (`test_collect_machine_roots_includes_control_and_kspine`,
`test_collect_structural_is_collect_all_plus_env0`, the A4.3 oracle's `OLD ⊆ NEW∪KEPT`) all
continue to hold unchanged — none of them is sensitive to the buffer identity, only the
resulting pointer multiset.

(`collect_structural` stays as-is — it is still called directly by
`test_collect_structural_is_collect_all_plus_env0` and is the documented A4.1 contract;
`collect_machine_roots` simply stops routing through it and instead composes `collect_all`
+ `collect_persistent_roots`, which is the cleaner decomposition the quiescence sites need.)

---

## Q-C — Flip ordering & the existing midloop oracle

**The A4.3 midloop oracle stays unchanged and stays the standing proof.** A4.4 does
**NOT** remove `collect_all_roots()` from the oracle's OLD computation — only from the
**live collector feed**. After the midloop flip:

- The oracle (eval_loop.rs:3560-3637) still computes
  `OLD = root_set.roots() ∪ collect_all_roots()` and
  `NEW = collect_machine_roots(...) ∪ deferred-drop`, `KEPT = ctx.collect_driver_roots`,
  and asserts `OLD ⊆ (NEW ∪ KEPT)`. It runs BEFORE the flipped live feed (placement: after
  the `clear_aba_sensitive_caches()`-preceding block closes, ~line 3637; the live midloop
  collector is at 3670). So at every safepoint the oracle independently re-derives the same
  `NEW ∪ KEPT` and proves it still covers the still-computed `OLD`.
- The flipped live feed (Q-flip-1 below) computes the SAME `NEW ∪ KEPT` the oracle just
  validated and hands it to `run_collection_if_triggered_midloop`. So the collector is fed
  exactly the set the oracle proved is a superset of the discovered set — the flip is a
  *proven* superset at the midloop site (this is the precise sense in which the task's
  "A4.3 oracle already PROVES `OLD ⊆ (NEW ∪ KEPT)`" makes the midloop flip safe-by-oracle).

`collect_all_roots()` (and the in-loop `RootSet` discovery assembly at 3520-3547) survive
intact until **A5** deletes the apparatus piece-by-piece. The midloop flip only changes
which Vec is passed to `run_collection_if_triggered_midloop`.

**Ordering within A4.4:** land the helper refactor (Q-A) first (byte-identical, green on
both arms with no behavior change), then the three flips together (they share the helper),
then the quiescence oracle (Q-B) — though the oracle is `#[cfg(debug_assertions)]` and can
be added in the same commit as its flip since it gates the very feed it validates. Single
commit is acceptable because the oracle (debug) + flip (release) are independently green.

---

## The three flip sites (exact before/after)

### Flip 1 — MIDLOOP safepoint (`eval_loop.rs:3670-3677`, opt-in `should_collect_midloop()`)

In scope at this site (verified): `machine_operand_stack` (S), `work` (current `WorkItem`,
C), `work_stack` (C), `continuations` (K), `deferred_shared_drops`
(`Vec<Arc<EnvShared>>`), `env` (`MettaEnvironment`; `env.shared` = E₀), `ctx`
(`&C: SessionContext` at the live `arena_engine::eval_trampoline` path), `root_set` (the
discovered `RootSet`).

**BEFORE:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {
    let mut midloop_roots: Vec<MettaValue> =
        crate::backend::models::collect_all_roots();
    midloop_roots.extend_from_slice(root_set.roots());
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered_midloop(
        &midloop_roots,
    );
}
```

**AFTER:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect_midloop() {
    // A4.4 FLIP (midloop): feed the collector from the STRUCTURAL machine reader —
    //   collect_machine_roots(S, C, K, E₀)  ∪  the deferred-drop transient register
    //   ∪  the driver-C program (MettaState.{source,output}) via ctx.collect_driver_roots.
    // This is EXACTLY the NEW ∪ KEPT the A4.3 oracle (above, ~3560) just proved is a
    // superset of the discovered OLD (collect_all_roots ∪ root_set) — so the flip is a
    // proven superset at this site. collect_all_roots()/root_set survive (still fed to the
    // oracle's OLD) until A5 deletes the apparatus.
    let mut midloop_roots: Vec<MettaValue> = Vec::with_capacity(root_set.len() + 64);
    crate::backend::eval::cesk::roots::collect_machine_roots(
        &mut midloop_roots,
        &machine_operand_stack,
        &work,
        &work_stack,
        &continuations,
        env.shared.as_ref(),
    );
    // The deferred-drop transient register (a per-activation local, not a machine-global).
    for deferred_env in &deferred_shared_drops {
        deferred_env.as_ref().collect_roots(&mut midloop_roots);
    }
    // The driver's program control (C) held ABOVE the trampoline, via the driver↔machine seam.
    ctx.collect_driver_roots(&mut midloop_roots);
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered_midloop(
        &midloop_roots,
    );
}
```

Note `deferred_env.as_ref().collect_roots(...)` is the exact call the oracle uses at
3583-3585 (the `EnvShared::collect_roots` inherent method); reuse it verbatim.

### Flip 2 — QUIESCENCE `eval()` (`eval/mod.rs:266-271`, `should_collect()`)

In scope (verified): `result.0` (`Vec`/`SmallVec<MettaValue>`, about-to-return values),
`result.1` (`MettaEnvironment`; `result.1.shared` = E₀), `state` (`&MettaState` param,
confirmed `eval(value, env, state: &MettaState)`). C∪K are EMPTY (post-`EvalGuard`); no
workers (single-threaded gate), so the parallel-dispatch providers in `collect_all_roots()`
are absent. **NB:** at this site `env` was moved into `eval_inner` and returned as
`result.1`; use `result.1.shared.as_ref()` for E₀ (NOT the consumed `env`).

**BEFORE:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect() {
    let mut roots = crate::backend::models::collect_all_roots();
    roots.reserve(result.0.len());
    roots.extend(result.0.iter().copied());
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots);
}
```

**AFTER:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect() {
    // ── A4.4 quiescence oracle (debug-only; index-gc only) ── see Q-B block below.
    #[cfg(debug_assertions)]
    if crate::backend::models::metta_value::gc_mode_is_index() {
        crate::backend::eval::cesk::roots::assert_quiescence_superset(
            &result.0,
            result.1.shared.as_ref(),
            state,
        );
    }
    // A4.4 FLIP (quiescence): the collector feeds from the PERSISTENT structural reader
    //   collect_persistent_roots(E₀)  — reach(E₀-env) ∪ global anchors ∪ K-spine
    //   ∪ result.0                    — the about-to-return values (a Rust local, not yet
    //                                    in any RootProvider)
    //   ∪ state.collect_driver_program_roots — the driver's program control (C), held
    //                                    above the trampoline. C∪K are empty here, so
    //                                    NO current WorkItem exists ⇒ collect_persistent_roots
    //                                    (not collect_machine_roots). collect_all_roots()
    //                                    survives in the oracle's OLD until A5.
    let mut roots: Vec<MettaValue> = Vec::with_capacity(result.0.len() + 64);
    crate::backend::eval::cesk::roots::collect_persistent_roots(
        &mut roots,
        result.1.shared.as_ref(),
    );
    roots.extend(result.0.iter().copied());
    state.collect_driver_program_roots(&mut roots);
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots);
}
```

### Flip 3 — QUIESCENCE `eval_with_tier` (`tier_forced.rs:284-293`, `should_collect()`)

In scope (verified): `outcome` (`TierEvalOutcome`; results in the `Ok{results,env,..}` /
`Demoted{results,env,..}` arms), `state` (`&MettaState` param, confirmed
`eval_with_tier(value, env, state: &MettaState, tier, policy)`). The `TierSelection::Auto`
arm early-returns (delegating to `eval()`, which already runs Flip 2), so this site only
covers the FORCED-tier paths. The E₀ env lives inside `outcome` (the `env` field of the
`Ok`/`Demoted` arms); bind it from there.

**BEFORE:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect() {
    let mut roots = crate::backend::models::collect_all_roots();
    if let TierEvalOutcome::Ok { results, .. } | TierEvalOutcome::Demoted { results, .. } =
        &outcome
    {
        roots.reserve(results.len());
        roots.extend(results.iter().copied());
    }
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots);
}
```

**AFTER:**
```rust
if crate::backend::eval::cesk::index_heap::index_gc::should_collect() {
    // A4.4 FLIP (quiescence, forced-tier): same shape as eval()'s flip. C∪K empty.
    // The E₀ env + result values are inside `outcome`'s Ok/Demoted arms; the driver-C
    // program is the `state` param. Non-result outcomes (NotApplicable) contribute only
    // the persistent + driver-C roots (no result values) — matching the BEFORE semantics
    // (which also added no result values for those arms).
    let mut roots: Vec<MettaValue> = Vec::with_capacity(64);
    if let TierEvalOutcome::Ok { results, env, .. }
    | TierEvalOutcome::Demoted { results, env, .. } = &outcome
    {
        // ── A4.4 quiescence oracle (debug-only; index-gc only) ──
        #[cfg(debug_assertions)]
        if crate::backend::models::metta_value::gc_mode_is_index() {
            crate::backend::eval::cesk::roots::assert_quiescence_superset(
                results,
                env.shared.as_ref(),
                state,
            );
        }
        crate::backend::eval::cesk::roots::collect_persistent_roots(
            &mut roots,
            env.shared.as_ref(),
        );
        roots.reserve(results.len());
        roots.extend(results.iter().copied());
    } else {
        // No env/results in this outcome (e.g. NotApplicable). Read E₀ is unavailable
        // here; the slab BEFORE also collected no results for this arm. The persistent
        // env-struct roots are still reachable via collect_all_roots in the oracle's OLD;
        // for the live feed, the global anchors + driver-C suffice for this rare arm.
        // (NotApplicable returns before allocating result values, so there is no transient
        // result garbage to protect — the global anchors + driver-C are the complete set.)
        crate::backend::eval::cesk::roots::collect_global_anchors(&mut roots);
        crate::backend::eval::cesk::k_spine::collect_k_spine(&mut roots);
    }
    state.collect_driver_program_roots(&mut roots);
    crate::backend::eval::cesk::index_heap::index_gc::run_collection_if_triggered(&roots);
}
```

**Residual-risk note on Flip 3's `else` arm.** When `outcome` is `NotApplicable` we have no
`MettaEnvironment` in scope (the forced tier never produced one). The persistent **env-struct**
roots (`E₀.collect_roots_into`) are therefore unavailable in the live feed for that arm. This
is sound: `NotApplicable` is returned by `tier_applicable` BEFORE any tier runs (no result
values allocated this call) — there is no per-call transient garbage to protect, and the env's
own values are pinned by the env Arc the caller still holds (the env is not dropped by an
`NotApplicable` return; it was never moved out). The quiescence oracle does NOT run on this arm
(it is inside the `if let` result arm), so the oracle never asserts on a missing env here.
A simpler alternative — **skip collection entirely** on `NotApplicable` (`return outcome` before
the `should_collect()` block when the tier was inapplicable) — is also acceptable and arguably
cleaner; pick at implementation time. Both are within the BEFORE behavior (which contributed no
result roots for that arm).

---

## Q-B — The quiescence oracle (`#[cfg(debug_assertions)]` + `gc_mode_is_index()`)

A single shared helper `cesk::roots::assert_quiescence_superset` is called from BOTH
quiescence flip sites (Flip 2, Flip 3) BEFORE their `run_collection_if_triggered`. It
mirrors the A4.3 midloop oracle (eval_loop.rs:3560-3637) but with the quiescence root
shapes: C∪K empty, result-vec transient, no `collect_machine_roots` (no current work).

```text
OLD  = collect_all_roots() ∪ result            (the discovered set the BEFORE feed used)
NEW  = collect_persistent_roots(env0) ∪ result  (the structural persistent reader + results)
KEPT = state.collect_driver_program_roots       (the driver's program control C: source+output)
ASSERT  OLD ⊆ (NEW ∪ KEPT)   (sorted-deduped inner_ptr multiset)
```

Why this is a clean superset at quiescence (and NON-VACUOUS):
- **No workers** (single-threaded `should_collect()` gate via `gate_open` ⇒ `!worker_ever_spawned()`)
  ⇒ the two parallel-dispatch providers in `collect_all_roots()` (`ParallelDispatch`/`Collapse`)
  are empty.
- **C∪K empty** post-`EvalGuard` ⇒ `CurrentIterRootProvider` (the current `work` item mirror)
  is empty (there is no current iteration). Confirmed: the current-iter mirror is set during the
  trampoline loop and there is no live loop at this site. So nothing in `collect_all_roots()`
  needs the control registers.
- **Env / tiers / promoted / caches** in `collect_all_roots()` are all covered by NEW:
  `collect_persistent_roots` = `env0.collect_roots_into` (the env's RootProvider body, byte-
  identical) ∪ `collect_global_anchors` (the 5 OnceLock caches `global_tiered_cache` /
  `global_space_registry` / `global_memo_cache` / bytecode-cache / compiler-atoms, plus the 4
  thread-local eval caches eval_memo/match/subgoal/thunk that `collect_all_roots()` reads via
  `collect_safepoint_roots()`/`CACHE_ROOT_HANDLE`) ∪ `collect_k_spine` (empty at quiescence,
  harmless).
- **`MettaStateGcRoots` (source+output)** is the ONE discovered provider NOT structural — it is
  exactly KEPT (`state.collect_driver_program_roots`), subtracted explicitly. NON-VACUOUS:
  KEPT is only those two named Vecs, so NEW alone must still cover every env / cache / k-spine
  root — a missing anchor still fails the oracle.
- **`result`** is on BOTH sides (the about-to-return values), so it cancels.

### Shared oracle helper (add to `roots.rs`, debug-only)

```rust
/// CESK Phase A4.4 — the QUIESCENCE machine-equivalence oracle. Asserts the discovered
/// quiescence root set (`collect_all_roots()` ∪ `result`) is covered by the structural
/// PERSISTENT reader (`collect_persistent_roots` ∪ `result`) UNION the legitimately-kept
/// driver-C program (`MettaState.{source,output}`). The safety direction (no protected
/// root dropped) for the A4.4 flip of the two quiescence collectors. Debug-only; the
/// callers gate it on `gc_mode_is_index()` (in slab mode collect_all_roots' frame-chain
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
    use crate::backend::models::MettaValueTrait;

    // OLD = the discovered set the BEFORE feed consumed: collect_all_roots()
    // (ROOT_REGISTRY ∪ SAFEPOINT_ROOTS) ∪ the about-to-return result values.
    let mut old: Vec<usize> = crate::backend::models::collect_all_roots()
        .iter()
        .map(|v| v.inner_ptr() as usize)
        .collect();
    old.extend(result.iter().map(|v| v.inner_ptr() as usize));

    // NEW = the structural PERSISTENT reader (reach E₀-env ∪ global anchors ∪ K-spine)
    // ∪ the about-to-return result values. (No collect_machine_roots: C∪K are empty at
    // quiescence, so there is no current WorkItem and no control registers.)
    let mut new_vals: Vec<crate::backend::models::MettaValue> =
        Vec::with_capacity(old.len() + 64);
    collect_persistent_roots(&mut new_vals, env0);
    new_vals.extend(result.iter().copied());
    let mut new: Vec<usize> = new_vals.iter().map(|v| v.inner_ptr() as usize).collect();

    // KEPT = the driver's program control (C): MettaState.source + .output, the one
    // discovered provider with no structural home (re-homed in A5.3b).
    let mut kept_vals: Vec<crate::backend::models::MettaValue> = Vec::new();
    state.collect_driver_program_roots(&mut kept_vals);
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
        let sample: Vec<String> =
            missing.iter().take(16).map(|p| format!("{:#x}", p)).collect();
        panic!(
            "A4.4 QUIESCENCE machine-equivalence oracle FAILED: discovered OLD is NOT \
             covered by (structural-persistent NEW ∪ driver-C KEPT). |OLD|={} |NEW|={} \
             |KEPT|={} |missing|={}\n  sample missing inner_ptrs (<=16): [{}]\n  At true \
             quiescence C∪K are empty, so a missing root means: (a) a thread-local cache \
             not in collect_global_anchors; (b) a transient register (a result value not \
             passed in, or a deferred-env not yet drained) unaccounted; (c) a global \
             anchor (tiered/bytecode/memo/space/compiler) the persistent reader omits; \
             (d) a driver-C root (MettaState.source/output) not exposed via \
             collect_driver_program_roots; (e) an env-struct root collect_roots_into \
             misses.",
            old.len(),
            new.len(),
            kept.len(),
            missing.len(),
            sample.join(", "),
        );
    }
}
```

The Flip 2 / Flip 3 call sites (shown in those blocks above) invoke this with the
respective `result` slice, `env0` (`result.1.shared.as_ref()` / `env.shared.as_ref()`),
and `state`.

### CI-invariant test (add to `roots.rs` tests, `#[cfg(all(debug_assertions, feature = "index-gc"))]`)

A must-not-panic test that drives a recursive + cache-populating eval through the actual
quiescence collector so `assert_quiescence_superset` fires with the gap sources populated:

```rust
#[cfg(all(debug_assertions, feature = "index-gc"))]
#[test]
fn a4_4_quiescence_oracle_holds() {
    // Force index mode + open gate for the duration; drive a recursive program with a
    // small MIN_BYTES so the quiescence collector fires (and the oracle with it).
    // (cnt 5000) > the watermark with MIN_BYTES low; recursive rule populates
    // EVAL_MEMO/MATCH/subgoal/thunk; the bare top-level adds MettaState.source roots.
    // Reaching the end without panic == OLD ⊆ NEW∪KEPT held at every quiescence.
    // (Use the same harness shape as the A4.3 test a4_3_oracle_holds_across_safepoint.)
}
```

(Implement against the same test scaffolding the A4.3 `a4_3_oracle_holds_across_safepoint`
CI test uses — set `gc_mode_is_index` true if not already, set `METTATRON_INDEX_GC_MIN_BYTES`
low, run a `(cnt 5000)`-style recursive eval via `eval()`/`eval_with_tier`, assert no panic.)

---

## Q-D — ASAN strategy (capped, FOREGROUND, MemorySwapMax=0)

**No dedicated ASAN script exists in `scripts/`** (only `ab_gc_diff.sh`, which is
release-only conformance). The single-threaded-collector RESULTS doc records the exact
ASAN command pattern that previously ran CLEAN (483/221/40, 840 cycles, 0 errors); reuse
it, but CAPPED and FOREGROUND per the standing resource-limits directive (the 2026-05-28
uncapped backgrounded ASAN + stress agent OOM-crashed the 125 GiB machine).

**Resource budget.** Machine has 125 GiB RAM (NOT 252). Cap the `-Zbuild-std` nightly ASAN
build at ≤32G with low `-j`; cap the run at ≤24G. ALWAYS `MemorySwapMax=0` (an OOM must kill
the cgroup, not thrash swap and freeze the box). FOREGROUND (never `run_in_background`).
Check for other sessions' builds first (`free -h` + `ps aux | grep -E 'cargo|rustc'`).

### Step 0 — pre-flight (FOREGROUND)
```bash
free -h
ps aux | grep -E 'cargo|rustc|mtt-conformance' | grep -v grep || echo "no other rust builds"
```

### Step 1 — capped ASAN build (FOREGROUND, ≤32G, low -j)
The `-Zbuild-std` rebuild of std+core under ASAN is the memory-heavy part; serialize it.
```bash
REPO="${REPO:-$(pwd)}"
LOG_DIR="${LOG_DIR:-$(mktemp -d -t a4_4.XXXXXXXX)}"
CONFORMANCE_DIR="${CONFORMANCE_DIR:?set CONFORMANCE_DIR to the conformance checkout}"
cd "$REPO"
systemd-run --user --scope \
    -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% -p TasksMax=256 \
    env RUSTFLAGS="-Zsanitizer=address -C target-cpu=native" \
    cargo +nightly build \
        --features index-gc \
        -Zbuild-std \
        --target x86_64-unknown-linux-gnu \
        --bin mtt-conformance \
        -j 4 \
    2>&1 | tee "$LOG_DIR/a4_4_asan_build.log" | tail -30
```
(`-j 4` bounds peak compiler RSS well under 32G; raise only if `free -h` shows ample
headroom and no other builds. The ASAN binary lands at
`target/x86_64-unknown-linux-gnu/debug/mtt-conformance`.)

> NOTE: ASAN here is a **debug** profile build (no `--release`), so `debug_assertions` is ON
> — the A4.4 quiescence oracle + the A4.3 midloop oracle BOTH fire during the ASAN run
> (belt-and-suspenders: ASAN catches UAF on the arena, the oracles catch a dropped root
> before it can be swept). The dev profile also keeps `debug_assert!(!seg.released)` live in
> `IndexArena::get`, so any dangling arena access aborts.

### Step 2 — SMALL forced-cycle workloads (FOREGROUND, ≤24G) — NOT full conformance

Two targeted programs that force index-gc collections **at the flipped sites**, kept small
so the ASAN run is minutes, not the full 483-fixture corpus:

**(a) Quiescence flip (Flip 2/3) — the multi-directive workload `stress_multidir.metta`.**
8000 `!(burn 200)` directives, each transient-and-dead at its quiescence — fires the
`eval()`/`eval_with_tier` quiescence collector between directives (the flipped feed). Low
`MIN_BYTES` so it collects often.
```bash
ASAN_BIN=target/x86_64-unknown-linux-gnu/debug/mtt-conformance
# stress_multidir is a program file; run it through the conformance binary's file path OR
# via the mettatron CLI built the same way. Simplest: build the CLI bin too (same flags) and run:
systemd-run --user --scope \
    -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% -p TasksMax=128 \
    env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
        METTATRON_INDEX_GC_MIN_BYTES=131072 \
        METTATRON_INDEX_GC_REPORT=1 \
        ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    ./target/x86_64-unknown-linux-gnu/debug/mettatron \
        examples/cesk-gc/stress_multidir.metta \
    2>&1 | tee "$LOG_DIR/a4_4_asan_quiescence.log" | tail -40
echo "rc=$? ; grep INDEX_GC_CYCLES_RUN $LOG_DIR/a4_4_asan_quiescence.log"
```
(Build the `mettatron` CLI bin with the SAME `systemd-run` + `-Zbuild-std` invocation as
Step 1, swapping `--bin mtt-conformance` → `--bin mettatron`, OR add `--bin mettatron` to
Step 1's single build. The CLI exercises Flip 2 via `eval()`.)

**(b) Midloop flip (Flip 1) — the single giant directive `stress_alloc.metta` + opt-in.**
`!(loop 8000 200)` is ONE directive (no inter-directive quiescence), so it exercises the
**midloop** collector — but ONLY when the opt-in is enabled (`should_collect_midloop` gates
on `midloop_enabled()`, env `METTATRON_INDEX_GC_MIDLOOP=1`, default OFF). This is the path
the A4.3 oracle validates and Flip 1 changes; run it under ASAN with the opt-in ON to
exercise the flipped midloop feed:
```bash
systemd-run --user --scope \
    -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% -p TasksMax=128 \
    env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
        METTATRON_INDEX_GC_MIDLOOP=1 \
        METTATRON_INDEX_GC_MIN_BYTES=131072 \
        METTATRON_INDEX_GC_REPORT=1 \
        ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    ./target/x86_64-unknown-linux-gnu/debug/mettatron \
        examples/cesk-gc/stress_alloc.metta \
    2>&1 | tee "$LOG_DIR/a4_4_asan_midloop.log" | tail -40
echo "rc=$? ; grep INDEX_GC_CYCLES_RUN $LOG_DIR/a4_4_asan_midloop.log"
```

**(c) A small conformance SUBSET under ASAN (optional, higher coverage of Flip 3).** A few
forced-tier fixtures via `mtt-conformance --module M11-bisimilarity-pt` (a SMALL module),
NOT the full corpus:
```bash
systemd-run --user --scope \
    -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% -p TasksMax=128 \
    env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
        METTATRON_INDEX_GC_MIN_BYTES=131072 \
        METTATRON_INDEX_GC_REPORT=1 \
        ASAN_OPTIONS=detect_leaks=0:abort_on_error=1 \
    "$ASAN_BIN" --strict \
        --conformance-dir "$CONFORMANCE_DIR" \
        --module M11-bisimilarity-pt \
    2>&1 | tee "$LOG_DIR/a4_4_asan_conf_subset.log" | tail -40
echo "rc=$? ; grep -E 'INDEX_GC_CYCLES_RUN|FAIL' $LOG_DIR/a4_4_asan_conf_subset.log"
```

**ASAN acceptance:** each run `rc=0`, `INDEX_GC_CYCLES_RUN > 0` (non-vacuous — the flipped
collector actually fired), ZERO ASAN lines (`heap-use-after-free` / `use-after-poison` /
released-segment access) in the logs, and no oracle panic (debug_assertions ON). The
non-vacuity bar matters: a flip that fed an over-broad set would never UAF but also never
prove tightness — `cycles>0` confirms the structural feed drove real reclamation while ASAN
watched.

(If building two separate bins is undesirable, the simplest single-bin path: add
`--bin mettatron` to the Step-1 build so the one capped ASAN build produces both binaries,
then run (a)/(b) via the `mettatron` CLI and (c) via `mtt-conformance`.)

---

## Q-E — Green-wall (all capped under systemd-run)

The flip CHANGES the index collector's live root source, so the index arm must be re-proven
byte-identical (the structural reader as the live source must produce the SAME observable
results); the slab arm must be byte-identical by construction (every flip is index-gated).

### RELEASE
```bash
# 1. SLAB nextest — byte-identical (flip is index-only; debug_assertions OFF in release).
systemd-run --user --scope -p MemoryMax=48G -p MemorySwapMax=0 -p CPUQuota=1800% \
    cargo nextest run --release 2>&1 | tee "$LOG_DIR/a4_4_slab_nextest.log" | tail -5
#   EXPECT: 4325 passed, 0 failed  (the single-threaded-collector RESULTS baseline:
#           4312 historical + the collector unit tests; re-confirm the current count).

# 2. INDEX nextest — byte-identical.
systemd-run --user --scope -p MemoryMax=48G -p MemorySwapMax=0 -p CPUQuota=1800% \
    cargo nextest run --release --features index-gc 2>&1 | tee "$LOG_DIR/a4_4_index_nextest.log" | tail -5
#   EXPECT: 4177 passed, 0 failed.

# 3. INDEX conformance — byte-identical 483/221/40 AND cycles>0 (the flipped feed fires
#    and produces identical results).
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% \
    cargo build --release --features index-gc --bin mtt-conformance 2>&1 | tail -1
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% \
    env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
        METTATRON_INDEX_GC_MIN_BYTES=262144 \
        METTATRON_INDEX_GC_REPORT=1 \
    ./target/release/mtt-conformance --strict \
        --conformance-dir "$CONFORMANCE_DIR" \
    2>&1 | tee "$LOG_DIR/a4_4_index_conf.log" | tail -10
#   EXPECT: 483 pass / M11-pt 221 / M11-he 40 / FAIL=0 / INDEX_GC_CYCLES_RUN > 0 (was 840).

# 4. SLAB conformance — byte-identical (sanity; the default binary).
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% \
    cargo build --release --bin mtt-conformance 2>&1 | tail -1
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% \
    ./target/release/mtt-conformance --strict \
        --conformance-dir "$CONFORMANCE_DIR" \
    2>&1 | tee "$LOG_DIR/a4_4_slab_conf.log" | tail -10
#   EXPECT: 483 / 221 / 40 / FAIL=0 ; INDEX_GC_CYCLES_RUN=0 (collector inert in slab).
```

### DEBUG (both oracles fire — index only; debug is slow, run a SUBSET not full 483)
```bash
# Build debug index-gc (debug_assertions ON ⇒ A4.3 midloop + A4.4 quiescence oracles active).
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=800% \
    cargo build --features index-gc --bin mtt-conformance 2>&1 | tail -1
# A conformance SUBSET (mmverify-style + a small module) — debug RSS ≈ 2-3× release, so NOT
# the full corpus; enough to fire both oracles across recursive + fork + cache-populating evals.
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=400% \
    env METTATRON_PARALLEL_FANOUT_DEPTH=0 \
        METTATRON_INDEX_GC_MIN_BYTES=131072 \
        METTATRON_INDEX_GC_REPORT=1 \
    ./target/debug/mtt-conformance --strict \
        --conformance-dir "$CONFORMANCE_DIR" \
        --module M11-bisimilarity-pt \
    2>&1 | tee "$LOG_DIR/a4_4_debug_subset.log" | tail -10
#   EXPECT: rc=0, no oracle panic, cycles>0.

# DEBUG index nextest (the index-gc cesk/gc subset; also fires both oracles in-process tests
# incl. the new a4_4_quiescence_oracle_holds CI test).
systemd-run --user --scope -p MemoryMax=32G -p MemorySwapMax=0 -p CPUQuota=1800% \
    cargo nextest run --features index-gc 2>&1 | tee "$LOG_DIR/a4_4_debug_nextest.log" | tail -5
#   EXPECT: 0 failed (the A4.3 + A4.4 oracle CI tests green).
```

### ASAN (Q-D)
Run Step 1 + Step 2 (a)/(b)/(c) above. EXPECT: each `rc=0`, `cycles>0`, ZERO ASAN errors,
no oracle panic.

### Acceptance gate (all must hold)
| Arm | Command group | Expected |
|---|---|---|
| RELEASE slab nextest | E.1 | 4325 / 0 fail (re-confirm count) |
| RELEASE index nextest | E.2 | 4177 / 0 fail |
| RELEASE index conformance | E.3 | 483 / 221 / 40 / FAIL=0 / **cycles>0** |
| RELEASE slab conformance | E.4 | 483 / 221 / 40 / FAIL=0 / cycles=0 |
| DEBUG index conformance subset | E.debug | rc=0, no oracle panic, cycles>0 |
| DEBUG index nextest | E.debug | 0 fail (A4.3+A4.4 oracle CI tests green) |
| ASAN quiescence (multidir) | D.2a | rc=0, cycles>0, 0 ASAN, no panic |
| ASAN midloop (alloc, opt-in) | D.2b | rc=0, cycles>0, 0 ASAN, no panic |
| ASAN conformance subset | D.2c | rc=0, cycles>0, 0 ASAN, no panic |

---

## Residual risk

1. **Flip 3 `NotApplicable` arm has no env in scope** (Q-C/Flip-3 note). Mitigated by the
   argument that `NotApplicable` allocates no result garbage and does not drop the env; OR
   skip collection on that arm entirely. The oracle does not assert on this arm (it is inside
   the `if let` result arm). Low risk; pick the skip-collection variant if any doubt.
2. **`collect_all_roots()` is NOT removed in A4.4** — only the live feed changes. It survives
   in BOTH oracles' OLD until A5. So A4.4 cannot regress the oracle's standing proof; it only
   changes which Vec reaches `run_collection_if_triggered`. The midloop flip is a *proven*
   superset (the A4.3 oracle validates the identical NEW∪KEPT every safepoint); the quiescence
   flips gain their own oracle (Q-B) in the same commit, validated across the debug + ASAN
   runs before any A5 deletion relies on them.
3. **Quiescence `CurrentIterRootProvider` emptiness** is argued, not yet oracle-observed
   prior to A4.4 (the A4.3 oracle is at the midloop, where C∪K are live). The new quiescence
   oracle (Q-B) is precisely what observes it: if the current-iter mirror were NON-empty at
   quiescence and pointed at a non-structural root, `assert_quiescence_superset` would panic
   with class (b). So this risk is *converted into a test* by Q-B — it cannot silently pass.
4. **`MettaEnvironment.shared` field access from `eval/mod.rs` / `tier_forced.rs`**: both are
   in-crate (`pub(crate)` suffices), and the A4.3 oracle already reads `env.shared.as_ref()`
   from `eval_loop.rs` — so the access pattern compiles. In Flip 2 use `result.1.shared`
   (NOT the consumed `env`).
5. **Build-count drift**: the RELEASE nextest expectations (4325 slab / 4177 index) are from
   the single-threaded-collector RESULTS doc; A4.1–A4.3 added unit tests
   (`roots.rs`/`k_spine.rs`/the A4.3 CI test), so the CURRENT baseline may be slightly higher.
   Re-confirm the live counts on the branch tip before treating a delta as a regression — the
   invariant is **0 failed + byte-identical pass/fail SET**, not the absolute number. A4.4
   itself adds one CI test (`a4_4_quiescence_oracle_holds`), bumping the index/debug count by 1.
