# Phase C — Generational collection over structural CESK roots (design)

Designed by a Plan agent against HEAD `861c3c1` (Phase A + B complete), source-grounded; reviewed and the
C/D adjudication **user-approved 2026-05-31** (with the CESK-alignment confirmation below). This is the
implementation record for Phase C.

## CESK-alignment (verified — the user's standing concern: genuine CESK, not a slab revision / vocabulary veneer)
Phase C is the **genuine CESK σ-collector** getting generational + abstract-GC-precise — NOT a slab-GC
revision, NOT a separate CESK-inspired GC. Grounded:
- **Roots ARE the structural CESK roots.** `roots.rs` computes `roots(⟨S,E,C,K,Store⟩) = addrs_in(S) ∪
  addrs_in(C) ∪ range(E) ∪ addrs_in(K)` (Van Horn–Might / Morrisett), read STRUCTURALLY from the reified
  machine (Phase A; E₀ via `collect_persistent_roots`/`collect_global_anchors`, S∪C∪K via
  `collect_machine_roots`/`collect_k_spine`), with NO `ROOT_REGISTRY` discovery apparatus (A5 cfg-scoped it
  to slab; the machine-equivalence oracle is a permanent CI invariant). **C1's minor marks from these SAME
  structural roots** — it adds no new root source. The generic safety theorem is now checked in
  `formal/lean/gc/StructuralRoots.lean`: if future machine touches stay inside the structural-root closure and
  sweep frees only unmarked nodes, a future-touched node cannot be freed.
- **C operates on the CESK store σ** (`IndexArena`/`IndexHeap`), NOT the slab `gc_allocator` (which stays
  byte-identical — the gate's slab nextest 4331/0 proves it). So C improves the CESK collector, not the legacy GC.
- **C2 is literally a CESK-machine GC technique** — Might–Shivers abstract garbage collection: the
  live-variable set per **continuation (K) frame**, derived from the machine's transition relation (which
  variables a future transition can read). The "theoretically-clean CESK form."
- **C1's soundness derives from the CESK store's structure** — no remembered-set / no write barrier needed
  because σ `Node` edges are immutable and the only mutable cell (`State`) lives in E₀ as `range(E)` (a root),
  never as an old→young σ back-edge. The structural-root invariant is preserved EXACTLY.

## Scope (user-approved) — C = {C1, C2}; C3 → D
- **C1 generational minor GC** — stays in C: real FANOUT=0 value now (the ~840-cycle conformance/mmverify/ASAN
  gate runs collector-ON at FANOUT=0 on EVERY commit) + the young/old segment-generation boundary D's parallel
  collector + E's allocate-black build on.
- **C2 live-variable marking** — stays in C: orthogonal to parallelism, pays at FANOUT=0 now + again under D.
- **C3 parallel root scan → DEFERRED to D** (same class as B2.3→D, B3-TLAB→D): needs D's rendezvous +
  parallel-collector substrate; dead code / inverted dependencies in C. Folds into D3.

## Key data finding driving C (from B4)
The index gap is **GC-bound, not JIT-bound** (B4 enabled JIT, gap held). Under default/parallel fanout the
index collector is OFF (`worker_ever_spawned` gate) → the **PLN-budget gap is parallel = Phase D's lever**
(RwLock-serialized arena). **C's win is confined to the FANOUT=0 collector** — so C's benchmark MUST be
FANOUT=0 with the collector ON, not parallel.

## Source corrections (the master plan was wrong on C2's substrate)
- `continuation_compression.rs` **does not exist**; `branch_analysis.rs` is a **purity** analyzer, NOT a
  live-var analyzer. So C2 is a **new** per-`Continuation`-variant field-liveness table, not a reuse.
- `incremental_gc.rs` is slab-era SECK scaffolding (`GenerationInfo`, "old gen = slab"); NOT wired to the
  IndexArena. Reuse only its reporting vocabulary, not the mechanism.

## C1 — generational minor GC (over the segment-based IndexArena)

**The generational invariant (theorem, verified):** a minor marks transitively from the structural roots but
**sweeps only young segments**. Sound because there are NO old→young σ edges:
1. Structural roots (registers + E₀) include every external edge into young σ — including E₀'s
   `states.values()` (the only mutable cells), so a `change-state!` storing a young value into an old state-id
   is reached as a root, not a σ-back-edge.
2. By bump-allocation order, a node is allocated AFTER its children ⇒ out-edges point to equal-or-older Addrs
   (young→old, never old→young) for immutable construction; and σ `Node` edges are immutable post-publish
   (`get_mut` is the arena's own cycle test only; `IndexHeap` exposes no mutable node borrow). ⇒ **no
   remembered-set, no write barrier.** `StoreCentricGC.tla:33-42` already formalizes "no embedded old→young
   handle in σ."

**Mechanism:**
- `IndexArena.young_floor: AtomicUsize` — segments `[young_floor, seg_count)` are young, `[0, young_floor)` old.
- `young_committed_node_bytes()` / `young_live_node_count()` accessors (Σ over `[young_floor, seg_count)`).
- **C1-a (recommended first cut): young-only SWEEP, full mark.** `mark` unchanged; `sweep_young_with(on_release)`
  iterates only `[young_floor, seg_count)`, does NOT clear old marks (old segments keep marks → never reclaimed
  by a minor). Saving = sweep is O(young) not O(all); prompt return of young transient memory.
- **C1-b (refinement): young-only MARK + sweep.** Mark worklist skips descending into old segments
  (`if addr.segment() >= young_floor` before pushing children), relying on the no-old→young theorem. Saving =
  mark is O(young reachable), removing the per-cycle full-E₀ re-mark (the big FANOUT=0 cost — E₀ holds rules /
  atom-space / caches, mostly old after warm-up). Gate hardest (the theorem is what's tested).
- **Promotion = free** (non-moving): after a minor, advance `young_floor = seg_count` (or `cur_seg`) — segments
  that survived become old by reclassification. No copying.
- **Trigger = young-allocation watermark.** New young watermark fires the minor; the existing `WATERMARK`
  becomes the **major** (full) threshold (coarser: total committed, or every N minors). Extends
  `mark_sweep_if_over_watermark` into a minor path + a major path.

**Files (grounded):**
- `index_arena.rs`: `young_floor` field + `young_*` accessors + `sweep_young_with(young_floor, on_release)`
  (parameterized `sweep_with` ranging `young_floor..seg_count`, not clearing old marks); optional young-only
  `mark` (C1-b). Current segment never released (existing invariant).
- `index_heap.rs` (`index_gc`): `young_*` forwarders; split `mark_sweep_if_over_watermark` into minor
  (young watermark, `sweep_young`) + major (full `sweep`); `hash_cons.retain` checks liveness only for
  young-segment entries during a minor (old entries retained — a minor never releases old segments); REPORT=2
  `minor`/`major` labels (reuse `incremental_gc.rs` `record_collection` vocabulary).
- `clear_inner_shadow()` after a minor still required (a reused young Addr invalidates the per-thread shadow).

## C2 — abstract-GC live-variable marking (NEW per-variant liveness; corrected)
`Continuation::collect_values` (`types.rs:1842`) pushes ALL of a variant's fields + walks all bindings. C2 adds
`collect_live_values` pushing only the fields a future transition can still read (e.g. `ProcessRuleMatches`
after a cut → `remaining_matches` dead). **Soundness mechanically discharged, not argued:** C2 NARROWS the
marked set, so a wrong "dead" classification is a UAF. Gate = `live ⊆ full` + the existing
`assert_quiescence_superset` oracle (`roots.rs:379`) run WITH C2 on (if C2 drops a still-live value, the oracle
fires). **Scope conservatively** to the 2-4 high-fanout variants the FANOUT=0 profile shows dominate root-build
self-time (likely `ProcessRuleMatches`/`ProcessGroundedOpFanout`/`CollectSExpr`), not all 76.

## Gate (per sub-increment)
Byte-identical green-wall (`scripts/a5_greenwall.sh <label> --with-oracle` → slab 4331/0, index 4174/0,
conformance 483/0 ~840 cycles, lib 49 both, oracle 0 — **C must not move these**) + ASAN@FANOUT=0 (force a
minor with a live young cell: `change-state!` storing a fresh young value → forced minor → `get-state` returns
it) + 20-run determinism (FANOUT=0, minors forced) + mmverify "Correct proof" + TLA+ `MinorSweepSafety` (extend
StoreCentricGC.tla: `youngFloor` var, `MinorSweep` reclaiming only `SegOf(a) >= youngFloor`, `Promote`,
invariant: post-minor every reachable young Addr is marked AND no old Addr reclaimed). All capped `systemd-run
-p MemoryMax=… -p MemorySwapMax=0`, FOREGROUND. **FANOUT=0 before/after benchmark** (REPORT=2 per-cycle
minor/major reclaim + wall — C's win is FANOUT=0, NOT parallel; debuginfo rebuild only to scope C2's variants).

## Commit split (green at each)
- **C0** this doc. **C1.a** `young_floor` + accessors + `sweep_young_with` + arena unit tests (mechanism inert) →
  byte-identical wall. **C1.b** drive the minor from `index_gc` (split watermark; REPORT=2 minor/major) →
  full gate + change-state!→young ASAN + 20-run + mmverify + TLA+ + FANOUT=0 benchmark (first observable win).
  **C1.c (optional)** young-only mark (gated hardest under ASAN — the no-old→young theorem). **C2** profile →
  `collect_live_values` for 2-4 variants → differential oracle → flip. **C3 → D** (not in C).

## C1.a — IMPLEMENTED (commit `7009c0b`) — inert arena mechanism
`IndexArena.young_floor: AtomicUsize` (`[young_floor, seg_count)` young); `sweep_with` refactored to a shared
`sweep_range(start_seg, clear_free_list, on_release)` (so `sweep_with == sweep_range(0, true)`, byte-identical);
`sweep_young_with == sweep_range(young_floor, false)` (young-only: OLD segments' marks AND slots untouched);
`young_committed_node_bytes` / `young_live_node_count` / `young_floor()` / `set_young_floor()`; +2 arena unit
tests. INERT (the collector still called full `sweep`), so the gate was byte-identical (slab 4333/0, index
4176/0, conf 483/0 @ 840 cycles, lib 49 both, oracle 0).

## C1.b — IMPLEMENTED (the live minor — first observable win)
Two-watermark generational driver in `index_heap.rs::index_gc`:
- `YOUNG_WATERMARK` (minor) beside `WATERMARK` (major). `should_collect`/`should_collect_midloop` fire on
  EITHER `committed > WATERMARK` (major) OR `young_live > YOUNG_WATERMARK` (minor).
- `mark_sweep_if_over_watermark`: `major_due` (checked first, subsumes minor) = full `mark` + full `sweep` +
  `promote_young` + rearm BOTH; else `minor_due` = full `mark` + `sweep_young` + `promote_young` + rearm YOUNG.
- **The mark is FULL in both paths** (conservative-complete): every live young node is marked regardless of
  any old→young edge, so the minor is sound WITHOUT the no-old→young theorem (that theorem is only C1.c's
  concern — see the prerequisite below). The minor's only saving over a major is the young-only SWEEP.
- `IndexHeap::sweep_young`: young hash-cons retain (keep OLD entries unconditionally — a minor never releases
  an old segment so their `Addr`s stay valid; keep YOUNG iff marked) + `arena.sweep_young_with` + young sides
  co-release.
- `IndexArena::promote_young` = `set_young_floor(cur_seg)` (non-moving: the just-swept segments reclassify to
  old; the active + future segments stay young). Called after EVERY collection.
- **The trigger is `young_LIVE_bytes`, NOT `young_committed_bytes` (thrash analysis).** `committed_node_bytes`
  is CAPACITY-based (`seg.capacity * per`) and does NOT drop when a minor reclaims slots — only on segment
  release — so triggering on it would re-fire a minor immediately after promotion (the freshly-promoted active
  segment contributes full capacity > the floor). `young_live_bytes` (high-water `seg.len` over the young
  range) instead RESETS at promotion (the swept segments leave the young range) and the rearm is
  `young_live * GROWTH`, so trigger and rearm are the same metric ⇒ minors fire on geometric doubling of the
  young high-water, never immediately re-fire. (`young_committed_bytes` was therefore dropped as unused.)
- REPORT=2 labels each cycle `minor`/`major` + emits `young_live_bytes`.

### C1.b — DATA-DRIVEN OUTCOME: minors ship OFF by default (the full-mark minor is not yet beneficial)
The benchmark (`scripts/c1b_fanout0_bench.sh`, PLN Robot @ FANOUT=0, collector ON) and the green-wall together
drove the shipped configuration:
- **The minor's benefit is sweep-only** — C1.b marks the FULL reachable set in BOTH paths (the young-only mark
  is C1.c, gated on the free-list fix below). So a minor pays a full mark (the dominant cost on a real heap)
  and saves only `full_sweep − young_sweep`.
- **On a mark-dominated heap the sweep saving is negligible and firing minors is a net loss** (more full-mark
  passes for little reclaim). Measured: PLN Robot FANOUT=0 = ~27.6 s over **14 full-mark majors**, RSS ~1.02 GB;
  the word-parallel sweep (B1) is a small fraction of each cycle.
- **With the natural watermarks the major preempts the minor anyway** — `committed > WATERMARK` (capacity-based)
  trips before the live-based young watermark on any heap ≫ the floor, so minors fire 0× on both the conformance
  (byte-identical 840 majors) and PLN (benchmark: 0 minors in BOTH the minors-on and minors-off configs).
- **Decision (data-driven, not a placeholder):** `young_min_threshold()` defaults to `usize::MAX` ⇒ **minors
  are OFF by default** (`index_heap.rs`). The sound minor MECHANISM ships dormant; **C1.c (cheap young-only
  mark) flips it on**. Forced via `METTATRON_INDEX_GC_YOUNG_MIN_BYTES` for validation. This makes C1.b
  **guaranteed byte-identical / no-regression** by construction (no minor ever fires), while the mechanism is
  validated under forced minors.
- **Gate (ALL GREEN):** green-wall slab nextest 4333/0, index 4176/0, conformance **483/0** (840 majors,
  byte-identical), oracle **0**, lib 49 both · TLC `StoreCentricGC_Generational` **exhaustive 32.1M states / 0
  queue / 0 errors** (all safety invariants + `MinorSweepOnlyReclaimsYoung`/`MinorNeverFreesReachable`, with
  old→young edges modeled) · ASAN @ FANOUT=0+MIDLOOP+**forced minors**: change-state!→young **0-UAF `[done]`,
  24 minors**; M11-pt **0-UAF, 221/0** · 20-run determinism **1 hash** · full conformance @ FANOUT=0+forced
  minors **483/0** · mmverify **Correct proof**.

## C1.c — APPROVED DESIGN (Plan agent, source-grounded): minors ON exclusively, no switch, beneficial
**User directive: ship minors ON, EXCLUSIVELY, NO on/off switch.** The full-mark minor (C1.b) is only
sweep-cheap → a net loss on a mark-dominated heap, so "ON" must be made a WIN by a YOUNG-ONLY MARK. Design:

**The soundness crux — exactly ONE old→young σ edge source (enumerated against source):** a fixed node WITH
child Addrs (`Error`/`Type`/`Quoted`/`Lazy`, `index_node.rs:97-101`) allocated via `alloc(&mut)` into a
REUSED OLD free slot whose children were freshly bump-allocated young. ALL other cases are benign, verified:
bump order is young→old (`cur_seg ≥ young_floor` always — `bump_in` asserts `seg==cur_seg`, `promote_young`
sets `young_floor:=cur_seg`); `change-state!`→young is an **E₀ ROOT not a σ edge** (`core.rs:2206-2210`
`states.values()` → `collect_persistent_roots`; `Node::State` is a leaf `index_node.rs:114`); σ nodes are
immutable post-publish (`get_mut` is the arena's cycle test only); SExpr/Conjunction child slices are
bump-only (never free-list-reused). ⇒ **the young-only mark needs NO remembered set** if `alloc` never
reuses an old slot.

**The fix (no barrier):**
1. **`alloc(&mut)` reuses YOUNG free slots only** — pop, skip (discard) any `addr.segment() < young_floor`
   (LIFO + minors append young ⇒ young on top ⇒ mostly young reuse, rare old-skip; skipped old slots stay
   slot-free, re-added by the next major / recovered by segment release), else bump young. ⇒ **all new
   allocation is young ⇒ no old→young edge.** (If ever rejected: the cheap correct fallback is a
   1-bit-per-old-segment "has-young-pointer" dirty bit + scan dirty old segs in `mark_young` — but the
   no-barrier rule is recommended + sound.)
2. **`mark_young`** (new): mark every root but DESCEND only into `seg ≥ young_floor` (skip old children — none
   are young by #1). Cheap: O(young reachable), not O(total live). The minor's real win.
3. **Nursery trigger = a `young_alloc_bytes` byte counter** (incremented on every young alloc in
   `alloc`/`alloc_bump`/`bump_in`, reset to 0 in `promote_young`). Tracks real young allocation INCLUDING
   free-list reuse (which `young_live` high-water misses), resets per minor (no thrash), one Relaxed
   `fetch_add` (cheaper than the `young_live` scan).
4. **`YOUNG_BUDGET ≈ 1 segment** (`DEFAULT_SEGMENT_CAPACITY * size_of::<Node>()`, ~6-8 MiB)** — a principled
   constant, NOT a switch. Sized ~1 segment so a minor's young gen ≈ the active `cur_seg`, so its reclaimed
   slots stay young (in the un-promoted `cur_seg`) and are reused — driving the promotion-stranding to ~0.
   Below the 8 MiB `min_threshold` major floor ⇒ minors fire BEFORE the major on multi-segment workloads.
5. **Minor-primary driver:** `minor_due = young_alloc_bytes > YOUNG_BUDGET` (PRIMARY); `major_due =
   committed > WATERMARK.max(min_threshold) OR minors_since_major ≥ 16` (backstop). Minor → `mark_young` +
   `sweep_young` + `promote_young`; major → full `mark` + `sweep` + `promote_young` + rearm. `clear_inner_shadow`
   after both stays.
6. **DELETE `young_min_threshold()`/`YOUNG_MIN_BYTES`.** Minors always on; fire naturally on multi-segment
   workloads ⇒ tested without any force-switch. `min_threshold`/`MIN_BYTES` stays (pre-existing major tuning).

**Gate (mechanical soundness, not argued):** extend `StoreCentricGC_Generational.tla` with a `YoungOnlyMarkStep`
+ the no-old→young constraint (encode #1) + invariant `YoungOnlyMarkReachesLiveYoung` (phase=sweeping ⇒ every
reachable young Addr marked) → TLC exhaustive 0-err; **+ a NEGATIVE model WITHOUT the constraint that MUST
produce the stranding counterexample** (proves the constraint is load-bearing). The unbounded theorem
`formal/lean/gc/YoungMark.lean` proves the core implication used here: bump-order/no-old-to-young edges plus
young-root marking and young-edge closure imply every reachable young node is marked, so a minor sweep retains
all reachable young nodes. ASAN @ FANOUT=0 with minors
firing NATURALLY: change-state!→young crux + a free-list-reuse case-2 exerciser (seed old free slots → major →
alloc `Error/Type/Quoted/Lazy` with young children → minor) + M11-pt → 0 UAF. `assert_quiescence_superset`
oracle green WITH young-only mark. Greenwall 483/0 (cycles>0, minors fire) + 20-run 1-hash + mmverify. Benchmark
= **GIT-VERSION A/B** (pre-generational all-majors binary vs this binary), NOT an env switch, on a multi-segment
FANOUT=0 workload (large persistent OLD gen + young churn): minors-on faster and/or lower-RSS, else ≥ no-regression.

**Commit (ONE increment — C1.b was never committed, so the switch never enters history):** fold C1.b's mechanism
+ all of the above → one gated commit "generational collector: minors ON (young-only mark), no switch."
Full design: this section + the Plan-agent transcript. C2 + C3→D unchanged downstream.

## ⚠️ C1.c PREREQUISITE (derived while implementing C1.b) — the free-list breaks the no-old→young theorem
The §C1 "no old→young σ edge" theorem rests on **bump-allocation order** (a node is allocated AFTER its
children ⇒ out-edges point to equal-or-older Addrs). **The global free-list VIOLATES this premise.** After a
major, `free_list` holds reclaimed slots from segments across the whole heap (old AND young). `alloc(&mut)`
pops the free-list FIRST — so a PARENT node can be constructed at a reused OLD slot while its children were
freshly bump-allocated YOUNG ⇒ a genuine **old→young edge** (`parent_old → child_young`).
- **C1.b is UNAFFECTED** — it marks the FULL reachable set, so it traverses the old parent and reaches the
  young children regardless. (This is exactly why C1.b uses the full mark.)
- **C1.c (young-only mark) would be UNSOUND as-is**: skipping old segments in the mark worklist would skip the
  old parent → never reach its young children → the live young children look unreachable → `sweep_young`
  reclaims them → UAF.
- **Therefore C1.c MUST first make new allocation never land in old**, one of: (1) a GENERATIONAL free-list
  (young free slots reused only for young allocs; old free slots refilled/reused only by a major — new allocs
  always go young, bump or young-free) — **recommended, preserves "no remembered-set"**; or (2) new allocs
  always BUMP young (never reuse old free slots until a major compacts) — simplest, wastes old free slots; or
  (3) a remembered set / write barrier on old→young (rejected — defeats the design's no-barrier premise).
  C1.c is gated on landing (1) or (2) first, then ASAN-proving the young-only mark with old→young edges
  present (forced via free-list reuse). Until then C1.b's full-mark minor is the shipped generational
  collector.
