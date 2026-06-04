# E1-FLIP — the rendezvous coordination-race root causes + converged fix

**Status:** root causes IDENTIFIED (experiments + 3 explore agents) + design RED-TEAMED TO CONVERGENCE
(5 rounds, Plan agent, 2026-06-02). Implementation pending. This is the ACTUAL E1-FLIP blocker —
distinct from (and orthogonal to) the CEX-1 root-completeness work (`e1-flip-cex1-principled-rooting.md`,
which is sound-but-not-the-cause).

## The bug (proven)
Dedicated-GC-thread "rendezvous" (FANOUT>0, `dedicated_gc_enabled()`, default OFF) corrupts PLN results
(robot @ FANOUT=8 ~5-7/8 wrong: a VALID-BUT-WRONG SUBSET — well-formed atoms, NO freed-memory/SEGV).
Controlled experiment: corrupts with ZERO reclamation (Arm C) + in the committed baseline ⇒ NOT a
GC root/sweep defect — a CONCURRENCY RACE in the park/resume coordination.

## Root causes (PLURAL — converged across 3 explore agents)
**① Two uncoordinated GC regimes run at once.** `parallel_gc_coop_enabled()` is hardcoded `true`
(`context.rs:277`), independent of `dedicated_gc_enabled()`. The LEGACY Phase-9 async-cooperative path
runs concurrently with the dedicated rendezvous: (a) the parent pump (`eval_loop.rs:2885-2927`) fires its
OWN `request_gc()` — a driver-less `GC_REQUESTED` producer no rendezvous clears (dual-trigger); (b) the
parent self-roots via `register_temporary_roots` WITHOUT parking — a participant by `n_threads()` count but
non-participant by protocol, so its in-flight PLN-fold transient is unrooted while the dedicated thread
sweeps → mis-folded → wrong subset. Worker `ParallelBranchContext::perform_safepoint` (`context.rs:463-470`)
is the same legacy path. Explains baseline + sweep-independence + the signature.

**①c (found by the red-team, Round 2 — B4) MORE driver-less `GC_REQUESTED` producers under dedicated:**
- the **GC cron** `gc_cron.rs:316-323` (`request_gc()` + `maybe_async_gc()` on a committed-bytes threshold)
  — an INDEPENDENT producer with no rendezvous driver → can STRAND a parked worker (no driver to resume) +
  `maybe_async_gc` is a 2nd collector regime;
- `SessionContext::perform_safepoint` `context.rs:78-81` (+`should_safepoint` :62) and `session_context.rs:242`
  — legacy `register_temporary_roots`+`request_gc` producers.
So the "exactly one regime" invariant requires gating ALL of them, not just `parallel_gc_coop_enabled`.

**② Mis-keyed admission gate.** WorkerEnter must check `dedicated_gc_enabled()`, not
`rendezvous_enabled()`; in the dedicated-ON/rendezvous-OFF regime a new worker doesn't park at admission
→ joins after the driver's `n=n_threads()` snapshot (`gc_driver.rs:177`) → "new mutator mid-cycle" hole.
The branch-dispatch worker was already keyed correctly; the collapse-dispatch worker had the same stale
legacy gate and was corrected on 2026-06-04.

**③ (possible) done-gate cancel disjunct.** `eval_loop.rs:15288` `done_now = remaining==0 ||
cancel_token.is_satisfied()`; merge skips `None` slots → a parked worker's branch dropped under non-`All`
demand. The enclosing `collapse` shadows demand to `Demand::All` (`eval_loop.rs:6663,6906`) ⇒ likely-negative
for robot; confirm with a debug-assert.

**REFUTED (do not pursue):** the `clear_aba_sensitive_caches` flush (content-addressed perf/ABA caches only;
does NOT clear MATCH_RESULT/EVAL_MEMO; recompute deterministic → NO_FLUSH effect is timing-Heisenberg); and
thread-local-arena reset across park (the park touches only sync counters; `clear_thread_arena`/
`clear_region_stack` are never called; in-flight values are Rust stack locals + the global store, intact).

## Converged fix (post-5-round red-team; establishes the SINGLE-REGIME invariant)
1. **①a** `context.rs:277` — `parallel_gc_coop_enabled() = !dedicated_gc_enabled()`.
2. **①c** gate the other legacy `GC_REQUESTED` producers under dedicated:
   - `gc_cron.rs:316-323` — `request_gc()`+`maybe_async_gc()` gated on `!dedicated_gc_enabled()`;
   - `context.rs:78-81`/`:62` — `SessionContext::perform_safepoint`/`should_safepoint` inert under dedicated;
   - `session_context.rs:242` — audit + gate if a producer.
   ⇒ INVARIANT: under dedicated the ONLY `GC_REQUESTED` producer is `request_concurrent_collection`
   (`gc_driver.rs:320`, always posts a `CollectRendezvous` driver) and the ONLY clearer is `resume_workers`.
3. **The parent PARKS via the existing branch (B)** (`eval_loop.rs:4122-4207`) — NO parent-finisher needed.
   Its transient is covered by `WaitForParallel::collect_values` (`types.rs:2388-2423`). NO DEADLOCK
   (proven): the parent parks as a PARTICIPANT; the SEPARATE GC thread drives; no worker needs the parked
   parent to pump (work-pool dispatch + done_pair do not block on the parent during a cycle); resume order =
   gen-bump → drop _gip → resume_workers. (This is why the dedicated-thread design — not the rejected
   Phase-D parent-as-requestor — makes parent-parking safe.)
4. **②** `eval_loop.rs` branch + collapse WorkerEnter — re-key early-park `rendezvous_enabled()` → `dedicated_gc_enabled()`.
5. **③** `eval_loop.rs:15282/15306` — `#[cfg(debug_assertions)]` assert the cancel disjunct doesn't drop a
   live `None` slot under `Demand::All` (confirm/refute; debug-only, byte-identical).
6. **Compose with CEX-1 — KEEP all** (D1 canonical collector + D2 LIVE_DISPATCHES anchor + D5 oracle): ①
   supplies the parent's participation (class 1); D1 the complete per-thread contribution branch (B)
   publishes; D2 the class-2 not-yet-started/admission-blocked coverage (orthogonal); D5 the standing oracle
   that ① stays wired (`parked >= n_snapshot`).

## Byte-identical when dormant
Every edit's primary conjunct is `dedicated_gc_enabled()` (cached const-OFF OnceLock) or
`#[cfg(debug_assertions)]`. Default build: `!dedicated`=true ⇒ legacy coop/cron/SessionContext UNCHANGED;
`dedicated`=false ⇒ no park, no re-key effect. Slab: `gc_mode_is_index()`/`index-gc` cfg-walls. FANOUT=0:
no dispatch, `n_threads()==1`, no branch (A), legacy path identical ⇒ 483/0 byte-identical.

## Validation ladder
1. Build slab + index-gc, 49-warn baseline (clear `target/release/.fingerprint/mettatron-*` first).
2. FANOUT=0 conformance 483/0 byte-identical, both backends × DEDICATED∈{0,1}.
3. 3-way discriminator robot @ FANOUT=8 ×16 → **0/16 on ALL arms** (reuse ON, reclaimed_slots>0):
   A `DEDICATED=1 MIN=131072`, B `DEDICATED=0`, C `DEDICATED=1 MIN=4294967295`.
4. V4 ASAN (`scripts/e1_flip_v4_asan.sh`): 0-UAF + rendezvous-cycles>0 + non-rendezvous==0.
5. ×20 determinism (robot @ FANOUT=8 DEDICATED=1, single canonical-output hash).
6. Debug build: 0 `assert_rendezvous_union_complete` panics.
Then: commit the coordination fix + CEX-1 together → re-V4 → the 1-line default flip (E1-FLIP Commit B).

## Red-team ledger (5 rounds, converged)
| Round | Attacks | Reversals |
|---|---|---|
| 1 | wakeup, straggler, snapshot timing, lock-order, finish-vs-park | none (overshoot tolerated) |
| 2 | done-gate, back-to-back, ②×finisher, **cron producer** | **B4 → add ①c (gate cron + legacy producers)** |
| 3 | resolve B4 + starvation/2nd-collector | establishes single-producer invariant (①c.1/.2/.3) |
| 4 | first-trigger, ② early-park, ③ mechanics, D2 lock-order, over-count, determinism | none (C3 debug-mechanics; C1 throughput note) |
| 5 | panics ×2, stale flag, byte-identical, slab-default, lost-wakeup | none — CONVERGENCE |

Throughput note (out of corruptness scope, Phase F): after all workers finish (`n_threads()`→1 but
`worker_ever_spawned()` latched), neither branch (A) (`n>1`) nor the midloop collector
(`!worker_ever_spawned`) fires until quiescence; gating the cron removes a (driver-less, already-broken)
mid-directive trigger, with quiescence collection as the backstop.
