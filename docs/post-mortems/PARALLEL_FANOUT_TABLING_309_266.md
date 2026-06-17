# Post-mortem & design: parallel-fanout tabling correctness (#309 / #266)

**Status:** fix landed across four mechanisms; formally verified (TLA+/TLC); runtime-validated under contention.
**Branch:** `feature/petta-semantics`. **Component:** `src/backend/eval/cesk/{tabling.rs,thunk.rs}`, `src/backend/eval/trampoline/eval_loop.rs`.
**Formal models:** `tla/{CrossThreadCycleDetection,MidDerivationClear,ThunkChannelClear,ThunkChannelSeed}.tla`.

---

## 1. Symptom

Parallel PLN inference (`PLN-main/examples/Robot.metta`) under `METTATRON_PARALLEL_FANOUT_DEPTH=8`
intermittently produced a **clean SUBSET** of the correct result bag, and occasionally **ran away**
(600 s wall timeout / unbounded memory). Single-threaded (`FANOUT=0`) was *always* correct and
byte-identical. The defect was therefore not in the inference logic but in how parallel fanout
interacts with the evaluator's **thread-local memoization / cycle-detection state**.

A subset-drop with no error and no spurious value is the signature of a **missed fixpoint cut**: a
recursive PLN derivation converges only because re-entering an in-flight subgoal/thunk contributes
the EMPTY bag (the cut). If that cut is missed on one thread, the derivation either tables a smaller
bag (→ a later reader drops a layer → subset) or fails to terminate (→ runaway).

## 2. Root-cause class

MeTTaTron detects recursive cycles with **thread-local** tables:

| Channel | Table                                       | Cut state                 | Writer                                    |
|---------|---------------------------------------------|---------------------------|-------------------------------------------|
| Subgoal | `ACTIVE_EVAL_SET` (`tabling.rs`)            | membership (refcount > 0) | `mark_eval_active` (CompleteSubgoal push) |
| Thunk   | `ThunkTable` / `THREAD_THUNKS` (`thunk.rs`) | `ThunkState::Blackhole`   | `lookup` Suspended→Blackhole              |

Single-threaded, a recursive re-entry of subgoal `S` (or thunk `H`) lands on the **same thread** that
holds it active/Blackhole, so it is cut at the first re-entry. Under fanout, a recursive
sub-derivation is dispatched to a **worker with a fresh thread-local table**, which does not see the
parent's in-flight state and so **misses the cut**. Two independent failure modes follow, on two
channels each — four mechanisms total.

```
                       ┌─────────────────────── parent thread ───────────────────────┐
   derive S  ─────────▶│  ACTIVE_EVAL_SET = {S}   ThunkTable[H] = Blackhole          │
                       │        │  recursive sub-goal of S / sub-thunk of H fans out │
                       └────────┼────────────────────────────────────────────────────┘
                                ▼   dispatch (parallel_dispatch / parallel_collapse_dispatch)
                       ┌──────── worker thread (fresh tables) ───────────┐
              BUG ───▶ │ ACTIVE_EVAL_SET = {}   ThunkTable = {}          │  re-enters S / H
                       │  → MISSES the cut → re-derives → subset/runaway │
                       └─────────────────────────────────────────────────┘
              FIX ───▶   seed the worker's tables from the parent at dispatch (below)
```

## 3. The four mechanisms and their fixes

### M1 — Subgoal cross-thread cut gap (root cause #1, subgoal) — *fixed earlier, committed `16c51dc2`*
A fanned-out worker re-deriving a parent-active subgoal misses the cut.
**Fix — the seed:** a thread-local `SEEDED_ACTIVE_SET` (disjoint, refcounted) seeded at dispatch from
`snapshot_active_hashes()` (parent's `ACTIVE_EVAL_SET ∪ SEEDED_ACTIVE_SET`, so nesting is transitive)
via the RAII `SeedActiveScope`. `is_actively_evaluating(h)` consults both sets; the seed probe
short-circuits on an empty seed, keeping FANOUT=0 byte-identical. `clear_active_eval_set` clears only
`ACTIVE_EVAL_SET`, so the seed persists for the worker's life (RAII-managed).
**Model:** `CrossThreadCycleDetection.tla` — `SeedActiveOnFanout=FALSE` → `NoDroppedResult` violated; `TRUE` → holds.

### M2 — Subgoal mid-derivation clear (root cause #2, subgoal) — *fixed earlier, committed `16c51dc2`*
A GC-rendezvous resume clears the fixpoint memos mid-derivation, dropping the in-flight cut marks.
**Fix:** gate the subgoal-table clear on `active_eval_set_is_empty()` (no `CompleteSubgoal` pending).
**Model:** `MidDerivationClear.tla` — `ClearGuarded=FALSE` → violated; `TRUE` → holds.

### M3 — Thunk mid-derivation clear (root cause #2, thunk channel)
The same GC-resume clear wiped the **thunk** table, but its guard keyed on subgoal-emptiness — and a
blackholed thunk is **not** in `ACTIVE_EVAL_SET`. So a worker mid-thunk-derivation with no subgoal
frame pending fell through the guard and `clear_thunk_table()` wiped the live `Blackhole` →
a sibling re-use missed the memo → subset drop.
**Fix:** give the thunk clear its OWN guard — `thunk_table_has_blackhole()` (`thunk.rs`), and in
`clear_all_worker_thread_local_caches` split the clears so the thunk table is cleared only when no
blackhole is in flight (`eval_loop.rs`).
**Model:** `ThunkChannelClear.tla` — `ThunkGuardChecksThunk=FALSE` (shares the subgoal guard) → violated; `TRUE` → holds.

### M4 — Thunk cross-thread cut gap (root cause #1, thunk channel) — *the runaway*
The thunk channel had no analog of the subgoal seed: a thunk recursion under nested fanout hit a
fresh worker `ThunkTable`, missed its `Blackhole`, and re-derived unboundedly → the 600 s runaway
(and/or a divergent smaller bag).
**Fix — the thunk seed (mirrors M1, zero new dispatch plumbing):**
- `ThunkTable::push_blackhole_seed_keys` + free `collect_blackhole_hashes` (`thunk.rs`) collect the
  **domain-tagged** hashes of all in-flight `Blackhole` thunks.
- `snapshot_active_hashes()` (`tabling.rs`) unions them, so both dispatch sites seed them via the
  existing `SeedActiveScope` — **the seed already flows**.
- `ThunkTable::lookup`'s **Absent** path probes `is_actively_evaluating(thunk_seed_key(expr_hash))`
  and returns `Blackhole` (cut) **without inserting** a stale Suspended (`thunk.rs`).

**H1 — the one genuine design hazard (namespace collision):** subgoal hashes and thunk hashes share
the one `SEEDED_ACTIVE_SET`. A raw u64 collision between a live subgoal hash and a live thunk hash
would let a subgoal seed spuriously cut a *genuine non-cyclic* thunk re-use (a drop). **Domain
tagging** (`thunk_seed_key(h) = h ^ THUNK_SEED_DOMAIN`, ASCII `"THNKSEED"`), applied symmetrically at
producer and consumer, places thunk seed keys in a disjoint domain from untagged subgoal hashes — the
spurious cut becomes impossible while the legitimate cut still fires.
**Model:** `ThunkChannelSeed.tla` — `DomainTag=FALSE` → spurious cut violates `NoWrong`; `TRUE` → holds for both the
spurious (no cut) and legitimate (cut) scenarios.

## 4. Why the thunk channel was the LAST one (boyscout scan)

Every eval-hot-path thread-local was classified. Only correctness-gating cycle/fixpoint state needs
seeding; pure caches merely recompute on a miss. The thunk table was the single remaining
correctness-gating thread-local unseeded across the fanout boundary:

| Thread-local | Verdict |
|---|---|
| `ACTIVE_EVAL_SET` | seeded (M1) |
| `ThunkTable` (`THREAD_THUNKS`) | **the M4 gap — now seeded** |
| `SubgoalTable` (`THREAD_TABLE`), `EVAL_MEMO` | `space_epoch`-revalidated; fresh worker copy is empty → recompute |
| `MATCH_RESULT_CACHE`, `FRESH_NAME_CACHE`, adaptive/incremental-index registries | pure perf; miss recomputes |
| `BINDING_CAPTURE_STACK`, demand/depth/scope counters, fork/cut/trail substrate | re-established per worker by existing guards / per-activation isolation |

## 5. Validation

- **Regression sentinel:** `FANOUT=0` byte-identical (404 lines, `ae1160b3edba`) — guaranteed by the
  empty-seed short-circuit and snapshot-only-at-dispatch; confirmed each build.
- **Contention determinism:** `Robot.metta` ×24 under `FANOUT=8` + 40 CPU stressors, 24 GiB cap.
  Failure modes by mechanism set (line-count metric, which counts bag multiplicity):
  - M1+M2 only (`16c51dc2`): 21/24, 3 drops, 0 timeout.
  - +M3 (thunk clear guard): 21/24, 1 drop, **2 timeouts** (exposed the M4 runaway).
  - +M4 (thunk seed): 20/24, 4 "drops", **0 timeout** — **the runaway is eliminated**.
- **The line-count metric over-counts.** Re-analysing the +M4 runs at the **conclusion-SET** level
  (`sort -u`) separates two distinct residuals:
  - **21/24 runs are SET-identical** (108 unique conclusions). Several "drops" (e.g. run 1: 322 vs 404
    lines) are **pure bag-multiplicity** — the identical conclusion set with fewer duplicate derivation
    paths. These are *not* missing conclusions.
  - **3/24 runs (~12.5%) are a GENUINE drop:** the query `(PLNobjectsOfCategory … bring)` yields only
    `((detection orange someCoords4))` instead of `((detection frisbee someCoords1) (detection orange
    someCoords4))` — the **frisbee** conclusion is lost, and the in-file test asserts `❌` (1 failure in
    the dropping run, 0 in a correct run). This is the true open residual (§6).
- **Formal wall:** all four models, each bug-cfg → invariant violated, fixed-cfg → holds.
- **No test regression:** `cargo nextest run --lib` (re-run after M3+M4).

## 6. Resolution — M4 reverted (structural over-cut), genuine fix = shared memo store

**Discrimination (done, 2026-06-17).** An A/B of the same binary with the M4 seed cut ON vs OFF
(`METTATRON_DISABLE_THUNK_SEED`, 16 contended runs each) was decisive: M4 ON = 11/16 correct, 4 drops,
1 timeout, 5 distinct result-sets; **M4 OFF = 15/16, 1 drop, 0 timeout, 2 sets**. M4 is **net-negative**.
A read-only source root-cause then proved the mechanism is **seed over-cut — structural, not a hash
collision**: the thunk key is content-only (no derivation lineage, `eval_loop.rs:7951-7959`) and the
seed is a **frozen** "all in-flight Blackhole thunks" snapshot never refreshed on Blackhole→Evaluated,
so a worker that merely **needs** a thunk a sibling is concurrently deriving — a **shared in-flight
dependency**, routine across the 4 independent `superpose` branches of the query — is miscut to a
Blackhole error and stripped. A content hash + frozen set **cannot distinguish a genuine cross-thread
cycle from a shared dependency**. (Mechanisms 2/3 above are thereby ruled out: no merge error is raised,
and the cut — not the depth-gate — is the A/B-proven cause.)

**M4 was REVERTED** (commented out with the root cause, per the never-delete mandate). M1+M2 (subgoal
seed/guards) + M3 (thunk mid-clear guard) are KEPT — the runaway was tamed by M1+M2, not M4.
Post-revert: FANOUT=0 byte-identical; 24-run FANOUT=8 SET-level **22/24** correct (vs M4-on's worse),
the frisbee over-cut essentially gone; residual = 1 drop + the rare runaway M4 had targeted.

## 7. The genuine fix (approved) — shared content-keyed memo store (staged)

The residual is the **structural** gap: one logical memoized fixpoint derivation is split across threads,
but the memo + cycle state is per-thread. The approved fix replaces the cross-thread-unsafe thread-local
thunk/subgoal tables with a **SHARED content-keyed memo store**:
- **Read** another thread's `Evaluated` result (fixes re-derivation runaway + bag multiplicity);
- **Await** an in-flight one — a true dependency — **never miscut it**;
- **Cut only a genuine cycle**, decided by a transported **derivation-id lineage** (`owner == me` or
  `owner is my ancestor`), with a **wait-for-graph** + **local-eval fallback** that *provably cannot
  deadlock* against the parent-blocks-on-workers merge (the blocked parent is always an ancestor, so a
  worker needing its thunk takes the non-blocking branch and never parks on the parent).

**Staged so every commit is correct** (Step 2 is a shippable correct floor at the conclusion-SET level;
Steps 3–5 recover parallelism):

| Step | What | Correct? | TLA model |
|------|------|----------|-----------|
| 1 | Lineage-id transport + `is_ancestor` (plumbing) | no-op | — |
| 2 | Serialize-on-contention fallback | ✓ 24/24 SET (floor) | `SerializeMemoizedDerivation` |
| 3 | `SharedMemoStore` (DashMap) + unit/loom tests | dormant | `SharedMemoAwaitNoDeadlock` |
| 4 | Wire thunk channel (read/await/cut) | ✓ + parallel | +`NoOverCut` |
| 5 | Wire subgoal channel, subsume M1 | ✓ both channels | retarget |

Validation each step: FANOUT=0 byte-identical sentinel; 24-run FANOUT=8 at the conclusion-SET level
(`sort -u`, not raw line count); `cargo nextest`; the step's TLA model (bug-cfg→violated /
fix-cfg→holds); and `scripts/verify_cesk_gc_all.sh` at the capstone.
