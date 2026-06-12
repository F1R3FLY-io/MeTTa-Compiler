# Arena-coresident Inner column v2 — the parallel-arm lever (exp21 candidate)

Status: DRAFT v2 — design rounds pending (R2-parallel …); pre-registration
(exp21) gates on (a) red-team convergence AND (b) the PARALLEL microbench
(below). Supersedes the v1 seed (REJECTED for the sequential arm by the
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
