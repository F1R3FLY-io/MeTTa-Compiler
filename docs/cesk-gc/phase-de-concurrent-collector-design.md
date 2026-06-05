# Phase D+E — Genuine-CESK Parallel/Concurrent GC (dedicated GC thread) — **v3**

**Status:** PROPOSED **v3** (post-round-2 red-team). Supersedes v2. Architecture is **settled and PASSED genuine-CESK review twice** (dedicated GC thread + cooperative-safepoint structural per-mutator self-root; rendezvous gate). v3 does NOT redesign the architecture — it **revises the mechanism** to close the 9 round-2 clusters. The 5 converged areas (genuine-CESK fidelity, allocate-black ordering, abort-to-STW, result-ordering determinism, cache-reuse ordering) are **preserved unchanged** and are not re-opened.

**Current implementation note:** the early v3 narrative below still records the parked-count design history. The
source-coupled implementation has superseded that gate with the per-slot reified witness:
`requestor_wait_for_all_reified_parked` sets `CURRENT_WITNESS_OK`, and `gate_open_rendezvous` reads
`current_witness_ok`. The formal ledger (`formal-verification-ledger.md`) is the current checked boundary.

All file:line citations below were re-verified against the working tree (branch `feature/petta-semantics`); where v2's doc cited an abbreviated or drifted line, v3 gives the **verified current** location.

---

## Part 0 — Verified primitive inventory (the substrate v3 builds on)

| Primitive | Verified location | Shape |
|---|---|---|
| `EvalGuard` | gc_allocator.rs:3447 | **unit struct** (`pub struct EvalGuard;`) — confirms the finisher cannot live in `Drop` |
| `EvalGuard::enter` | gc_allocator.rs:3460; fast-path `ACTIVE_EVALUATORS.fetch_add` :3467; `GC_IN_PROGRESS` check :3468; `EVAL_GUARD_DEPTH+1` :3488 | admission via `GC_IN_PROGRESS` only |
| `EvalGuard::drop` | gc_allocator.rs:3493; `EVAL_GUARD_DEPTH-1` :3496; `ACTIVE_EVALUATORS.fetch_sub` :3502; `prev==1` quiescent-notify :3503-3510 | |
| `active_evaluator_count()` | gc_allocator.rs:3515 | **SUM of guard-increments** (depth>1 thread contributes >1) |
| `EVAL_GUARD_DEPTH` | thread-local `Cell<u32>` gc_allocator.rs:4631 | bumped :3488 / :4691, dec :3496 / :4644 |
| `WORKERS_PARKED_FOR_GC` | gc_allocator.rs:2887 | `AtomicU32` |
| `WORKER_ROOT_BUFFER` | gc_allocator.rs:2906 | `Mutex<Vec<MettaValue>>` |
| `RENDEZVOUS_MUTEX`/`_CONDVAR` | gc_allocator.rs:2919/2920 | parked-count handshake |
| `RESUME_MUTEX`/`_CONDVAR` | gc_allocator.rs:2929/2930 | resume handshake (keyed on `GC_REQUESTED`) |
| `worker_park_and_root` | gc_allocator.rs:3037 (buffer-append → `WORKERS_PARKED_FOR_GC.fetch_add(1,AcqRel)` :3040 → notify under RENDEZVOUS_MUTEX → `worker_wait_for_resume`) | |
| `worker_wait_for_resume` | gc_allocator.rs:3071 (waits `is_gc_requested()` under RESUME_MUTEX) | **waits on GC_REQUESTED** |
| `requestor_wait_for_all_reified_parked` / `current_witness_ok` | gc_allocator.rs:3404 / :3482 | current per-slot witness gate; supersedes parked-count as the collection predicate |
| `reset_rendezvous_counters` | gc_allocator.rs:3178 (`WORKERS_PARKED_FOR_GC.store(0,Release)` + buffer clear; **no mutex**) | |
| `resume_workers` | gc_allocator.rs:3206 (RESUME_MUTEX across `GC_REQUESTED.store(false)` + notify) | |
| `drop_eval_guard_for_safepoint` | gc_allocator.rs:4643; **asserts depth>0** :4646; drops **ONE** level :4649 + single `fetch_sub` :4652; `prev==1` notify :4654 | |
| `reacquire_eval_guard_after_safepoint` | gc_allocator.rs:4663; **waits on GC_IN_PROGRESS** :4670-4684; single `fetch_add` :4670; `EVAL_GUARD_DEPTH+1` :4691 | |
| `GcInProgressGuard::try_enter` | gc_allocator.rs:3612 (CAS `false→true` AcqRel → `Option<Self>`); `Drop` clears under GC_PROGRESS_MUTEX + notifies GC_PROGRESS_CONDVAR :3624-3633 | |
| `note_worker_spawned` / `worker_ever_spawned` | gc_allocator.rs:3524 / :3533 | |
| `bump_gc_sweep_epoch` | gc_allocator.rs:4886 — **`pub(crate)`** | usable by the index collector and cache epoch guards |
| `gc_sweep_epoch` | gc_allocator.rs:3683 — `pub` | |
| `mark_sweep_if_over_watermark` | index_heap.rs:2010; **`global_index_heap().write()` at :2073 held across `mark`:2075 AND `sweep`:2076** ("true quiescence" :2062-2064) | the STW-by-RwLock |
| alloc fast path | index_heap.rs:1249 (`.read()` :1257) | concurrent bump |
| `clear_aba_sensitive_caches` | eval_loop.rs:211 (MORK :213-214, VALUE_HASH_CACHE :217, hash_cons :220, **`clear_operator_cache()` :223**) | |
| index sweep cache clears | index_heap.rs:2119 `clear_aba_sensitive_caches`; :2122 `clear_inner_shadow`; :2128 `clear_eval_memo`; :2129 `clear_match_result_cache` | |

---

## Part 1 — The SPINE (CLUSTER 1, CRIT): finisher-at-finish-site + `n_threads` thread-count gate + admission-in-enter

This is the load-bearing revision. Three coupled sub-fixes, each verified.

### 1.1 The finisher moves to the eval_loop FINISH SITE (NOT `EvalGuard::drop`)

**Why Drop is infeasible (verified):** `EvalGuard` is a unit struct (gc_allocator.rs:3447). Its `Drop` (:3493) has no access to ⟨C,K⟩ — by the time `Drop` runs, `eval_trampoline_with_carrying` has already returned its `(eval_results, _new_env)` and they are owned by the caller's stack frame. A finisher in `Drop` could self-root nothing.

**The fix — two verified finish sites where the result values + machine are still in scope:**

- **Branch worker** (`parallel_dispatch` closure), eval_loop.rs:**2522** (`Ok((eval_results, _new_env))`). Insert the finisher BETWEEN computing `eval_results` and the `guard[slot] = Some(eval_results.into_iter().collect())` store at :2529 — `eval_results` and `_new_env` are both live locals here.
- **Collapse worker** (`parallel_collapse_dispatch` closure), eval_loop.rs:**3043** (`eval_results` bound) through the store at :3059 — same shape.
- **Batch worker** (`evaluate_batch_parallel_arena` closure), rholang_integration.rs:**504** (`eval_results` bound) through the slot store at :509.

The finisher logic, gated on `is_gc_requested()`:

```text
// at the finish site, registers (eval_results, env) still in scope:
if gc_allocator::is_gc_requested() {
    let mut roots = Vec::with_capacity(/* result + machine */);
    roots.extend(eval_results.iter().flat_map(values_of));   // the about-to-return result values
    // env / E_local already structural via the worker's E₀ handle (carried by Arc)
    gc_allocator::worker_finish_into_buffer(&roots);  // NEW: buffer-append + parked-count bump under the cycle protocol (§1.3)
    // THEN return normally — DO NOT park. The result lands in the parent's
    // WaitForParallel slot; the parent's K-frame keeps it rooted thereafter.
}
```

`worker_finish_into_buffer` is the finisher analogue of `worker_park_and_root` (gc_allocator.rs:3037) WITHOUT the `worker_wait_for_resume` step-4 park. It does steps (1)-(3): buffer-append, `WORKERS_PARKED_FOR_GC.fetch_add(1,AcqRel)` (the release fence for the append — HB2), notify the GC thread under RENDEZVOUS_MUTEX. It is a **new sibling**, reusing the exact buffer/counter/mutex primitives (honoring the keep-the-dormant-code/REUSE choice).

**Genuine-CESK preserved:** the finisher self-reads its own result registers — same structural form as the parker. `WORKER_ROOT_BUFFER` remains pure transport, not a discovery oracle.

### 1.2 Count THREADS, not guard-increments: introduce `n_threads`

**The defect (verified):** `active_evaluator_count()` (gc_allocator.rs:3515) is `ACTIVE_EVALUATORS.load()`, and `ACTIVE_EVALUATORS` is `fetch_add`'d once per `EvalGuard::enter` (:3467) — so a depth-2 thread contributes 2. A gate `WORKERS_PARKED_FOR_GC == active_evaluator_count()` would compare a per-THREAD park count against a per-INCREMENT sum and never balance for any depth>1 thread.

**The fix — a separate per-thread counter incremented at the OUTERMOST enter only:**

```text
static N_THREADS: AtomicU32 = AtomicU32::new(0);   // NEW, gc_allocator.rs near :2887

// in EvalGuard::enter (gc_allocator.rs:3488), at the EVAL_GUARD_DEPTH bump:
EVAL_GUARD_DEPTH.with(|d| {
    let prev = d.get();
    d.set(prev + 1);
    if prev == 0 { N_THREADS.fetch_add(1, Ordering::AcqRel); }  // depth 0→1 only
});

// in EvalGuard::drop (gc_allocator.rs:3496):
EVAL_GUARD_DEPTH.with(|d| {
    let depth = d.get();
    if depth > 0 {
        d.set(depth - 1);
        if depth == 1 { N_THREADS.fetch_sub(1, Ordering::AcqRel); }  // depth 1→0 only
    }
});
```

The gate becomes `WORKERS_PARKED_FOR_GC == n_threads_snapshot` where `n_threads_snapshot = N_THREADS.load(Acquire)` taken under the GC_IN_PROGRESS admission (§1.4). `requestor_wait_for_parked_count(n)` (gc_allocator.rs:3143) is reused verbatim with `n = n_threads_snapshot`.

**CRITICAL coupling with reacquire (verified at gc_allocator.rs:3555 + :4691):** `reacquire_eval_guard_after_safepoint` bumps `EVAL_GUARD_DEPTH` (:4691) and `ACTIVE_EVALUATORS` (:4670) but is "not an actual eval" (per the :3555 comment). A parking worker does `drop_eval_guard_for_safepoint` (depth→0, so it would `N_THREADS.fetch_sub`) then later `reacquire` (depth 0→1, so it would `N_THREADS.fetch_add`). That is **correct**: while parked, the worker is genuinely not in the active set; on resume it rejoins. So `N_THREADS` hooks go in BOTH the `EvalGuard` enter/drop AND `drop_eval_guard_for_safepoint`/`reacquire` at their depth-0→1 / 1→0 transitions — but see §1.4: the snapshot is taken AFTER admission closes, so a parked-then-resumed worker cannot rejoin mid-cycle (the resume itself is gated). The depth-transition hook is the single source of truth for `N_THREADS`; `drop_eval_guard_for_safepoint` (which today drops only ONE level, :4649) is upgraded to the full-depth drain (§Part 8 / Cluster 8+9) and decrements `N_THREADS` exactly when it takes depth to 0.

### 1.3 Synchronize the parked-count lifecycle (cycle-generation + RENDEZVOUS_MUTEX)

**The defect (verified):** `reset_rendezvous_counters` (gc_allocator.rs:3178) does `WORKERS_PARKED_FOR_GC.store(0, Release)` with NO mutex. The parker bumps under no lock except the notify (the `fetch_add` at :3040 is lock-free; only the notify at :3043-3046 takes RENDEZVOUS_MUTEX). A straggler finisher from cycle K (one that read `is_gc_requested()==true` for cycle K, was descheduled, and bumps after cycle K's reset) could increment cycle K+1's count → the gate over-counts → `requestor_wait_for_parked_count` hangs (waits for a count it will never see) OR under-marks.

**The fix — a cycle-generation counter, all parked-count writes under RENDEZVOUS_MUTEX:**

```text
static GC_CYCLE_GEN: AtomicU64 = AtomicU64::new(0);   // NEW

// Parker / finisher (worker_park_and_root :3037 and the new worker_finish_into_buffer):
//   capture my_gen = the cycle-gen I observed when I saw is_gc_requested()==true
//   (read it alongside the GC_REQUESTED flag, before self-rooting).
{
    let _lock = RENDEZVOUS_MUTEX.lock();                 // CHANGED: hold across the bump
    if GC_CYCLE_GEN.load(Ordering::Acquire) == my_gen {  // still the same cycle?
        WORKER_ROOT_BUFFER.lock().extend_from_slice(roots);
        WORKERS_PARKED_FOR_GC.fetch_add(1, Ordering::AcqRel);
        RENDEZVOUS_CONDVAR.notify_all();
    } // else: my cycle already ended; my roots are stale-and-irrelevant — drop them, do not bump.
}

// reset_rendezvous_counters (:3178) — CHANGED: bump the generation and reset under the SAME mutex:
pub(crate) fn reset_rendezvous_counters() {
    let _lock = RENDEZVOUS_MUTEX.lock();
    GC_CYCLE_GEN.fetch_add(1, Ordering::AcqRel);          // open a new generation
    WORKERS_PARKED_FOR_GC.store(0, Ordering::Release);
    WORKER_ROOT_BUFFER.lock().clear();
}
```

Now a straggler from cycle K finds `GC_CYCLE_GEN != my_gen` under the mutex and does NOT bump — its (stale) roots are discarded. This is sound because a finisher whose cycle already ended necessarily left `active` BEFORE the snapshot of the NEXT cycle (its result is already in the parent's `WaitForParallel` K-frame, structurally rooted), so dropping its buffer contribution loses nothing live. The buffer-append is moved INSIDE the mutex so the drain (also under RENDEZVOUS_MUTEX via `requestor_wait_for_parked_count`) composes cleanly.

**HB argument unchanged:** `WORKERS_PARKED_FOR_GC.fetch_add(1, AcqRel)` still release-fences the buffer append (HB2); the GC thread observing `== n_threads` (Acquire, :3144) sees all `n_threads` workers' writes. The mutex is added for the reset/straggler exclusion, not for the HB (which is single-location Acquire/Release → still SC-faithful, no SeqCst → TLA+ models it faithfully).

---

## Part 2 — Admission gate symmetry (CLUSTER 2, HIGH)

**The defect (verified):** `EvalGuard::enter` (gc_allocator.rs:3460) gates on `GC_IN_PROGRESS` only — it does NOT gate on `GC_REQUESTED`. The non-worker / batch entrants — top-level `eval` (eval/mod.rs:240, :370), batch worker (rholang_integration.rs:500), and the dispatch workers — all call bare `EvalGuard::enter()`. So between the `n_threads` snapshot and the moment the GC thread sets `GC_IN_PROGRESS`, a fresh entrant can `fetch_add` into `ACTIVE_EVALUATORS`/`N_THREADS` and become uncounted-but-live → its machine's nodes are not in the union → UAF.

**The fix — Option (B), the simplest race-free choice: take `GC_IN_PROGRESS` via `try_enter` BEFORE snapshotting `n_threads`, and keep `GC_IN_PROGRESS` as the single authoritative admission gate already in `enter`.**

The GC thread's driver sequence:

```text
1. request_gc(): GC_REQUESTED.store(true, Release)        // soft signal — makes mutators poll+park
2. reset_rendezvous_counters()                            // opens a fresh GC_CYCLE_GEN, zeroes the count
3. let _gip = loop { match GcInProgressGuard::try_enter() // CAS false→true AcqRel (:3612)
       { Some(g) => break g, None => park-retry } };       // now GC_IN_PROGRESS==true
4. let n = N_THREADS.load(Ordering::Acquire);             // SNAPSHOT under GC_IN_PROGRESS
5. requestor_wait_for_parked_count(n)                     // wait WORKERS_PARKED_FOR_GC == n
6. ... mark (E2: under .read(); E1: under .write()) ...
7. ... sweep under .write() ...; bump_gc_sweep_epoch() if reclaimed
8. drop(_gip)  →  GC_IN_PROGRESS=false + GC_PROGRESS_CONDVAR.notify_all() (:3624)
9. resume_workers(): GC_REQUESTED.store(false) + RESUME_CONDVAR.notify_all() (:3206)
10. end-of-cycle: reset for next (reset_rendezvous_counters at the TOP of the next cycle)
```

**Why this is race-free (the interleaving proof):** `EvalGuard::enter` (gc_allocator.rs:3467-3478) does `ACTIVE_EVALUATORS.fetch_add(1)` then, if `GC_IN_PROGRESS.load(Acquire)` is true, `fetch_sub(1)` and parks. The `N_THREADS.fetch_add` (depth 0→1, §1.2) is placed AFTER the `GC_IN_PROGRESS` check passes — i.e. only a thread that has cleared admission bumps `N_THREADS`. Two cases for any entrant E racing the snapshot at step 4:

- E's `GC_IN_PROGRESS.load` happens-before the GC thread's `try_enter` CAS (step 3) sets it: then E saw `false`, proceeded, and bumped `N_THREADS` BEFORE the CAS's AcqRel. The snapshot at step 4 (Acquire, sequenced-after the CAS's release) observes E's bump → E is counted → E will park (E will itself poll `GC_REQUESTED==true`, set at step 1, at its next safepoint) and contribute to `WORKERS_PARKED_FOR_GC` → gate balances.
- The CAS (step 3) happens-before E's `GC_IN_PROGRESS.load`: E reads `true`, executes `fetch_sub(1)`, does NOT bump `N_THREADS`, and parks in `enter` on `GC_PROGRESS_CONDVAR`. E never enters `active` for this cycle → uncounted AND not-live → safe.

The single synchronizing edge is the `try_enter` CAS's AcqRel paired with the snapshot's Acquire and `enter`'s Acquire load of the same `GC_IN_PROGRESS` location. No entrant can be uncounted-but-live. (Placing `N_THREADS.fetch_add` after the admission check is the one new ordering requirement; it is a thread-local depth read + one atomic, on the slow path only when depth was 0.)

This subsumes the v2 "WorkerEnter gate at every closure top" — there is now ONE gate (`GC_IN_PROGRESS` in `enter`/`reacquire`), authoritative for ALL entrants (top-level, batch, dispatch). We do NOT need a separate `is_gc_requested()` admission check in `enter`; `GC_REQUESTED` remains the *poll-and-park* signal for already-`active` threads, and `GC_IN_PROGRESS` is the *admission* gate. (We keep the existing `is_gc_requested()` poll at the worker closure tops as a latency optimization — it lets a fresh worker park early rather than enter-then-immediately-poll — but it is no longer load-bearing for soundness.)

---

## Part 3 — E2 mark-under-read-lock split (CLUSTER 3, CRIT, E2 only)

**The defect (verified):** `mark_sweep_if_over_watermark` (index_heap.rs:2010) takes `global_index_heap().write()` at :2073 and holds it across `heap.mark(&addrs)` (:2075) AND `heap.sweep()` (:2076). The comment at :2062-2064 calls this "true quiescence for the store." Because the concurrent alloc fast path takes `.read()` (index_heap.rs:1257), the write-across-mark makes E2's whole premise ("minors continue / allocate-black during marking") un-exercisable: no concurrent alloc can run while mark holds the write lock. Holding `GcInProgressGuard` across the full mark additionally freezes every `EvalGuard::enter`.

**The fix (E2 ONLY — E1-STW keeps write-across-mark as the abort backstop):** split the lock regime.

```text
// E2 concurrent collection (NEW path, gated on gate_open_rendezvous, index_heap.rs:1870):
fn mark_sweep_concurrent(roots, n_threads_snapshot) {
    // (a) BRIEF root-snapshot + handshake under GcInProgressGuard:
    //     GcInProgressGuard is held ONLY here (admission gate §Part 2) — closes enter().
    //     Snapshot n_threads, wait on the per-slot reified witness, drain WORKER_ROOT_BUFFER ∪ E₀ ∪ driver-C.
    //     Arm the SATB barrier (marking_in_progress() := true) and allocate-black BEFORE releasing the gate.
    //     Then DROP GcInProgressGuard so mutators may re-enter (alloc concurrently).
    // (b) CONCURRENT MARK under global_index_heap().read():
    let heap = global_index_heap().read();      // SHARED — alloc_*_concurrent (:1257) + allocate-black run in parallel
    heap.mark_concurrent(&addrs);               // transitive mark from initial roots; barriers synchronously mark evicted roots
    drop(heap);
    // (c) BRIEF SWEEP ALONE under global_index_heap().write():
    let mut heap = global_index_heap().write(); // EXCLUSIVE — only for the reclaim step
    let stats = heap.sweep();                    // current E2 path is full-major-only
    heap.promote_young();
    drop(heap);
    marking_in_progress() := false;
    if stats.reclaimed_anything() { bump_gc_sweep_epoch(); }  // Cluster 5
    // cache clears as today (clear_aba_sensitive_caches etc.) — but see §Part 5 for the parked-thread epoch story
}
```

**The soundness invariant for a `.read()`-locked concurrent mark** (the theorem E2 rests on):

> **Invariant E2-MARK.** A node N that is live at any instant during the concurrent mark is marked black before the sweep frees it, PROVIDED: (i) **SATB** — every mutator overwrite/deletion/eviction that drops the last mutator-visible reference to a still-unmarked node shades that node synchronously (Yuasa pre-image / evicted-value barrier, §Part 4); (ii) **allocate-black** — every node allocated during marking sets its mark bit (`set_mark` AcqRel, index_arena.rs:193) BEFORE its slot is published into `len` (Release, :304/:139), and the marker scans by `len` (Acquire, :228), so any slot the marker can observe was marked-black-first (the converged finding #8, preserved); (iii) **reified witness snapshot of K** — every mutator that was `active` at the snapshot satisfies the strict per-slot witness and has self-rooted its machine ⟨C,E_local,K⟩ into `WORKER_ROOT_BUFFER` before the mark reads roots, so its stack-reachable nodes are in the initial root set.

Under (i)+(ii)+(iii), the only nodes the sweep can reclaim are those unreachable from {roots ∪ SATB-shaded ∪ allocate-black} = the snapshot-live set plus everything that became reachable or was retained during the mark — i.e. exactly the dead set. Floating garbage (a node that died during the mark but was shaded by SATB) is retained this cycle and reclaimed next (no-recycle idempotence, index_heap.rs:940-957). This is the standard Yuasa-SATB soundness argument, now grounded in the verified `len`/`set_mark` ordering and the verified reified-witness HB.

**E1 unchanged:** the STW path (`gate_open_midloop` / quiescence at eval/mod.rs:276-309, where `active==0`) keeps `write()`-across-mark (index_heap.rs:2073). E1 is the **abort-to-STW backstop** (converged finding #9): on SATB abort, the E2 path re-issues `request_gc`, re-acquires `GcInProgressGuard`, waits on the per-slot reified witness, drains structural roots, performs a full mark/sweep under `.write()`, then closes the cycle. The two paths share `mark`/`sweep`; E2 adds `mark_concurrent` (the read-locked variant), synchronous SATB barrier marking, and the brief-gate split.

---

## Part 4 — SATB completeness (CLUSTER 4, HIGH): re-derived from `collect_persistent_roots`

**The site set is re-derived (verified) from the canonical root reader `collect_persistent_roots` (roots.rs:404):**

```
collect_persistent_roots = env0.collect_roots_into        (roots.rs:411)
                         ∪ collect_global_anchors          (roots.rs:413, body :301-322)
                         ∪ collect_k_spine                 (roots.rs:415)
```

`collect_global_anchors` (roots.rs:301) reads NINE anchor families. SATB needs a deletion/overwrite/eviction barrier on **every one that can drop a MettaValue while marking**:

| Anchor (root reader) | Mutation sites needing a barrier | Verified line | Barrier shape |
|---|---|---|---|
| `global_tiered_cache` (:307) | tiered-cache eviction/overwrite | act_tiered evicts; `(compact-space!)` `variable_atoms.write().clear()` | act_tiered.rs:781 | shade evicted value |
| `global_space_registry` (:308) | `remove` / `clear` / `register`-overwrite | space_registry.rs:118 (`remove`→`.remove` :119), :143 (`clear`→:144), :111 (`register` overwrite) | shade removed/overwritten handle's values |
| `global_memo_cache` (:309) | `evict_lru` / `insert`-overwrite / `clear` | memo_cache.rs:151 (`evict_lru`), :132 (`insert`→evict_lru :137), :165 (`clear`) | **shade the EVICTED entry** (the value `evict_lru` drops) |
| `collect_bytecode_cache_roots` (:310) | bytecode-cache eviction | (cache.rs) | shade evicted |
| `collect_compiler_atom_roots` (:311) | compiler-atom overwrite/clear | (compiler) | shade overwritten |
| `EVAL_MEMO` (:318) | silent LRU eviction + stale-pop | dispatch_hints.rs:692 (`.put` silent LRU evict), :675 (`memo.pop` stale) | shade evicted value |
| `MATCH_RESULT_CACHE` (:319) | silent LRU eviction | dispatch_hints.rs:810 (`.put` silent LRU evict) | shade evicted value |
| subgoal table (:320) | lookup-stale-evict + `remove_entry` + `clear` | tabling.rs:173 (lookup-stale `.remove`), :212 (`remove_entry`), :228 (`clear`) | shade evicted value |
| thunk table (:321) | lookup-stale-evict (×2) + `clear` | thunk.rs:166, :187 (lookup-stale `.remove`), :249 (`clear`) | shade evicted value |

**The LRU-eviction barrier shape (the key v3 correction):** for an LRU cache the barrier is **"when `marking_in_progress()`, shade the value RETURNED by `put`/`pop`/`remove` — i.e. the EVICTED entry"** — NOT "read the OLD value before insert." For a fresh `put` that evicts the LRU victim, the victim is the displaced value; for `pop`/`remove`, the returned value is the one leaving the root set. Concretely, `eval_memo_put` (dispatch_hints.rs:683) currently does `memo.put(...)` (:692) and discards the `LruCache::put` return (the evicted `(query_gen, epoch, gen, entries)`); the barrier captures that return and shades the `entries`' MettaValues when `marking_in_progress()`.

**Corrections to the v2 SATB framing (verified):**

1. **`remove_from_space_shared` was MIS-CITED.** v2 called `btm.remove(mork_bytes)` (core.rs:3059) "the smoking gun." Verified: `remove_from_space_shared(&self, value: &V)` is at core.rs:3043; `btm.remove` at :3059 removes a **byte-key from the PathMap and drops NO `MettaValue`** (PathMap stores byte-keys, not values). The actual V-drop is `rule_index.remove_rule(&lhs, &rhs)` at **core.rs:3111** (and `remove_rule_by_debruijn` at :3099) — and `RuleIndex remove/clear/bulk` is **already in the v2 KEEP list** (fix #5). So `remove_from_space_shared` does NOT need a new barrier on `btm.remove`; the `remove_rule*` family already covers the real deletion. v3 strikes the `btm.remove` barrier from the set.

2. **`(compact-space!)` `variable_atoms.clear`** (act_tiered.rs:781, inside `compact_space` :722): verified T0-only — `bytecode/mod.rs:578` routes `"compact-space!"` to T0 (`=> false` from tier-routing, comment :574 "T0-only special forms"). **Resolution:** compact-space runs on the T0 driver thread as a directive-level special form. Two safe options; v3 chooses **(a) ADD a barrier** on the values dropped by `variable_atoms.write().clear()` (shade them when `marking_in_progress()`), because compact-space CAN run mid-directive while workers (and thus the E2 marker) are live — it is NOT guaranteed quiescent. If a future audit proves compact-space holds the GC gate (no concurrent marker), downgrade to **(b) a debug-assert `!marking_in_progress()`** at act_tiered.rs:781. v3 ships (a) and documents the downgrade condition.

3. **Append-only non-sites stay DROPPED** (v2 fix #5, preserved): `tokenizer::register_token_value` (`.push`), `register_inferred_type` (`.push`) — appends never drop a root.

**KEEP (v2, preserved and re-verified):** `change_state`; RuleIndex `remove_rule`/`remove_rule_by_debruijn`/clear/bulk (core.rs:3099/3111 et al.); CoW `make_owned`/`update_pathmap`; `add_to_space_shared` overwrite (core.rs:2918); `bind` re-bind (symbol_bindings.rs:27); `add_type_generic`/`remove_type_generic` overwrite (type_system.rs:76/:197); `SpaceRegistry::register` overwrite (space_registry.rs:111).

**Guard-rail extended to ALL anchor caches:** the `pattern_cache` debug-assert (core.rs:2217, verified writer-less today: `pattern_cache: RwLock<LruCache<MettaValue,Vec<u8>>>` :383, only read at :2217/:2178) — `debug_assert!(barrier_present || !marking_in_progress())` — is replicated at EVERY anchor cache's (current or future) eviction/overwrite/clear site. Any future writer that lacks the barrier trips CI loudly.

---

## Part 5 — Cache-epoch (CLUSTER 5, HIGH/MED): +OPERATOR_CACHE, pub(crate), NOT over-invalidating

**5.1 ADD OPERATOR_CACHE to the epoch-guard set.** Verified: `OPERATOR_CACHE` (dispatch_hints.rs:908) is keyed on `head.as_ptr()` (`op_cache_key` :917) and guarded ONLY by `RULE_EPOCH` (`operator_cache_get` :929-934). It IS cleared by `clear_aba_sensitive_caches` → `clear_operator_cache()` (eval_loop.rs:223) — but that clears only the *calling thread's* thread-local. Under E2 (concurrent marker, §Part 3), a PARKED worker's OPERATOR_CACHE is not reached by the GC/sweeping thread's `clear_operator_cache()`. With Addr-reuse after a concurrent sweep, that parked thread's pointer-keyed entry becomes a stale read on resume. **Fix:** add a `gc_sweep_epoch` cell to OPERATOR_CACHE, mirroring `VALUE_HASH_CACHE`'s `ensure_value_hash_cache_epoch_current` (metta_value.rs:130-138):

```text
thread_local! { static OPERATOR_CACHE_EPOCH: Cell<u64> = const { Cell::new(0) }; }
fn ensure_operator_cache_epoch_current() {
    let cur = gc_allocator::gc_sweep_epoch();
    OPERATOR_CACHE_EPOCH.with(|e| if e.get() != cur { clear_operator_cache(); e.set(cur); });
}
// call at the top of operator_cache_get (dispatch_hints.rs:928)
```

Now a parked-then-resumed worker self-invalidates lazily on its next `operator_cache_get`, exactly like VALUE_HASH_CACHE.

**5.2 `bump_gc_sweep_epoch` → `pub(crate)`.** Verified `pub(super)` (gc_allocator.rs:3694); the index sweep lives in `backend::eval::cesk::index_heap`, NOT in `gc_allocator`'s `super`, so it cannot call it (confirmed by index_heap.rs:2116-2118: "the index collector cannot call the `pub(super)` epoch bump, so it uses the same public clear path"). Change to `pub(crate)` so the index sweep can `bump_gc_sweep_epoch()` after a reclaiming concurrent sweep (§Part 3 step b). This lets the lazy-epoch mechanism (VALUE_HASH_CACHE + OPERATOR_CACHE + MORK) carry the parked-thread invalidation instead of the GC thread reaching into every thread-local.

**5.3 DO NOT blanket-add `gc_sweep_epoch` to EVAL_MEMO/MATCH/subgoal/thunk** (reverses v2 fix #3's over-reach). Reasons, both verified: (a) **over-invalidation** — these are large content-addressed caches (EVAL_MEMO 16384 entries, dispatch_hints.rs:415; MATCH 4096, :755); a gc_sweep_epoch conjunct would flush them every reclaiming cycle → Robot memo collapse (the same regression class as the cache-root-refresh asymmetry documented at eval_loop.rs:2505-2515 and 3033-3036: adding a cache guard "regresses Robot.metta from 40-85 to 19-21"). (b) **unnecessary** — these caches are **rooted** (they appear in `collect_global_anchors` :318-321, so their values survive the mark) AND **content-addressed** (keyed by `expr_hash`, not by Addr). The only Addr-keyed staleness is in the hash layer (VALUE_HASH_CACHE), which self-clears on the epoch; once VALUE_HASH_CACHE is epoch-correct, a content-hash lookup into EVAL_MEMO/MATCH is correct. The index STW path's outright `clear_eval_memo`/`clear_match_result_cache` (index_heap.rs:2128-2129) is retained for E1 (Addr-reuse across directives, comment :2123-2127); for E2 the rooted+content-addressed argument means we do NOT clear them on every concurrent cycle — they stay warm.

**5.4 INNER_SHADOW epoch-clear guard-rail.** Verified: `clear_inner_shadow()` (index_heap.rs:2122) drops the laundered `&'static MettaValueInner` shadow; the danger (index_heap.rs:1916) is "freeing them while a worker holds a ref." State the invariant + assert:

> **Invariant NO-LAUNDER-ACROSS-POLL.** No laundered `&'static MettaValueInner` or `ValueView` borrowed from a side-`Box` may be held live across a safepoint poll (the point where a worker may park and the GC thread may sweep/clear INNER_SHADOW). Every materialized inner is either (i) re-derived after resume, or (ii) kept alive by a structural `MettaValue` handle (which IS a root), never by a bare `&'static`.

Debug-assert: at each safepoint poll, a thread-local "no live laundered borrow" sentinel (set when a `ValueView`/`&'static inner` is taken from a side-Box, cleared when dropped) must be clear; trips CI if a future code path laundered an inner across a poll.

---

## Part 6 — Index-mode JIT/VM polls (CLUSTER 6, HIGH, liveness)

**The defect (verified):** the v2 "drop the `is_worker` gate at call_support.rs:172 / sexpr_ops.rs:169 / vm/mod.rs:1341" targets the WRONG code. Verified:
- **call_support.rs:168** — the entire `is_worker` safepoint block is `#[cfg(not(feature = "index-gc"))]` → DEAD under index-gc (comment :161-167: "SLAB-ONLY (B4.2 cfg-wall)... Under index-gc the JIT roots are STRUCTURAL — the `VmLeaf::Jit` K-leaf"). Real path: actual file is `src/backend/bytecode/jit/runtime/call_support.rs`.
- **sexpr_ops.rs:166** (`src/backend/bytecode/jit/runtime/sexpr_ops.rs`) — NOT cfg-gated (compiles under index-gc), but calls the non-parking stub `worker_cooperative_safepoint` (:175), gated on `is_worker` (:167-169).
- **vm/mod.rs:1262** (`src/backend/bytecode/vm/mod.rs`) — `run_cooperative_safepoint` is NOT cfg-gated; it `collect_roots_into`s value_stack+locals+results (:1270) then calls the stub (:1276). The VM poll fires every 256 instructions (:1338), gated `is_worker && is_gc_requested()` (:1339-1342).
- **`worker_cooperative_safepoint`** (eval_loop.rs:174) is the Phase-9 NON-PARKING stub: it only `register_temporary_roots(roots)` (:197); the `collect_frame_chain_roots` at :188 is `#[cfg(not(feature="index-gc"))]`. NO `drop_eval_guard_for_safepoint`/park/reacquire.

So under index-gc the VM poll *compiles and fires* (passing operand-stack/locals/results as roots) but **never parks** (the stub no-ops the park), and the JIT poll is *dead*. A JIT/VM worker therefore never parks → the gate `WORKERS_PARKED_FOR_GC == n_threads` (§Part 1) can never balance → hang.

**The fix — two parts:**

**6.1 Make `worker_cooperative_safepoint` ACTUALLY park on the index path.** Add the park/reacquire dance under `#[cfg(feature = "index-gc")]` (the existing body stays for slab):

```text
pub(crate) fn worker_cooperative_safepoint(extra_roots: &[MettaValue]) {
    if !gc_allocator::is_gc_requested() { return; }      // fast path unchanged (eval_loop.rs:178)
    let mut roots = Vec::with_capacity(extra_roots.len() + 16);
    #[cfg(not(feature = "index-gc"))]
    frame_chain::collect_frame_chain_roots(&mut roots);   // slab only, unchanged
    roots.extend_from_slice(extra_roots);
    clear_aba_sensitive_caches();
    #[cfg(feature = "index-gc")]
    {
        // GENUINE park on the index path (the Cluster-6 fix):
        let my_gen = gc_allocator::current_cycle_gen();          // §1.3
        let saved_depth = gc_allocator::drop_eval_guard_for_safepoint_full(); // full-depth drain (§Part 8/9)
        gc_allocator::worker_park_and_root_in_cycle(&roots, my_gen);          // §1.3 cycle-checked buffer+park
        gc_allocator::reacquire_eval_guard_after_safepoint_full(saved_depth); // atomic re-add (§Part 9)
    }
    #[cfg(not(feature = "index-gc"))]
    let _root_handle = gc_allocator::register_temporary_roots(roots);  // slab path unchanged
}
```

**6.2 Widen the gate at the index-compiling poll sites + add a real index JIT poll.** The `is_worker` predicate at vm/mod.rs:1339 and sexpr_ops.rs:167 is widened: under index-gc the poll must fire for ANY thread that holds an `EvalGuard` and observes `is_gc_requested()` (not only `IS_PARALLEL_WORKER`-flagged threads), because under the dedicated-GC-thread model the driver and batch threads must also park. Concretely the gate becomes `is_gc_requested() && (cfg!(index-gc) || is_worker)`. For call_support.rs:168 (currently fully cfg'd-out), add a NEW `#[cfg(feature = "index-gc")]` poll on the JIT tier-return edge that passes the JIT register file + the local `arg` as `extra_roots` (via the structural collector — the index path already has `VmLeaf::Jit` for the K-spine; the poll adds the *liveness* park that the structural rooting does not provide):

```text
// call_support.rs:168 — ADD an index arm beside the slab arm:
#[cfg(feature = "index-gc")]
if crate::backend::models::gc_allocator::is_gc_requested() {
    let mut roots = Vec::with_capacity(64);
    roots.push(arg.clone());
    gc_roots::collect_jit_roots_into(ctx_ref, &mut roots);   // register file + pending result
    worker_cooperative_safepoint(&roots);                    // now genuinely parks (6.1)
}
```

The VM poll (vm/mod.rs:1338) already passes value_stack+locals+results via `run_cooperative_safepoint` (:1270) — it now actually parks via 6.1. `run_cooperative_safepoint` is NOT cfg-gated (verified :1262), so no new function is needed there; only the `is_worker` widening at :1339.

**Verified which safepoint fns compile under index-gc:** `run_cooperative_safepoint` (vm/mod.rs:1262) — yes; `worker_cooperative_safepoint` (eval_loop.rs:174) — yes (the body's frame_chain call is the only cfg'd-out part); the JIT `jit_pre_eval_arg` block (call_support.rs:168) — NO (cfg'd out), so it needs the new index arm; `jit_maybe_pre_eval_structural` (sexpr_ops.rs:166) — compiles, needs the `is_worker` widening.

---

## Part 7 — Batch-results caller window (CLUSTER 7, HIGH for E2)

**The defect (verified):** `evaluate_batch_parallel_arena` (rholang_integration.rs:466) returns `Vec<(usize, Vec<MettaValue>, bool)>` at :545 (sorted at :544). The gather-fn-scoped root handle (v2 finding #1's `SAFEPOINT_ROOTS` walk of the `results` Arc, :484) ends at the fn boundary. The CALLER holds the returned `batch_results` at :413 and :436 and consumes it via the push loops (:414-420 / :437-443) — pushing each `result` into `result_state.output` (:418/:441). Under E2 (concurrent marker can run while the caller iterates), those `MettaValue`s are unrooted between the fn return and the `output.push`.

**The fix — root `batch_results` in the CALLER until consumed.** Push directly into the GC-rooted `result_state` before any drop, OR register a temporary root over `batch_results` at the caller. v3 chooses the **push-directly** form (simplest, no extra handle lifetime to reason about):

```text
// rholang_integration.rs:413 and :436 — wrap the consume in a caller-scoped root:
let batch_results = evaluate_batch_parallel_arena(current_batch, env.clone()).await;
let _caller_root = gc_allocator::register_temporary_roots(           // NEW: roots ALL slots
    batch_results.iter().flat_map(|(_, rs, _)| rs.iter().copied()).collect()
);
for (_batch_idx, results, should_output) in &batch_results {          // borrow, not move
    if *should_output {
        let mut output = result_state.output_mut();
        for &result in results { output.push(result); }              // output_mut is already a root (driver-C)
    }
}
drop(_caller_root);  // safe: every `result` is now in result_state.output (rooted via SAFEPOINT_ROOTS / driver-C)
```

`register_temporary_roots` is the verified narrow driver-transport channel (used at eval/mod.rs:307 and the v2 batch finding). `result_state.output` is part of the driver-C program (`MettaState.output`, rooted via `collect_driver_program_roots`, eval/mod.rs:302 + `collect_safepoint_roots` :307). This closes the window at BOTH caller sites (:413 and :436).

---

## Part 8 — MORK park depth==0 guard (CLUSTER 8, MED)

**The defect (verified):** `drop_eval_guard_for_safepoint` (gc_allocator.rs:4643) asserts `depth > 0` (:4646-4649). The type-fixpoint path runs AFTER the `EvalGuard` drops — `result.1.maybe_run_type_fixpoint()` at eval/mod.rs:316 is explicitly "OUTSIDE EvalGuard scope" (comment :313, guard dropped at :257). If `run_type_fixpoint` reaches a MORK conversion (`mork_bindings_to_generic`, mork_convert.rs:1180, building in-flight `bindings` at the :1196 loop) that hits a safepoint at depth==0, the assert fires → panic/hang.

**The fix — guard `EVAL_GUARD_DEPTH > 0` before every `drop_eval_guard_for_safepoint` call site.** At depth==0 take the **finisher path (bump count only, no park)** if `is_gc_requested()`, else no-op:

```text
// at every safepoint call site (worker_cooperative_safepoint §6.1, the MORK park
// mork_convert.rs:~1196, the type-fixpoint path reached from eval/mod.rs:316):
let depth = gc_allocator::eval_guard_depth();
if depth == 0 {
    if gc_allocator::is_gc_requested() {
        // depth==0 ⇒ not in active set ⇒ no machine to park; just self-root any
        // in-flight values (e.g. MORK bindings) and bump via the finisher (§1.1),
        // OR no-op if there are no in-flight roots. NEVER call drop_eval_guard_for_safepoint.
        gc_allocator::worker_finish_into_buffer(&inflight_roots);  // or no-op
    }
    return;
}
// depth > 0: the normal park path (drop_full → park → reacquire_full).
```

The MORK park (mork_convert.rs:1196) roots the in-flight `bindings` map values before the finisher/no-op. A depth==0 caller is, by construction, not counted in `n_threads` (§1.2 increments only at depth 0→1), so it does NOT need to contribute to `WORKERS_PARKED_FOR_GC` — the no-op is sound. (If it DOES hold in-flight roots, the finisher publishes them as transport, harmless.)

`eval_guard_depth()` is a new trivial accessor (`EVAL_GUARD_DEPTH.with(|d| d.get())`), mirroring the test-only read at gc_allocator.rs:7810.

---

## Part 9 — Resume flag unification + atomic depth-drain reacquire (CLUSTER 9, HIGH)

**The defect (verified two parts):**
1. **Two-flag resume.** The parker (`worker_wait_for_resume`, gc_allocator.rs:3071) waits on `is_gc_requested()` (i.e. `GC_REQUESTED`) under `RESUME_MUTEX`/`RESUME_CONDVAR`. But `reacquire_eval_guard_after_safepoint` (gc_allocator.rs:4663) waits on `GC_IN_PROGRESS` under `GC_PROGRESS_MUTEX`/`GC_PROGRESS_CONDVAR` (:4670-4684). The GC thread's `resume_workers` (:3206) clears `GC_REQUESTED` + notifies `RESUME_CONDVAR`; `GcInProgressGuard::Drop` (:3624) clears `GC_IN_PROGRESS` + notifies `GC_PROGRESS_CONDVAR`. A parked worker that goes park→reacquire crosses BOTH handshakes — fragile, and ordering-sensitive (if `GC_IN_PROGRESS` clears before `GC_REQUESTED`, the worker can wake from the park then re-block in reacquire; if the reverse, fine — but the order is not pinned).
2. **Depth>1 partial-increment.** `drop_eval_guard_for_safepoint` (gc_allocator.rs:4649) drops only ONE depth level (`d.set(depth-1)` + single `fetch_sub` :4652) — NOT the full depth. A depth-2 worker that parks via the current primitive leaves `active`/`N_THREADS` mis-counted by 1. And `reacquire` (:4670) does a single `fetch_add`; restoring depth-2 would require two gated single-increments, each of which can race the gate → partial increment → next cycle's `n_threads` is wrong.

**The fix:**

**9.1 Unify the resume flag.** The parker and the reacquire both wait on the **same flag + condvar the GC thread's resume targets.** v3 unifies on `GC_REQUESTED` + `RESUME_CONDVAR` (the `resume_workers` target, :3206), because `GC_REQUESTED` is the semantic "GC wants you parked" signal and `resume_workers` already clears it last in the driver sequence (§Part 2 step 9):

```text
// reacquire_eval_guard_after_safepoint_full (gc_allocator.rs:4663) — CHANGED:
// wait on !is_gc_requested() under RESUME_MUTEX/RESUME_CONDVAR (the SAME pair the
// parker uses, :3071), NOT on GC_IN_PROGRESS/GC_PROGRESS_CONDVAR.
fn reacquire_eval_guard_after_safepoint_full(saved_depth: u32) {
    {
        let mut lock = RESUME_MUTEX.lock();
        while is_gc_requested() { RESUME_CONDVAR.wait_for(&mut lock, RENDEZVOUS_WAIT_TIMEOUT); }
    }                                                    // single handshake, same flag as park
    // now pass the admission gate ONCE (§9.2) and re-add the full depth atomically.
    ...
}
```

`GC_IN_PROGRESS` remains the *admission* gate for FRESH `EvalGuard::enter` (§Part 2) — a different concern from *resume*. The driver clears `GC_IN_PROGRESS` (drop `_gip`, step 8) BEFORE `resume_workers` (step 9), so by the time a parked worker observes `!is_gc_requested()` and re-enters, admission is already open — no re-block. This pins the order the v2 mechanism left ambiguous.

**9.2 Atomic full-depth drain + reacquire.** Upgrade the safepoint primitives to handle the full depth in one shot:

```text
// drop_eval_guard_for_safepoint_full (replaces the one-level :4649) — returns saved depth:
pub fn drop_eval_guard_for_safepoint_full() -> u32 {
    let depth = EVAL_GUARD_DEPTH.with(|d| { let v = d.get(); d.set(0); v });
    assert!(depth > 0);                                   // callers guard depth==0 (§Part 8)
    let prev = ACTIVE_EVALUATORS.fetch_sub(depth, Ordering::AcqRel);  // drain ALL at once
    if prev == depth { /* quiescent notify (:4654 generalized prev==depth) */ }
    N_THREADS.fetch_sub(1, Ordering::AcqRel);             // §1.2: this thread leaves the active set
    depth
}

// reacquire_eval_guard_after_safepoint_full(saved_depth) — pass the gate ONCE, then one fetch_add:
pub fn reacquire_eval_guard_after_safepoint_full(saved_depth: u32) {
    // §9.1 single resume wait on !is_gc_requested() ...
    // admission: one GC_IN_PROGRESS check (the gate is now closed only during a NEXT cycle):
    loop {
        if !GC_IN_PROGRESS.load(Ordering::Acquire) { break; }
        let mut lock = GC_PROGRESS_MUTEX.lock();
        while GC_IN_PROGRESS.load(Ordering::Acquire) { GC_PROGRESS_CONDVAR.wait_for(&mut lock, GC_WAIT_TIMEOUT); }
    }
    ACTIVE_EVALUATORS.fetch_add(saved_depth, Ordering::AcqRel);  // SINGLE add of the full depth
    N_THREADS.fetch_add(1, Ordering::AcqRel);                    // §1.2: rejoin the active set
    EVAL_GUARD_DEPTH.with(|d| d.set(saved_depth));               // restore depth in one set
}
```

The admission check is passed ONCE and the `fetch_add(saved_depth)` is a single atomic — never a loop of gated single-increments. This eliminates the partial-increment / mis-computed-`n_threads` class. The depth-balance test (gc_allocator.rs:7810) is extended to depth>1: assert `depth_before == depth_after` across a park/reacquire at depth 2 and 3.

This also reconciles the v2 fix #7 ("drain the FULL depth") with the verified current code (which drained only one level): v3 makes the full-drain the actual primitive.

---

## Part 10 — Updated commit ladder + verification

**Ladder (revised; E1 STW-first, then E2 concurrent):**

- **E1-a** — GC thread + `GcInProgressGuard::try_enter` driver (§Part 2 sequence) on the index STW sweep; `n_threads` counter + depth-0→1/1→0 hooks (§1.2); admission-via-GC_IN_PROGRESS-before-snapshot (§Part 2). *Output-equivalent* (dedicated-thread timing band, converged finding #10).
- **E1-b** — finisher-at-finish-site (§1.1, the 3 verified sites) + cycle-generation parked-count protocol (§1.3) + `worker_finish_into_buffer`.
- **E1-c** — full-depth-drain park + atomic reacquire + unified resume flag (§Part 9); depth==0 guard at every safepoint site (§Part 8); MORK park (mork_convert.rs:1196).
- **E1-d** — index-mode JIT/VM polls: `worker_cooperative_safepoint` actually parks on index (§6.1); `is_worker` widening + new index JIT arm (§6.2). Closes the JIT/VM hang.
- **E1-e** — batch gather-root + `WorkerEvalScope` routing for batch workers (rholang_integration.rs:500) + caller-window root (§Part 7).
- **E1-f** — cache-epoch: `bump_gc_sweep_epoch` → `pub(crate)` (§5.2), OPERATOR_CACHE epoch guard (§5.1), INNER_SHADOW no-launder guard-rail (§5.4). Oracle-gated (machine-equivalence oracle eval_loop.rs:3690 as the L3-1 detector).
- **E1-FLIP** — drop `worker_ever_spawned` from the driver predicate `gate_open_rendezvous` (index_heap.rs:1870-1871; verified this is the real fn, v2's "gate_open_concurrent:1871" was the abbreviation); default ON. The one non-value-equivalent commit; gate ASAN FANOUT>0 + PLN budgets at FANOUT=8.
- **E2-a** — SATB at the complete site set (§Part 4): the 9-anchor-derived set with the LRU-evicted-value barrier shape; guard-rail at all anchors; `remove_from_space_shared` correction (no `btm.remove` barrier; `remove_rule*` already KEPT); compact-space barrier (§Part 4 item 2).
- **E2-b** — concurrent marker (§Part 3): `mark_concurrent` under `.read()` + brief full-major `.write()` sweep + brief-gate split + synchronous SATB barrier marking + allocate-black PRIMARY (converged #8) + abort-to-STW backstop (converged #9, drops to E1's write-across-mark) + in-place not-done pump (converged #10). Welch benchmark.

**Gate (per increment):** ASAN FANOUT>0 0-UAF + `cycles_run()>0` ×20, capped `systemd-run -p MemoryMax=20G -p MemorySwapMax=0` (heavy ASAN builds: `-Zbuild-std` ≤48G low-`-j`, foreground, tee'd; the 125 GiB cap for the heaviest). **TLA+ (E5):** extend `StoreCentricGC.tla` — GcThread-driver (not in `active`); **per-slot reified witness** gate (single-location HB, SC-faithful); finisher-as-immediate-counter (no park); **cycle-generation straggler exclusion** (NEW — a bump with `gen != cur_gen` is a no-op); mid-spawn admission via GC_IN_PROGRESS-before-snapshot (the §Part 2 interleaving); SATB Shade over the 9-anchor set incl. LRU-evicted-value; allocate-black; **mark-under-read-lock soundness** (NEW — Invariant E2-MARK: the read-locked concurrent mark + synchronous SATB shades + allocate-black + reified-K snapshot preserves every live node); re-prove QuiescenceInvariant / NoUseAfterFree / NoConcurrentFree / NoUnshadedDeletion / NoUnpublishedAllocSwept / MarkTerminates(+abort backstop) + **NoUncountedLiveMutator** (NEW, §Part 2) + **NoStragglerBump** (NEW, §1.3) + **DepthBalance** (NEW, §Part 9). **loom:** GC-thread-driver + finisher + cycle-gen + full-depth reacquire (the relaxed-memory obligation). **TSan** on FANOUT>0 (esp. the read-locked concurrent mark vs `alloc_*_concurrent`). 20-run determinism; machine-equivalence oracle as the L3-1 detector; mmverify "Correct"; HE-bisim 40/40; conformance 483/221/40 value-equivalent cycles>0; PLN budgets (Robot ≤12s/≤8GB, Toothbrush/FlyingRaven ≤25s/≤4GB).

---

## Part 11 — Genuine-CESK argument (PASSED twice; preserved + extended for v3)

Union = `⋃ᵢ σ|_Reachable(⟨Cᵢ,E_localᵢ,Kᵢ⟩) ∪ reach(E₀) ∪ driver-C`, every term read structurally. v3 changes NOTHING about the structural-root claim:
- per-mutator `collect_machine_roots_live` from its OWN registers (roots.rs:373; the midloop narrowing is a machine-state property);
- E₀ via `collect_persistent_roots` = `collect_roots_into` ∪ `collect_global_anchors` ∪ `collect_k_spine` (roots.rs:404-415);
- dispatch results via the parent's `WaitForParallel` K-frame; driver-C via the KEPT narrow `register_temporary_roots`/`collect_safepoint_roots` channel (eval/mod.rs:307);
- the **finisher** (§1.1) self-reads its own result registers at the finish site — same structural form as the parker; `WORKER_ROOT_BUFFER` (gc_allocator.rs:2906) remains TRANSPORT for already-structural roots, NOT a discovery oracle (categorically unlike the A5-deleted `Weak<dyn RootProvider>`);
- the **caller-window root** (§Part 7) and **MORK in-flight `bindings` root** (§Part 8) are structural handles pushed through the same transport, not discovered.

NO registry/RootProvider; structural roots only. The JIT/VM index polls (§Part 6) add *liveness* (the park) on top of the *already-structural* `VmLeaf::Jit`/`VmLeaf::Vm` K-leaves (vm/mod.rs:1239-1255) — they do not introduce a discovery channel; the extra_roots they pass (register file / operand stack) are the same values the K-leaf decodes structurally, passed eagerly for the park-window snapshot.

---

## Part 12 — Pre-empted round-3 attacks

1. **"The finisher at the finish site races the slot store — a worker that finishes, bumps the count, then the parent reads the slot before the store completes."** No: the finisher self-roots the result VALUES into `WORKER_ROOT_BUFFER` (transport) BEFORE the `guard[slot] = Some(...)` store; the GC thread marks from the buffer (gated on the count), independent of the slot store. The parent reads the slot only after `remaining.fetch_sub` (eval_loop.rs:2550), which is after the store. Two independent paths; no race.

2. **"`N_THREADS` double-counts on reacquire (a parked worker decrements at drop_full, increments at reacquire_full — net zero, but a concurrent snapshot mid-park sees the wrong value)."** No: the snapshot (§Part 2 step 4) is taken AFTER `GC_IN_PROGRESS` is set (step 3) and the resume (which clears `GC_REQUESTED`, allowing reacquire's `N_THREADS.fetch_add`) happens at step 9 — strictly after the gate balances (step 5) and the sweep (step 7). A parked worker cannot `reacquire`-and-rejoin until resume, which is post-sweep. So during the window [snapshot, sweep] `N_THREADS` is monotone non-increasing (drops as workers park, never rises) — the gate `WORKERS_PARKED_FOR_GC == n_threads_snapshot` is reached and held.

3. **"Cycle-generation wraps (AtomicU64 overflow) — a straggler with `my_gen == cur_gen` by wraparound bumps the wrong cycle."** Bounded: U64 at one cycle per (minimum) microsecond wraps in ~585,000 years. Documented as a non-concern; if paranoid, a 128-bit gen or an epoch-fence at 2^63 is a trivial follow-up.

4. **"The read-locked concurrent mark sees a torn `len` (a slot half-published) and marks garbage."** No: `len` is read with Acquire (index_arena.rs:228), paired with the publisher's `compare_exchange(.., Release)` (:304); a slot in `0..len` is fully written (the converged #8 invariant, re-verified). The marker never reads `bump` (the claim cursor); only `len` (the publish cursor). Allocate-black sets the mark before publish (:193 before :304).

5. **"SATB barrier on EVAL_MEMO's silent LRU eviction (dispatch_hints.rs:692) fires on the HOT path — Robot memo collapse returns."** No: the barrier shades the EVICTED value (cheap: one `IndexArena::mark` of the displaced entry) ONLY when `marking_in_progress()` (a Relaxed load, false on the hot path between cycles). It does NOT flush or epoch-guard the cache (that was the v2 over-reach, explicitly rejected in §5.3). Steady-state Robot memo hit-rate is unchanged.

6. **"compact-space! clears `variable_atoms` (act_tiered.rs:781) while the E2 marker is mid-mark — the cleared values are unshaded."** Closed by §Part 4 item 2: v3 ships a barrier that shades the cleared values when `marking_in_progress()`. If compact-space is later proven gate-holding, the barrier downgrades to a debug-assert.

7. **"The abort-to-STW re-acquires `GcInProgressGuard` while a concurrent-mark `.read()` lock is held — deadlock (write-after-read on the same RwLock)."** No: the abort path (§Part 3, E2-b) first leaves the SATB function, dropping the read-locked mark scope and RAII-cleaning any open rendezvous/request; then it re-issues `request_gc`, re-acquires `GcInProgressGuard` (admission), and uses the normal STW write-across-mark path. The read and write locks are never held simultaneously by the GC thread.

8. **"A depth==0 type-fixpoint MORK conversion (eval/mod.rs:316) that allocates heavily never yields to GC → OOM on the 4GB budget."** Bounded: depth==0 means `active==0` for that thread (it's the driver post-eval); the index quiescence collector (eval/mod.rs:276-309) already fires at this exact point BEFORE the type-fixpoint (the `should_collect()` block precedes `maybe_run_type_fixpoint`). If the type-fixpoint itself allocates past the watermark, the next directive's quiescence collection reclaims it; and abort-to-STW (2× watermark) bounds RSS within the fixpoint if a worker is concurrently live.

9. **"Two top-level `eval` calls (eval/mod.rs:240 and the trace variant :370) on different threads each take an outermost `EvalGuard` — `N_THREADS==2` — but only one drives GC; the other is an uncounted driver."** No: BOTH are counted (each bumps `N_THREADS` at its depth 0→1, §1.2) and BOTH poll `is_gc_requested()` at their safepoints and park via the finisher/park path. The GC thread is neither — it's the dedicated thread taking `GcInProgressGuard::try_enter`, never an `EvalGuard`. So `n_threads_snapshot` counts both drivers; the gate waits for both to park/finish. (This is the v2 "the driver is the GC thread, not a mutator" property, now robust to multiple concurrent top-level evals.)

10. **"`worker_finish_into_buffer` (no park) bumps the count, but the finished worker keeps running and allocates after the bump — allocate-black should cover it, but it raced the barrier-arm."** No: the SATB barrier + allocate-black are armed (§Part 3 step a) BEFORE `GcInProgressGuard` is released, i.e. before any finisher can observe `is_gc_requested()` and run. Wait — `GC_REQUESTED` is set at step 1, before the barrier arm at step a. Correction/closure: the finisher's post-bump allocations are covered because allocate-black is armed when `marking_in_progress()` becomes true (step a), and the finisher's `is_gc_requested()` observation (step 1's flag) only triggers the *count bump*; the worker's subsequent allocations during the concurrent mark hit the allocate-black path (mark-bit-before-publish). Any allocation in the tiny window [step 1, step a] is BEFORE marking starts, so it is in the initial heap the mark scans by `len` — covered. No gap.

---

### Critical Files for Implementation
- `src/backend/models/gc_allocator.rs` (EvalGuard enter/drop :3460/:3493, the parked-count/RENDEZVOUS primitives :2887-3210, drop/reacquire-for-safepoint :4643/:4663, GcInProgressGuard :3600, bump_gc_sweep_epoch :3694 — the spine: §1.2 N_THREADS, §1.3 cycle-gen, §Part 2 admission, §Part 9 full-depth/unified-resume, §5.2 pub(crate))
- `src/backend/eval/cesk/index_heap.rs` (mark_sweep_if_over_watermark :2010 with write()-across-mark :2073-2076, gate_open_rendezvous :1870, cache clears :2119-2129 — §Part 3 mark-under-read split + brief-write sweep, E1-FLIP)
- `src/backend/eval/trampoline/eval_loop.rs` (worker closures finish sites :2522/:3043, worker_cooperative_safepoint stub :174, clear_aba_sensitive_caches :211 — §1.1 finisher placement, §6.1 index park)
- `src/backend/eval/cesk/roots.rs` (collect_persistent_roots :404, collect_global_anchors :301 — the canonical 9-anchor SATB derivation, §Part 4)
- `src/rholang_integration.rs` (evaluate_batch_parallel_arena :466, batch EvalGuard::enter :500, caller consume :413/:436 — §Part 2 batch admission, §Part 7 caller window) and `src/backend/eval/trampoline/dispatch_hints.rs` (OPERATOR_CACHE :908, EVAL_MEMO/MATCH evictions :692/:810 — §Part 4 LRU barrier, §5.1 OPERATOR_CACHE epoch)


## Red-team log

### Round 1 — NET-ADDITIVE (architecture + genuine-CESK SURVIVED; mechanism revised → v2)
Genuine-CESK: **PASS** (WORKER_ROOT_BUFFER = transport, not a discovery oracle). Viability: SURVIVES for
E1-STW (CEX-1 closed-by-held-guard). Deep root cause: v1's bare `active==0` gate was provably weaker than the
proven parked-count gate → spawned L2-1/L2-2/L2-3. 13 findings (ranked CRIT→LOW): L1-1/CEX-2 batch gather UAF;
L2-2 StoreLoad race; L3-1/L3-2 stale parked-worker Addr-keyed caches; L2-1 `active==0` unreachable
mid-directive (is_worker-gated polls + non-parking stub); L4-1 SATB site set incomplete
(`remove_from_space_shared` + 5 more); L4-6 premature dormant-code deletion; L2-3/L1-4 unstated `depth==1`;
L4-2 no-minor-during-major OOM; L4-3 abort budget undefined; L4-5 byte-identical over-claim + buggy in-place
pump; L2-4 MORK no-park; L2-6 not the sole `GcInProgressGuard` taker. ALL fixed in v2 above.

### Round 2 — NET-ADDITIVE (architecture + genuine-CESK PASS AGAIN; residue clustered + shrinking → v3)
CONVERGED (net-subtractive) in: genuine-CESK fidelity (PASS — batch gather-root/finisher/SATB-barriers all
structural); allocate-black ordering (#8 SOUND — `set_mark` before `len`-publish; marker scans by `len`, not
the bump cursor); abort-to-STW (#9 SOUND — through the per-slot witness gate and fresh fallback request; full
STW mark/sweep terminates under the write lock); cache-reuse ordering (worker self-clear then read — NO FLAW; slot reuse is write-lock-gated, epoch-bump
Release before reuse); result-ordering determinism (results sorted by slot index → value-equivalence holds);
`inner_ptr` cache inventory (VALUE_HASH_CACHE + MORK covered). **No round-2 finding refutes the ARCHITECTURE**
— all are mis-placed/incomplete/mis-cited mechanisms (refinement, not refutation). Convergence is near.

Net-new findings (→ v3), clustered:
- **CLUSTER 1 — finisher-in-Drop infeasible + gate arithmetic [CRIT/MED]** (RT2-L1 #1/#2/#4/#5): `EvalGuard`
  is a unit struct; its `Drop` has NO access to ⟨C,K⟩ (the inner trampoline frame already returned) → the
  finisher can't self-root. Check-then-`active--` is racy → a counted-in-`n` mutator can finish without
  bumping → hang/UAF. The gate counts `active_evaluator_count()` (guard-increment SUM) but membership is
  per-THREAD; depth>1 desyncs `parked==n`. reset vs straggler finisher bump unsynchronized. → FIX: handle
  "finish while GC requested" at the eval_loop FINISH SITE (registers in scope), not in Drop; count THREADS
  (`n_threads` = a depth-0→1 counter), gate `parked==n_threads`; bracket parked-count writes under
  RENDEZVOUS_MUTEX + a cycle-generation check.
- **CLUSTER 2 — admission asymmetry [HIGH]** (RT2-L1 #3): non-worker/batch `EvalGuard::enter`
  (eval/mod.rs:240, rholang_integration.rs:500) skip the WorkerEnter gate → enter `active` AFTER the snapshot,
  before `try_enter` sets GC_IN_PROGRESS → uncounted-but-live → UAF. → FIX: put the admission gate INTO
  `EvalGuard::enter` (ALL entrants gate on GC_REQUESTED), OR take GC_IN_PROGRESS before the snapshot.
- **CLUSTER 3 — E2 STW-vs-concurrent contradiction [CRIT/HIGH]** (RT2-L2 A/B): `mark_sweep_if_over_watermark`
  holds `global_index_heap().write()` across the WHOLE mark+sweep = STW-by-RwLock → "minors continue /
  allocate-black" is never exercised; `GcInProgressGuard` across the full mark freezes all `EvalGuard::enter`.
  → FIX (E2 ONLY; E1-STW keeps the locks): the concurrent marker MARKS under `.read()` (concurrent
  `alloc_*_concurrent` + allocate-black), SWEEP alone under `.write()`; do NOT hold `GcInProgressGuard` across
  marking.
- **CLUSTER 4 — SATB set STILL incomplete [HIGH]** (RT2-L4 #1/#2/#3): v2 derived from `collect_roots_into`
  ONLY, missing the `collect_global_anchors` family (roots.rs:301-322): `global_memo_cache` evict_lru
  (memo_cache.rs:151), EVAL_MEMO/MATCH silent LRU eviction (dispatch_hints.rs:690/810), space_registry
  remove/clear (space_registry.rs:118/143), subgoal/thunk lookup-stale-evict (tabling.rs:173, thunk.rs:187);
  + `(compact-space!)` `variable_atoms.clear` (act_tiered.rs:781, T0-only → assert quiescent). The
  `remove_from_space_shared` "smoking gun" is MIS-CITED (`btm.remove` drops no V — the actual drop is
  `rule_index.remove_rule`). → FIX: re-derive from `collect_persistent_roots` (roots.rs:404); shade the value
  RETURNED by put/pop/remove (the evicted entry); extend the guard-rail to ALL anchor caches.
- **CLUSTER 5 — cache-epoch [HIGH/MED]** (RT2-L3 A/B/D/F): OPERATOR_CACHE is a surviving stale-read (no epoch
  field; omitted from the list) — ADD it; `bump_gc_sweep_epoch` is `pub(super)` → won't compile from the
  index sweep, make `pub(crate)`; do NOT add the blanket `gc_sweep_epoch` conjunct to EVAL_MEMO/MATCH/
  subgoal/thunk (over-invalidates → Robot memo collapse; UNNECESSARY — those are rooted + content-addressed,
  correct once VALUE_HASH_CACHE is epoch-guarded); INNER_SHADOW epoch-clear is safe only by an unstated
  no-launder-across-poll invariant → add a guard-rail.
- **CLUSTER 6 — index-mode JIT/VM polls DON'T EXIST [HIGH, liveness]** (RT2-L3 E): the "drop the `is_worker`
  gate" fix targets `#[cfg(not(feature=index-gc))]` DEAD code; the index JIT/VM tiers have no cooperative poll
  on the tier edge + `run_cooperative_safepoint` routes to the non-parking stub → a JIT/VM worker never parks
  → hang. → FIX: add genuine index-mode safepoint polls on the JIT/VM tier edges (pass register file/operand
  stack as extra_roots); complete `worker_cooperative_safepoint` to actually park on the index path.
- **CLUSTER 7 — batch-results CALLER window [HIGH for E2]** (RT2-L3 C): the gather-fn-scoped root handle
  doesn't cover the caller's handoff (rholang_integration.rs:413-419) → unrooted under E2. → FIX: root
  `batch_results` in the caller until consumed.
- **CLUSTER 8 — MORK park depth==0 panic [MED]** (RT2-L2 D): the type-fixpoint path runs AFTER the guard
  drops; a MORK park at depth==0 → `drop_eval_guard_for_safepoint` asserts depth>0 → panic/hang. → FIX: guard
  depth>0 at every safepoint call site (depth==0 ⇒ take the finisher path or no-op).
- **CLUSTER 9 — two-flag resume + depth>1 reacquire [HIGH]** (RT2-L2 C): `reacquire` waits on GC_IN_PROGRESS,
  the parker on GC_REQUESTED (fragile 2nd handshake); depth>1 `reacquire` can partially-increment → mis-compute
  `n`. → FIX: unify the resume flag; `reacquire(d)` atomic (one `fetch_add(d)` AFTER passing the gate once).

### Round 3 — CONVERGENCE CHECK: NET-SUBTRACTIVE for the E1 foundation (E1-a UNBLOCKED, verified primitive-by-primitive by 2 independent lenses; architecture + genuine-CESK hold). 3 net-new flaws, ALL scoped to LATER increments, each with a verified fix → v3.1. No E1-a blocker.
- **F1 [CRIT, E1-b scope] — §1.3 straggler-drop soundness is FALSE for the batch finish site** (rholang_integration.rs:504): the "result is in the parent's WaitForParallel K-frame" license holds for sites #1/#2 (dispatch/collapse) but NOT #3 (batch — its result lands in the gather Mutex :484, driven by the UNCOUNTED async `run_state_async` :389 which holds no EvalGuard + no root provider). A cycle-gen straggler-drop + a back-to-back cycle can free the batch result in the gap [worker returns, §Part-7 caller-root created]. → FIX: the batch finisher self-roots its result into the PERSISTENT `SAFEPOINT_ROOTS` channel (gen-independent, `collect_safepoint_roots` gc_allocator.rs:4412) BEFORE its EvalGuard drops — NOT the cycle-scoped WORKER_ROOT_BUFFER; AND sequence E1-e (structural batch routing) BEFORE enabling the §1.3 gen-drop for the batch finisher. (Pre-empt #9 omitted the batch async driver rholang_integration.rs:389.)
- **F2 [HIGH, E1-c scope] — §9.1 unified-resume-on-`GC_REQUESTED` re-creates the two-flag fragility**: ≥10 non-driver callers set `GC_REQUESTED` (gc_cron.rs:322, eval_loop.rs:2748/2849, context.rs:80/469, …), so a back-to-back cycle's `GC_REQUESTED=true` can re-block a resuming worker in `reacquire` (or make it miss its wake) → liveness inversion / 5 s-timeout starvation. The CURRENT `reacquire`-on-`GC_IN_PROGRESS` (gc_allocator.rs:4670) is driver-EXCLUSIVE and correct; §9.1 INTRODUCED the bug. → FIX: gate `reacquire`'s resume-wait on `GC_CYCLE_GEN != my_gen` (the gen I parked for has ended) — reuse the §1.3 gen counter; immune to a new cycle re-setting the boolean.
- **LRU [HIGH, E2-a scope] — the "shade the value RETURNED by `put`" SATB barrier is WRONG for `lru`-crate caches**: `lru::LruCache::put(k,v)->Option<V>` returns the old value ONLY for a same-key overwrite; on a CAPACITY eviction it returns None and silently drops the victim (lru-0.12.5 lib.rs:300-319) — the barrier would miss every capacity eviction = the exact silent-LRU case. Affects EVAL_MEMO (dispatch_hints.rs:692), MATCH (:810), and the NOW-VERIFIED **BYTECODE_CACHE** (cache.rs:210, a real SATB anchor v3 left unverified). → FIX: for lru-crate caches use `push()->Option<(K,V)>` / `peek_lru()` (which surface the capacity victim), NOT `put`; ADD BYTECODE_CACHE to the SATB set. The custom/HashMap caches (memo_cache `evict_lru` :160, tabling/thunk `remove`) keep "shade the returned value" (correct). STRIKE `collect_compiler_atom_roots` (roots.rs:311 — verified write-once OnceLock, a NON-site).
- **Verified-clean (net-subtractive confirmations):** §Part 2 admission interleaving HOLDS (N_THREADS bump realizable after the :3488 admission break); §Part 3 SATB-arm visibility + read→write-gap HOLD (allocate-black armed across the gap; marker scans by `len`); VM `collect_roots_into` IS exhaustive incl. `choice_points` (vm/mod.rs:1086 — refutes a potential backtrack-UAF); `collect_jit_roots_into` EXISTS (gc_roots.rs:43); §5.2 `pub(crate)` necessary+sufficient; §Part 7 covers both caller sites. The 10 pre-empts hold except the F1 (#9-gap) / F2 (#2-gap) seams.

**E1-a is UNBLOCKED** (primitives verified present: `try_enter` :3612, N_THREADS-after-admission :3488, `requestor_wait_for_parked_count` :3143 reusable verbatim). **v3.1 = v3 + {F1, F2, LRU}** folded into E1-b / E1-c / E2-a respectively (apply at the named increment).

### Round 4 — FINAL: **CONVERGED.** Net-subtractive — no new architecture/E1-a flaw; LRU fully converged; the 2 pending fixes sharpened to their exact in-tree-proven form. E1-a safe to implement NOW (verified by all 4 rounds).
- **LRU fix (E2-a): CONVERGED** — `push()`/`peek_lru()` verified correct for BOTH the same-key-overwrite AND capacity-evict returns (lru-0.12.5 lib.rs:317-414); recency side-effect identical to `put`; the BYTECODE_CACHE shading walker `collect_bytecode_cache_roots`/`collect_chunk_constants` exists (cache.rs:261-296, iterative/stack-safe); `CAN_COMPILE_CACHE` correctly excluded (holds no MettaValue).
- **F2 fix REFINED (E1-c): the gen-gated reacquire is correct ONLY with the GEN bump at cycle END.** Round 4 caught that v3 §Part 2 step 2 placed `reset_rendezvous_counters` (the GEN bump) at cycle START → a parker (my_gen=K) would block until cycle K+1 STARTS (may never fire) → starvation/hang. **FIX: bump `GC_CYCLE_GEN` + reset the parked-count at cycle END (after sweep, immediately before `resume_workers`) — exactly as the existing in-tree D2.3 driver already does (eval_loop.rs:4022→4023).** With reset-at-END: a parker captures my_gen=K-1, the end-of-cycle bump→K makes `reacquire` see `K != K-1` and release immediately, AND the §1.3 straggler (my_gen=K-2) stays excluded. §Part 2's driver sequence is corrected to bump-gen-at-END (move the reset from step 2 to adjacent step 9). E1-a's driver follows this in-tree placement from the start.
- **F1 fix REFINED (E1-b/E1-e): the batch `SafepointRootHandle` must RIDE the shared `results` structure, not be a closure-local** (a closure-local drops at worker-return = the exact gap). FIX: extend the gather tuple (or a parallel `Vec<Option<SafepointRootHandle>>`) to CARRY the handle worker→caller; the caller drops it after `output.push`. `SafepointRootHandle` is auto-`Send` (single `usize` field, gc_allocator.rs:4356) → the cross-thread move is sound. The soundness DIRECTION (SAFEPOINT_ROOTS gen-independent + in the index union, `collect_safepoint_roots` gc_allocator.rs:4412 read at eval/mod.rs:307) is confirmed correct.
- **Holistic E1 sweep: E1-a CONVERGED + UNBLOCKED** — all primitives verified present: `GcInProgressGuard::try_enter` :3612; `EvalGuard::enter` admission-break (:3469) then depth-bump (:3488) → §1.2 N_THREADS hook realizable after admission clears; `requestor_wait_for_parked_count` :3143 reusable verbatim; the live D2.3 driver (eval_loop.rs:3963-4027) is the working template. Cross-fix composition clean (F1∥§Part2 orthogonal; F1∥§1.3 two transport paths coexist; F2∥§1.3∥§Part2 resolved by reset-at-END). No E1-a blocker.

**VERDICT: CONVERGED after 4 rounds.** Architecture + genuine-CESK held EVERY round (R1→R4). Trend: 13 fundamental (R1) → 9 placement (R2) → 3 late-increment (R3) → 0 new + 2 fix-refinements + LRU-converged (R4, net-subtractive). **v3.1-final = v3 + {F1-via-results-tuple, F2-reset-at-END, LRU-push}.** Implement E1-a now; apply F1/F2/LRU at E1-b/E1-c/E1-e/E2-a respectively (each = "do what the in-tree code already does"). The genuinely-hard E2-b concurrent marker gets a dedicated final red-team + TLA+/loom against the real E1 code when reached.
