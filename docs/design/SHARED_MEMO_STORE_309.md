# Design: shared content-keyed memo store (#309/#266 architectural fix)

**Status:** approved; Step 1 (lineage) landing. Companion to the post-mortem
`docs/post-mortems/PARALLEL_FANOUT_TABLING_309_266.md` (root cause + revert history).

## 1. Problem

One logical memoized fixpoint derivation is split across threads by parallel
fanout, but the memo + cycle-detection state is **thread-local** (`ThunkTable`/
`THREAD_THUNKS` keyed by a content hash; `SubgoalTable`/`THREAD_TABLE` + the
`ACTIVE_EVAL_SET` cycle set). Three failure modes result:

- **(A) over-cut drop** — a tactical cross-thread seed (the reverted M4) miscuts a
  *shared in-flight dependency* (a worker needs thunk `T` a sibling is deriving) as
  a *cycle*, fabricating a `Blackhole` error the success-biased filter strips → a
  conclusion (the `frisbee`) silently drops. The thunk key is content-only (no
  lineage) and the seed was a frozen "all in-flight" snapshot, so cycle vs shared
  dependency were **indistinguishable**.
- **(B) re-derivation runaway / multiplicity** — each worker re-derives shared
  sub-thunks from a fresh table → OOM runaway / bag blowup.
- **(C) bag-multiplicity nondeterminism** — different schedules table different
  duplicate counts (benign at the conclusion-SET level, but noisy).

## 2. Design space (evaluated)

| Option | Fixes A/B/C? | Parallelism | Hazard |
|--------|--------------|-------------|--------|
| (i) shared content-keyed store + await + cross-thread cycle detection | A,B,C | maximal | **deadlock** vs parent-blocks-on-workers merge; GC of in-flight values |
| (ii) per-branch ancestor lineage (cut only genuine ancestors) | A only | maximal | none new — but **half a fix** (no B/C) |
| (iii) serialize the memoized sub-derivation (fan out only independent work) | A,B,C | minimal on PLN | none — but loses the fanout win |

**Recommended: hybrid (i)+(iii) with (ii)'s lineage as the cut discriminator.**
The shared store provides cross-thread memoization (read/await) for the common
case; at the one edge where awaiting would deadlock, the requester *cuts* (if a
genuine lineage cycle) or *evaluates locally* (the (iii) fallback — bounded
re-derivation, never blocks). This is correct-by-construction *and* recovers the
parallelism (i) gives, with (iii) only as the rare deadlock-edge fallback.

## 3. Data structures

```text
SharedMemoEntry {
  state:       AtomicU8,                       // EMPTY|INFLIGHT|DONE|ERROR
  owner:       AtomicU64,                       // derivation-id of the in-flight owner
  results:     OnceLock<SmallVec<[MettaValue;2]>>,  // published once on DONE
  space_epoch: u64,                             // captured at INFLIGHT claim
  waiters:     Mutex<()> + Condvar,             // park/notify, lazily used
}
SharedMemoStore {
  thunks:   DashMap<u64, Arc<SharedMemoEntry>>, // thunk_hash  -> entry
  subgoals: DashMap<u64, Arc<SharedMemoEntry>>, // tabling_hash -> entry
  waitfor:  WaitForGraph,                        // derivation-id -> awaited ids (deadlock detection)
  query_epoch: u64,                              // bumped per top-level `!`
}
```

- **Concurrency:** lock-free `DashMap` (dep `dashmap 6.1`) for the maps; per-entry
  `OnceLock` for the published result; a tiny per-entry condvar only for the rare
  *await*. Hot path (read a DONE entry) takes no lock; INFLIGHT claim is one
  `compare_exchange` on `state`. Fallback if `loom`/`tsan` flags a DashMap-vs-GC
  ordering issue: sharded `RwLock<HashMap>` with the GC taking read guards.
- **Derivation-id lineage** (Step 1, `cesk/shared_memo.rs`): each fanned-out worker
  enters a `DerivationScope` with a fresh id whose parent is the dispatching
  thread's id (transported beside the active-eval seed). `is_ancestor(owner, me)`
  walks the parent chain — the discriminator M4 lacked.

## 4. Lookup / derive / publish protocol (per channel; gated by `worker_ever_spawned()`)

```text
DONE(e)    if e.space_epoch == now && e.query_epoch == cur  => READ e.results        // cross-thread memo hit (B,C)
ERROR(e)                                                     => propagate stored error
EMPTY                                                        => CAS EMPTY->INFLIGHT, owner=me, push CompleteThunk (I derive + publish)
INFLIGHT(e) if e.owner == me                                 => CUT to fixpoint EMPTY  // genuine same-derivation cycle
INFLIGHT(e) else:
    if waitfor.would_cycle(me -> e.owner):                                            // awaiting would deadlock
        if is_ancestor(e.owner, me) then CUT to fixpoint EMPTY                        // real cross-thread cycle
        else                          EVAL LOCALLY (iii fallback; never blocks)       // shared dependency
    else: add edge; PARK on e.waiters until DONE|ERROR; remove edge; re-dispatch      // await a true dependency
```

`CompleteThunk`/`CompleteSubgoal` publish: on the existing torn-read guard
(`start_epoch == now`), `OnceLock::set(results)`, `state=DONE`, `notify_all`; on
torn read or panic (RAII guard mirroring `CompletionGuard`), `state=ERROR` +
`notify_all` so awaiters re-derive rather than park forever.

**FANOUT=0 byte-identity:** the whole shared path is gated behind
`worker_ever_spawned()` (mirrors the M1 empty-seed short-circuit); single-threaded
keeps using the thread-local table verbatim. INFLIGHT-by-me cuts exactly where
thread-local `Blackhole` cut; DONE-by-me reads exactly where `Evaluated` read.

## 5. Proofs

**Deadlock-freedom.** The wait-for graph is acyclic by construction — an await
edge is added only after `would_cycle` returns false. The parent (blocked in
`WaitForParallelCollapse` on `remaining`) is an **ancestor** of every worker it
spawned; when a worker requests a thunk the parent owns, `is_ancestor(parent, me)`
is true and `would_cycle(me→parent)` is true (the parent transitively awaits the
worker via `remaining`), so the worker takes the **non-blocking** branch (cut or
local-eval) and never parks on the blocked parent. ∴ the parent-blocks-on-workers
deadlock is structurally impossible.

**No over-cut.** A cut is emitted only when (1) `owner == me` (a genuine
same-derivation re-entry, identical to the single-threaded `Blackhole` cut) or (2)
`would_cycle ∧ is_ancestor(owner, me)` (the owner is a true ancestor on my lineage
= a real fixpoint cycle). A shared in-flight dependency has `owner != me` and
`owner` is **not** an ancestor (it is a sibling/cousin), so it is **awaited** or
**locally evaluated** — never cut. The M4 frisbee over-cut cannot recur.

**Determinism.** A content hash's published *value* is program-determined,
independent of which thread computed it; the DETERM gate hashes the **sorted SET**
(`verify_cesk_gc_all.sh`), so local-eval's extra duplicate paths (bag multiplicity)
are invisible. Local-eval and the awaited result cannot disagree (same content hash,
same `space_epoch`).

**GC.** The store is a named global anchor in `collect_global_anchors` (beside
`collect_thunk_roots`/`collect_subgoal_roots`); `store.collect_roots` walks every
DONE entry's `results` and every INFLIGHT entry's reserved slot, so an in-flight
shared value owned by a *parked* worker stays rooted. Every mutation wraps
`with_satb_deletion_barrier`.

## 6. Staged implementation (each step compiles + commits green)

| Step | What | Correct? | TLA model |
|------|------|----------|-----------|
| 1 | `shared_memo.rs` derivation lineage + transport (no behavior change) | no-op | — |
| 2 | serialize-on-contention fallback at the thunk site | ✓ 24/24 SET (floor) | `SerializeMemoizedDerivation` |
| 3 | `SharedMemoStore` data structure + unit/loom tests (dormant) | dormant | `SharedMemoAwaitNoDeadlock` |
| 4 | wire the thunk channel (read/await/cut/local-eval) | ✓ + parallel | + `NoOverCut` |
| 5 | wire the subgoal channel; subsume M1 | ✓ both channels | retarget `CrossThreadCycleDetection` |
| 6 | capstone (`verify_cesk_gc_all.sh`) + post-mortem close | — | — |

**Validation gates (every step):** FANOUT=0 byte-identical sentinel (404 /
`ae1160b3edba`); 24-run FANOUT=8 contention at the conclusion-SET level (`sort -u`,
not raw line count); `cargo nextest`; the step's TLA+ model (bug-cfg → violated /
fix-cfg → holds); `scripts/verify_cesk_gc_all.sh` at the capstone.

## 7. Residual risk (honest)

Confined to (a) DashMap-vs-GC interaction (mitigated by the sharded-`RwLock`
fallback + `loom`/`tsan` gate) and (b) the throughput cost of the local-eval
fallback under pathological contention — a *performance* regression, never a
correctness one, bounded by fanout depth. Both acceptable under "correct
architecture over clever fragile one."
