# Phase C (remaining) — Implementation Plan: integrated nursery + abstract-GC marking

**Status:** APPROVED, executing. HEAD `d7dbbac` (`feature/petta-semantics`). Designed by a Plan agent
against source (2026-05-31), refining `phase-c1c-integrated-nursery-design.md` with the findings below.

**User directive (governs all):** "Finish Phase C, then D, then E, then F, as planned … remain faithful to
the goal of migrating to the CESK-based garbage collector and integrated allocator and not merely amending
the existing GC/allocator or implementing a GC merely influenced by the nomenclature of CESK." Every
increment must GENUINELY embody CESK + an integrated allocator↔GC, verified against source.

## DONE (commits, do not redo)
- `b8d96f1` C1.c foundation: young-only-mark generational minor + cur_seg-only FIXED-node reuse (minors ON,
  no switch), TLA+-verified (`tla/StoreCentricGC_GenerationalYoungMark.tla`, positive 121M states/0-err +
  negative counterexample).
- `674dbc5` CHANGE #1 (node-slot half): variable-length free-list reuse + slab-parity cache invalidation
  (`clear_aba_sensitive_caches()` at the collector — the fix for the set-op/jit regressions). Conf 483/0.
- `d7dbbac` side-free (`free_reclaimed_side_slots`) made documented-INERT.

## 5 CORRECTIONS to the baseline design (each source-verified, load-bearing)
1. **Commit history**: `phase-c-generational-design.md`'s "C1.b never committed / minors OFF" is FALSE at
   HEAD; ground on HEAD (`b8d96f1`/`674dbc5`/`d7dbbac`), not that doc's narrative.
2. **C2 substrate (the directive itself was wrong)**: `branch_analysis.rs` = RHS PURITY classifier;
   `continuation_compression.rs` = dead-RULE filter. NEITHER is a K-frame field-liveness analyzer. C2 is a
   NEW `Continuation::collect_live_values` table, not a reuse.
3. **LATENT BUG (`sweep_range` rem-tail)**: `index_arena.rs:933-944` partial-last-word loop pushes to
   `free_list` but OMITS `reclaimed_out.push(a)` (contrast full-word arms :917/:927). With the side-free
   re-enabled this silently under-frees the last <64 slots of every segment's published prefix → payload
   leak. **Increment A.0 fixes it first** (byte-identical while side-free inert).
4. **Side-free is LOAD-BEARING (finding #1, CONFIRMED)**: `committed_bytes` (index_heap.rs:665) = node-slab
   + side SPINE; `intern_*_in` append-only; node reuse recycles only the ~`size_of::<Node>()` slot, never
   the payload `Box`. Without the side-free a churning workload leaks payloads (the RSS bulk) until rare
   whole-segment release ⇒ the minor is NOT a net RSS win. The side-free is the RSS half of CHANGE #1.
5. **Spine never shrinks ⇒ `committed` is the WRONG major trigger (finding #3, CONFIRMED)**: no-recycle
   frees the `Box` but leaves the spine slot (`len()` unchanged) ⇒ `committed_bytes` doesn't drop ⇒ the
   major MUST trigger on `old_live`, not `committed` (CHANGE #3). A/B benchmark must measure process peak
   RSS (`/usr/bin/time -v`), not `committed`, to see the side-free win.

## INCREMENT A — Side-free SOUND re-enable (RSS half of CHANGE #1)
- **A.0** Fix the `rem`-tail `reclaimed_out` omission (`index_arena.rs:933-944`): add `reclaimed_out.push`
  in the unmarked arm, mirroring :925-928. Byte-identical (side-free inert).
- **A.1** Re-enable `free_reclaimed_side_slots` (un-comment impl; remove `let _ = reclaimed;`); GATE to
  quiescence at the driver. Mechanism: `sweep`/`sweep_young` stop calling it internally (stash `reclaimed`
  via a `last_reclaimed` field / return); the driver calls it ONLY when `phase == "quiescence"`.
- **A.2 Soundness (rigorous, vs the launder callers)**: `materialize_inner` (:419) launders `&'static`
  into the side `Box`, cached in `INNER_SHADOW` (metta_value.rs:885); every external launder consumer
  (`inner_ref`/`inner_raw` at pathmap_par_integration.rs:40, wide_mork/encoding.rs:142/314,
  metta_value_trait.rs:284) executes only DURING eval ⇒ `active_evaluator_count() >= 1`. **Quiescence is
  SOUND** (`gate_open()` ⇒ `active==0` ⇒ no live stack launder'd ref; the only refs into side `Box`es are
  `INNER_SHADOW` entries, which the driver's post-sweep `clear_inner_shadow()` (:1323) drops before any
  next eval). **Midloop is UNSOUND** (`active==1`; a grounded-op `inner_ref` result is live on the stack
  across the synchronous sweep) → excluded by the gate. A midloop collection still reclaims the node SLOT
  (sound) + defers the side `Box` to the next quiescence sweep (no-recycle idempotence makes this correct).
- **A.3 No-recycle + debug-asserts**: `s.children[i]=None` (drop `Box`), no free-list push; assert bounds
  + `Some→None`/already-`None` idempotence + non-released segment.
- **A.4 ASAN (the discharge; TLA+ N/A — side `Box` lifetime ≡ node-slot, no model-state change)**: two
  arms over an extended `examples/cesk-gc/change_state_young.metta` (live young `change-state!` cell +
  >2 MiB young churn ⇒ natural minors + a materialized launder'd ref): **quiescence arm** (directive-boundary path,
  MUST be 0-UAF) + **midloop arm** (default mid-loop path; the gate keeps the side `Box` un-freed ⇒ MUST be
  0-UAF, proving the gate is what makes it sound). High `MIN_BYTES` so no major preempts; assert minors>0 +
  reclaimed_slots>0.
- **A.5 Gate**: `a5_greenwall.sh side_free --with-oracle` (slab MUST NOT move; index/conf byte-identical
  PASS, cycles>0); ASAN both arms; 20-run; mmverify; A/B (git A=`b8d96f1`, B=HEAD) **peak RSS↓**, no wall
  regression.
- **A faithful**: the GC's sweep proves a node dead ⇒ the allocator's side arena returns the payload,
  bounding σ to `σ|_Reachable`; gating derives from the launder lifetime contract, not a flag.

## INCREMENT B — CHANGE #3: live-based major trigger + RSS bounding + cap
- **B.1** `IndexArena::old_live_node_count()` (Σ `[0, young_floor)` seg.len, mirror of `young_live_node_count`
  :720) + `IndexHeap::old_live_bytes`. R2: high-water over-estimates ⇒ major fires slightly early ⇒ safe.
- **B.2** `DEFAULT_ABSOLUTE_COMMITTED_CAP = 4 GiB` + `max_bytes()` (env `METTATRON_INDEX_GC_MAX_BYTES`,
  mirror `min_threshold()`).
- **B.3** Rewrite `major_due` (all 3 sites — driver :1257, `should_collect` :1075, `should_collect_midloop`
  :1167, kept identical): `old_live > WATERMARK.max(min_threshold()) || committed > max_bytes() ||
  MINORS_SINCE_MAJOR >= MAJOR_CADENCE`. `committed` now ONLY in the hard-ceiling clause.
- **B.4** Rearm `WATERMARK = old_live_after.saturating_mul(GROWTH).max(min_threshold())` (measured after
  `promote_young`). Trigger==rearm-metric (both `old_live`) ⇒ geometric, no thrash. Minors hold total live
  flat ⇒ old grows only with PROMOTED survivors ⇒ major fires only on genuine old-gen growth.
- **B.5 R3 anti-thrash**: a cap-major that released 0 segments raises `WATERMARK = committed` for one
  `MAJOR_CADENCE` window (slab's floor-at-committed).
- **B.6** Safety trivial (changes WHEN not WHAT the major marks; full mark/sweep unchanged). No TLA+ change.
  Discharge = A/B benchmark (majors DROP, RSS bounded; REPORT=2 + new `old_live` field).
- **B faithful**: the GC's own old-gen live high-water controls the major (not a raw allocator capacity) —
  the two generations are one integrated policy; cap+R3 mirror the slab's `gc_threshold`+floor.

## INCREMENT C — CHANGE #2: backpressure-triggered minor (faithful to slab `apply_backpressure_tierN`)
- **C.1 Faithfulness mapping** (the index is SYNCHRONOUS at safepoints ⇒ signal-then-collect, the slab's
  `request_gc`/`GC_REQUESTED` shape, not throttle-while-in-flight): committed/threshold→young_alloc/YOUNG_BUDGET;
  `BACKPRESSURE_LEVEL`→`index_backpressure_level`; `request_gc`/`GC_REQUESTED`→`nursery_full_pending`;
  `BackpressureEventuallyRelaxes`→`promote_young` clears the signal+odometer.
- **C.2** `nursery_full_pending: AtomicBool` (index_arena.rs struct :391) set in `open_segment` (:446, the
  "nursery grew" event, `&self` atomic store), cleared in `promote_young` (:758). Accessor + IndexHeap fwd.
- **C.3** `index_backpressure_level(young_alloc) -> u8`: `≥2×→3, ≥1.5×→2, ≥1×→1` (byte-identical ladder to
  slab :3159-3162), advisory + folds into the trigger.
- **C.4** `minor_due = young_alloc > YOUNG_BUDGET || heap.nursery_full_pending()` (all 3 sites). Level-3
  preference: when both due, `do_major = major_due && !(level==3 && minor_due && committed<=max_bytes() &&
  MINORS_SINCE_MAJOR<MAJOR_CADENCE)` (defer a major by ≤1 minor; ceiling/cadence major still wins).
- **C.5** No-thrash/eventual-relax: every minor's `promote_young` zeroes `young_alloc` + clears
  `nursery_full_pending` ⇒ both re-arm false ⇒ no immediate re-fire (index analogue of
  `BackpressureEventuallyRelaxes`).
- **C.6** Safety: changes only WHEN a minor fires + which of minor/major when both due; minor mark/sweep
  unchanged (TLA+ `Next` already covers more `MinorSweep` steps). No new obligation.
- **C.7 FULL gate** (this is where minors FINALLY fire under conformance): greenwall with **`minors>0`**
  assertion (add `grep -c "minor cycle"`); ASAN @ FANOUT=0 natural minors 0-UAF; 20-run; mmverify; A/B
  (minors dominate, faster and/or lower RSS).
- **C faithful**: a real `AtomicBool` set by the allocator at `open_segment`, read by the collector at the
  safepoint — the literal allocator→GC backpressure channel, faithful to the slab's TLA+-verified relax.

## INCREMENT D — C2: abstract-GC live-variable marking (most genuinely-CESK)
- **D.1** NEW `Continuation::collect_live_values(&self, out)` (beside `collect_values` types.rs:1842) — a
  per-variant table pushing only still-readable fields, consulting the variant's own deadness predicates
  (e.g. `ProcessRuleMatches` cut-fired ⇒ `remaining_matches` dead, :530-532). NOT a reuse of branch_analysis.
- **D.2 Scope** (conservative): start with `ProcessRuleMatches` + `ProcessGroundedOpFanout` (the doc's
  likely-dominant); measure; expand only on throughput headroom. Do NOT touch all 76.
- **D.3 Wiring**: NEW `RootSet::collect_from_continuations_live` (roots.rs, beside :183); the MINOR's root
  walk uses `_live`; the MAJOR keeps full `collect_values`. (Bounding caveat: a minor's wrong-narrow +
  `sweep_young` reclaim is an IMMEDIATE UAF — the major does NOT save it; the oracle is the real discharge.)
- **D.4 Soundness**: `Reachable_via_K_C2(frame) ⊇ {addrs any transition from frame can touch}` — a sound
  under-approximation of deadness, read from the machine's transition relation. Refines the K register's
  `σ|_Reachable(⟨C,E,K⟩)` contribution from "all fields" to "fields the reduction relation can read."
- **D.5 Discharge (the quiescence oracle is INSUFFICIENT — it can't see K)**: (1) per-variant differential
  tests `collect_live_values ⊆ collect_values` + dropped-field-provably-unreadable (primary, deterministic);
  (2) NEW `assert_midloop_live_superset` oracle (roots.rs, beside :379) at a MIDLOOP safepoint (K populated):
  `marked(live-roots) ⊇ Reachable(full-roots) ∩ live`; (3) ASAN midloop+C2 0-UAF on cut/superpose-heavy PLN.
  TLA+ OPTIONAL (only if C2 expands beyond 2 variants — a NEW `StoreCentricGC_AbstractGC.tla`).
- **D.6 Gate**: greenwall + differential tests + midloop oracle 0-panic over corpus + ASAN midloop+C2 +
  20-run + mmverify + throughput micro-bench (root-build self-time, C2 on vs off). C3 (parallel) → Phase D.
- **D faithful**: Might–Shivers abstract GC over the reified `Continuation` — the cleanest CESK-derived item.

## Commit order (each green-gated, no switches, heavy ops capped `MemorySwapMax=0` FOREGROUND tee'd)
| # | Commit | Gate beyond greenwall | New discharge |
|---|---|---|---|
| pre | greenwall `minors` assertion + git-version A/B bench script (no Rust) | byte-identical | — |
| A | side-free (quiescence-gated, no-recycle) + `rem`-tail fix | ASAN both arms 0-UAF (natural minors), A/B peak-RSS↓ | ASAN (TLA+ N/A) |
| B | live-based major (`old_live` trigger+rearm) + cap + R3 floor | A/B majors↓, RSS bounded | benchmark (no TLC) |
| C | backpressure minor (`nursery_full_pending` + level ladder + lvl-3 pref) | FULL: minors>0 in conf, ASAN natural minors, 20-run, mmverify, A/B minors-dominate | relax correspondence |
| D-1 | `collect_live_values` 2 variants + differential tests (unwired) | byte-identical + `live⊆full` tests | per-variant differential |
| D-2 | wire minor root walk to `_live` + midloop oracle + ASAN midloop+C2 | FULL + throughput A/B | NEW midloop live-superset oracle + ASAN |

## Greenwall additions (precursor, no Rust)
- `grep -c "minor cycle"` assertion (≥0 at A/B, >0 at C/D); re-baseline slab/index/conf-cycle numbers
  EMPIRICALLY at A (hard invariants: slab MUST NOT move; conf 483 PASS byte-identical).
- Replace stale `c1b_fanout0_bench.sh` (old one-binary young-minor env-switch A/B) with `scripts/c_ab_bench.sh`:
  A=`b8d96f1`, B=HEAD, FANOUT=0, PLN Robot + synthetic large-old+young-churn, ×3, CPU-pinned, capped,
  wall + `/usr/bin/time -v` peak RSS + REPORT=2 minor/major split + `old_live`/`committed`.

## Faithfulness ledger (the standing concern)
- **A**: σ-store reclaim completing node-slot reuse (allocator's side arena bounded by GC liveness), gated
  by the launder lifetime contract — not a flag.
- **B**: GC's old-gen live high-water controls the major — one integrated 2-gen policy, mirroring slab.
- **C**: literal allocator→GC backpressure `AtomicBool`, faithful to slab's verified relax.
- **D**: Might–Shivers abstract GC over the reified `Continuation` — K's `σ|_Reachable` contribution refined.
- The young-only-mark + cur_seg-only-reuse soundness (TLA+) is PRESERVED by every increment (A/B/C change
  when/whether-to-free + when-to-collect, never the mark/sweep mechanism; D narrows roots with a
  mechanically-discharged under-approximation). None reintroduces a registry or side-channel.
