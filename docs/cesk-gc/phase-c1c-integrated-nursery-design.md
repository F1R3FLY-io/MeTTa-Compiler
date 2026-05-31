# Phase C1.c — Allocator↔GC-Integrated Generational Nursery (minors ON exclusively, no switch)

**User directive:** ship minors ON, EXCLUSIVELY, NO on/off switch; integrate the allocator and GC
"where sensible (akin to the allocation-backpressure changes from the current allocator/GC)."

**Why this rework (measured obstacles, this session):** the young-only-mark generational minor is sound
and implemented (C1.c substrate) but does NOT fire beneficially on the single-threaded quiescence
collector, because (1) the major triggers on `committed` (CAPACITY ≫ live when dead transient
accumulates) and (2) a minor cannot reduce `committed` — the variable-length path (`bump_in`, used by
SExpr/Conjunction/Atom/String/Spanned) NEVER reuses the free-list (only bumps), and hash-cons scatters
live content so young segments rarely fully die (`released_segs=0`). So a minor cannot displace the
major (minor-then-major = strictly more work). PLN Robot FANOUT=0: 0 minors / 14 full-mark majors / 28s.

**The fix = three coupled changes that integrate the allocator with the GC.** Designed by a Plan agent
against source (HEAD with the C1.c substrate); reviewed. The young-only-mark + no-old→young soundness is
already done (and TLA+/ASAN-validated for the full-mark variant) — this rework is about FIRING + MEMORY-
BOUNDING while PRESERVING no-old→young.

## Substrate already in place (build on; do not redo)
`alloc(&mut)` reuses YOUNG free slots only (skips/discards old → no old→young); `mark_young` (young-only
mark, sound: a live young node's parent can't be old, so it's reached via an all-young path from a young
root); `young_alloc_bytes` odometer (reset at `promote_young`); minor-primary driver; the
`YOUNG_MIN_BYTES` switch is DELETED. `MAJOR_CADENCE=16`, `YOUNG_BUDGET=2 MiB (≈¼ segment)`.

## CHANGE #1 — Variable-length free-list reuse (the core allocator↔GC coupling; hardest)
Today `Node::SExpr(ChildRef{idx})` etc. carry a segment-relative u32 into `sides[seg].{children,strings,
spans}: Vec<Box<_>>`, which is APPEND-ONLY (reset only on whole-segment release). So a minor's reclaimed
young SExpr node-slot is never reused and the side `Box` never freed → committed never drops.
- **Side-arena becomes reuse-capable:** `Vec<Box<_>>` → `Vec<Option<Box<_>>>` + per-segment per-kind
  `free_children/free_strings/free_spans: Vec<u32>` (LIFO freed indices). `Option<Box<[T]>>` is the same
  size (niche) ⇒ no memory cost; address-stable (the launder contract holds — the `Box` pointee never
  moves; a freed slot is re-occupied in place). Readers (`children`/`str_slice`/`span_at`/
  `materialize_inner`/the mark child resolvers) become `[idx].as_deref().expect(...)`.
- **Generational node-slot reuse drives side reuse:** new `IndexArena::pop_young_free_slot(&mut) ->
  Option<Addr>` (pops a YOUNG free node-slot, inheriting `alloc`'s skip-old discipline; returns the Addr
  without writing) + `write_reused(&mut, Addr, N)` (the factored reuse-arm body). The variable-length
  allocators (`alloc_sexpr`/`alloc_conjunction`/`alloc_atom`/`alloc_string`/`alloc_spanned`) become:
  reuse a young free node-slot + intern children into THAT slot's segment (`intern_children_in(seg,…)`,
  reuse-or-append via the segment's `free_*`), else the unchanged bump fallback.
- **Co-location invariant:** a reused node-slot in segment `S` MUST co-locate its side data in `sides[S]`
  (so whole-segment co-release + `children(addr)` indexing `sides[addr.segment()]` stay correct). Since
  reuse pops only YOUNG slots (`S ≥ young_floor`), an OLD segment's side-`Vec` is never touched.
- **Lockstep node↔side free (new sweep coupling):** add an `on_reclaim: FnMut(Addr)` callback to
  `sweep_range`/`sweep_young_with` (sibling to `on_release(seg)`), called per reclaimed UNMARKED young
  slot in the partially-live loop BEFORE the free-list push and while node bytes are still intact;
  `IndexHeap::sweep_young` supplies it: read the dead node's `ChildRef/ByteRef/SpanRef`, push its index to
  `sides[addr.segment()].free_*`, set the slot `None`. A released segment skips `on_reclaim` (its
  `sides[seg]` is reset wholesale). Keeps node-slot reuse and side-slot reuse rearming together.
- **`&self`/`&mut self`:** the index collector is single-threaded (FANOUT=0); variable-length allocation
  holds the heap WRITE lock (`&mut`), so reuse is `&mut self` — same regime as `alloc`. The `&self`
  lock-free `bump_in` stays bump-only (preserve B2's `NoConcurrentFree`; D inherits the `&mut` reuse as
  its quiescence specialization).
- **Soundness (no old→young preserved):** reuse pops only young ⇒ reused node is young ⇒ its edges are
  young→(old/young), never old→young (identical to a bumped young SExpr). No torn/aliased side data: a
  side index is freed only for a mark-proven-dead young node; reuse stores `Some(new)` dropping the dead
  old `Box` (nothing reachable freed); quiescence `&mut` ⇒ exclusive of readers.
- **R1 (risk flag):** `on_reclaim` must fire only while node bytes are intact (before clobber) and only
  on unmarked, non-released slots — else it frees the wrong side index → UAF. Debug-assert + ASAN-exercise.

## CHANGE #2 — Backpressure-triggered minor (faithful to the slab's `apply_backpressure_tierN`)
Slab model (`gc_allocator.rs:3152-3179,3618-3633`): `BACKPRESSURE_LEVEL` 0..3 from committed/threshold
ratio (`≥2×→3, ≥1.5×→2, ≥1×→1`), tier1 throttles-while-GC-in-flight + skip-when-progressing, TLA+
`BackpressureEventuallyRelaxes`. The index collector runs ONLY at safepoints under the write lock
(synchronous mid-alloc collection would free live VM-stack values — the `gate_open_midloop` hazard), so
allocation SIGNALS and the next safepoint collects:
- `IndexArena.nursery_full_pending: AtomicBool` — set in `alloc_bump`/`bump_in` when about to
  `open_segment()` with an empty young free-list (= the nursery filled; the "about to grow" event the
  slab throttles). Cleared in `promote_young`.
- `index_backpressure_level() -> u8` from `young_alloc/YOUNG_BUDGET` ratio (the index mirror of
  committed/threshold; same 1/2/3 ladder).
- Level ≥ 1 ⇒ fold `nursery_full_pending || young_alloc > YOUNG_BUDGET` into `should_collect(_midloop)` +
  `minor_due` (schedule a minor at the next safepoint, never synchronous).
- Level == 3 ⇒ the driver PREFERS the minor over the major for one cycle (the nursery is the pressure
  source; a young-only minor is the cheap relief) — a bounded ≤every-other-cycle inversion; the major
  still fires within `MAJOR_CADENCE`.
- No-thrash + eventual-relax: every minor resets the odometer + `nursery_full_pending` at `promote_young`
  ⇒ trigger == rearm ⇒ cannot immediately re-fire.

## CHANGE #3 — Live-based major trigger + RSS bounding + absolute cap
- Add `IndexArena::old_live_node_count()/old_live_bytes` (live high-water of `[0, young_floor)`, mirror of
  `young_live_node_count`).
- `major_due = old_live_bytes > WATERMARK.max(min_threshold()) || MINORS_SINCE_MAJOR >= MAJOR_CADENCE ||
  committed > ABSOLUTE_COMMITTED_CAP`. Rearm `WATERMARK = old_live_after_major * GROWTH` (post-promote,
  survivors are now old). Since a minor (with #1) holds total live flat by reusing young, `old_live` grows
  only with PROMOTED survivors ⇒ the major fires only when the OLD gen genuinely grows. This is the slab's
  live-based threshold specialized to the old generation.
- **Why committed/RSS stays bounded:** #1 reuse holds committed flat across minors (no new segment opens);
  the major releases fully-dead old segments as backstop; `ABSOLUTE_COMMITTED_CAP` (e.g. 4 GiB node-slab,
  env `METTATRON_INDEX_GC_MAX_BYTES`-overridable) is the hard ceiling — `committed > cap ⇒ major`.
- **The crux discharged:** before #1 a minor could not reduce committed (variable-length reclaim wasted);
  after #1 a minor's young reclaim is reused-in-place ⇒ committed flat across minors ⇒ the minor genuinely
  displaces the major (the capacity threshold that preempted is gone). Minor = net win.
- **R3 (risk flag):** if an adversary fragments every old segment (1 live node/segment), the cap forces a
  major that releases nothing → could spin. Fix = the slab's floor-at-committed trick: after a cap-major
  that released 0 segments, raise the effective threshold to `committed` for one cadence.

## Gate
- **TLA+ (new `StoreCentricGC_GenerationalYoungMark.tla` + negative model):** young-only `MarkStep`
  (descend only from MARKED YOUNG nodes) + no-old→young constraint (encode #1) + invariant
  `YoungOnlyMarkReachesLiveYoung` (`phase=sweeping ⇒ ∀ reachable young : marked`) → TLC exhaustive 0-err;
  + a NEGATIVE model (drop the constraint) that MUST produce the stranding counterexample (proves the
  constraint is load-bearing). Keep the C1.b full-mark `StoreCentricGC_Generational.tla` for the major path.
- **ASAN @ FANOUT=0, minors firing NATURALLY (no force-switch):** `change_state_young.metta` (sized so
  the churn > 2 MiB young) + a NEW free-list-reuse exerciser (seed young free-list, re-alloc SExprs forcing
  variable-length reuse, live State persists) → 0 UAF; `assert_quiescence_superset` oracle green.
- **Greenwall** `a5_greenwall.sh c1c --with-oracle` (slab 4324/0, index 4167/0, conformance 483/0 with
  cycles>0 AND minors>0 — add a `grep -c "minor cycle"` assertion) + 20-run determinism (1 hash) + mmverify.
- **GIT-VERSION A/B benchmark** (the env-switch A/B is impossible — switch deleted): A = pre-rework
  all-majors binary (`7009c0b`), B = this HEAD; PLN Robot FANOUT=0 + a synthetic large-old-gen+young-churn
  workload; ×3 replicates, CPU-pinned, capped; wall + peak RSS + minor/major split (REPORT=2, add a
  `committed` field). SHIP gate: B faster and/or lower-RSS with minors dominating; at minimum no-regression
  on every greenwall metric.

## Commit plan (green at each; switch never reintroduced)
- **C1.c-0** TLA+ young-only-mark model + negative model (no Rust change ⇒ greenwall byte-identical).
- **C1.c-1** side-arena reuse substrate INERT (`Option<Box>` + `free_*` + `_in` interns + `on_reclaim`
  wiring; allocators still bump, interns never pop) + unit tests → greenwall byte-identical.
- **C1.c-2** turn ON variable-length reuse (#1) + ASAN free-list-reuse exerciser → greenwall + oracle.
- **C1.c-3** live-based major + absolute cap (#3) → greenwall + benchmark (majors drop).
- **C1.c-4** backpressure-triggered minor (#2) → FULL gate (ASAN natural minors, greenwall minors>0,
  20-run, mmverify, A/B benchmark).
- **C1.c-5** docs + replace `c1b_fanout0_bench.sh`'s (now-inert) env-switch A/B with the git-version A/B.

## File anchors
`index_arena.rs`: struct (+`nursery_full_pending`), `alloc` (factor `pop_young_free_slot`/`write_reused`),
`alloc_bump`/`bump_in` (set pending), `sweep_range` (+`on_reclaim`), `promote_young` (clear pending),
+`old_live_*`. `index_heap.rs`: `SegmentSideArenas` (`Option<Box>`+`free_*`), `intern_*`/`_in`, the
`alloc_*` reuse branches, readers `.as_deref()`, `sweep`/`sweep_young` (`on_reclaim`), the `index_gc`
driver (minor_due+pending, live-based major_due, `index_backpressure_level`, level-3 preference, cap,
rearm from old_live), +`ABSOLUTE_COMMITTED_CAP`+`max_bytes()`. `gc_allocator.rs`: read-only reference.

## Risk register
- **R1** side/node free desync → UAF (ASAN-exercise; debug-assert intact+unmarked+non-released).
- **R2** `old_live` high-water overestimates live → major fires slightly early (conservative, safe).
- **R3** absolute-cap thrash on pathological fragmentation → slab's floor-at-committed fix.
