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
  structural roots** — it adds no new root source.
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
