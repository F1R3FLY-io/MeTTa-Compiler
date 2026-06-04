# E1-FLIP validation FAILED — the converged coordination fix does NOT work; the prior "0/16" was a metric artifact

**Date:** 2026-06-02 (autonomous tick, during the E1-FLIP commit-gate).
**Verdict:** 🔴 **DEDICATED=1 (the concurrent dedicated-GC-thread collector) is BROKEN by a logic bug.** Do NOT commit
the fix as validated; do NOT flip the default. Production is SAFE (default `METTATRON_INDEX_GC_DEDICATED=0`; the
broken path is opt-in only, and the working-tree fix is UNCOMMITTED — HEAD is `8070c78`, the dormant Commit A).

## What the prior session claimed (the compacted summary)
"The converged single-regime coordination fix (①a `parallel_gc_coop_enabled()=!dedicated_gc_enabled()` + ①c gate
cron/SessionContext + ② re-key WorkerEnter) achieves **A=0/16, B=0/16, C=0/16** on the robot @ FANOUT=8
discriminator with the rendezvous sweeping — the genuine fix."

## What is actually true (clean-machine empirical re-test, this tick)
The prior "0/16" was measured by **counting `❌` marks** in robot's output. That metric is **blind to the dominant
failure mode**: when a collection drops a live atom, robot's test harness **aborts on the first failed assertion**,
so the run is *truncated before later test lines execute* → it has **zero `❌`** and was scored "clean."

Re-tested on a **completely clear machine** (75 GiB free, no sibling-build contention — the contention confound is
ruled out), using `✅`-presence + line-count as the metric (not just `❌`):

| Config | Runs | Result |
|---|---|---|
| `DEDICATED=0` (baseline) | 5/5 | ✅ present, **404 lines**, rc=0 — perfect + deterministic |
| `DEDICATED=1 MIN=131072` (arm A, sweep) | 4/4 | **`✅` NEVER appears**, 14-17 lines, `❌` in 3/4, **1 OOM-killed (rc=137, >16 GiB)** |
| `DEDICATED=1 MIN=4294967295` (arm C, no major) | 2/2 | **broken**, 12-16 lines, 1 OOM-killed; and **HANGS** in a longer run |

## The precise failure mode (captured)
Under `DEDICATED=1`, the first ~13 `(SELECTED …)` results are **byte-identical to the correct DEDICATED=0 path**.
Then `INDEX_GC_CYCLES_RUN=1` (a collection fires), and immediately:

```
is ((detection frisbee someCoords1)), should ((detection frisbee someCoords1) (detection orange someCoords4)). ❌
[(Error "test mismatch: is ((detection frisbee someCoords1)), should ((detection frisbee someCoords1) (detection orange someCoords4))"
        (test (PLNobjectsOfCategory ((detection frisbee someCoords1) (detection (SELF) someCoords2)
                                     (detection person someCoords3) (detection orange someCoords4)) bring)
              ((detection frisbee someCoords1) (detection orange someCoords4))))]
```

⇒ **A live atom — `(detection orange someCoords4)` — is DROPPED by the collection cycle.** robot's harness aborts on
that first failed assertion, truncating the remaining output (the 404→~16 "loss" is a *cascade abort*, not bulk loss).
The intermittent `❌` and OOM (`rc=137`, >16 GiB — collection not reclaiming / unbounded growth) confirm a
nondeterministic, collection-coupled fault.

## Why this overturns the "coordination-race, NOT root-completeness" conclusion
The prior pivot rested on: *"arm C (MIN=4G = no sweep) still corrupts ⇒ corruption is sweep-INDEPENDENT ⇒ it's a
park/resume coordination race, not a dropped root."* But **MIN=4G only disables the MAJOR sweep — MINOR (young,
const 2 MiB `YOUNG_BUDGET`) collection still fires** under arm C. So "arm C corrupts" does NOT prove
collection-independence. The fresh evidence shows corruption **co-occurring with `INDEX_GC_CYCLES_RUN=1` and a
dropped live atom** — i.e. a collection cycle reclaims a still-live value. This is (at least partly) a
**root-completeness / live-value-loss** fault, exactly the class CEX-1 targeted and the summary dismissed.

⚠️ Caveat: the tested binary carries the UNCOMMITTED coordination edits (①a/①c/②) + CEX-1 + ③ — a DIFFERENT binary
than the summary's `8070c78` baseline. It is possible the coordination edits (gating OFF the legacy coop GC path
under dedicated) **regressed** the failure (the dedicated collector becomes the sole collector and either drops a
root or fails to reclaim → OOM). This must be disambiguated by re-testing `8070c78` DEDICATED=1 vs the current
working tree DEDICATED=1 — but that needs a build of HEAD (git ops on uncommitted work → defer to the user).

## ✅ ROOT CAUSE — CONFIRMED by source analysis (general-purpose agent, read-only, 2026-06-02)
**A root-publication TIMING hole, whose proximate cause is the uncommitted coordination edit ①a.** Not a static
root-completeness gap (the structural reader IS complete) and not "coordination, NOT completeness" (the prior
framing) — it is a coordination defect that *causes* a live value to be swept. Mechanism, end to end:

1. The PARENT trampoline thread holds its `EvalGuard` for the entire directive (`mod.rs:240`) ⇒ it **is counted in
   `n_threads()`** — a snapshot participant the collector waits for.
2. **①a `parallel_gc_coop_enabled() = !dedicated_gc_enabled()` (`context.rs:293-295`) turns OFF the parent's
   continuous pump rooting** (`register_temporary_roots` in `pump_parallel_wait`, `eval_loop.rs:2887-2930`) AND the
   worker safepoints (`context.rs:429-431,479`). Under DEDICATED=1 the parent's ONLY self-root becomes the coarse
   **4096-iteration** park (`eval_loop.rs:3871 gc_counter & 0xFFF == 0` → branch-B park+self-root `:4124-4197`,
   `:4151 collect_complete_thread_contribution(Trampoline{work_stack,…})`).
3. **The merge escapes the rooted region** (`WaitForParallel` done-arm, `eval_loop.rs:15307-15359`, AmbConcat
   `:15347`): the orange `Addr` moves from `results[slot]` (rooted by CEX-1's anchor + `WaitForParallel::collect_values`
   `types.rs:2406-2415`) into `merged` on the parent's `work_stack` (`:15356`), and the `return` (`:15359`) **drops
   `handle._live_dispatch` → deregisters the `LIVE_DISPATCHES` anchor slot** (`gc_allocator.rs:4785-4793`). Now the
   atom is reachable ONLY via the parent's *unpublished* `work_stack`.
4. **The parked-count gate is FUNGIBLE** (`index_heap.rs:1779-1785` `workers_parked_for_gc() >= n_threads_at_snapshot()`;
   `gc_driver.rs:182 requestor_wait_for_parked_count(n)`): the count reaches `n` via OTHER threads' bumps — branch
   finishers (`eval_loop.rs:2667`) + the **zero-root EvalGuard-drop bump** (`gc_allocator.rs:3766-3774`) — so the
   driver proceeds to mark+sweep **while the parent is mid-pump, not yet parked, holding `merged` unpublished**. The
   sweep marks from `drain(WORKER_ROOT_BUFFER) ∪ collect_safepoint_roots ∪ collect_live_dispatch_anchors`
   (`gc_driver.rs:185-198`) — none contains the parent-only `work_stack` value ⇒ swept ⇒ slot reused
   (`index_heap.rs:1989-2031`) ⇒ stale-but-valid read ⇒ the wrong subset.

**Why CEX-1 misses it:** its `LIVE_DISPATCHES` anchor (`types.rs:362-382`) covers the *not-yet-started-worker* class;
the orange atom is lost through a DIFFERENT class — the parent's **post-merge transient register** — and the anchor is
*deregistered at the same `return`* that moves the value to `work_stack`, so its coverage interval ends exactly when
the value becomes parent-only. The D5 oracle (`gc_driver.rs:253`) can't catch it: it only asserts registered-dispatch
coverage + `parked >= n` — it has **no term asserting the parent's `work_stack` was published**.

**The missing invariant (the genuine-CESK completeness theorem the protocol needs):** *the sweep must not begin until
EVERY thread counted in the `n`-snapshot has ACTUALLY published its roots* — fungible bump-counting (`count >= n`)
violates this because bumps are interchangeable across threads (a finisher's bump "covers" for the parent's
not-yet-published state).

### Discriminating experiment (NOT run — requires editing the uncommitted edits; left for the user)
Make `parallel_gc_coop_enabled()` UNCONDITIONALLY `true` (i.e. undo ONLY ①a), keep DEDICATED=1 + CEX-1, re-run robot
×16 with the corrected metric. **Prediction:** corruption disappears/sharply drops (the always-on pump
`register_temporary_roots` re-covers the parent's branch transit) — confirming the loss is the parent-publication hole
①a opened. Second arm: keep ①a but shrink the FANOUT>0 parent cadence `& 0xFFF`→`& 0xF`; markedly-less corruption
pins the window to the inter-safepoint stride.

### Recommended fix DIRECTIONS (NOT implemented — user-gated; the prior design is falsified)
1. **PRINCIPLED (theorem-restoring): make the parked-count gate PER-THREAD, not a fungible count.** Block the driver
   until every thread in the `n`-snapshot has published *this cycle* (per-thread published-gen set / stable-index
   bitmap), distinguishing a finisher's bump from the parent's not-yet-published state. This is the genuine-CESK fix:
   the collector witnesses the actual machine of every participant. (`gc_allocator.rs:3355-3378` is the fungible counter
   to replace.)
2. **MITIGATION (re-establish the removed net, dedicated-compatible): do NOT disable `parallel_gc_coop_enabled()` under
   dedicated** — keep the parent publishing during the pump + at the merge, but route it through the rendezvous park
   (publish into `WORKER_ROOT_BUFFER` + bump + park) instead of the legacy `register_temporary_roots` (which the
   dedicated GC thread never drains). I.e. ①a was the wrong shape — the parent must STILL publish under dedicated, just
   via the rendezvous channel.
3. **AVOID alone:** extending the anchor's coverage interval — a point-patch of the same enumerated shape as CEX-1
   (violates the standing "guaranteed-by-construction, not per-escape-path" directive).

**Recommendation: (1) as the principled fix + (2) to validate/unblock. This means ①a as written is WRONG — the
single-regime invariant is right, but it must keep the parent PUBLISHING (via the rendezvous), not merely SILENCE the
legacy path.**

## Open root-cause hypotheses (SUPERSEDED by the confirmed root cause above; retained for history)
1. **Live-value loss during the dedicated collection** (root-completeness): the rendezvous root union misses the
   `(detection orange someCoords4)`-holding location (a worker's in-flight match transient / a not-yet-reified
   register / a work-pool pending task), so the cycle sweeps it. (CEX-1 aimed here but is necessary-not-sufficient.)
2. **The coordination edits regressed it:** gating OFF `parallel_gc_coop_enabled` + cron under dedicated removed a
   path that was doing real collection/rooting work → the dedicated thread alone drops roots and/or OOMs.
3. **Sweep/reclaim-of-live** under the rendezvous "rendezvous" phase (Addr free-list reuse → silent substitution).
4. Deadlock on the no-major arm (arm C HANG) — a separate coordination defect in the rendezvous when no major fires.

## H2 (regression-vs-pre-existing) — attempted, deferred; conclusion holds regardless
Tried to build HEAD `8070c78` (no coordination edits) in a temporary git worktree to test whether DEDICATED=1
breaks there too. **Build failed:** the repo is a `[workspace]` with **relative path-deps** (`../MORK/kernel`,
`../PathMap`, `../f1r3node-rust/models`) that don't resolve from arbitrary temp locations. **Retry recipe:** put
the worktree at a **sibling path** of the repo so `../` resolves identically; `scripts/e1_flip_h2_head_compare.sh`
now derives that path by default. **But H2 is not
decisive:** the summary already records HEAD `8070c78` DEDICATED=1 as broken (its "V4" run), and my edits only
change the failure *shape* (HEAD ran BOTH the legacy coop path AND the rendezvous; my ①a/①c gate the legacy path
OFF under dedicated → sole-collector → adds OOM/truncation). **Either way the root cause is the same: the
dedicated collection cycle drops a live value** (`INDEX_GC_CYCLES_RUN=1` co-occurs with the lost
`(detection orange someCoords4)`). So the redesign must re-target **root-completeness of the rendezvous root union
under FANOUT>0** (which holder of the orange atom is unrooted at collection time?) — CEX-1 aimed here but is
necessary-not-sufficient. H2's only payload is whether the redesign STARTS from HEAD or keeps the coordination
edits; that's a user call (the prior design is falsified).

## Standing facts
- Default build + slab: untouched, byte-identical (cargo check 49-warn baseline both backends — done this session).
- Both backends compile cleanly with the full working-tree fix (slab + index-gc cargo check ✅).
- Evidence logs: discriminator scratch dir, D0/D1 run logs, and the probe `b220itbc2.output`.
- The discriminator script `scripts/e1_flip_discriminator.sh` (written this tick) must be FIXED to score on
  **`✅`-presence + line-count**, not `❌`-count (the artifact). A truncated/❌-free run is a FAILURE, not a pass.
