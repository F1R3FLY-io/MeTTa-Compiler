# F1 lever: rendezvous young cycles (SATB full-major-only → classified routing)

**Date:** 2026-06-11 · **Base:** `42180ca9` (`feature/petta-semantics`) · **Experiment:** pgmcp #12
(`satb-young-cycles-default-env-index-gc-gap-lever-f1`) · **Work item:** pgmcp #308 under Phase F epic #49 ·
**Model:** `tla/SATBYoungSweepStaleOldMark.tla`

## 1. Problem (measured)

The F1 migration gate measures the CESK index collector ~1.41× slab at `FANOUT=0`, but the
**shipped default env** (fanout enabled → every trigger routes to the dedicated SATB driver)
measures **1.65×** (fresh 30-rep control arm, Toothbrush, 2026-06-11; ledger had 1.67×).

Census at HEAD, default env, `METTATRON_INDEX_GC_REPORT=2`:

| workload | cycles | `rendezvous satb-major` | minors | trigger telemetry |
|---|---|---|---|---|
| Toothbrush | 14 | 13 (+1 quiescence major) | **0** | every cycle `young_alloc_pre` ≈ 2.2–3.0 MiB |
| Robot | 84 | 83 (+1 quiescence major) | **0** | same; late cycles `old_live_after` = 30 MiB < watermark 60 MiB |

**Every rendezvous cycle fired on the young budget** (~2 MiB) and none on old-live/cap/cadence,
yet each ran as a full SATB major: full concurrent mark + full `mark_revisit` (a second complete
traversal — `mark_from_roots_with_revisit` dedups via a separate `seen` set, it cannot prune) +
full `sweep` over all committed segments + full `hash_cons` retain. Robot's late cycles
full-mark ~36 MiB live **twice** and full-sweep ~85 MiB committed to reclaim a ~2.5 MiB young window.

`sweep_after_concurrent_mark` (index_heap.rs) documents this as *"deliberately full-major-only"*:
a young sweep after a SATB window would leave SATB-shaded **old** marks stale.

## 2. Why stale old marks are unsound (the discriminator)

`mark_from_roots_with` (the full mark, index_arena.rs:1224) **prunes at already-marked nodes**
(`if self.mark(k)` gates descent). A stale pre-set mark on old node X makes the next full mark
treat X as visited ⇒ X's live children are never marked ⇒ swept ⇒ **use-after-free**.
This is exactly `tla/SATBYoungSweepStaleOldMark.tla`'s `NoStaleOldMark` invariant:
`MC_..._full.cfg` (ClearOldMarks=TRUE) passes; `MC_..._young_only.cfg` (FALSE) is the
expected-fail discriminator (`scripts/verify_cesk_gc_formal.sh:634-636`).

### 2b. The hazard exists TODAY on the abort path (latent bug, found by this analysis)

`gc_driver_satb_rendezvous_cycle` panics (e.g. `assert!(swept)` after a gate refusal) **after**
`enter_satb_marking()` armed the deletion barrier and shades/allocate-black may have set **old**
marks. The `catch_unwind` fallback `gc_driver_stw_rendezvous_cycle` →
`run_collection_if_triggered_rendezvous` → `mark_sweep_if_over_watermark`, whose classifier can
take the **minor** arm (young budget still over — the aborted cycle never promoted). The STW
minor (`mark_young` + `sweep_young`) neither sets nor clears old marks ⇒ the stale shaded old
marks survive into the next full mark ⇒ the §2 under-mark. Narrow (requires a mid-SATB panic +
a minor-classified fallback) but real. The lever's ClearOldMarks=TRUE wiring closes it.

## 3. Design

### 3.1 Rejected: a "concurrent young SATB cycle"

A young SATB cycle (concurrent young mark + young final remark + young sweep) buys nothing:
the final remark cannot prune (revisit semantics; and `mark_young` is itself a conservative
**full-graph** traversal — old→young handle edges via first-class `SpaceHandle`s force it),
so the STW window cost equals a plain STW minor's traversal while the concurrent window
**adds** a second full traversal, a second rendezvous, and an armed barrier window.
Strictly worse on both wall time and pause than:

### 3.2 Adopted: classify at the rendezvous; route young-only triggers to the proven STW minor

In `gc_driver_rendezvous_cycle` (gc_driver.rs), after `acquire_gc_in_progress_for_rendezvous()`
+ `prepare_rendezvous_roots()` (workers parked, per-slot witness proven, complete root union
⋃ᵢ machineᵢ ∪ E₀ ∪ driver-C ∪ dispatch-C already drained):

- **`rendezvous_major_due()`** (new, index_heap.rs; source-coupled to the
  `mark_sweep_if_over_watermark` major clauses): `old_live > WATERMARK.max(min_threshold)`
  ∨ `committed > max_bytes().max(CAP_FLOOR)` ∨ `MINORS_SINCE_MAJOR ≥ MAJOR_CADENCE` (16).
  Read under the held rendezvous (workers parked ⇒ no allocation ⇒ metrics stable; `_gip`
  held ⇒ no concurrent cycle mutates the counters) — no TOCTOU.
- **major due** → the existing full SATB cycle (`gc_driver_satb_rendezvous_cycle`), unchanged,
  with the existing STW-fallback wrapper.
- **young-only due** → `run_open_stw_rendezvous_cycle(roots, gip)` — the **existing proven**
  STW rendezvous body (`run_collection_if_triggered_rendezvous` →
  `mark_sweep_if_over_watermark(roots, "rendezvous")`), whose classifier independently agrees
  (`major_due` false ⇒ `do_major` false ⇒ minor arm: `mark_young` + `sweep_young` + promote +
  the full slab-parity cache hygiene + minor bookkeeping incl. `RENDEZVOUS_MINOR_CYCLES_RUN`).
  **One rendezvous, one traversal, no SATB window** ⇒ no new stale-mark source. The cadence
  backstop keeps every 16th cycle a full major (bounds old dead + re-clears all marks ≤16 cycles).

### 3.3 ClearOldMarks=TRUE wiring (model → source)

New `IndexArena::clear_old_marks(&self)`: `clear_marks()` (per-word `Release` stores) over
non-released segments `[0, young_floor)` — for Robot-scale heaps ~11 segs × 4096 words ≈ 45k
stores ≈ µs. Forwarded by `IndexHeap::clear_old_marks`. Called in the
`mark_sweep_if_over_watermark` **minor arm iff `phase == "rendezvous"`** (after `sweep_young`,
before promote): the rendezvous phase is the only one that can follow an armed SATB window
(§2b abort fallback) — and the routed young cycle of §3.2 shares that arm, so BOTH young
rendezvous shapes realize the model's `FinalSweep` with `ClearOldMarks=TRUE`:
`oldMarked' = FALSE` ⇒ `NoStaleOldMark` at promote. Quiescence/midloop minors are untouched
(no SATB window can precede them — barriers arm only inside `gc_driver_satb_rendezvous_cycle`,
whose every exit path ends in a full sweep or the rendezvous-phase fallback).

### 3.4 Soundness inventory (young rendezvous cycle ≡ proven STW minor + clear_old_marks)

1. **Root completeness**: same per-slot witness + drained union as the SATB major (the V4 flip
   oracle (a)+(b) applies unchanged — `assert_rendezvous_union_complete` runs in debug).
2. **Young completeness**: `IndexHeap::mark_young` is the conservative full traversal through
   old nodes/spaces (old→young handle edges covered); proven by the existing minor path since C1.
3. **Old liveness untouched**: `sweep_young` sweeps `[young_floor, seg_count)` only; minors
   never free old slots; `hash_cons` retain keeps old entries (sweep_young's retain arm).
4. **clear_old_marks safety**: the young sweep consults only young segments' marks; old marks
   are dead state between cycles (every other cycle shape ends all-clear: full sweeps clear all
   swept segments' marks; STW minors never set old marks). Clearing them re-establishes the
   all-clear invariant the next full mark's prune-at-marked traversal REQUIRES (§2).
5. **Cache/ABA hygiene**: identical to every other cycle (shared arm): `bump_gc_sweep_epoch` +
   `clear_aba_sensitive_caches` + `clear_inner_shadow` + eval-memo/match-cache clears.
6. **Side-`Box` deferral**: phase=="rendezvous" ⇒ `should_drain_side_reclaims` defers side
   frees (parked workers may hold laundered `&'static` side refs) — unchanged, load-bearing.
7. **Watermark bookkeeping**: minor arm does NOT rearm the major watermark; promote resets the
   young odometer + backpressure flag — trigger==rearm, no thrash (unchanged).

### 3.5 Slab / FANOUT=0 byte-identicality

- Slab: `gc_driver_rendezvous_cycle` is unreachable (`dedicated_gc_enabled()` false);
  `mark_sweep_if_over_watermark` is index-gated. No behavior change.
- Index FANOUT=0: the rendezvous trigger requires `parallel_fanout_enabled() && n_threads()≥1`
  — never fires; quiescence/midloop arms byte-identical (the clear is phase-gated).

## 4. Gates (owed; task #3)

- TLC: `satb_young_sweep_full` (pass) + `satb_young_sweep_young_only` (expected-fail) — already
  in `verify_cesk_gc_formal.sh`; the implementation now REALIZES the TRUE side. Source-coupling
  comments added at the clear site.
- Rocq obligation: mechanize the 3-phase model + `ClearOldMarks=TRUE ⇒ NoStaleOldMark` (no
  admits/axioms), source-coupled. (Scoped at gate time with the existing formal harness layout.)
- Unit: arena `clear_old_marks` clears `[0, floor)` and preserves young marks; heap-level
  stale-old-mark discriminator (pre-set old mark → rendezvous minor → mark cleared → subsequent
  full mark descends through the node).
- ASAN forced young rendezvous cycles (default-env PLN + cut_young-style fixture), 0-UAF.
- Conformance 483 both arms; greenwall; `e1_flip_v4_asan`; mmverify Correct; 20-run determinism.
- F1 Welch re-run **with a default-env arm added** (the FANOUT=0 pin hides the SATB cost).

## 5. Experiment protocol (pgmcp #12, criterion locked pre-measurement)

- Primary: `toothbrush_default_index_wall_s`; Welch t one-tailed (less), α=0.05, min d=0.5;
  30 reps + 3 warmup per arm; taskset 8-15; performance governor; control = HEAD `42180ca9`
  release binaries (`f1_f1-rerun-paged`, source-identical); treatment = lever build.
- Pre-registered practical bar: treatment mean ≥10% below control on the primary; secondary:
  `robot_default_index_wall_s`, census young-cycle fraction, default-env index/slab ratio.
- Perf attribution: `profile.profiling` build (release+debug=1, strip=false), `perf record
  --call-graph dwarf` (Zen 3: no Intel LBR), never used for wall-time arms.

## 6. Expected effect (mechanical estimate)

Per young-triggered cycle, replace {2× full traversal + full sweep + full hash_cons retain +
2 rendezvouses} with {1× conservative traversal + young sweep + old-bitmap clear + 1 rendezvous}.
FANOUT=0 evidence (minors-dominant regime) sits at 1.41× vs default-env 1.65× on the same
binaries — the lever targets most of that ~17% delta; the cadence keeps 1-in-16 majors.

## 7. Results (2026-06-11, lever built at 42180ca9+working-tree)

Unit gates: 3 new tests green (arena clear-only-old; heap under-mark discriminator; heap
rendezvous-minor sequence) + 2 neighbors; `StaleOldMarkClear.v` compiles (closed, no admits);
release build warnings 49 = baseline.

**Census flip (single runs, default env, REPORT=2):**

| | HEAD | lever |
|---|---|---|
| Toothbrush cycles | 13 `rendezvous satb-major` + 1 q-major | **12 `rendezvous minor`** + 1 q-major |
| Robot cycles | 83 `rendezvous satb-major` + 1 q-major | **87 `rendezvous minor` + 5 `satb-major`** (cadence 87/16≈5 ✓) + 1 q-major |
| Robot wall / RSS | 16.59 s / 2.50 GiB | **10.86 s (−34.5%) / 1.66 GiB (−34%)** |
| Toothbrush wall | 2.71 s | 2.82 s (≈flat) |

**Toothbrush (the pre-registered primary): NO effect** — treatment 2.805 s ± 0.064 (n=51,
zero outliers) vs control 2.78 s pooled (n=51, two ~3.84 s outliers). The census explains it:
Toothbrush's old generation is EMPTY every cycle (`old_live_after=0` — the live set fits in
the current segment), so a full major ≈ a minor there; its 1.65× gap is NOT major-driven.
The primary hypothesis as pre-registered is expected to be REJECTED (wrong workload chosen
before the census existed); the mechanism-bearing workload is Robot (old gen ~30 MiB), where
the single-run effect is −34.5% wall and −34% RSS. Honest verdict + a refined hypothesis
(old-gen-populated workloads) recorded in experiment #12.

**Tail latency / hang exposure:** the control arm produced ~3.8 s Toothbrush outliers (2 in 51
runs) and one full WEDGE on Robot (all threads futex-parked, 34+ min, bug #309, specimen
PID 4026128); the lever arm produced zero outliers in 51 Toothbrush runs. The lever's 1
rendezvous per young cycle (vs 2 per SATB cycle) cuts the exposure windows ~16×; the
resilient Robot loop (scripts/f1_robot_resilient_bench.sh) counts hangs per arm and farms
SIGUSR1 dumps on stall.
