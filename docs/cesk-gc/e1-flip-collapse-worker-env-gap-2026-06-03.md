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

## Validation plan (after the primary-fix release build)
1. Quick: robot @ FANOUT=8 DEDICATED=1 MIN=131072 ×5 → ✅ + 404, no ❌/OOM/HANG.
2. Full discriminator `scripts/e1_flip_discriminator.sh` ×16 arms A/B/C (A reclaim>0 non-vacuity).
3. FANOUT=0 conformance 483/0 both backends × DEDICATED={0,1} (byte-identical dormant).
4. V4 ASAN 0-UAF + cycles>0; slab nextest; ×20 determinism; per-slot oracle (debug-assertions); `cargo test --lib`.
5. Default flip (`gc_allocator.rs` `dedicated_gc_enabled` `== Ok("1")` → `!= Ok("0")`) RESERVED for explicit user go-ahead.
