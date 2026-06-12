# Arena-coresident Inner column v2 — the parallel-arm lever (exp21 candidate)

Status: **v3 — R2-parallel folded (1 BLOCKER + 4 MAJOR resolved below;
verdict was NOT-CONVERGED; one confirming pass R3-parallel gates exp21).**
The parallel gate PASSED (86.7% aggregate-CPU win at N=8, `929040a5`;
R2's fidelity audit erodes the floor only to ~84% ≫ the 30% bar). Supersedes the v1 seed (REJECTED for the sequential arm by the
exp19 gate: single-threaded per-call 3.02 vs 3.39 ns — the column lost) —
**resurrected by the parallel decomposition** (`ecc7a5f9`).

## Why now (the datum v1 lacked)

Default-env (shipped) Robot: slab 15.04s CPU / 3.33s wall vs index
**33.77s CPU** / 8.39s wall at similar parallel efficiency — the 2.6× gap
is **2.25× multiplied work**: the thread-local `INNER_SHADOW` makes every
worker re-materialize every shared value it touches (N-fold; slab shares
its heap). A SHARED column is built once at intern and read by all
workers — the multiplication class dies. Projection (to be gated by the
parallel microbench, not trusted): index parallel CPU → ~15-18s ⇒ wall
~4-4.5s ⇒ default-env ratio 2.60× → ~1.2-1.35×. Side bonus: deletes the
mmverify fresh-thread shadow cost class (208k threads × zero setup).

## v3 resolutions (R2-parallel findings 1-9)

1. **F1 BLOCKER — Space/Memo storage (the column's entry type is now POD):**
   column cells for `TAG5_SPACE`/`TAG5_MEMO` are NEVER WRITTEN. Instead a
   never-freed per-heap id-keyed store holds prebuilt
   `MettaValueInner::Space/Memo` (the `view_at` launder precedent,
   index_heap.rs:869; spaces/memos are few, long-lived, cold — no hot
   profile shows them). `inner_ref_index` v2 branches on tag5 for those
   two tags and launders from that store — `as_space()`'s `&SpaceHandle`
   signature is preserved. Every column-stored variant is then
   Copy-payload (laundered refs + scalars): the cells are POD, drop is
   trivial, `write_reused` overwrite leaks nothing, and segment release
   frees wholesale — the B1 sweep hot loops stay untouched (F8 verified
   this claim under exactly this resolution).
2. **F2 MAJOR — publish ordering: POST-publish column writes (option b).**
   The pre-publish requirement is dropped (it would force a 3-phase split
   of the pinned B2 claim/write/publish protocol). Defense: the ONLY
   column reader is handle-mediated `inner_ref_index` — no scanner reads
   column cells by published-`len` (hash-cons verify reads children;
   mark/sweep runs at write-lock quiescence). The factory writes the
   column cell after `try_bump_in`/`alloc_bump` returns and BEFORE the
   handle escapes; cross-thread visibility rides the handle's own escape
   synchronization (F3's enumerated channels). Write points re-enumerated
   to include `IndexArena::alloc`'s internal reuse and
   `alloc_*_concurrent`'s `alloc_bump` path.
3. **F3 MAJOR — the lock-free read path needs a STATIC out-of-RwLock
   column directory**: `static COLUMN_DIR: [AtomicPtr<ColumnSeg>; MAX_SEGMENTS]`
   (never-moving; maintained at `ensure_side_seg` and segment release —
   readers re-derive raw pointers per call; release only of fully-dead
   segments, the I2 argument). This is also what makes "two loads" true.
   NEW STATED INVARIANT: with the heap lock off the read path, every
   handle-escape channel's Release/Acquire is correctness-critical (pool
   task handoff, Arc<Mutex> results + Condvar, space RwLock, hash-cons
   write lock, thread::spawn edges — R2 traced all; no pre-Release escape
   exists). Obligation: a loom model covering reuse-write → guard release
   → channel → lock-free read.
4. **F4 MAJOR — the debug oracle is NODE-GROUNDED**: on every column read,
   `#[cfg(debug_assertions)]` takes `global_index_heap().read()` and
   asserts payload identity column-vs-NODE (pointer equality of laundered
   slices/strs/spans; value equality for scalars) — tag-vs-column alone
   cannot see same-variant reuse desync (the common free-list case).
   Plus a forced-reuse greenwall fixture (alloc → unroot → sweep →
   realloc → read).
5. **F5 MAJOR — exp21 pre-registers MaxRSS gate columns** (Robot
   default-env AND mmverify) with an acceptance band vs the 0.43-0.63×
   baseline; the ledger admits the mmverify RSS SIGN FLIP openly (its
   win is CPU; the column charges 32B × the whole set.mm corpus while
   the per-thread shadows it deletes were transient).
6. **F6 MINOR — intern-side cost named** (POD construction allocates
   nothing; ≤ single-digit % of an intern critical section that already
   Box-allocates children) + the exp13 convoy canary (default-env
   Toothbrush interleaved A/B) added to the exp21 protocol.
7. **F7 MINOR — bench fidelity**: add a PASSES-sensitivity arm and the
   real two-load+directory read shape before registration (floor ~84%).
8. **F8 MINOR — the per-entry generation is DELETED** (no consumer:
   handles carry no gen; reachability names the current occupant; POD
   cells own nothing so sweep needs no snapshots). Entries are bare
   `MaybeUninit<MettaValueInner>` ≈ 32B. Write-point 2 = plain
   overwrite-then-escape.
9. **F9 INFO — mmverify module-load build answered**: the corpus interns
   pre-FANOUT under the write lock (write-point 1 builds every entry);
   the 208k threads read with zero setup (spawn edges = the Acquire).

## Inherited, MANDATORY (adjudicated in R1, design doc f1-profile-post309):

- **R1-F2 (CRITICAL): the reuse-rewrite protocol** — specified below; the
  v1 seed's "rewritten at re-intern" described nonexistent machinery.
- R1-F5: Space/Memo entries store **ids**; `as_space`/`as_memo` lazily
  Arc-clone from the side table (cold paths; lifecycle decoupled).
- R1-F3: publication ordering — a column write BEFORE the node-segment
  `publish` Release is covered for any reader that Acquire-observes the
  node; no new fencing.
- R1-F4: `INNER_SHADOW` + `INNER_SHADOW_EPOCH` + `clear_inner_shadow` +
  `ensure_inner_shadow_epoch_current` + the eval-cache epoch handshake
  for the shadow are DELETED in full (the launder fn and SideColumn gen
  machinery stay).

## The reuse-rewrite protocol (R1-F2 resolution — Option A, per-entry generation)

Storage: a per-segment column co-owned by the node segment (the
`SideColumn` discipline, index_heap.rs ~3000-3280, IS the template):
`entries: Vec<MaybeUninit<(u32 /*gen*/, MettaValueInner)>>` with the same
never-move/publish-len rules.

Write points (exactly the node-slot write points, enumerated by R1):
1. **Fresh bump** (`try_bump_in`/concurrent publish): construct the
   `MettaValueInner` from the node + side data (all inputs address-stable
   at this moment — R1-F1), write `(gen=0, inner)` at the slot's column
   cell, THEN publish the node (Release covers the column write).
2. **Slot reuse** (`write_reused`, index_arena.rs:710-722): bump the
   cell's gen, overwrite the inner, THEN return the Addr to the (single)
   allocating caller; cross-thread visibility rides the value's own
   escape (same argument as the node bits themselves).
3. **Sweep/segment-release**: entries die with the slot/segment — column
   cells are dropped where node slots are freed (the SideColumn deferred-
   reclaim generation check rejects stale snapshots, identical mechanism).

Read (`inner_ref_index` v2): `addr → segment dir → column cell → launder
&'static Inner` — two loads, no TLS, no RefCell, no epoch, no lock,
SHARED. The DEBUG I1 tripwire (tag-vs-variant) moves onto this read.

## The PARALLEL microbench gate (pre-committed before exp21 registration)

Extend `mtt-hitpath-bench` with an N=8-thread arm over the SAME 1M
handles: arm A = today's per-thread shadows (each thread materializes its
own copies); arm B = one shared prebuilt column. Measure AGGREGATE CPU
and wall. **Decision rule: B must beat A by ≥30% aggregate CPU at N=8, or
the column stays dead and the parallel campaign pivots to
collector-on-with-workers first.** (The v1 gate failed at N=1 — the wrong
N for the question this lever answers.)

## Separate lever (not this design): collector-on-with-workers

`worker_ever_spawned` latches the index collector OFF under FANOUT —
monotone heap growth ⇒ ever-colder caches and unbounded committed pages.
Re-enabling collection with workers is its own campaign item (rendezvous
machinery exists — E1/E5 proved it; the latch was a Phase-B
simplification), measured independently.

## Red-team ledger (v2)

- R2-parallel (pending): attack the reuse protocol's cross-thread
  visibility argument at write-point 2; the MaybeUninit/Drop story for
  `MettaValueInner` entries (String/SExpr hold laundered refs — drop is
  trivial? Space/Memo as ids ⇒ no Arc drops); column RSS (+~40-48B/node)
  vs the deleted per-thread shadows (N workers × touched set) — net RSS
  on Robot FANOUT-default; the I1 tripwire relocation; mmverify's
  208k-thread interaction (each thread's first column READ is now free —
  but the column itself is built by whom for module-load values?).
