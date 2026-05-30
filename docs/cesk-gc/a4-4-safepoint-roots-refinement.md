# A4.4 refinement — SAFEPOINT_ROOTS is a KEPT driver-transport channel

Addendum to `a4-4-collector-flip-design.md`. The A4.4 quiescence oracle (which the
design added) CAUGHT a real gap on the first green-wall — exactly its purpose. This
documents the gap + the fix.

## The gap the oracle caught (non-vacuous)
First A4.4 green-wall: RELEASE byte-identical (slab 4325 / index 4177 / conformance
483·221·40 with the flipped collector) + DEBUG nextest green, BUT the debug M11-he
conformance hit the quiescence oracle:
`|OLD|=12 |NEW|=14 |KEPT|=2 |missing|=1`. Enhanced the oracle to print the missing
value's `Debug` → the missing root was `Spanned(Quoted(Atom("foo")))` — fixture 017's
directive RESULT, held by the conformance runner's **cross-directive result
accumulator** (`all`), registered via `register_temporary_roots` → `SAFEPOINT_ROOTS`.

## Root cause
`collect_all_roots()` = the ROOT_REGISTRY providers **∪ `collect_safepoint_roots()`**
(gc_allocator.rs). The A4.4 flip replaced ALL of `collect_all_roots()` with the
structural reader (`collect_persistent_roots` ∪ result ∪ driver-C) — but that
**dropped `collect_safepoint_roots()`**. `SAFEPOINT_ROOTS` is the plan's
*keep-narrow* driver-transport channel (the driver's accumulated `!`-results +
the thread-local cache snapshot via `CACHE_ROOT_HANDLE`); it is NOT replaced by the
structural reader. Dropping it from the live feed would free the driver's accumulated
results → use-after-free (release "passed" only because no later fixture re-touched the
freed value; the oracle caught the latent UAF).

## Fix (7 edits)
- `gc_allocator.rs`: `collect_safepoint_roots` `fn` → `pub fn`;
  `models/mod.rs`: re-export it (alongside `collect_all_roots`).
- KEPT now = driver-C (`MettaState.{source,output}`) **∪ `collect_safepoint_roots()`**
  in BOTH machine-equivalence oracles:
  - `roots.rs::assert_quiescence_superset` (quiescence oracle).
  - `eval_loop.rs` midloop oracle (`driver_c_vals += collect_safepoint_roots`).
- The live feed now KEEPS `collect_safepoint_roots()` in ALL 3 flips:
  - `eval/mod.rs` (quiescence `eval()`), `tier_forced.rs` (quiescence `eval_with_tier`),
    `eval_loop.rs` (midloop).

So `SAFEPOINT_ROOTS` is treated uniformly as a kept apparatus channel (per the plan's
"SAFEPOINT_ROOTS = transport, KEEP narrow"); A5.4 narrows it to just the driver's C.

## Non-vacuity preserved
KEPT containing `collect_safepoint_roots()` (which, at quiescence, includes the cache
snapshot via `CACHE_ROOT_HANDLE`) could mask a cache dropped from NEW — BUT the A4.3
midloop CI test `a4_3_oracle_holds_across_safepoint` drives via `eval_trampoline`, which
does NOT call `refresh_thread_local_cache_roots`, so its `CACHE_ROOT_HANDLE`/
`SAFEPOINT_ROOTS` is empty → that test still requires the 9 caches to be covered by NEW
(`collect_global_anchors`). Cache-coverage-by-structural-reader thus remains tested.

## Verification
M11-he full re-run (debug, oracle live, `MIN_BYTES=131072` ⇒ fires on every collection):
**40/40 pass, 0 fail** — the oracle holds. Full re-green-wall (RELEASE byte-identical +
DEBUG full-corpus base/M11-pt/M11-he under both oracles) + capped ASAN follow.
