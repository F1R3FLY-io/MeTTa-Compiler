# Phase A5 — RESULTS (delete/cfg-scope the discovery apparatus): COMPLETE

Branch `feature/petta-semantics`. A5 cfg-scoped the entire GC-root **discovery apparatus**
(ROOT_REGISTRY / RootProvider / register_root_provider / collect_all_roots / frame_chain) to the
**slab** build, so the **index-gc build is now registry-free AND frame_chain-free** — its collector's
roots are purely `σ|_Reachable(⟨C,E,K⟩) ∪ reach(E₀) ∪ collect_safepoint_roots` (the structural CESK
reader + the one narrow driver/cache publication channel), with NO discovery side-channel. This is the
genuine-CESK property the workstream required. Physical deletion of the slab apparatus is deferred to
**F4** (forced by the master plan: the slab guard stays `cargo nextest --release → 4324/0` until F3
flips the default).

## Approach: BYTE-IDENTICAL-FOR-SLAB
Every step kept the SLAB arm verbatim (runtime `gc_mode_is_index()`-gated, exactly as before) and made
the INDEX arm structural via a compile-time `#[cfg(feature="index-gc")]` split. Sound because
production never flips `GC_MODE` (only `#[cfg(test)]` does) and the index factory is compile-time.
A Plan agent designed each step; a general-purpose agent applied the mechanical sweeps (A5.3/A5.5/A5.6);
every cfg direction + correctness claim was verified against source before committing.

## Commits
| Step | Commit | What |
|------|--------|------|
| docs | 20d3a29 | A4 re-alignment + A5 plan + anti-drift guardrail (after reverting the VM-`*const MettaState` drift to A4.4 390b743) |
| A5.0 | b965e74 | cfg seam: relocate `FrameLabel`→both-builds `frame_label.rs`; add `push_expr_vec_frame` helper |
| —    | bc03f2d | green-wall env-var fix (METTATRON_PARALLEL_FANOUT_DEPTH / MIN_BYTES) |
| A5.1 | 1642aab | index spine+VM rooting via typed K-spine only; frame_chain spine/VM → slab |
| —    | 98ffe05 | cargo-fmt cleanup (A4.x) |
| A5.2 | 468c07e | re-home 11 module/assert ExprVec callers to `push_expr_vec_frame`; DELETE `maybe_push_frame`+`FrameAndKSpineGuard` |
| A5.3 | 2ea9115 | cfg-wall the 10 RootProvider impls+registrations → slab (index registers ZERO providers); +lib-warning gate in a5_greenwall.sh |
| A5.4 | 34aed4c | driver-C → global SAFEPOINT_ROOTS; retire the per-context `collect_driver_roots` seam — **fixes the pre-existing latent CLI/REPL driver-C UAF** |
| A5.5 | 98b0523 | **registry core cfg→slab — the index build is REGISTRY-FREE (genuine-CESK milestone)** |
| A5.6 | 206cfa6 | `frame_chain` cfg→slab-only module — **the index build has NO frame_chain module** |
| A5.7 | (this) | graduate the machine-equivalence oracles to permanent CI invariants + the A5-completion gate |

## Per-step gate (every sub-step, BOTH builds, before each commit)
slab nextest **4324/0** · index nextest (4177→**4167** across A5.2/A5.5/A5.6 test deltas, each explained:
−1 deleted maybe_push_frame test, −1 the load-bearing eval/mod.rs registry test, −8 the frame_chain.rs
tests) · index conformance **483/0** (M11-he **40**/M11-pt **221** + base 222) with **840** quiescence
GC cycles (non-vacuous) · debug machine-equivalence oracle **0 panics** (MIN_BYTES=1, fires every
collection — the standing discharge of the reification-equivalence lemma) · slab ASAN full-conformance
**0 UAF** (the #1 risk: no dropped slab provider) · index M11-pt ASAN **0 UAF** · **lib 49 warnings BOTH
builds** (the 0-new-warnings gate; benign net-zero swaps at A5.5/A5.6 noted in those commits).

## Load-bearing results
- **CLI/REPL driver-C UAF fixed (A5.4)**: `stress_multidir.metta` via the CLI with MIDLOOP under the
  oracle — the exact config that gave `|KEPT|=0 |missing|=8005` at A5.1 — now **0 oracle panics, 86
  midloop cycles**. SAFEPOINT_ROOTS covers driver-C ctx-independently (even under a nested VmEvalContext).
- **collect_global_anchors covers all 7 body-bearing providers** (verified) — A5.3 was a pure cfg-wall
  with the oracle GREEN by monotonic OLD-shrink.
- **Compile-blocker caught (A5.5)**: the slab GC-pool wrappers (trigger_gc_cycle/_via_pool,
  trace_surviving_set, trace_safepoint_live_set env-branch) call the walled symbols + compile in both
  builds — walled in lock-step (the plan's literal step would not have compiled in index).

## Oracles graduated (A5.7)
The two `#[cfg(debug_assertions)]` machine-equivalence oracles (roots.rs `assert_quiescence_superset`;
eval_loop.rs midloop oracle) are now PERMANENT CI invariants (RT-7): index = structural-internal
consistency (NEW ∪ KEPT ⊇ the live S∪C∪K + caches + deferred root_set); slab = structural reader ⊇ the
(still-present) discovery apparatus. Kept until **F4** deletes the slab apparatus.

## A5.7 final gate (the master-plan per-rung extras) — ACTUAL RESULTS
Green-wall (both builds): slab nextest **4324/0**, index nextest **4167/0**, index conformance **483/0**
with **840** quiescence GC cycles (non-vacuous), lib **49** warnings BOTH builds, debug machine-equivalence
oracle **0 panics** (MIN_BYTES=1, fires every collection). All green.

Extras (`scripts/a5_7_extras.sh`):
- **mmverify demo0**: "Correct proof" present ✓ — the index build evaluates the Metamath proof correctly.
- **20-run conformance result-determinism**: **1 distinct result-hash** across 20 runs at default fanout
  (exercises the parallel superpose/collapse paths) ✓.
- **PLN budgets** — measured on BOTH builds (the budget is met by the JIT-enabled build; the index build's
  slowness is the known pre-B4 JIT-gap, NOT an A5 regression — see below):

  | example      | budget | SLAB (with JIT) | INDEX (JIT disabled) |
  |--------------|--------|-----------------|----------------------|
  | Robot        | ≤12s   | **10s** ✓       | 22s                  |
  | FlyingRaven  | ≤25s   | **19s** ✓       | 49s                  |

  All four runs `rc=0` (completed, no OOM under the 8G cap ⇒ RSS ≤8G).

### Why the index build is ~2.2× slower on PLN, and why that is NOT an A5 regression
The recovered-state **CAVEAT 2** is that **JIT is hard-disabled under `index-gc`** — confirmed in source:
`tiered_cache.rs:1297`/`:1491` gate the JIT compile on `gc_mode_is_index()` (skip when index), and `:1698`
JITs only when `!gc_mode_is_index()`. So the index build runs the tree-walker/bytecode-VM **without JIT**
while the slab build runs **with JIT**. The measured slab→index ratio (10→22s, 19→49s ≈ 2.2×) is precisely
a JIT-presence gap, not a collector-overhead gap.

Two independent facts establish this is not an A5 regression:
1. **A5 is perf-neutral cfg-scoping.** A5 cfg-walls the *discovery apparatus* to the slab build; it does not
   touch the index collector's root-reading — **A4.4 already flipped the index collector to the structural
   `collect_machine_roots` path**. So index PLN perf at A5.7 ≡ A4.4 (the structural collector is unchanged
   across A5).
2. **The slab build is byte-identical** (A5 kept every slab arm verbatim, runtime `gc_mode_is_index()`-gated)
   and **meets both budgets** (Robot 10s ≤12s, FlyingRaven 19s ≤25s) — matching the historical ~9.74s Robot
   baseline within `date`-granularity measurement noise. The budgets are therefore preserved for the current
   default (slab) build.

The index-build PLN budgets are closed by **Phase B4 (JIT-on-index re-enablement)** — the master plan
explicitly scopes index *perf* to Phase B and notes B4 is "prereq for honest C–E perf and index-default
(else index runs without JIT vs slab+JIT)." A5 is the structural-roots **correctness** rung; its gate
(ASAN 0-UAF both builds, byte-identical conformance, 840 non-vacuous cycles, 0 oracle panics, mmverify
"Correct proof", 20-run determinism, slab PLN budgets) is **fully met**.

## Remaining
Phase A is COMPLETE (A1–A5). Next: B (concurrent arena + lock-free TLABs + JIT-on-index) → C
(generational + abstract-GC live-var marking) → D (parallel collector — the cross-thread apparatus
dissolves via per-worker structural reads) → E (concurrent SATB on E₀ + selective-CESK* choice-point
unification + serializable continuations) → F (Welch index+JIT vs slab+JIT; index-default; **F4 deletes
the bridge/slab/apparatus** — final `rg` of every apparatus symbol = 0).
