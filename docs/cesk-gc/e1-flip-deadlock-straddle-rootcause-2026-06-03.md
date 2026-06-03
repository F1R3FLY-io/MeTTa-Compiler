# E1-FLIP residual deadlock — straddle re-park TOCTOU (root-caused 2026-06-03)

The OTHER E1-FLIP residual (besides the corruption): under the dedicated collector
(`METTATRON_INDEX_GC_DEDICATED=1`, FANOUT=8), robot `(collapse …)` HANGS ~0.5-1% of runs
(`timeout` kill). One mechanism was already fixed (`e18bc16`: lost-notify in `witness_release_slot`,
which only cost 5 s/occurrence since every `wait_for` re-checks on a 5 s timeout). The residual is a
**lost-PROGRESS (permanently-false-predicate) deadlock** — root-caused source-conclusively by a
read-only audit agent.

## Mechanism (THE answer)

It lives in the **straddle re-park loop** of `reacquire_eval_guard_after_safepoint_full`
(`src/backend/models/gc_allocator.rs:6061-6086`) and is a **TOCTOU race between the loop's UNLOCKED gen
read and the driver's 3-store / 3-lock cycle-end teardown**.

The driver's cycle-end (`gc_driver.rs:250-252`) is three independent stores under three different locks:
1. `end_rendezvous_cycle()` → `GC_CYCLE_GEN: K→K+1` under `RENDEZVOUS_MUTEX` (gc_allocator.rs:3976)
2. `drop(_gip)`            → `GC_IN_PROGRESS: true→false` under `GC_PROGRESS_MUTEX` (gc_allocator.rs:4717)
3. `resume_workers()`      → `GC_REQUESTED: false` + notify under `RESUME_MUTEX` (gc_allocator.rs:4013)

The gen (which names the **next** cycle) becomes visible at step 1, BEFORE `GC_IN_PROGRESS` is cleared at
step 2. The straddle loop reads `g = current_cycle_gen()` (unlocked, gc_allocator.rs:6062) then branches
on `gc_in_progress() && g != my_reparked_gen` (6063). A worker A that lands its reads in the **narrow
window between step 1 and step 2** sees `g = K+1` AND `gip = true`, interprets it as "a NEW cycle K+1 is
collecting, I must re-park", and calls `worker_park_and_root_in_cycle(_, K+1)` (6070). Inside, it stamps
`published = K+1` (the gen IS K+1) and enters `worker_resume_wait_for_cycle(K+1)` — waiting for
`GC_CYCLE_GEN != K+1`. **But no driver ever runs cycle K+1** (the driver was merely finishing K's
teardown and returns to idle at `gc_driver.rs:110`). So `GC_CYCLE_GEN` stays K+1 forever; A is parked for
a PHANTOM cycle; the collapse parent's `WaitForParallel` K-frame joins on A forever → hang.

The defect is an **ordering inversion**: at cycle end, the gen (naming the next cycle) is published before
`GC_IN_PROGRESS` (signalling the current cycle is done) is cleared, so the straddle test cannot
distinguish "tail of K, gen pre-bumped" from "K+1 genuinely collecting". The strict-`>` witness predicate
is correct for SAFETY (an occupied-unpublished slot is correctly waited on); the failure is pure LIVENESS.

Rare (~0.5-1%) because it needs A's two unlocked reads to fall in the few-instruction window between two
of the driver's teardown stores — but robot fires thousands of FANOUT=8 cycles per run. Matches the
observed signature exactly (all mutators parked/blocked; the GC thread idle, not waiting).

The design doc already flagged this class as an accepted liveness residual:
`docs/cesk-gc/e1-flip-pathB-v2-impl.md:93,159` ("miss = HANG not UAF"). The whole protocol is
fail-safe-toward-HANG, which is why it's a `timeout` kill, never a UAF.

## Fix (to implement after the corruption fix is committed)

**Fix B (preferred — unambiguous start-vs-end gen).** Add `GC_CYCLE_STARTED: AtomicU64`, set to `cur_gen`
when the driver COMMITS to a cycle (gc_driver.rs:~197, paired with the straddle read's lock), distinct
from `GC_CYCLE_GEN` (bumped at END). The straddle re-park condition becomes:
```rust
let started = current_cycle_started();   // gen of an ACTUALLY-STARTED cycle
if started > my_reparked_gen {           // a real, started, later cycle needs me
    witness_restamp_acquired(started);
    worker_park_and_root_in_cycle(reparked_roots, started);
    my_reparked_gen = started; continue;
} else if !gc_in_progress() {
    break;                                // rejoin — no phantom re-park
} else {
    worker_resume_wait_for_cycle(my_reparked_gen);  // my cycle still draining
    continue;
}
```
A worker can never park for a cycle that has not been started by a live driver. Local to the straddle
loop + driver; needs no re-ordering of the safety-critical `_gip`/gen sequence.

**Fix A (alt — re-order teardown).** Drop `_gip` BEFORE bumping the gen (+ merge gip-clear and gen-bump
under one lock to avoid a new entrant reading the pre-bump gen). Then `gc_in_progress() && g != my_gen`
can never be true for the just-finished cycle's bump. Slightly riskier (touches the safety-critical
sequence); prefer Fix B.

**W2 notify hardening (defence-in-depth).** `worker_resume_wait_for_cycle` (W2) keys resume on
`GC_CYCLE_GEN != my_gen` (bumped under `RENDEZVOUS_MUTEX`) but is woken under `RESUME_MUTEX` — different
locks ⇒ a latent lost-wakeup that today self-heals only via the 5 s timeout. Also notify `RESUME_CONDVAR`
from `end_rendezvous_cycle` (which holds `RENDEZVOUS_MUTEX` + does the gen bump), OR move the gen bump
under `RESUME_MUTEX`. Removes the per-occurrence 5 s stalls that widen the hang window.

## Validation: a loom model (the reliable tool)

The existing `loom_rendezvous` (gc_allocator.rs:9630-9875) models ONE cycle with a fungible parked-count,
NO witness slots / gen / straddle / back-to-back cycles → structurally cannot reach this. A new
`#[cfg(loom)] mod loom_straddle` must add: (i) per-slot witness `{acq, pub_, occ}`, (ii) `GC_CYCLE_GEN`
bumped at END, (iii) the driver's 3-store teardown as 3 separate steps in the buggy order,
(iv) the straddle re-park loop verbatim (unlocked gen read + 3-way branch), (v) `GC_CYCLE_STARTED` for the
Fix-B variant. **Critical loom adaptation: model `wait_for(_, 5s)` as a plain `wait` (NO timeout)** — so
loom reports a deadlock IFF a predicate is permanently false (this bug), ignoring the benign
5 s-recoverable lost-wakeups. Threads: worker A (enter→park→straddle-loop→resume-wait→release) + driver
(try_enter→wait-all-parked→sweep→3-store teardown). Assertion: `a.join()` returns for every schedule.
- BUG-REPRO variant (current order, no `started`) → `a.join()` deadlocks → prints the §3 schedule.
- Fix-B variant (`started` gate) → all joins return; safety co-assertion (no sweep while A
  occupied-and-unpublished for the swept gen) still holds.
Run: `RUSTFLAGS="--cfg loom" cargo test --release loom_straddle -- --nocapture`, `LOOM_MAX_PREEMPTIONS=3`
(needs ~2 preemptions: slip A's gen-read between the driver's gen-bump and _gip-drop, then let the driver
finish past A). Full surrogate map + thread bodies are in the audit agent's report (this session).

## ⚠️ CONVERGED DESIGN (red-teamed 2026-06-03) — Fix B alone is INSUFFICIENT; needs the B-closure

A Plan-agent red-team (source-conclusive) found that **Fix B as first drafted RELOCATES rather than removes
the hang.** Corrections (all verified against source):

**Premise fix (load-bearing):** `GC_CYCLE_GEN` is bumped ONLY at cycle END (`end_rendezvous_cycle`,
gc_allocator.rs:3974); there is NO start-of-cycle bump. The driver READS `cur_gen` at prologue
(gc_driver.rs:197) and the witness predicate keys on it. So `GC_CYCLE_STARTED` := that `cur_gen` —
it equals `GC_CYCLE_GEN` DURING the active cycle (K during K), and is always `≤ GC_CYCLE_GEN`; in the
bug window `started=K < gen=K+1`. The straddle gating on `started` thus does NOT re-park in the window
(`started=K ≯ my=K`) — correct.

**`GC_CYCLE_STARTED` MUST be a lock-free `AtomicU64`** (init **0**, not 1), Release store at the driver
prologue, Acquire read in the straddle. It **MUST NOT** be read/stored under `RENDEZVOUS_MUTEX`: the
straddle body calls `worker_park_and_root_in_cycle` → `RENDEZVOUS_MUTEX.lock()` (non-reentrant), so any
mutex-guarded `started` read self-deadlocks 100% (an already-rejected round). HB comes from the gip-CAS
(`try_enter`, AcqRel) ordered-before the `set_current_cycle_started` Release; debug-assert
`gc_in_progress()` right before the store to pin the order.

**THE B-CLOSURE (non-negotiable):** the bare 3-way `started`-gate closes the original step1→step2 window
but leaves the **terminal-break mis-skip** — A breaks with stale `started=K ∧ !gip`, then the driver
starts K+1 (`started=K+1`, begins witness wait on A's `occupied, published=K`), and A sits in the
post-loop NON-publishing `GC_IN_PROGRESS` admission wait (gc_allocator.rs:6090-6110) → driver waits forever.
This is the SAME shape that empirically falsified the earlier "delete-rejoin-wait FIX A". Fix is invariant
**R1**: whenever the driver has `set_current_cycle_started(s)` and is in `requestor_wait_for_all_reified_parked(_,s)`,
every worker with `occupied ∧ published<s` MUST re-observe `started≥s` and re-park (publish ≥s) WITHOUT a
terminal break that leaves it occupied-unpublished. Concretely: the rejoin/break path must **re-read
`current_cycle_started()` and re-enter the straddle re-park if it advanced past `my_reparked_gen`** (or
publish on rejoin) — the bare break is unsound.

**Converged straddle loop** (gc_allocator.rs:6061-6110), `started`-gated + B-closure + spin-elimination:
```rust
let mut my_reparked_gen = my_gen;
loop {
    let started = current_cycle_started();                 // Acquire
    if started > my_reparked_gen {
        witness_restamp_acquired(started);                 // acquired = started (Release)
        worker_park_and_root_in_cycle(reparked_roots, started); // gen-gated republish + note_reified_park(started)
        my_reparked_gen = started; continue;
    }
    if gc_in_progress() {
        if current_cycle_gen() == my_reparked_gen {
            worker_resume_wait_for_cycle(my_reparked_gen); // my cycle genuinely draining
        } else {
            // teardown window: resume-wait would return instantly ⇒ wait on gip transition, don't spin
            let mut l = GC_PROGRESS_MUTEX.lock();
            while GC_IN_PROGRESS.load(Acquire) && current_cycle_started() <= my_reparked_gen {
                let _ = GC_PROGRESS_CONDVAR.wait_for(&mut l, GC_WAIT_TIMEOUT);
            }
        }
        continue;
    }
    break;                                                  // started<=my && !gip ⇒ no cycle waits on me
}
// REJOIN TAIL: must NOT be a terminal occupied-unpublished state. Re-read current_cycle_started()
// inside the admission loop; if it advanced past my_reparked_gen, go back into the straddle re-park
// (B-closure). The current bare non-publishing admission wait is the relocated-hang and must change.
```

**W2-notify hardening (complementary):** in `end_rendezvous_cycle` (under RENDEZVOUS_MUTEX, after the gen
bump) ALSO `{ let _r = RESUME_MUTEX.lock(); RESUME_CONDVAR.notify_all(); }` — removes the per-occurrence
5 s stall (W2 keys on GC_CYCLE_GEN under RENDEZVOUS_MUTEX but is woken under RESUME_MUTEX). New
`RENDEZVOUS→RESUME` nest is safe today (the reverse nest doesn't exist — verified) but the loom model
MUST assert the lock-order. Also have `set_current_cycle_started` notify `GC_PROGRESS_CONDVAR` (for the
else-arm wait + the symmetric-TOCTOU wake).

**Loom (`mod loom_straddle`) MUST assert R1** (the relocated-hang catch): driver loops TWO cycles (K, K+1);
`wait_for(5s)`→plain `wait` (no timeout); BUG-REPRO (gen-gate, no started)→deadlock; Fix-B-without-closure
→R1 fails (post-break K+1-starts schedule); Fix-B-with-closure→all pass. Plus the safety co-assertion
(no sweep while A occupied ∧ published<cur_gen) + the lock-order assertion.

**Fix A (reorder teardown)** is a higher-risk fallback only (touches the safety-critical gip/gen admission
sequence; risks witness-gen aliasing + a new GC_PROGRESS×RENDEZVOUS lock-order edge). Prefer Fix B+closure.

Residual risks (ranked): (1) terminal-break mis-skip — HIGHEST, closed only by the B-closure; (2) Acquire-lag
transient — benign iff R1 holds; (3) lock-free `started` visibility — pin with the gip-CAS HB + debug-assert;
(4) W2 lock-order — assert in loom; (5) teardown-window spin — removed by the else-arm condvar wait.

## Status
Root-caused source-conclusive + **design CONVERGED via red-team (Fix B + the B-closure + W2 hardening +
loom-R1)**. The red-team caught that the bare `started`-gate would relocate the hang — the B-closure is
mandatory. To IMPLEMENT after the corruption fix (Fix #1 forked-env rooting) is validated + committed —
sequence the two concurrency changes, not juggle them. Implement WITH build+loom feedback (do not write
blind). This is task #17 (E5) for the deadlock half of E1-FLIP.
