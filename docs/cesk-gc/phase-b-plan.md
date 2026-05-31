# Phase B — Concurrent arena + lock-free TLABs + JIT-on-index (the perf substrate over σ)

Branch `feature/petta-semantics`. Designed by a Plan agent against HEAD `dbc6fa8` (Phase A1–A5
complete; structural CESK roots; index build registry-free + frame_chain-free). This document is the
implementation record for Phase B; B1+B2 are designed in full here, B3+B4 sketched (detailed after B2).

**Verified baseline** (`docs/cesk-gc/a5-deletion-RESULTS.md`): slab nextest **4324/0**, index nextest
**4167/0**, index conformance **483/0** with ~**840** quiescence cycles, lib **49** warnings BOTH builds,
debug machine-equivalence oracle **0 panics**. (The `scripts/a5_greenwall.sh` header comment still says
4325/4177 — predates the A5.2/A5.5/A5.6 test deltas; trust the live `cargo nextest` Summary.)

**Compilation scope (hard constraint).** `cesk/mod.rs:46-48` declares `pub mod index_arena/index_heap/
index_node` with NO `#[cfg(feature="index-gc")]` → they compile in BOTH builds (all carry
`#![allow(dead_code)]`). Every B2 edit MUST compile + pass nextest under the default (slab) build, even
though the index arena is never *constructed* at runtime in slab mode. The in-isolation arena unit tests
(`index_arena.rs` tests; `index_heap.rs` tests) run in both builds — the cheapest B2 regression net.

**`gc_mode_is_index()`** (`metta_value.rs:520`) is a `#[inline(always)]` relaxed load of a write-once
static — NOT a `const fn`. It does not literally const-fold under the feature flag; it is a
perfectly-predicted branch. (Affects only wording of B1's nursery skip; not correctness.)

---

## Sub-steps and status

| Step | What | Status |
|------|------|--------|
| B1.a | skip the dead slab nursery in index mode (eval_loop.rs:3846) | **DONE** `83ec811` (verified present + safe) |
| B1.0 | correct the stale 4325/4177/744 comments in a5_greenwall.sh | **DONE** (folded into B1.b `0f53d6c`) |
| B1.b | word-parallel sweep + all-live/all-dead fast path (index_arena.rs) | **DONE** `0f53d6c` |
| B1.c | retain LIVE hash-cons entries at sweep (is_marked-retain, not released-only) | **DONE** `b8ef27e` |
| B2 | lock-free-capable arena interior: UnsafeCell directory + two-cursor bump/publish + `alloc(&mut)`/`alloc_bump(&self)` split + `unsafe impl Send/Sync` + loom model (was B2.1/B2.2; B2.3 concurrency-flip → D) | **DONE** `efb7d35` |
| B3 | Release/Acquire mark ordering (set_mark AcqRel; is_marked/is_fully_dead/sweep_with Acquire; clear_marks Release) — for D's concurrent marker; byte-identical | **gating** |
| B3-TLAB | ~~lock-free TLABs (64 KiB CCD-local)~~ → **DEFERRED to Phase D** (user-approved 2026-05-31). Inert until D (IndexHeap serializes; collector gate latches off) + incompatible with B2's contiguous-`len` ⇒ co-design with D's concurrent collector. Full backlog = D-TLAB.1–6 (see `phase-b3-tlab-deferral` below). | **→ D** |
| B4 | JIT-on-index re-enablement (ungate tiered_cache.rs:1297/1491/1698) — closes the ~2.2× index PLN gap | pending |

---

## B1 — quick wins (near-zero risk)

Each item is either (a) skipping a provably-dead op in index mode, or (b) an observably-equivalent fast
path. Commit each separately after a green wall.

### B1.a — nursery skip (DONE, `83ec811`)
`eval_loop.rs:3846` runtime-gates `with_nursery_collector` on `!gc_mode_is_index()`. In index mode the
slab nursery frees nothing (`NurseryState::collect` delegates to the main GC = the store-centric collector),
so the live-pointer materialize+sort (~5.64% FlyingRaven self-time on the slab path) is pure waste in
index mode and the gate skips it. Byte-identical in slab mode (gate true → runs verbatim). VERIFY-ONLY.

### B1.b — word-parallel sweep + all-live/all-dead fast path (`index_arena.rs`)
Equivalence-preserving loop strength-reduction; same free-list and same `SweepStats`, only faster.
1. **`is_fully_dead` (182-189):** OR the complete mark words covering `0..len`; mask the final partial
   word to `len & 63` low bits. One `AtomicU64::load(Relaxed)` per 64 slots instead of per slot. Same
   predicate (a segment is fully dead iff no in-range bit set). `len==0` → true (matches).
2. **reclaim loop in `sweep_with` (429-437):** per complete word: `word==u64::MAX` ⇒ `live += 64` (skip
   per-bit); `word==0` ⇒ push 64 contiguous free addrs, `reclaimed += 64`; else per-bit. Final partial
   word (`len & 63 != 0`) always per-bit, bounded by `len`. Push order stays increasing-`off` (so the
   LIFO slot-reuse order is identical → determinism preserved).
3. **Mark Ordering stays `Relaxed`** in B1 (Release/Acquire is B3's job; B1 sweep runs single-threaded
   at quiescence under the write lock).
Gate: full wall + 2-3 new equivalence unit tests (all-MAX word, all-0 word, mixed word, partial final
word) asserting exact free-list + SweepStats vs the spec. Touches the hot reclaim loop → full wall (the
840-cycle conformance exercises it live), not unit-test-only.

### B1.c — retain only LIVE hash-cons entries at sweep (`index_heap.rs`)
`IndexHeap::sweep` (398-408) does `self.hash_cons.clear()` unconditionally every sweep (the whole
8192-cap intern table), losing cross-sweep ground-SExpr dedup on sweep-heavy workloads (840 cycles).

**⚠️ The Plan agent's first design (drop only *released-segment* entries) is UNSOUND — rejected.** It
claimed the lookup-time re-validation at `intern_ground_sexpr` (183-190; re-reads `children(addr)`,
compares `tagged`) neutralizes reclaimed-to-free-list staleness. It does not: a kept entry whose slot was
reclaimed to the free list **but not yet reused** still has intact bytes → a re-intern *hits* and returns
a free-list `Addr`; if that slot is later popped by a fixed-node `alloc` (SExprs bump, but `alloc_fixed`
pops the free list), the caller's handle now aliases different content → UAF/corruption. The re-validation
only catches *already-reused* slots, not the free-but-intact window.

**Sound design (implemented): retain iff the entry's `Addr` is still LIVE (marked).** At `sweep()` entry
— BEFORE `sweep_with` clears marks, and right after the collector's `heap.mark(&addrs)` (verified flow:
`run_collection_if_triggered` 986-991, both under the write lock) — do
`self.hash_cons.retain(|_, v| v.as_arena_addr().is_some_and(|a| self.arena.is_marked(a)))`.
- A marked addr is reachable ⇒ live ⇒ its segment is not fully-dead ⇒ not released ⇒ the entry stays
  valid; future hits return a LIVE addr. Live ground content keeps its canonical `Addr` across sweeps
  (more-stable `inner_ptr` identity than `clear()`, which re-allocs each cycle).
- An unmarked addr is dead (about to be reclaimed to the free list OR in a fully-dead segment about to be
  released) ⇒ dropped ⇒ no later intern can hand back a reclaimed/released slot.
- **Safety of `is_marked(a)` at sweep entry:** by induction every retained entry points at a marked (⇒
  non-released) segment, and new inter-sweep entries point at freshly-bumped live segments; release happens
  only inside `sweep_with` (after the retain). So at every sweep entry all hash-cons addrs are in existing
  segments ⇒ the bounds-safe read holds. No-mark case (a test calling `sweep` without `mark`): all marks 0
  ⇒ retain drops everything ⇒ identical to the old `clear()` (still sound).
This is the cheap, sound replacement for `clear()`. **Determinism:** the retained set is a function of the
(deterministic) mark set ⇒ deterministic; identity is *more* stable, not less. Gate: full wall + 20-run +
debug oracle (the least-trivially-safe B1 item).

---

## B2 — concurrent arena interior (THE PARALLELISM GATE, CAVEAT 3)

> **Precise line-level design + adversarial-review verdict: `docs/cesk-gc/phase-b2-impl-design.md`.**
> Key reframing from that review (supersedes the B2.1/B2.2/B2.3 split below): `IndexHeap` keeps its
> `RwLock` `.write()` for side-arena mutation, so making `IndexArena` `&self` is **byte-identical at
> runtime** (still serialized). B2 therefore makes the arena interior lock-free-*CAPABLE* and proves the
> bump/publish protocol with a **loom model in isolation** — the end-to-end FANOUT>0 concurrency-flip
> (former "B2.3") moves to **D-phase** (when the IndexHeap side-arena spine also goes `&self`). Net:
> **B2 = ONE commit** (UnsafeCell directory + two-cursor protocol + `alloc(&mut)`/`alloc_bump(&self)`
> split + `unsafe impl Send/Sync` + loom), gated by **wall (byte-identical) + ASAN@FANOUT=0 + loom**
> (NOT TLA+ — TLA+ extension defers to D's actual concurrency). `get_mut` stays `&mut self` (no non-test
> caller). 128 KiB directory/arena accepted (one runtime arena, ~1 page resident; ~4 MiB transient tests).

Make `alloc_bump`/`bump_in`/`ensure_bump_room`/`open_segment` take `&self` (lock-free w.r.t. each other and
a concurrent marker reading published slots); keep `alloc` (free-list reuse) + `sweep`/`release` `&mut self`
at quiescence. Dissolves the #1 wall-clock lever: the single `RwLock<IndexHeap>` that serializes ALL
allocation (delivered end-to-end in D, once the side-arena spine joins).

### B2.1 — never-realloc segment directory
`segments: Vec<Segment<N>>` (222) reallocs on `open_segment`'s `push` (272) → invalidates any `&Segment`
a concurrent reader holds. Replace with:
```
segments: Box<[UnsafeCell<MaybeUninit<Box<Segment<N>>>>]>   // length MAX_SEGMENTS (16384), allocated once
seg_count: AtomicUsize                                       // published directory length
cur_seg:   AtomicUsize                                       // current bump-target index
```
- Directory = one `Box<[_]>` of 16384 pointer-cells, allocated once (= 128 KiB fixed). Backing store
  never reallocates → a `*const Segment` from it is stable for the arena's life. Each cell holds a
  `Box<Segment>` so the Segment is heap-stable independent of the directory.
- **Growth (`open_segment(&self)`):** rare; serialize with a tiny **directory mutex** (`Mutex<()>` inside
  IndexArena, or repurpose the IndexHeap write guard at the open boundary) held ONLY for the append:
  under it, re-read `seg_count`, write `*cell[idx].get() = MaybeUninit::new(Box::new(Segment::new(cap)))`,
  then `seg_count.store(idx+1, Release)` (publishes the segment ptr), `cur_seg.store(idx, Release)`.
- **Reader (`segment(&self,i)`):** `debug_assert!(i < seg_count.load(Acquire))`, then
  `&*(*self.segments[i].get()).assume_init_ref()`. The Acquire pairs with open_segment's Release.
- **Rejected:** pre-reserved `Vec<Box<Segment>>` — `Vec::push` under `&self` is unsound; `&mut self` push
  defeats lock-free alloc. (Keep the `Vec<Box<Segment>>` phrasing in the commit msg; implement the
  UnsafeCell form.)
B2.1 lands the directory but keeps `alloc`/`bump_in` `&mut self` (pure refactor, no concurrency yet).

### B2.2 — atomic bump cursor + `&self` alloc
Per-segment storage:
```
nodes: Box<[UnsafeCell<MaybeUninit<N>>]>   // length = capacity, allocated once in Segment::new
len:   AtomicUsize                          // PUBLISH cursor: count of fully-written, published slots
bump:  AtomicUsize                          // CLAIM cursor: high-water of reserved slots
marks: Box<[AtomicU64]>                      // unchanged; B3 flips Ordering
capacity; released: AtomicBool              // released → AtomicBool (future-proofs D4; read Relaxed)
```
Two cursors make `&self` bump correct under concurrency:
- `bump_one(&self) -> Option<usize>`: `let off = bump.fetch_add(1, Relaxed); if off >= capacity { None }`
  — atomically reserves a UNIQUE offset (no two threads get the same slot).
- write: `unsafe { (*nodes[off].get()).write(node) }` — exclusive (off uniquely claimed).
- **publish (contiguous):** `while len.compare_exchange_weak(off, off+1, Release, Relaxed).is_err()
  { spin/yield }` — slot `off` publishes only once `len==off` (all earlier slots published). Keeps
  `[0,len)` a contiguous written prefix (what `is_fully_dead`/`sweep`/`live_node_count` iterate). Under
  the selected B2 scope (per-segment single bumper, §B2.3) the CAS degenerates to `store(off+1, Release)`,
  but implement the CAS form so the invariant survives a future relaxation.

**Happens-before chain (makes a concurrently-allocated node safe for the marker):**
1. Allocator T writes node bytes into `nodes[off]` (plain write through UnsafeCell, exclusive).
2. T does `len.CAS(off→off+1, Release)` — Release orders T's prior writes before the store.
3. Marker M does `len.load(Acquire)`; observing `n>off` ⇒ M saw T's Release ⇒ M's reads of `nodes[0..n]`
   see T's bytes. M reads `nodes[off]` only for `off < len.load(Acquire)` ⇒ never an uninit slot.
4. Directory: M reads `segment(i)` only for `i < seg_count.load(Acquire)`, paired with open_segment's
   Release ⇒ the segment ptr + its initial `len=0` visible before M indexes it.

**`unsafe impl Send/Sync`** (UnsafeCell is `!Sync` → required):
```
unsafe impl<N: Copy + Send> Send for IndexArena<N> {}
unsafe impl<N: Copy + Send + Sync> Sync for IndexArena<N> {}
```
SAFETY (the most-scrutinized comment in B2): concurrent `&self` touches a slot only via the publish
protocol — (i) unique claim via `bump.fetch_add` (no writer aliasing); (ii) reader reads `nodes[off]`
only for `off < len.load(Acquire)` with Release-ordered bytes; (iii) directory read only for
`i < seg_count.load(Acquire)` with the open Release; (iv) free-list reuse (the only occupied-slot
rewrite) runs only at quiescence (gate closed for workers) → never races a reader/writer. ⇒ no data race.

### B2.3 — free-list reuse at quiescence + determinism
- **Free-list reuse stays `&mut self` at quiescence.** `sweep` rebuilds `free_list` from scratch at
  quiescence (gate `!worker_ever_spawned() && active_evaluator_count() ∈ {0,1}` + write lock).
  **Concurrent alloc claims ONLY fresh bump space** (`bump.fetch_add`), NEVER pops the free list — a
  reused slot under a race is the ABA/torn-node class B2 must avoid. Since the only producer of free slots
  is `sweep` at quiescence, and the collector gate is *closed* whenever workers exist, no sweep runs
  concurrently with bump alloc → TLA+ `NoConcurrentFree` preserved verbatim.
- **Determinism (the 20-run gate):** `bump.fetch_add` racing across threads makes `Addr` assignment
  nondeterministic across runs. But observable determinism does NOT depend on `Addr` values — it depends
  on structural equality + `inner_ptr` *content*-identity (hash-cons keyed by content). Two confounds:
  (a) hash-cons identity — keep `intern_ground_sexpr`/side-arenas under the IndexHeap write lock in B2 so
  hash-cons stays serialized + content-deterministic; (b) partition fresh bump per worker by segment
  (per-worker `cur_seg`, the B3 TLAB direction) so within a segment there is a single bumper and offset
  assignment is sequential. The 20-run gate runs at FANOUT>0 and asserts byte-identical *observable*
  output (NOT identical Addrs).

### B2 scope boundary
B2 makes ONLY `IndexArena`'s node-slot path `&self`. `IndexHeap` side-arenas (`sides: Vec<...>`),
`space_table`/`memo_table`/`hash_cons` stay under the RwLock write lock (their `Vec`/`HashMap` spine
append is a separate, larger atomic-cursor change → D-phase). The parallelism gate still opens at the
arena level (the substrate B3 TLABs + the D parallel collector build on). Land the arena interior first
(smallest reversible increment), measure, then decide on the side-arena spine. Document this in the
commit: "B2 = lock-free IndexArena interior; IndexHeap side-arena append remains lock-guarded pending D."

---

## TLA+ obligation for B2 (`tla/StoreCentricGC.tla` + `MC_StoreCentricGC.cfg`)

The model abstracts alloc as one atomic step and frees only in `Sweep` at quiescence (5 proven safety
invariants incl `NoConcurrentFree`/`NoUseAfterFree`/`NoLostObjects` over 4 Addrs/2 workers). B2 makes
alloc non-atomic (claim-then-publish) + adds directory publication. Model edits:

| Element | Change | New invariant |
|---|---|---|
| `store` codomain | + `"claimed"` (between `"free"` and `"live"`) | `NoMarkOfClaimed`, `NoClaimedWhileCollecting` |
| `Alloc` | split → `ClaimSlot(w)` (fresh bump, set `"claimed"`) + `PublishSlot(w)` (`"claimed"`→`"live"`, join ψ) | `NoUnpublishedRead` (marking marks only `"live"`/`"marked"`, never `"claimed"`) |
| `seg_count` (new) | monotone directory counter + `OpenSegment` (under `dirLocked` flag) | `SegmentPublishedBeforeUse` (no claim/mark of an Addr in an unpublished segment) |
| free-list reuse | factored to a quiescence-only `AllocFromFreeList` (enabled when `Cardinality(activeEvaluators) ≤ 1`); `ClaimSlot` claims fresh bump only | `NoConcurrentFree` (preserved; premise made explicit) |
| `FairSpec` | + WF on `PublishSlot` (a claimed slot is eventually published) | `GCEventuallyCompletes` re-checked |

Re-run `MC_StoreCentricGC.cfg` (4 Addrs/2 workers) with the new invariants added to `INVARIANT` and the
new actions in `Next`; the `"claimed"` state widens `store` from `3^4`→`4^4` (still tractable, ~80s base).
Cap per the cfg header: `systemd-run --user --scope -p MemoryMax=96G -p CPUQuota=1800% tlc -workers auto`.

---

## Per-rung gate

Harness: `scripts/a5_greenwall.sh <label> [--with-oracle]` (slab nextest 4324/0 + index nextest 4167/0 +
index conformance 483/0 ~840 cycles + lib-49-both + debug oracle 0 panics). Non-vacuity env:
`METTATRON_PARALLEL_FANOUT_DEPTH=0 METTATRON_INDEX_GC_MIN_BYTES=131072 METTATRON_INDEX_GC_REPORT=1`. ALL
heavy ops capped `systemd-run … -p MemoryMax=… -p MemorySwapMax=0`, FOREGROUND.

- **B1.0/B1.b:** full wall `--with-oracle` + B1.b's new equivalence unit tests (both builds).
- **B1.c:** full wall `--with-oracle` + 20-run determinism + debug oracle (identity-retention change).
- **B2 (concentrated on B2.3):**
  - Always: full wall `--with-oracle` (the "B2 doesn't perturb slab" check = slab nextest 4324/0 + lib 49).
  - **ASAN at FANOUT>0** (`scripts/a5_asan_both.sh` extended — it hard-codes `FANOUT_DEPTH=0`; B2 needs a
    `FANOUT_DEPTH≥2` variant so concurrent bump is actually exercised). Capped `-p MemoryMax=32G
    -p MemorySwapMax=0 -p TasksMax=256 -j4`, FOREGROUND. Workload: `mtt-conformance --module
    M11-bisimilarity-pt` (spawns workers) + a NEW parallel-superpose alloc-heavy stress
    (`stress_parallel_alloc.metta` — the existing `stress_alloc.metta` is sequential → never latches
    `worker_ever_spawned` → does NOT exercise concurrent alloc).
  - **loom** model (new `#[cfg(loom)]` test in index_arena.rs) of the bump/publish protocol: 2 loom
    threads claim+write+publish, a 3rd does `len.load(Acquire)` + reads `nodes[0..len]`, asserting (i) no
    two threads share an offset, (ii) every read slot is fully written, (iii) `len` is a contiguous
    prefix. loom explores all Acquire/Release interleavings — the mechanical proof of the happens-before.
  - **20-run determinism at FANOUT>0** (byte-identical observable output, NOT Addrs).
  - **TLA+** (extended model) green before flipping concurrency on (B2.3).

---

## Data-driven validation (before/after; the JIT confound)

Under index-gc, T2/T3 JIT is gated OFF (re-enabled in B4) → index-vs-slab wall-clock is dominated by the
JIT gap, not GC. So **all B1/B2 perf measurement is index-vs-index (before/after), never index-vs-slab.**
(Welch index+JIT-vs-slab+JIT is F1.) Metrics per run: wall, peak RSS (`/usr/bin/time -v`), GC cycles
(`METTATRON_INDEX_GC_REPORT=1/2`), and alloc throughput (`IndexArena::alloc_count()`/wall).

- **B1.b:** per-sweep cost (`REPORT=2`) on `stress_multidir.metta` (840 sweeps) + `stress_alloc.metta`.
- **B1.c:** `alloc_count` + peak RSS on `stress_multidir.metta` (repeated equal ground content → hits
  replace re-allocs).
- **B2 (isolate ALLOCATION throughput — the whole point):** index-vs-index, before/after, at FANOUT ∈
  {0,2,4,8} on a NEW parallel alloc-heavy workload (superpose over many `(burn N)` branches so ≥2 workers
  bump concurrently). Primary metric = `alloc_count`/wall vs FANOUT: before B2 (RwLock-serialized) is flat
  or worse as FANOUT rises (contention); after B2 (lock-free bump) scales with FANOUT. The slope is the
  clean isolation of the lock-removal win (JIT off in both arms; same MIN_BYTES/workload). ≥10 replicates,
  CPU-pinned/freq-locked, capped, FOREGROUND; record in the pgmcp experiment ledger (#6) to pre-stage F1.

---

## Recommended order

B1.0 → B1.b → B1.c → B2.1 (directory, still `&mut`) → B2.2 (`&self` cursor, single-thread callers) →
B2.3 (concurrency on, FANOUT>0) → (B3/D: side-arena spine, Release/Acquire marks). Each B2 sub-step is
independently revertible and passes the wall; the heavy ASAN/loom/20-run/TLA+ gate concentrates on B2.3.
If B2.3 fails ASAN, B2.1+B2.2 (a strictly-better directory + `&self` API) still stand.

## Risk register (top items)
- B2 edit breaks the **slab build** (arena compiles in both) → slab nextest 4324/0 + lib-49 gate; keep all
  changes inside index_arena/index_heap; UnsafeCell/unsafe-impl compile but are never constructed in slab.
- **Torn/uninit node** read by marker → the two-cursor publish + Acquire/Release; ASAN FANOUT>0 + loom +
  TLA+ `NoUnpublishedRead`.
- **`len` non-contiguous** (out-of-order publish) → CAS-extend-prefix publish; loom asserts `[0,len)`
  written.
- **Determinism regression** (Addr nondeterminism leaks) → hash-cons stays lock-guarded; per-worker
  segment partition; 20-run FANOUT>0 localizes.
- **`unsafe impl Sync` unsound** (most dangerous line) → SAFETY block ties Sync to the 4 enumerated
  invariants; loom+ASAN+TLA+ together.
- **Re-introducing a discovery side-channel** (Phase A's whole point) → the permanent machine-equivalence
  oracle (eval_loop.rs:3662) panics; B2 touches only the σ substrate, never the root set.
- **ASAN OOM** (once crashed the 125 GiB box) → `-p MemoryMax=32G -p MemorySwapMax=0`, FOREGROUND, serial.

---

## B3 — Release/Acquire mark ordering (the clean half; TLABs → D)

User-approved 2026-05-31: B3 ships **only** the mark-ordering strengthening; the lock-free TLABs defer to D
(rationale below). Edits in `index_arena.rs` (Plan-agent designed `help-me-complete-…` B3 report, source-verified):
- `set_mark`: `fetch_or(bit, Relaxed)` → **`AcqRel`** — Release half publishes the marker's prior writes
  (allocate-black node bytes) to a sweep/second-marker that Acquire-observes the bit; Acquire half orders a
  parallel marker's worklist reads + keeps `prev & bit` idempotence correct across racing markers.
- `is_marked`: `load(Relaxed)` → **`Acquire`** — observing the bit happens-after the AcqRel set.
- `is_fully_dead` (×2 mark-word loads) + `sweep_with` reclaim loop (×2): `load(Relaxed)` → **`Acquire`** — the
  sweep is the mark CONSUMER; it must observe every bit any marker set (else it reclaims a live slot → UAF).
- `clear_marks`: `store(0, Relaxed)` → **`Release`** — cycle N's zeroing ordered before cycle N+1's first set.

**Byte-identical at the current runtime** (proof): Relaxed→stronger only ADDS happens-before; the marker
(`heap.mark`) + sweep run single-threaded under the IndexHeap `RwLock` write lock and only while
`!worker_ever_spawned()`, so with one thread the orderings are observably identical (same bits, same
SweepStats, same conformance output). `bump`/`len`/`released`/`alloc_count` keep their own orderings.

**B3 gate** = green-wall `--with-oracle` (byte-identical: slab 4330/0, index 4173/0, conformance 483/0 ~840
cycles, lib 49 both, oracle 0) + ASAN@FANOUT=0 (regression backstop — the mark-ordering is byte-identical
*memory accesses*, so ASAN, which checks memory safety not ordering, is a sanity backstop, not informative).
**No loom/TLA+ in B3**: the ordering's PURPOSE is D's concurrent marker, which doesn't exist in B; the
mechanical concurrency proof (loom of mark↔sweep + the TLA+ `claimed`/`published` model) is a **D deliverable**
done against the real marker — consistent with deferring the TLAB it serves. The ordering is sound by
construction (textbook Release/Acquire synchronizes-with) + documented + byte-identical-gated now.

## TLABs deferred to D — the concrete backlog (D-TLAB.1–6, nothing hand-waved)

A **TLAB** (thread-local allocation buffer) = each worker bulk-reserves a slot range (`bump.fetch_add(N)`) and
bump-allocates within it locally (non-atomic `next += 1`), amortizing the shared-cursor atomic ~N× and giving
CCD/cache locality. **Why deferred:** (1) **incompatible with B2's contiguous-`len`** — independent TLAB ranges
publish out of order, but `len` is a contiguous prefix, so a higher range must wait for lower ranges → serializes,
defeating the parallelism; (2) **inert until D** — IndexHeap still serializes all allocation behind its RwLock, and
the collector gate latches off the moment a worker spawns, so a TLAB delivers 0 benefit + can't be exercised in
B's single-threaded runtime; (3) its correctness obligation (a concurrent marker never reads a claimed-unpublished
slot; sweep never frees one) is a **D invariant** with no B-phase validator.

D must build (per the Plan-agent report):
- **D-TLAB.1** Per-slot publication replacing single-segment contiguity. Two candidates: (a) a per-segment
  `published` bitmap alongside `marks` (slot readable iff published-bit set, Acquire; marker/sweep AND it with
  `marks`); **(b) segment-as-TLAB** — a worker claims a WHOLE segment via the already-`&self` `open_segment`, keeping
  B2's contiguous-`len` *verbatim* (one bumper per segment ⇒ contiguity free), cross-segment parallelism from
  different workers owning different segments. **Prefer (b)** (lowest risk; preserves the B2 protocol) unless a
  per-worker segment-count explosion forces (a).
- **D-TLAB.2** 64 KiB sizing: under (b) a TLAB *segment capacity* (~2730 slots) distinct from the 6 MiB production
  segment (L2/CCD-resident); under (a) a 64 KiB range within a segment.
- **D-TLAB.3** The `Tlab { seg, base, next, end }` thread-local + bulk-claim + non-atomic local bump + re-claim.
- **D-TLAB.4** `tlab_flush_and_publish`: Release fence before `ACTIVE_EVALUATORS--` (park / EvalGuard drop), so a
  concurrent marker observing the evaluator-count drop also observes the TLAB's published slots.
- **D-TLAB.5** Gate interaction: only after D5 drops `!worker_ever_spawned()` can a TLAB coexist with live collection
  ⇒ TLAB correctness is a D-gate obligation (ASAN@FANOUT>0 + 20-run + loom + TLA+), not a B-gate one.
- **D-TLAB.6** loom (multi-range model: N writers claim disjoint ranges, publish independently, a marker reads the
  published set) + extend `tla/StoreCentricGC.tla`'s `claimed`/`PublishSlot` to model out-of-order range
  publication under the concurrent marker (`NoUnpublishedRead`/`NoMarkOfClaimed`).
