# E1-FLIP DEDICATED=1 corruption — ROOT CAUSE: the collapse worker omits the B2′ live-env registration

**Date:** 2026-06-03
**Status:** root-caused (source-conclusive) + primary fix applied; validation pending build
**Supersedes the fix-by-fix in:** `e1-flip-VALIDATION-FAILED-2026-06-02.md`, `e1-flip-FIX-design-iterations.md`

## Symptom

`robot` (`/home/dylon/Workspace/f1r3fly.io/PLN-main/examples/Robot.metta`) under
`METTATRON_PARALLEL_FANOUT_DEPTH=8 METTATRON_INDEX_GC_DEDICATED=1 METTATRON_INDEX_GC_MIN_BYTES=131072 --gc index`
produces a **non-deterministic wrong subset** from the `(collapse …)` /
`PLNobjectsOfCategory … bring` filter:

```
should ((detection frisbee someCoords1) (detection orange someCoords4))
is     ((detection (SELF) someCoords2) (detection person someCoords3))   ← garbage, differs each run
```

Control matrix (the discriminator):
- `DEDICATED=0` (default) → **correct** (✅ + 404 lines).
- `DEDICATED=1`, `MIN_BYTES=4294967295` (rendezvous fires, **never sweeps**) → **correct**.
- `DEDICATED=1`, `MIN_BYTES=131072` (rendezvous fires **and sweeps**) → **corrupt**.

⟹ the rendezvous *coordination* is sound; the **sweep frees a live value**. The GC report shows a
single **minor** cycle (`rendezvous minor cycle (young)`, `old_live_after=0`, `reclaimed_slots≈96796`):
the swept-live value is **young**, and `old_live_after=0` means effectively all live data is young, so
a minor is a near-full sweep — any unrooted live young `Addr` is reclaimed.

## Why the four prior fixes did not converge

The dedicated collector's root union (`gc_driver.rs::gc_driver_rendezvous_cycle`, the rendezvous path)
is the union of exactly four sources:

1. `drain_worker_root_buffer` — each **parked** worker's self-rooted machine ⟨C,E,K⟩ ∪ E₀.
2. `collect_safepoint_roots` — `SAFEPOINT_ROOTS` (batch-finisher / directive-result roots).
3. `collect_live_env_anchors` — **B2′** global live-env registry (`LIVE_ENVS`/`register_live_env`).
4. `collect_live_dispatch_anchors` — `LIVE_DISPATCHES` (each dispatch's INPUT `branches` + OUTPUT `results`).

The four prior fixes (witness flip; parent-pump park; full-Trampoline reification in the pump;
clear-aba-caches-after-pump-park) each closed a real hole but all targeted sources (1)/(2)/(4) and the
witness gate. None touched (3) **for the collapse worker** — which is exactly where the hole is.

## ROOT CAUSE (source-conclusive)

**The collapse-worker closure does not register its env in the B2′ live-env registry, whereas the
branch-worker closure does.**

- Branch worker (`parallel_dispatch`), `eval_loop.rs:2594-2603`:
  ```rust
  #[cfg(feature = "index-gc")]
  let _worker_live_env = {
      if dedicated_gc_enabled() {
          let dyn_env: Arc<dyn EnvRoots> = env.shared.clone();
          Some(register_live_env(&dyn_env))   // ← source (3) coverage
      } else { None }
  };
  ```
- Collapse worker (`parallel_collapse_dispatch`), `eval_loop.rs:3358-3493`: closure has
  `EvalGuard::enter()` (`:3372`) and consumes `env` at `:3432` — but **NO `register_live_env` block**
  (mechanically verified: a grep for `register_live_env` over the collapse closure body returns nothing;
  over the branch closure body returns the block above).

### Why none of the four sources cover the collapse worker's env

The collapse worker's `env` starts as `(*result_env).clone()` (same `shared` Arc as the parent). **The
instant the item's `eval_trampoline_with_carrying` (`:3432`) binds a variable, the env CoW-forks into a
NEW `shared` Arc** (`core.rs:916`; the branch-worker comment at `:2588-2592` states this exact fact).
That forked `shared` is the collapse worker's E₀, and:

- **Not (3):** it was never `register_live_env`'d (the missing block).
- **Not the parent's `eval/mod.rs` registration:** that covers the *parent's* `shared` Arc — a
  *different* Arc after the fork.
- **Not (4) `LIVE_DISPATCHES`:** `ParallelCollapseRootProvider::collect_dispatch_roots` walks `items`
  (original unbound items) + `results` (returned values) — **never the worker's binding table**.
- **Not the finisher:** a collapse worker that returns via the finisher (`ThreadContribution::TierLeaf`,
  the common `Demand::All` case) publishes its OUTPUT but **`TierLeaf` has no `env0` term** (`roots.rs` —
  it walks `extra ∪ collect_persistent_roots_no_env0`, explicitly minus the env struct). So the forked
  E₀ is only ever covered while the worker is *parked at branch-B* — not at its finisher.

Therefore a young `Addr` reachable **only** through a collapse worker's forked-E₀ binding (e.g. a
freshened `$__fr_*` alias target or a CoW-inserted intermediate referenced by binding rather than by
value) is in **NONE** of the four sources at sweep time → unmarked → swept → its arena slot bump-reused
→ the parent's collapse/filter merge reads a stale value. This precisely reproduces the observed
signature (minor / young / non-deterministic wrong subset; `(detection (SELF) someCoords2)` is a
partially-overwritten reused slot).

This is the genuine-CESK reading: the forked env **is** the collapse worker's machine component E₀, so
it must be rooted like every other machine — and because it is an `Arc`-shared `Send+Sync` value the GC
thread cannot read off another thread's stack, it is registered as a global anchor (B2′), exactly as the
branch worker already does.

## Corroboration

- **Oracle (`assert_rendezvous_union_complete`, `gc_driver.rs:284`, `#[cfg(debug_assertions)]`)** checks
  only (a) dispatch-fan-out coverage and (b) per-slot witness coverage — **neither models a worker's
  forked env**. Prediction: it passes silently while corruption persists ⟹ the gap is oracle-invisible.
  The debug-assertions binary run produced **no oracle panic** (consistent; the run was too slow/heavy
  under the 12G cap to reach the final verdict cleanly, so corroborating-not-conclusive).

## FIX

### Primary (closes the under-mark) — APPLIED
Add the identical B2′ registration to the collapse worker closure, after `EvalGuard::enter()` (`:3372`)
and before `env` is consumed (`:3432`), mirroring `eval_loop.rs:2594-2603`. The RAII `_worker_live_env`
handle is held for the whole closure body ⟹ source (3) `collect_live_env_anchors` walks this worker's E₀
every cycle, park-timing-independently (running / parked / finished-via-finisher / admission-blocked).
Structural, not a point-patch; byte-identical when dormant (`#[cfg(index-gc)]` + `dedicated_gc_enabled()`
short-circuit, default OFF).

### UPDATE 2026-06-03 — the env fix was NECESSARY BUT INSUFFICIENT; the real remaining cause is cross-thread thread-local σ-caches

After the primary env fix, the root count rose 12142→12662 (the collapse-worker envs ARE now walked) **but robot still corrupts** (❌ ×5/5, same minor-cycle/young signature). So the forked-env gap was real but not the whole story. A second exhaustive + red-teamed Plan-agent pass found the **dominant** structural cause:

**The dedicated collector's post-sweep cache invalidation runs on the WRONG THREAD.** `index_heap.rs:2119-2129` (the post-sweep cleanup) calls `clear_aba_sensitive_caches()` + `clear_inner_shadow()` + `clear_eval_memo()` + `clear_match_result_cache()` — ALL **thread-local** clears (EVAL_MEMO `dispatch_hints.rs:424`, MATCH_RESULT_CACHE `:756`, VALUE_HASH_CACHE `metta_value.rs:108`, INNER_SHADOW `:885`, subgoal/thunk tables — all `thread_local!`). In the single-threaded collector this runs on the eval thread (correct). Under the **dedicated** collector it runs on the **separate GC thread**, clearing the GC thread's (empty) thread-locals — the **worker threads' eval-scoped caches are never invalidated**. The design *self-publishes* a PARKED worker's caches as roots (`collect_global_anchors` in `collect_complete_thread_contribution`, `roots.rs:301-333,480-523`), so an active/parked worker's values survive — but two holes remain:

- **H1 (idle pool worker):** a work-pool worker that FINISHED a task drops its `EvalGuard` (witness slot released) but its `thread_local!` caches PERSIST on the pooled thread across the next cycle. Not a participant ⇒ not published; clear runs on the GC thread ⇒ not cleared. Its cached σ-Addrs are unrooted → swept → reused; next task hits the cache → stale.
- **H2 (post-sweep ABA on a parked mutator):** even a parked worker that self-published needs its caches INVALIDATED after the sweep reuses slots (`index_heap.rs:2123-2127`: a memo entry "keyed on its (now-wrong) content hash"). That invalidation never reaches the worker thread.

**The collapse-specific smoking gun (CONFIRMED):** the collapse worker reads `is_memoized_normal_form(&item_expr)` (`dispatch_hints.rs:109`) to decide *skip-eval vs eval* (`eval_loop.rs` collapse closure). Its bloom key is `value.hash_value()`, served by the **thread-local Addr-keyed `VALUE_HASH_CACHE`**. After a sweep reuses `item_expr`'s Addr, a stale VALUE_HASH_CACHE entry returns the prior occupant's hash → wrong bloom probe → **flipped normal-form classification** → the wrong member is kept/dropped in the `(collapse …)`/`PLNobjectsOfCategory` filter → the observed nondeterministic wrong subset. VALUE_HASH_CACHE *would* self-heal via `ensure_value_hash_cache_epoch_current()` (`metta_value.rs:130`) — but only if `gc_sweep_epoch` is bumped, and **the index collector never bumps it** (`index_heap.rs:2115`), so under index-gc the self-heal never fires.

**The complete structural fix (IMPLEMENTED) — genuine-CESK invariant:** *"a worker thread holds σ-derived values in thread-local caches ONLY while it holds an `EvalGuard`; at `EvalGuard` drop (task teardown) it clears ALL of them; and at park-resume after a sweep it clears the ABA-sensitive eval-scoped ones."* The thread-local σ-caches ARE part of the worker's abstract machine state (`roots.rs:308-316` calls them "machine-global σ-value holders"); a machine that surrendered its `EvalGuard` is no longer a σ participant, so retaining σ-Addrs is the unsoundness. The fix runs the SAME clear set the single-threaded collector runs, **on the thread that owns the caches**:
- New `clear_all_worker_thread_local_caches()` (`eval_loop.rs`) = `clear_aba_sensitive_caches` (incl. VALUE_HASH_CACHE) + `clear_inner_shadow` + `clear_eval_memo` + `clear_match_result_cache` + `clear_subgoal_table` (DIRTY-gated) + `clear_thunk_table` (DIRTY-gated).
- New `WorkerCacheTeardownGuard` RAII (Drop gated on `dedicated_gc_enabled()`), declared after `EvalGuard::enter()` in **both** worker closures (branch + collapse) → **closes H1** (idle workers leave empty caches; drops after the finisher published, publish copies Addrs by value ⇒ race-free).
- The 4 genuine post-park-resume clears (worker_cooperative_safepoint, both parent pumps, branch-B post-resume) upgraded `clear_aba_sensitive_caches()` → `clear_worker_caches_on_resume()` → **closes H2** (drops the value memos the aba-set excludes). Byte-identical when not dedicated.

**Red-team — the critical R1 (the 429e798 "Robot 40→19" regression risk) resolved on the rooting-vs-clearing axis:** that regression came from adding `CacheRootRefreshGuard` to the collapse worker, which is a *rooting* mechanism (`register_temporary_roots` → keeps values alive LONGER → freshened-binding chain grows past the `freshened_count < 1024` canary). This fix does the OPPOSITE: it **clears** (reduces retention, registers no roots), so it cannot re-trigger that regression. And no σ-cache here is correctness-load-bearing cross-task — all are invalidation-keyed memos where a *miss* is always safe (recompute); the only correctness coupling is the *wrong* direction (a stale hit is the bug). Full ledger (R1-R11) in the Plan-agent transcript; design is convergent.

### Companion (secondary cache-ABA — SUBSUMED by the cross-thread cache-hygiene fix above)
The `remaining > 0` gate on **both** pump park blocks (`pump_parallel_wait` / `pump_parallel_collapse_wait`)
means that on the **final** pump iteration (`remaining==0`, no park) the parent **skips the
`clear_aba_sensitive_caches()` that lives only inside the park block**. The merge then reads the
**non-epoch-protected** MORK-byte / ground-fragment / operator-dispatch caches (`clear_*` at the helper),
which — unlike VALUE_HASH_CACHE and the hash-cons table (both `gc_sweep_epoch`-auto-clear) — can be stale
after a concurrent sweep+reuse. Two candidate fixes: (i) run the cache clear on every pump return under
`dedicated_gc_enabled()` regardless of the `remaining>0` gate; or, more principled, (ii) give those three
caches the same `gc_sweep_epoch()` auto-clear the other two already have (eliminates the
"remember-to-clear-at-every-resume-site" fragility entirely). **Decision deferred to the empirical result
of the primary fix** (scientific isolation: test one variable first).

## GATE RESULTS (2026-06-03, post-commit `803bd19`)

**Headline GREEN (the realistic/default configs):**
- robot @ FANOUT=8 DEDICATED=1 **MIN=131072** (major+minor): discriminator **arm A 0/16** ×2 independent runs + ASAN **0-UAF with 96 rendezvous cycles** + correct.
- discriminator **arm B 0/16** (DEDICATED=0 control).
- raven @ FANOUT=8 DEDICATED=1 MIN=131072: ASAN **0-UAF, 19 cycles**, correct.
- conformance **DEDICATED=0 FANOUT=0 = 483/0** (byte-identical dormant) and **DEDICATED=1 FANOUT=8 MIN=131072 = 483/0** (no regression).

**Two residual issues surfaced by the gate (NEITHER blocks the headline; the default flip is user-gated and the default config is clean):**

1. **arm C (DEDICATED=1, MIN=4294967295 = 4 GiB major floor → MINOR-only regime): rare ~1/16 (6%) intermittent FAILURE** — one HANG (rc=124) + one wrong-subset corruption across ~32 runs. This is an ARTIFICIAL config (disables major collection; no realistic deployment sets a 4 GiB floor). The headline major+minor config (arm A) is clean. Root-cause UNCONFIRMED: an Explore pass hypothesized "minors don't bump `GC_CYCLE_GEN` → witness hangs" but that is **REFUTED** (`end_rendezvous_cycle` bumps the gen unconditionally per cycle, gc_allocator.rs:3955). The real mechanism is a rare race in the high-frequency minor-only rendezvous — pending a runtime deadlock-dump. Candidate safe fix (if confirmed): under `dedicated_gc_enabled() && FANOUT>0`, promote a due minor to a full major in the rendezvous (the validated path), or defer minors to the next major — both gated, correctness-by-construction, perf-only cost. Because a fix touches the delicate 4-red-team-round witness protocol (high regression blast radius) for an edge config, it is being root-caused + red-team-designed before any change.

2. **stress_multidir.metta @ FANOUT=8: PRE-EXISTING crash (NOT this work).** DEDICATED=0 (the validated baseline path) **SIGSEGVs (rc=139)**; DEDICATED=1 the dedicated collector's assertion catches it first as a panic (`index_heap.rs:694 str_slice "live Atom/String slot"`, rc=101, 0 UAF). Both fail ⇒ the fixture has a pre-existing rooting gap in the FANOUT=8 directive-parallel index path, independent of the dedicated collector (memory: "baseline-confirmed, NOT mine"). Separate issue.

## RESIDUAL #1 (the deadlock) — ROOT-CAUSED + FIXED (FIX A), red-teamed to convergence

The ~6% residual is (mostly) a **rendezvous DEADLOCK** in the straddle-rejoin of
`reacquire_eval_guard_after_safepoint_full` (gc_allocator.rs:6034-6087). Confirmed by a
debug-assertions diag dump (all 68 threads `futex_do_wait`, the dedicated GC thread waiting) +
code inspection + two Plan-agent red-teams.

**Mechanism:** on resume with `!gc_in_progress()`, the worker `break`-ed, restamped `acquired`
ONLY (never `published`), then BLOCKED in a non-publishing `GC_IN_PROGRESS`-wait admission loop.
If a new cycle K+1 began in that window, the driver's live witness re-walk saw the still-occupied
slot (`published=K < cur_gen=K+1`) and waited forever, while the worker — blocked in the
non-publishing wait, not running to its next safepoint where it would re-park+publish — waited for
`GC_IN_PROGRESS` to clear, which the stuck driver never cleared. Circular wait.

**⚠️ FIX A FAILED VALIDATION (2026-06-03) — UNCOMMITTED, diagnosis falsified.** Robot @ FANOUT=8
DEDICATED=1 MIN=131072 with FIX A: **3 hangs / ~55 runs ≈ 5.5%** — statistically UNCHANGED from the
pre-fix ~6%. So removing the rejoin non-publishing admission loop did NOT fix the deadlock ⇒ the
"rejoin-admission-wait is the deadlock" root-cause (two Plan-agent red-team rounds, "guaranteed-by-
construction") is **empirically WRONG or incomplete**. (Corruption: 0/55 — possibly incidentally
reduced, inconclusive.) This is the SECOND falsified "guaranteed" analysis of this witness protocol
(cf. the prior coordination fix in `e1-flip-VALIDATION-FAILED-2026-06-02.md`). CONCLUSION: this race
defeats source/hand analysis; the principled next tool is **FORMAL MODELING (task #17 E5: TLA+ of the
witness/rendezvous + loom of the park/resume/straddle handshake)** to find the interleaving analysis
keeps missing — plus a SYMBOLIZED FIX-A hang dump (rebuild sym+FIXA, repro-to-hang) to see whether
FIX A's hang is still the rejoin path or a distinct mechanism. FIX A is left UNCOMMITTED in the
working tree (NOT reverted, per the standing no-revert-without-approval directive); recommend the
user decide: revert to the known `803bd19` (~6% deadlock) baseline, or keep FIX A as a step and
pursue E5. The headline commit `803bd19` is INTACT and is the user's actual goal (the ~100%
corruption → fixed). The original (superseded) FIX-A rationale follows for the record:

**FIX A (FAILED — see above):** delete the redundant non-publishing admission loop;
on `!gc_in_progress()` restamp `acquired` + restore counters + return (no wait). The witness
invariant ALREADY supplies the admission safety: a cycle cannot sweep until every occupied slot
is published (strict-`>` predicate + live re-walk + `current_witness_ok` gate), so a rejoined-and-
running worker (`published=K < cur_gen=K+1`, `acquired=K+1 ≯ K+1`) is WAITED-FOR by any concurrent
cycle (it cannot sweep) and discharges that wait at its next safepoint (`is_gc_requested()` still
set) or its outermost `EvalGuard::drop`. The resume is gen-gated POST-sweep, so zero overlap with
the just-parked cycle either. Guaranteed-by-construction; gated `#[cfg(index-gc)] &&
dedicated_gc_enabled()` ⇒ byte-identical dormant.

**Convergence (net-subtractive):** a fresh red-team REJECTED the alternative "wrap the rejoin +
driver prologue in `RENDEZVOUS_MUTEX`" fix — it **self-deadlocks 100%** (the re-park calls
`worker_park_and_root_in_cycle`, whose first line locks the same non-reentrant `RENDEZVOUS_MUTEX`)
and adds a new lock-order edge to close a non-existent hole. FIX A is correct AND minimal.

## RESIDUAL #2 (the corruption) — DISTINCT, still open

The red-team determined the ~3% wrong-subset CORRUPTION is **distinct** from the deadlock (the
strict-`>` predicate prevents the deadlock race from sweeping-too-early). It is a separate residual
root-completeness hole. Highest-value suspects (per the red-team): (i) the FANOUT-pump early-return
path (parent returns from the pump before parking when `done` is already set, leaving an in-flight
fold transient momentarily unrooted before its next safepoint); (ii) a pooled worker reused for a
second task whose thread-local σ-caches were cleared only on the prior task's `EvalGuard` drop. To
be root-caused + fixed AFTER FIX A is validated (Explore→Plan→converge, same pattern).

## RESIDUAL #2 (corruption) — bloom hypothesis BOUNDED to PARTIAL-at-best (2026-06-03)

The Explore agent's leading hypothesis (the residual ~3.75% corruption flows through the global
`NORMAL_FORM_BLOOM` skip-eval decision keyed via the thread-local `VALUE_HASH_CACHE`, whose
`gc_sweep_epoch` self-heal never fires under index-gc) was tested with a DISCRIMINATING EXPERIMENT: a
dormant env kill-switch `METTATRON_DISABLE_NORMAL_FORM_BLOOM=1` (added to `dispatch_hints.rs`
`is_memoized_normal_form`; byte-identical when unset) that forces every collapse item to be evaluated.

Result (robot @ FANOUT=8 DEDICATED=1 MIN=131072): **bloom-DISABLED = 1 hang + 1 corrupt / 60** vs
**bloom-ENABLED control = 1 hang + 3 corrupt / 80**. So disabling the bloom *may* have roughly halved
the corruption (3.75%→1.67%) — but the samples are too small to be conclusive, and it **did NOT
eliminate** it. ⟹ the corruption is **MULTI-MECHANISM**: the bloom is at most ONE partial contributor
(its epoch-self-heal-under-index-gc fix — bump an index sweep-epoch so VALUE_HASH_CACHE + the bloom
self-heal — would address only that partial share), and a DISTINCT missed-root mechanism remains (the
Explore agent's secondary candidate: a grounded op CoW-forking `shared` mid-VM-tier-run, covered by
neither the closure-top B2′ registration nor the env0-less `TierLeaf` contribution — the same CLASS as
the 803bd19 collapse-worker-env gap, one level deeper).

## OVERALL STATUS (2026-06-03) — 2 fixes committed; 2 rare multi-mechanism residuals remain

| Item | State |
|---|---|
| Headline ~100% DEDICATED=1 corruption | **FIXED + committed `803bd19`** (collapse-worker env + cross-thread cache hygiene) |
| Lost-notify deadlock (witness_release_slot) | **FIXED + committed `e18bc16`** (one confirmed mechanism; reduced hang ~6%→~1.5%) |
| Rejoin-deadlock "FIX A" | **FALSIFIED + reverted** (validation 3 hang/55, unchanged) |
| Bloom-corruption hypothesis | **PARTIAL-at-best** (kill-switch experiment: 1 corrupt/60 vs 3/80; not eliminated) |
| Residual hang (~1.25-1.67%) | OPEN — rare, multi-mechanism (lost-notify fixed; a subtler residual remains) |
| Residual corruption (~1.67-3.75%) | OPEN — rare, multi-mechanism (bloom partial; a distinct missed-root remains) |

**The dedicated collector is opt-in (default OFF), so both residuals block ONLY the default-flip, not
the production default.** Two leading "guaranteed-by-construction" hypotheses (rejoin-deadlock,
bloom-corruption) have been empirically refuted/bounded — this race class defeats autonomous source +
single-experiment analysis. **Recommended next tools (deliberate efforts, deserve fresh context / user
direction): task #17 E5** — TLA+ of the witness/rendezvous + park/resume/straddle/release, and loom of
the same — for the rare deadlock; and a **systematic root-coverage audit** (enumerate EVERY live-value
holder under FANOUT>0 vs the 4 rendezvous root sources, incl. mid-VM-run CoW-forked sub-envs) + possibly
TSAN, for the missed-root corruption. The bloom kill-switch is kept as a permanent dormant diagnostic.

## Validation plan (after the primary-fix release build)
1. Quick: robot @ FANOUT=8 DEDICATED=1 MIN=131072 ×5 → ✅ + 404, no ❌/OOM/HANG.
2. Full discriminator `scripts/e1_flip_discriminator.sh` ×16 arms A/B/C (A reclaim>0 non-vacuity).
3. FANOUT=0 conformance 483/0 both backends × DEDICATED={0,1} (byte-identical dormant).
4. V4 ASAN 0-UAF + cycles>0; slab nextest; ×20 determinism; per-slot oracle (debug-assertions); `cargo test --lib`.
5. Default flip (`gc_allocator.rs` `dedicated_gc_enabled` `== Ok("1")` → `!= Ok("0")`) RESERVED for explicit user go-ahead.

## RESIDUAL #2 (corruption) — ✅ ROOT-CAUSED (SOURCE-CONCLUSIVE) + FIXED (2026-06-03)

**This SUPERSEDES the "PARTIAL-at-best / multi-mechanism" framing above** (which was an artifact of
small samples + an incomplete first-pass). After the cross-thread cache-hygiene fix (`88485f1`:
VALUE_HASH_CACHE epoch-self-heal via `bump_gc_sweep_epoch`, + EVAL_MEMO/MATCH_RESULT_CACHE gc-epoch
guards) the corruption was bounded to **~1% / 100**, and an `eval-caches-disabled` discriminator
(EVAL_MEMO + MATCH_RESULT + bloom all OFF) was **0 / 40**. Clearing the bloom *on the dedicated sweep*
was **refuted** (2 corrupt / 150) — proving the bad probe is BETWEEN sweeps. A focused root-cause agent
(read-only, source-conclusive) then isolated the exact mechanism:

**The global `NORMAL_FORM_BLOOM` is the ONLY skip-eval cache with no validity tag, and a FACT added to
the space during eval invalidates the value-memos but NOT the bloom.**
- `EVAL_MEMO` entries carry `(query_gen, mutation_epoch, scope_gen)` and are rejected on lookup if any
  differ (`dispatch_hints.rs:741`); `MATCH_RESULT_CACHE` carries `(query_gen, rule_epoch, mutation_epoch, …)`
  (`:861`). `NORMAL_FORM_BLOOM` (`:59`) carries **nothing** — `is_memoized_normal_form` trusts raw
  membership (`:160`). Its correctness depends ENTIRELY on being cleared on every state change that can
  flip a normal-form verdict.
- `add_rule` (`rule_management.rs:2925`) and ALL `remove_*` paths (`core.rs:2767/2881/3191`) call
  `invalidate_normal_form_memo()`. But `add_to_space_shared` — the **interior-mutability fact-add used
  during eval** (`core.rs:2930`, the Gap-A global-atomspace path) — bumps only `mutation_epoch`
  (`eval_loop.rs:16528`) and **never clears the bloom**. `mutation_epoch` is thread-local
  (`dispatch_hints.rs:455`); the bloom is a single GLOBAL filter — so a worker's fact-add invalidates
  only that worker's value-memos and never touches the bloom at all.
- Robot's collapse filter (`PLNcategorizeObject` → `PLN.Query`) is **space-dependent** (queries the KB +
  PLN-derived inheritances added during the run via `add_to_space_shared`). A worker memoizes value V as
  normal-form (correct against the space at that instant); another worker adds a fact making V reducible
  *without clearing the bloom*; a later probe of V returns a stale `true` ⟹ the collapse worker emits V
  verbatim instead of re-querying the now-larger space ⟹ the wrong `(detection …)` subset.

**Why dedicated-specific / benign on slab+D0:** the staleness window is bounded by clear frequency. On
slab / single-threaded-index the bloom is cleared eagerly *in-line on the eval thread* by the GC sweep
(`index_heap.rs:2145`) AND at every `!` query boundary (`clear_normal_form_memo_for_new_query`), keeping
the live entry set tiny ⟹ the gap stays benign (conformance 483/0). Under the dedicated collector the
sweep runs on a SEPARATE thread at witness-gated rendezvous points, so far more entries persist between
clears while workers keep adding facts ⟹ the staleness rises into the observed ~1%.

This explains EVERYTHING the experiments showed: disabling the probe → 0 (every value re-evaluated);
clearing on the sweep → still corrupt (the bad probe is between sweeps, after a fact-add); the
VALUE_HASH/memo epoch fixes didn't touch it (it is *logical* staleness, not a hash/Addr artifact —
`hash_value` is content-based + epoch-self-healed, so insert-hash == probe-hash).

### FIX (applied; #1 = the correctness fix for the dedicated case)
**Gate the bloom skip-eval OFF under `dedicated_gc_enabled()`** (`dispatch_hints.rs`
`is_memoized_normal_form` + `memoize_normal_form` return early; `#[cfg(feature = "index-gc")]` ⟹ slab
byte-identical; off by default ⟹ index-D0 byte-identical). Rationale: a normal-form skip is purely an
advisory hint, so re-evaluating every item is always semantically safe (only slower), and the dedicated
path's gap is GC/alloc-bound not skip-eval-bound (per MEMORY benchmark finding) ⟹ small cost. The
determination-vs-insert window for a *global* bloom under concurrent fact-adds cannot be closed
race-free cheaply (a per-entry epoch is impossible for a bloom; a bare clear races a concurrent insert),
so the concurrent collector simply does not use the optimization. The discriminator PROVES this yields
0. The moot sweep-time clear (tried + refuted) was reverted (`index_heap.rs` post-sweep block).

### Serial-path soundness (#2, follow-up) — the latent gap on slab/D0
The fact-add-does-not-clear-the-bloom gap is a PRE-EXISTING latent soundness hole on ALL paths (a
fact-add within a single `!` query that makes a memoized value reducible → a stale skip), merely masked
on slab/D0 by the frequent query-boundary + in-line-sweep clears (robot/slab + conformance all correct).
The principled close is an `add_to_space_shared` bloom invalidation (mirroring `add_rule`/`remove_*`) —
to be done race-safely (epoch-distrust to preserve the within-query optimization rather than
clear-on-every-fact-add) as a separate, separately-validated increment, NOT gated to dedicated.

| Item | State |
|---|---|
| Residual corruption (the bloom) | ❌ REFUTED — bloom off under dedicated still ~1% (`bj0r7i22h`: 2 corrupt / 200). Bloom is NOT robot's cause. |
| Serial-path latent bloom gap (fact-add) | follow-up increment (benign on slab/D0; epoch-distrust close) — a real but robot-irrelevant hole |
| Residual deadlock hang (~0.5-1%) | OPEN — task #17 E5 (TLA+/loom of witness/rendezvous + park/resume/straddle/release) |

## ⚠️ REFRAME 2026-06-03 (afternoon): the corruption is CACHE-INDEPENDENT — all three cache hypotheses REFUTED by experiment; the real cause is a MISSED-ROOT

**This corrects the "ROOT-CAUSED + FIXED" claims above.** Three successive cache hypotheses were each
plausible-from-source but **refuted by larger-sample experiment**:
1. **Normal-form bloom** (the `add_to_space_shared`-never-clears-it gap, source-conclusive): disabling
   it under dedicated → STILL 2 corrupt / 200 (`bj0r7i22h`). Robot's PLN is purely functional anyway
   (threads the KB as a value; no `add-atom`/`match`), so the bloom's logical-staleness never arises.
2. **Logical memo staleness** (thread-local `mutation_epoch` not bumped cross-thread): refuted — no space
   mutation in robot's collapse.
3. **Memo-held swept σ-`Addr`s** (EVAL_MEMO/MATCH_RESULT hold `Addr`s unrooted on non-parking workers):
   disabling EVAL_MEMO + MATCH_RESULT + bloom ALL TOGETHER under dedicated → STILL **3 corrupt / 172
   clean-load runs** (`bt1un1zo3`, ~1.7%). Two of the corrupts (run145, run172) were under clean load
   (no concurrent benchmark), so it is not a contention artifact.

**The decisive methodological error:** the "all-caches-off → 0/40" and "→ 0/60" discriminators that drove
hypotheses 1–3 were **lucky 0-samples at a ~1% true rate** (0/100 at p=0.01 ≈ 37% by chance). At ~1%,
0/N for N≈40–100 does NOT establish 0 — it is statistically indistinguishable from the unfixed rate.
Guess-and-check discriminators are UNRELIABLE here and must be replaced by (a) ≥200-run arms AND/OR
(b) a deterministic root-completeness ORACLE (forced-sweep-every-alloc, or a per-slot generation tag that
panics on a live-handle reused-slot read, or a shadow complete-root scan asserting 4-source ⊇ shadow).

**Conclusion:** the ~1% wrong-subset is a **cache-independent MISSED-ROOT** — a live index-arena `Addr`
held by a FANOUT=8 collapse worker that is in NONE of the dedicated driver's 4 root sources
(`drain_worker_root_buffer` [published parks only], `collect_safepoint_roots`, `collect_live_env_anchors`,
`collect_live_dispatch_anchors`), so a concurrent sweep reclaims + **bump-reuses its arena slot** and the
holder reads the wrong (new occupant's) content. ASAN-INVISIBLE (the slot is valid memory, repurposed —
not a heap free), which is why it survived the ASAN gates. Leading candidates (a fresh systematic
root-coverage audit is in flight): the FANOUT-pump PARENT-side in-flight branch transients in the
publish-buffer↔`results[slot]` window (eval_loop.rs:2667/2674); a branch worker returning early without
parking; a mid-VM CoW-forked `TierLeaf` sub-env. The fix must ROOT the holder (genuine-CESK
`σ\|_Reachable`), NOT toggle another cache.

**Status of the uncommitted cache-disable edits** (dispatch_hints.rs bloom-off + `eval_caches_disabled`
dedicated gate + put-guards): they do NOT fix robot's corruption, so they are NOT committed as such. They
close real-but-robot-irrelevant latent holes (bloom staleness for space-MUTATING fixtures; memo-held
`Addr`s) — keep-vs-revert deferred until the audit identifies the real missed-root and whether a clean
base is wanted to apply the real fix. The dedicated collector remains opt-in (default OFF), so this blocks
ONLY the default-flip, not the production default.

## ✅ ROOT CAUSE CONFIRMED (source-conclusive, 3rd audit) — the missed-root is a FORKED ENV's bindings

The cache-independent missed-root is: **a `fork_for_nondeterminism()`-forked environment created INSIDE a
collapse worker's own trampoline reduction**, whose `bindings`/`types`/`states` maps hold live young
σ-`Addr`s, reachable by NONE of the dedicated driver's 4 root sources. Two compounding structural gaps:
1. **The structural root walk DROPS the `env` field.** `WorkItem::collect_values`
   (`src/backend/eval/trampoline/types.rs:1837-1849`) and every `Continuation::collect_values` arm
   destructure with `..`, discarding `env: SharedEnv`. The design (roots.rs:209/248) assumed
   "E_local rides inside C/K as `carrying_bindings`" / "env roots managed via RootProvider" — FALSE for a
   forked env's own `bindings`. So even a PARKED worker's self-root (`collect_machine_roots` →
   `collect_values`) never publishes the forked env's Addrs; and a short collapse item (<4096 iters)
   never even reaches the `0xFFF` publishing park.
2. **The index-regime `try_register_env_roots` is an EMPTY stub** (`gc_allocator.rs:6286`, vs the slab
   ROOT_REGISTRY version `:6245`). So `fork_for_nondeterminism` (`core.rs:894`, deep-copies bindings at
   `:916`) registers the forked env NOWHERE under index-gc.
A concurrent STW-at-store sweep (the 4-source mark set, `gc_driver.rs:205-228`; `index_heap.rs:2073`)
reclaims + bump-reuses (`index_arena.rs:555 write_reused`, same `Addr`) those slots; the worker resumes,
resolves `$prem`/`$conc` from the forked env's bindings (`index_arena.rs:605 get`), reads the new occupant
→ wrong `(detection …)` subset. **This is the DEEPER variant of the 803bd19 fix** (which registered only
the collapse worker's INITIAL env at eval_loop.rs:3493 — not forks created during the reduction). It
manifests on robot because PLN's multi-premise `match`-against-KB derivations fork an env per matched
template during the collapse item's reduction. Cache-independent + ASAN-invisible — explains every prior
refutation.

**THE FIX (genuine-CESK — root the E-component structurally, NOT a cache toggle):** make
`WorkItem`/`Continuation::collect_values` ALSO fold each frame's `env` roots, walking only the
fork-distinct (CoW-diverged) `bindings`/`types`/`states` (not the Arc-shared E₀) — so E becomes part of
`σ|_Reachable(⟨C,E,K⟩)`, exactly the CESK formula. The dropped `env` IS the bug. (Alt: register forked
envs in LIVE_ENVS via the index `try_register_env_roots` — more invasive.) Design red-team (Plan agent)
+ a reliable validator (low `MIN_BYTES` amplifies the ~1% to a high rate for one-run confirmation) in
flight. This will be the genuine corruption fix; the cache-disable edits will then be reverted (they
target robot-irrelevant secondary/tertiary holes).

## ⭐ NO-RECYCLE SWEPT-BITMAP ORACLE (2026-06-03, on the post-Fix#1 `58598e7` + `cb94de8` + `762c4db` tree) — the RESIDUAL corruption is REUSE-DEPENDENT and BYPASSES `get()`

A deterministic swept-bitmap oracle (env `METTATRON_INDEX_GC_SWEPT_ORACLE=1`; committed in the arena and
completed for collector scopes / `INNER_SHADOW` hits in `c14d5ff`): per-slot `swept: Box<[AtomicBool]>` allocated only when
on, marked Release on **every** sweep-reclaim arm (both the all-dead-word fast path AND the per-bit arm),
**NO-RECYCLE** (`pop_young_free_slot` returns `None` + the sweep SKIPS the free-list push ⇒ no slot is
ever reused ⇒ a swept slot's bytes are never overwritten), and `IndexArena::get()` panics on a NON-collector
read of a swept slot (collector reads exempt via a thread-local `COLLECTOR_READ_DEPTH` scope held by
`gc_driver_main` for the GC thread's whole lifetime; `gc_driver.rs` + `index_arena.rs`).

**Result** (robot @ FANOUT=8 DEDICATED=1 MIN=131072, debug-symbols release build): **38/38 completed runs
byte-identical CORRECT** (run 38 ends with the expected `✅` detection result; run 39 was killed mid-stream
when the subagent's systemd scope tore down — no panic, no error), and the **SWEPT-SLOT READ panic NEVER
fired.**

**Two ROBUST conclusions** (robust because the marking is verified complete — the no-recycle is *total*:
the free-list is neither populated nor popped, so `corruption→0` *proves* the corrupting reuse path was
gated, hence its slot WAS marked swept):

1. **The residual corruption is REUSE-DEPENDENT.** No-recycle eliminated it entirely (38/38 correct vs
   ~4.7% corrupt with recycling). The mechanism IS a swept slot bump-reused while a handle still points at
   it — confirming the swept-Addr-reuse family at the mechanism level.
2. **The stale read BYPASSES `arena.get()`.** The corrupting slot is marked swept and never reused, so a
   `get()` of it would panic (the swept bit stays set forever under no-recycle, and the reader is a
   worker/parent — NOT the exempt collector thread). The panic never fired ⟹ the holder reads that slot
   through a path that never calls `get()`. **Prime suspect: a laundered `&'static` `MettaValue`/`Node`
   view (`view()`/`inner_ref()`) or a cached raw pointer held across a collection** — the get()-based
   oracle is blind to it BY CONSTRUCTION.

**Why this is consistent with Fix#1 (`58598e7`) reducing-but-not-eliminating the corruption:** Fix#1
rooted forked-env bindings that the worker resolves via `index_arena.rs:605 get()` — a get()-PATH holder.
The residual is a DISTINCT get()-BYPASSING holder, untouched by Fix#1. (Methodological win over the
prior ~6 refuted hypotheses: a *deterministic* oracle gave a robust mechanism + read-path narrowing, vs
the lucky-0 guess-and-check discriminators that the 2026-06-03-afternoon REFRAME flagged as unreliable.)

**NEXT** (Explore agent `a8f67d71` in flight): enumerate every get()-bypassing read path (laundered
`&'static` views, cached raw pointers, direct `node_at`/`inner_ptr` reads held across a sweep+reuse) +
which of the dedicated driver's 4 root sources fails to cover the holder. Then red-team a CESK-faithful
fix (root the holder structurally OR extend the value-view release handshake) with a Plan agent; CONFIRM
empirically by extending the oracle's swept-check to the identified bypass read site (targeted, cheap,
no-build-cost) and/or TSan (path-agnostic: allocator reuse-write vs holder-read race). The swept-bitmap
oracle is a KEEPER diagnostic — env-gated, default-off, byte-identical when off.

### Source-tracing the get()-bypass (2026-06-03) — the SOLE bypass is the `INNER_SHADOW` cache HIT (ABA), NOT a laundered slice

Traced every node-content read under index-gc:
- `IndexHeap::children` (665), `str_slice` (685), `view_at` (730), `materialize_inner` (776) ALL read the node
  via `self.arena.get(addr)` — the swept-checked `IndexArena::get()` (724). So a `view()`/`inner_ref()`
  fresh-materialize of a SWEPT Addr would PANIC under the oracle. It never did ⟹ no holder fresh-reads a
  swept Addr through get().
- The laundered `&'static [MettaValue]` / `&'static str` (`launder(self.children(a))` /
  `launder(self.str_slice(a))`, index_heap.rs:752-760, 778-808) point into the **side-arena**
  (`side.children` / `side.strings`), which per D-TLAB-1.0 is **append-only, NEVER recycled**. So a held
  laundered slice/str reads its original (correct) bytes even after the owning NODE slot is reused ⟹ the
  laundered slices are **SAFE** — NOT the bug. (This also means TSan would likely find NO data race here:
  the only mutated-then-read memory is the node slot, and every node read goes through get().)
- ⟹ the SOLE get()-bypassing read of node *content* is the **`INNER_SHADOW` cache HIT** in
  `MettaValue::inner_ref_index` (`metta_value.rs:1006-1025`): on a hit it returns the cached
  `Box<MettaValueInner>` WITHOUT calling `materialize_inner`/`get()`, keyed by the raw `Addr` (u32). A
  swept+reused Addr can HIT a STALE box (the prior occupant's `MettaValueInner`). **This is an ABA
  hazard, and it is THREAD-LOCAL LOGICAL STALENESS — not a data race** (the box is thread-local, the read
  is single-threaded), which is precisely **why TSan would be BLIND to it** — confirming TSan is the wrong
  tool here and the get()-oracle's "no panic" is genuine (the read truly bypasses get(), via the cache).

This fits ALL evidence: get()-bypassing (cache hit, no swept-check), reuse-dependent (no-recycle ⟹ Addr
never reused ⟹ never a stale-key hit ⟹ 38/38 correct), and consistent with the post-`58598e7`/`88485f1`
residual (those rooted get()-path holders + cleared caches on worker teardown/resume; a remaining
INNER_SHADOW-hit window — likely on the PARENT pump path, or a clear-timing gap — would still ABA).

**CONFIRMATION EXPERIMENT IN FLIGHT (agent `ab95e87b`):** extend the oracle to panic on an `INNER_SHADOW`
HIT of a swept Addr (`was_hit && swept_oracle_enabled() && !in_collector_read_scope() &&
heap.is_addr_swept(addr)` → panic), rebuild (debug-symbols, no -Zbuild-std → fast), re-run robot under
no-recycle. If it TRIPS → the backtrace names the ABA holder (CONFIRMED; fix = clear/validate
INNER_SHADOW on that thread/path after every dedicated sweep, OR make the cache key ABA-safe — e.g.
distrust on a per-Addr sweep-generation). If it does NOT trip in 40 runs → REFUTES INNER_SHADOW-ABA and a
different get()-bypass remains (next: a per-read sweep-generation tag, since TSan is ruled out for a
thread-local staleness).

### VERDICT (2026-06-03) — INNER_SHADOW-ABA **REFUTED** (61/61 clean); the swept-oracle CANNOT pinpoint by construction; pivot to TSan

The INNER_SHADOW-hit-of-swept oracle ran **61 robot runs total (11 + 50), ALL clean, NO trip** (one 42577-
vs-42574-byte run = benign 3-byte ordering variance, still carries the correct result — not the
wrong-subset signature). At ~4.7%, P(no trip in 61) is about 0.05, so **the holder does NOT do an
`inner_ref` cache-hit on a swept Addr** — INNER_SHADOW-ABA is refuted.

**The deeper methodological finding:** the swept-oracle family (NO-RECYCLE) can only *confirm reuse-
dependence* — it **cannot pinpoint the holder BY CONSTRUCTION**, because no-recycle disables the very
thing under study (slot reuse). It also REFUTES the two simplest hypotheses: (a) a genuinely-missed-root
slot would stay swept under no-recycle and the holder's `get()` would have PANICKED — it never did across
all runs; (b) the INNER_SHADOW ABA — 61 clean. Net: **under no-recycle no thread ever reads (via `get()`
or an `inner_ref` hit) a currently-swept slot.** So the corruption is NOT "read a swept slot" — it is a
**cross-thread reuse RACE on the Node slot bytes that only manifests WITH reuse on**: one thread's
`write_reused`/alloc-write (or the sweep's free-list manipulation) races another thread's `get()`/deref-
read of the same slot, with missing happens-before. Two surviving mechanisms, both cross-thread races:
- **(R-FL) Free-list management bug** — `write_reused` hands out a slot that is still live (a stale/
  duplicate free-list entry, or a minor/major free-list interaction), overwriting a value a *rooted*
  holder still reads. (Consistent: no-recycle disables reuse so no overwrite; the slot is never swept so
  no swept-panic.)
- **(R-TOCTOU) Timing-dependent missed-root** — a transient is momentarily unrooted exactly when the
  dedicated sweep fires (e.g. the parallel-dispatch result-publication window, `result_roots`->buffer
  before `results[slot]`), swept, reused, then read; no-recycle's altered heap-growth/GC-timing shifts
  the window so it doesn't coincide.

**CORRECTION to the earlier "TSan is ruled out" note:** that applied ONLY to the (now-refuted) thread-
local INNER_SHADOW ABA. Both surviving mechanisms are **cross-thread races on the slot bytes**, which is
exactly what TSan detects. **TSan binary build + robot-in-NORMAL-recycle-mode IN FLIGHT (agent
`a9872e28`):** `-Zsanitizer=thread -Zbuild-std` (RUSTFLAGS keeps `-Ctarget-cpu=native` for gxhash),
`DEDICATED=1 FANOUT=8 MIN=131072`, x3 runs, capped 32G build / 48G run. Deliverable = both racing stacks
(the WRITE — `write_reused`/alloc/sweep — and the READ — `get`/`node_at`/`children`/deref in the
collapse-merge path) + the two threads. If TSan finds NO race across 3 runs, the race is happens-before-
masked by the witness/rendezvous protocol -> redirect to a sweep-time **root-coverage assertion**
(4-source roots superset of an over-approx reachable set scoped to in-flight result transients + thread
caches — finds R-TOCTOU at sweep time) and/or a **free-list integrity check** (`write_reused` asserts the
popped slot is not marked-live + no duplicate free-list entries — finds R-FL).

### TSan VERDICT (2026-06-03) — ZERO data races; triangulation ⟹ R-FL (free-list integrity bug), NOT a missed-root

TSan build succeeded (`-Zsanitizer=thread -Zbuild-std`, target-cpu=native, 4m39s, 49-warning baseline) and ran robot ×3 in NORMAL recycle mode (DEDICATED=1 FANOUT=8 MIN=131072), **77 threads live (64 work-pool + dedicated GC), all correct, and "ThreadSanitizer" appears NOWHERE — zero data races, zero warnings of any kind.** The concurrent corruption regime was genuinely exercised. ⟹ the reuse-write and the holder-read are **happens-before-ordered** by the witness/rendezvous protocol (collector mark/sweep under the heap `.write()`; workers self-root+park; `write_reused` is `&mut self` at quiescence), NOT concurrent.

**Triangulation (neither tool alone pinpoints; together they eliminate):**
| Evidence | Rules out |
|---|---|
| no-recycle: no swept-slot read in 99 runs (post-Fix#1) | **missed-root** (its holder read would hit the swept slot → panic; never did) |
| TSan: 0 data races in 3 concurrent runs | **memory-level race** |
| no-recycle: 0 corruption | confirms **reuse-dependent** |

The TSan agent proposed "logical missed-root," but that is **inconsistent with the no-recycle no-panic**: a missed-root slot is *swept* (the oracle marks it), stays swept under no-recycle, and the holder's `get()` would panic — it never did across 99 runs, and the only get()-bypasses (INNER_SHADOW, laundered slices) are refuted/safe. So the residual is **NOT a missed-root** (Fix#1 + any uncovered missed-root would both show as swept-reads).

**By elimination ⟹ R-FL: a free-list integrity bug.** `write_reused` hands out a slot whose `Addr` is **erroneously on the free-list while the slot is still LIVE** (rooted). The slot is therefore never swept (marked live ⟹ no swept-bit ⟹ no-recycle keeps it intact ⟹ no panic, no corruption); the clobbering reuse-write and the holder's read are lock-ordered (⟹ TSan-silent); disabling reuse removes the overwrite (⟹ reuse-dependent). The prime mechanism: a slot pushed to the free-list by a **major**, then **re-pushed by a later minor** (minors append + retain the major's entries; if the minor's sweep re-pushes a slot already on the retained free-list ⟹ a **DUPLICATE entry** ⟹ two `write_reused` pops hand out the same slot for two different values ⟹ the first is clobbered when the second is allocated). Other R-FL variants: a double-free, or a pop of a slot re-allocated since being freed.

**CONFIRMATION EXPERIMENT (next): a free-list integrity check** (reuse stays ON, unlike no-recycle): a per-slot `on_free_list` shadow bit — set on push (sweep reclaim), reset on pop (`write_reused`/`pop_young_free_slot`) and on `free_list.clear()` (major rebuild); **assert `!on_free_list` on every push** (catches the duplicate) and `on_free_list` on every pop. Build debug-symbols (no -Zbuild-std → fast), run robot NORMAL recycle mode → the assertion backtrace names the exact duplicate push/pop site + cycle type → fix (dedup the minor's push against the retained free-list, or the correct free-list lifecycle). This is deterministic and reuse-ON, so it pinpoints where the no-recycle oracle (reuse-OFF) structurally cannot.

### ✅ R-FL CONFIRMED (2026-06-03, deterministic, run 1) — duplicate free-list push of the current segment

The free-list integrity check **tripped on run 1**, deterministically:
```
DUPLICATE FREE-LIST PUSH: addr=Addr(561) seg=0 off=561 sweep=minor   (index_arena.rs partial-last-word push arm, on mettatron-index-gc)
```
Zero `POP OF NON-FREELIST` ⟹ the shadow stayed consistent ⟹ a genuine still-on-list re-push, not an instrumentation gap. `sweep=minor` (not major) ⟹ the clear-site reset works (a major's own clear+rebuild does not false-fire); only a MINOR re-pushing a slot a preceding sweep already listed fires.

**The bug (source-cited):** the current bump segment is **always young** (`promote_young` index_arena.rs:1089 sets `young_floor=current_seg()`, but the doc-comment at :1085 states "the current segment stays young (the live allocation target)"), so EVERY collection re-sweeps it, and `sweep_range`'s free-list push is **non-idempotent**:
1. MAJOR `do_major` (index_heap.rs:2092) → `sweep_with`→`sweep_range(0, clear=true)`: `free_list.clear()` (index_arena.rs:1207) then rebuilds, pushing every unmarked slot in `[0,seg_count)` INCLUDING the current segment (partially-live) → `Addr(561)` pushed.
2. `promote_young` (index_heap.rs:2114) sets `young_floor=current_seg`; current segment stays young.
3. MINOR `do_minor` (index_heap.rs:2098) → `sweep_young_with`→`sweep_range(young_floor, clear=false)` APPEND: re-sweeps the current segment, finds `Addr(561)` still unmarked + still on the list → pushes it AGAIN (index_arena.rs:1330) → DUPLICATE.
Two `pop_young_free_slot` (index_arena.rs:770) pops hand the SAME slot to two `MettaValue`s → the first is clobbered → the wrong-subset corruption. The `sweep_young_with` doc-comment's invariant "a young slot is swept at most once before promotion" is **FALSE for the current segment** (never promoted out, re-swept every cycle).

**This explains EVERYTHING** (every triangulation result): never swept (slot stays live ⟹ no swept-panic, no-recycle keeps it intact), lock-ordered clobber+read (⟹ TSan/ASAN silent), reuse-dependent (⟹ no-recycle eliminates it).

**Scope:** the bug is in `sweep_range` ⟹ **mode-INDEPENDENT** (both the single-threaded FANOUT=0 index collector AND the dedicated FANOUT>0 collector). Conformance passes today ONLY because its fixtures do 0 GC cycles (young<2MiB). The fix is a **core index-gc free-list-lifecycle fix, NOT dedicated-gated**; slab is a separate collector (byte-identical). **minor→minor duplicates are likely ALSO possible** (every consecutive pair of collections re-sweeps the current segment), so the fix must be robust to all duplicate patterns.

**FIX IMPLEMENTED + SCOPED VERIFIED:** `add0585` implements the converged fix: idempotent free-list push via a persistent per-slot `free_bit`, pop clear-before-branch, major reset-before-clear, and the direct allocator regression for repeated current-segment minors. `ae61034` closes the post-implementation audit edge where a D/TLAB fresh-bump interleaving can advance `cur_seg` while a prior young segment still has listed free slots; release now drains that segment's free-list entries before dropping its bitmaps. `70ab067` fixes the `index-gc` integration-test compile fallout from the extra `eval` return field.

Scoped verification completed: allocator tests with `METTATRON_INDEX_GC_FREELIST_CHECK=1` passed 23/23; `cargo check --features index-gc` passed; TLC fixed config `MC_RFL_freebit.cfg` passed `NoDuplicateFreeListEntries` and `FreeBitExact`; TLC bug config `MC_RFL_bug.cfg` still fails as expected with `freeList = <<0, 0>>`. **Not yet claimed:** the full forced-MeTTa/robot/conformance/ASAN/determinism gate; the current forced-churn fixture attempt timed out and needs a bounded replacement before V1/V2/V3 can be counted.
