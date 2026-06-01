# Phase D — Parallel Collector: source-verified design

**Status: EXECUTING. The D-TLAB sub-phase (concurrent side arenas + un-serialized allocation) is COMPLETE +
committed + gated. NEXT = D1–D5 (the parallel collector itself: rendezvous → worker-self-root → parallel
mark/sweep → concurrent release → gate-flip).** Authoritative plan:
`~/.claude/plans/help-me-complete-the-shimmying-mochi.md` (Phase D). This doc records the VERIFIED current
state and the corrections the source forces on the plan's D1/D2 assumptions.

**Progress (2026-06-01) — D-TLAB sub-phase COMPLETE:**
- ✅ **D-TLAB-1.0** (`a574bf5`): inert `SideColumn<T>` concurrent chunked column + unit tests.
- ✅ **D-TLAB-1.1** (`f7f49e0`): wired `SideColumn` into `SegmentSideArenas` (&mut, byte-identical) + the
  D-TLAB-1.0 two-level-lazy-directory fixup (`SIDE_PAGE_BITS=10`, full u32 ceiling; the `MAX_SIDE_CHUNKS=66`
  cap panicked rc=101 once wired — cur_seg reuse decouples never-recycled side appends from node count).
- ✅ **D-TLAB-1.2** (`98b7c99`): `&self`-capable side append — `sides` → never-realloc directory +
  `ensure_side_seg(&self)`/`side(&self)`; `intern_*_in` → `&self`; `IndexArena::try_bump_in(&self)` (TOCTOU
  guard, co-location single-pick + retry); `bump_in` delegates (slab untouched); `unsafe impl Send/Sync`.
  IndexFactory stays `.write()` (byte-identical). NEW loom side-publish-before-node-publish test passes.
- ✅ **D-RLOCK.1/.2** (`24b1a11`): the read-lock allocation fast path — `IndexFactory` hot sites flip to
  `try_write()` (reuse) → `.read()` + `alloc_*_concurrent(&self)` (bump-only), so parallel-eval workers stop
  serializing on the heap lock. Ground hash-cons / space / memo / not_reducible stay `.write()`. SAFE because
  the collector is gated OFF under FANOUT>0 (`!worker_ever_spawned()`) ⇒ alloc never races a live collector
  (temporally disjoint + RwLock-excluded + every freeing path is `&mut self`, type-uncallable under `.read()`).
  Gate: byte-identical FANOUT=0 483/0 + DEBUG oracle 483/0; FANOUT=8 output-determinism == unmodified-HEAD
  baseline (canonical hash, Robot+FlyingRaven); TSan 0; ASAN FANOUT=8 0-UAF; loom 12/12; mmverify; 20-run
  1-hash; slab 4343/0, index 4186/0. **TLA+ coverage:** the existing `StoreCentricGC.tla` (NumWorkers
  concurrent `Alloc` + collect-at-quiescence, 5 invariants green) OVER-APPROXIMATES D-RLOCK (collector OFF
  under FANOUT>0) ⇒ already covers concurrent-alloc safety; the real TLA+ EXTENSION is owed at D1–D5 (parallel
  collection). PRE-EXISTING (not D-RLOCK): a FANOUT=8 SEGV in `stress_multidir.metta` reproduces at identical
  rate on unmodified HEAD `98b7c99` — a latent parallel-eval scale-crash, independent of this work.
- ⏳ **D1–D5** (NEXT): the parallel collector — D1 fresh rendezvous (EvalGuard/condvar/GC_STATE), D2
  worker-self-root, D3 parallel mark/sweep, D4 concurrent dead-segment release behind the launder+epoch
  handshake, D5 flip the `!worker_ever_spawned()` gate. Load-bearing: ASAN FANOUT>0 + 20-run + re-derived
  TLA+ + loom/TSan. This is where collection runs WITH workers (the no-collection-while-parallel half of the
  perf gap).

## Three contradictions the source forces on the written plan (verified file:line)

1. **The STW rendezvous the plan says to "REUSE" was DELETED in Phase 9.** `safepoint_wait_for_quiescence`
   is gone (`gc_allocator.rs:4218-4236`). The slab GC went fully **async snapshot-by-value**
   (`maybe_async_gc`, `gc_allocator.rs:3478`) WITHOUT an `ACTIVE_EVALUATORS==0` gate, relying on exactly the
   `ParallelDispatchRootProvider`/`register_temporary_roots`/`CurrentIterRootProvider` discovery apparatus A5
   dissolved (protocol comment `:2760-2773` literally says its purpose is to AVOID scanning trampoline
   stacks — the anti-CESK snapshot model). In index-gc mode both `maybe_quiescent_gc` (`:3380`) and
   `maybe_async_gc` (`:3478`) are runtime-inert (`gc_mode_is_index()` early-returns). **Reusable: the
   PRIMITIVE VOCABULARY** — `GC_REQUESTED`/`request_gc`/`is_gc_requested` (`:2564`), `ACTIVE_EVALUATORS`
   counter, `EvalGuard` enter/drop (`:2901-2947`), the lost-wakeup-safe condvar-park pattern,
   `drop_eval_guard_for_safepoint`/`reacquire_eval_guard_after_safepoint` (`:4090`/`:4119`). **NOT reusable:
   a STW worker-park rendezvous — D1 BUILDS IT FRESH** from those primitives (modeled on `EvalGuard::enter`'s
   CAS-loop + condvar park), NOT on `maybe_async_gc`.

2. **The collector CANNOT read a parked worker's registers — so D2 is WORKER-SELF-ROOTING.** Each worker
   runs its own `eval_trampoline` (`eval_loop.rs:3193`) with THREAD-LOCAL registers; the K-spine
   (`k_spine::collect_k_spine`, `roots.rs:415`) reads thread-local statics `SUSPENDED_ACTIVATIONS`/
   `LIVE_VM_STACK` (`k_spine.rs:81-88`) holding **raw pointers into the owning worker's native Rust stack**
   (`*const Vec<WorkItem>`, `*const GenericBytecodeVM`, `k_spine.rs:43-79`), valid only while that worker is
   parked at its push site. A different (collector) thread cannot legally read another thread's thread-locals
   or those raw pointers. **Therefore D2 = each parked worker SELF-COLLECTS its own structural roots**
   (`collect_machine_roots` over its own `operand_stack`/`work`/`work_stack`/`continuations` + its
   `collect_k_spine` + its `deferred_shared_drops`) into a shared rendezvous buffer, then signals ready; the
   collector unions the per-worker buffers ∪ E₀ ∪ the driver-C `collect_safepoint_roots` and marks. **Still
   genuinely CESK** (per-worker roots = that machine's registers, read STRUCTURALLY from the registers) — the
   only correction is that the READER of each worker's registers is the worker itself (registers are
   thread-local by construction). NO Arc root-provider, NO publish-by-value, NO drop-worker rooting.

3. **B3 lock-free TLABs were NEVER built — D-TLAB is the real, load-bearing perf work (Inc-5).** No TLAB code
   exists in `cesk/` (`index_heap.rs:29` calls it "Inc 5"; `:758` "replaced by lock-free TLABs" = future
   tense). All 16 `IndexFactory` alloc sites take `global_index_heap().write()` (`index_heap.rs:790…952`) and
   use the arena's `&mut self` free-list `alloc`, NEVER reaching the concurrent `alloc_bump(&self)`. **B2's
   concurrent arena interior is present but INERT — `IndexHeap` serializes ALL allocation behind one
   `RwLock`.** This is exactly the perf gap (`perf`: index PLN slowdown is GC-bound = RwLock-serialized
   `IndexHeap` + no-collection-while-parallel + Addr-indirection). **D-TLAB closes it.**

Plus: **`tla/StoreCentricGC.tla` is STW-at-quiescence, NOT concurrent** — it has the structural-Ψ +
`WorkerEnter`-gated cooperative-drain skeleton (`:288-290`) and proves NoUseAfterFree/NoLostObjects/
SegmentReleaseSafety/QuiescenceInvariant/NoConcurrentFree (`:69-76`), but has NO R14/TLAB/allocate-black/
concurrent-mark. It is the correct base to EXTEND (the parallel transitions are newly DERIVED). Do NOT cite
`SlabGC_Quiescent.tla` (Phase-9 deviation).

## Verified-good (CONFIRMS plan)
- B2 concurrent arena interior is real (`index_arena.rs`): never-realloc `segments` directory (`:351`),
  per-Segment `bump`(claim)/`len`(publish Release CAS `:301`) two-cursor, `alloc(&mut)`/`alloc_bump(&self)`
  split (`:511`/`:574`); B3 mark ordering DONE (AcqRel `set_mark` `:193`, Acquire `is_marked` `:202`).
  `Segment::release(&mut)` + `free_list` consume only at quiescence (`:315`,`:364`).
- Structural root reader real: `collect_machine_roots`(`roots.rs:341`)/`_live`(`:373`)/`collect_persistent_roots`
  (`:404`); the PERMANENT machine-equivalence oracle (`assert_quiescence_superset` `:432`; A4.3 midloop oracle
  `eval_loop.rs:3672`) stays a CI invariant.
- Launder linchpin: `launder`(`index_heap.rs:85`)→`view_at`(`:379`)/`materialize_inner`(`:425`) hand out
  `&'static` into side `Box`es; `free_reclaimed_side_slots` is QUIESCENCE-ONLY (`:584`, rationale `:571-574`)
  — D4 must preserve this across threads via the epoch + launder-name handshake.

## Sub-increments (each independently committable + green; commit at every green sub-increment)
Gate legend: **G-slab** default nextest 4312/0 (every step); **G-idx** index nextest + conf 483/221/40
byte-identical, cycles>0; **G-det** ≥20-run; **G-asan** 0-UAF forcing cycles; **G-mm** mmverify Correct;
**G-tla** TLC invariant; **G-loom**/**G-tsan**; **G-oracle** machine-equivalence oracle. Cap all heavy ops
(`systemd-run -p MemoryMax=… -p MemorySwapMax=0`, FOREGROUND; ASAN `-Zbuild-std` ≤32G), `tee`.

### D-TLAB (the perf lever; RECOMMENDED START) — the real Inc-5

**Explore + Plan correction (2026-06-01): D-TLAB.1 ("read-lock + `alloc_bump` fast path") CANNOT stand
alone.** An Explore pass verified that ALL 8 `IndexHeap::alloc_*` methods + `intern_ground_sexpr` append
variable-length data to a per-segment side arena (`children`/`strings`/`spans`, `index_heap.rs:67-72`) or a
handle table (`space_table`/`memo_table`, `:102-104`) via `&mut Vec::push` (`intern_*_in` `:288-310`) — NONE
is node-only. So the side arenas must become `&self`-appendable FIRST. The corrected sequence (a Plan agent
designed the concurrent structure):

- **D-TLAB-1.0 (FIRST committable; wholly inert)** — add `SideColumn<T>`: a never-realloc chunked directory
  mirroring `IndexArena` (`chunks: Box<[UnsafeCell<MaybeUninit<Box<[UnsafeCell<MaybeUninit<Option<Box<T>>>]>>>]>`,
  `chunk_count`/`bump`/`len` atomics, `grow_lock`), with `push(&self)→u32` (Relaxed claim → write `Box` →
  Release-CAS publish, verbatim the arena `publish` `index_arena.rs:301`), `get(&self,idx)` (Acquire), `free(&mut)`
  (quiescence-only, sets `None`, never recycles). + `#[cfg(test)]` single-threaded unit tests. NOT referenced
  yet. Gate: G-slab + G-idx (compile + tests). **Chosen over a parallel side-`Segment`** because side indices
  are an independent monotone per-field counter, never recycled, decoupled from node-slot reuse.
- **D-TLAB-1.1** — swap the three `SegmentSideArenas` `Vec<Option<Box<…>>>` fields to `SideColumn`; rewrite
  `intern_*_in`/`children`/`str_slice`/`span_at`/`view_at`/`materialize_inner`/`mark`/`mark_young`/
  `free_reclaimed_side_slots` to use `SideColumn` — but keep every `intern_*_in` **`&mut self`** (sound via `&mut`).
  Move `sides` to a never-realloc directory + `sides_count`. Single-threaded ⇒ byte-identical. Gate: +G-asan
  (`c_A_asan.sh` 3 arms — side-free now over `SideColumn::free`) +G-det.
- **D-TLAB-1.2 (concurrency-capability gate)** — flip `intern_*_in`/`ensure_side_seg` to `&self` (lazy
  Acquire-checked `sides` growth under `sides_dir_lock`); add `alloc_*_bump(&self)` that **choose `seg` ONCE**
  and use a `bump_in`-returns-`Option` **retry** for the co-location TOCTOU (the latent double-`ensure_bump_room`
  `index_heap.rs:191-193` that only bites concurrent — side append into seg_A, node bumps seg_B after a racing
  `open_segment`); an orphaned side entry is the no-recycle steady state. **Side-publish BEFORE node-publish**:
  the node's existing Release-CAS transitively gates side visibility (no new fence). `IndexFactory` still calls
  the `&mut` reuse-or-bump path — NO `.read()` caller yet (byte-identical). **Handle tables + `hash_cons` stay
  `&mut`/`.write()`** (rare; not the hot path). **Free-list stays `&mut`/quiescence** (preserves the arena's
  ABA-freedom proof + determinism; concurrent alloc claims fresh bump only). Gate: +**G-tla** (new
  `SideAppendThenPublish` split-`Alloc` + invariant `NoObservableNodeNamesUnpublishedSide`) +**G-loom**
  (`loom_side_append_before_node_publish`) +**G-tsan** +G-asan.

The **read-lock fast path** (flip `IndexFactory` to `.read()` + `alloc_*_bump`) is the SUBSEQUENT increment,
out of scope here — D-TLAB-1.0..1.2 unblock it. Full design: dispatched-Plan-agent output (this session).

### D1 — fresh STW rendezvous (built, gated OFF → byte-identical)
- **D1.1** — `GC_STATE: AtomicU64` (epoch‖phase‖requested) + helpers in a NEW `cesk` rendezvous module (slab
  `gc_allocator.rs` untouched). Gate: G-slab + G-idx (additive) + G-tla (bind to existing `phase`/`gcRequested`).
- **D1.2** — cooperative safepoint poll + bounded park at the midloop safepoint (`eval_loop.rs:3599`) + long
  grounded-op/MORK regions: on `is_gc_requested()`, worker self-roots (D2) + TLAB-flush + `drop_eval_guard_
  for_safepoint` + park until `phase=mutating` + `reacquire`. Gate: G-slab+G-idx+G-det + **G-tla** (re-validate
  `QuiescenceInvariant` for the REBUILT rendezvous — the "re-validate, don't trust SlabGC_Quiescent" item) +
  **G-loom** (park/unpark). Collector still runs only at quiescence + gate not flipped ⇒ G-idx byte-identical.

### D2 — worker-self-rooting at the rendezvous (the CESK parallelism realization)
- **D2.1** — rendezvous-scoped `RootHandoff`; each parking worker appends its structural roots; collector
  unions per-worker ∪ E₀ ∪ driver-C when `parked_count==ACTIVE_EVALUATORS_at_request`. Invariant (CESK
  theorem): union = `⋃_i σ|_Reachable(⟨C_i,E_i,K_i⟩) ∪ reach(E₀)`. Gate: G-idx+G-det + **G-oracle** generalized
  to the multi-worker union (permanent CI) + G-asan.

### D3 — parallel mark + parallel sweep
- **D3.1** parallel mark (CCD-partitioned by `Addr.segment()`, crossbeam-deque, prefetch; AcqRel marks).
  Gate: G-idx+G-det + **G-tla** (`ParallelMark`, `NoLostObjects` multi-marker) +G-loom +G-tsan +G-asan.
- **D3.2** parallel sweep (per-segment; reuse word-parallel `all_marked`/`all_dead` `index_arena.rs:228`; STW
  after mark). Gate: same.

### D4 — concurrent dead-segment release behind the unified-epoch + launder handshake (highest risk after D5)
- **D4.1** epoch-stamp dead segments (`dead_epoch=gc_epoch()`) instead of immediate `release`.
- **D4.2** release only when `dead_epoch < min over threads observed_epoch` AND no live laundered `&'static`
  names the segment (each worker clears `INNER_SHADOW`/drops stack `ValueView`s before advancing observed_epoch).
  Gate: G-idx+G-det + **G-asan** (force release while a sibling holds a `materialize_inner`) + debug-assert +
  **G-tla** (`SegmentRelease` refined + new `NoLaunderDangle`) +G-tsan.

### D5 — flip the gate (the one irreversible concurrency-gate flip; FULL gate)
- **D5.1** drop `!worker_ever_spawned()` on the parallel path (`gate_open_midloop` `index_heap.rs:1263`); keep
  FANOUT=0 on the single-threaded gate for byte-identical shipping until F3; fix the stale `gate_open_midloop`
  doc (`:1252-1261`). Gate (load-bearing): G-slab + G-idx + **G-det ≥20-run (parallel superpose/collapse +
  forced sweep)** + **G-asan FANOUT_DEPTH>0 + parallel stress 0-UAF** + **G-mm** + HE-bisim 40/40 + PLN budgets +
  **G-tla (fully-extended StoreCentricGC)** + **G-loom + G-tsan**; then re-measure throughput at FANOUT>0 (F1).

## Verification plan
- **Extend `tla/StoreCentricGC.tla`** (not SlabGC_Quiescent): add `AllocBump` (D-TLAB.1, re-prove
  NoConcurrentFree under `~mut` alloc), `R14_TlabFlushBeforePark`+`AllPublishedBeforeMark` (D-TLAB.3),
  `ParallelMark` multi-pid (D3.1, re-prove NoLostObjects), refined `SegmentRelease`+`NoLaunderDangle` (D4),
  re-validate `QuiescenceInvariant` for the rebuilt D1 rendezvous. Run via `MC_StoreCentricGC*.cfg`, record
  `tla/RESULTS.md`, capped.
- **loom**: D1.2 park/unpark lost-wakeup; D-TLAB.2/.3 side-publish + TLAB-flush ordering; D3.1 mark bitmap.
- **TSan**: D-TLAB.2/.3 concurrent alloc; D3 parallel mark/sweep; D4 release-vs-laundered-read; D5 full path.
- **ASAN**: D4 (release under live launder); D5 load-bearing FANOUT>0 + parallel stress.

## Risk register → proof obligation
| Risk | Fix (inc) | Proof |
|---|---|---|
| Concurrent UAF (mark/sweep races mutator) | STW at rebuilt rendezvous (D1.2) | QuiescenceInvariant re-validated + TSan + ASAN FANOUT>0 |
| Worker registers unreadable by collector (`k_spine.rs:81-88`) | worker SELF-roots (D2.1) | G-oracle generalized to multi-worker union |
| No reusable STW rendezvous (`gc_allocator.rs:4218` deleted) | build fresh from EvalGuard/condvar | new QuiescenceInvariant + loom park/unpark |
| Launder `&'static` dangle at concurrent free (`index_heap.rs:571-574`) | epoch + launder-name handshake (D4.2) | NoLaunderDangle TLA + ASAN + debug-assert |
| RwLock-serialized alloc inert-izes B2 (perf gap) | alloc_bump + TLABs (D-TLAB.1–3) | AllocBump+R14 TLA + Welch re-measure (F1) |
| allocate-black ⊗ generational young-mark | parallel mark is FULL at rendezvous (no concurrent alloc-black — that's Phase E SATB) | C young-only-mark theorem untouched + G-tla |
| Determinism w/ Addr-reuse across threads | free-list reuse stays at quiescence; concurrent alloc claims fresh bump | ≥20-run incl. parallel superpose + forced sweep |
| Slab regressed | new machinery in `cesk`; slab paths cfg-walled | G-slab 4312/0 every commit |

## Recommended START: D-TLAB-1.0 (the `SideColumn<T>` type + unit tests, inert)
The Explore pass moved the start: the read-lock fast path is blocked until the side arenas are
`&self`-appendable, so the first committable increment is the wholly-inert `SideColumn<T>` data structure +
its single-threaded unit tests (byte-identical trivially — nothing references it yet; commits green on
G-slab + G-idx). It introduces the concurrent structure with zero behavioral coupling, then D-TLAB-1.1
(swap fields, keep `&mut`) and D-TLAB-1.2 (the `&self` flip + co-location single-pick, gated by
TLA+/loom/TSan/ASAN) follow. This attacks the verified perf root cause (RwLock-serialized `IndexHeap`; all 16
`.write()` alloc sites never reach `alloc_bump`) while leaving the irreversible D5 gate flip last.
