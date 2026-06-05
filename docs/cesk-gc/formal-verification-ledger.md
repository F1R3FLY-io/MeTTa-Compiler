# CESK GC formal verification ledger

This ledger tracks the mechanically checked proof artifacts for the CESK-based `index-gc` collector. It is not about
the legacy slab mark-sweep collector.

## Verified implementation boundary

The default live collector verified here is the CESK-based generational `index-gc` collector's E1 path:
single-threaded quiescence and rendezvous collection compute structural roots, then mark and sweep while holding the
index heap write lock. Mid-loop collection remains opt-in (`METTATRON_INDEX_GC_MIDLOOP=1`): the theorem covers it when
the caller supplies a complete structural root set, but the default verified boundary does not claim that opt-in path is
shipped-on until its separate ASAN/root-completeness gate is green. There is also an opt-in E2 SATB major path
(`METTATRON_INDEX_GC_SATB=1`): the dedicated GC thread
uses the same witness/root-union rendezvous to capture the initial structural roots, arms SATB deletion barriers and
allocate-black, releases workers while it marks under a shared heap read lock, then requests a second rendezvous,
waits out in-flight deletion barriers by dropping the SATB guard, and performs a full-major final mark/sweep under the
heap write lock. If that SATB rendezvous aborts, or if the final sweep reports that its rendezvous witness gate closed,
RAII closes any open rendezvous/request and the driver immediately runs a fresh normal STW rendezvous. E2 SATB
currently sweeps as a full major only; young-only SATB needs a separate proof that old marks introduced by SATB
barriers cannot remain stale across a minor.

## Checked obligations

- `formal/rocq/gc/FreeList.v` and `formal/lean/gc/FreeList.lean`: the R-FL free-list lifecycle preserves
  `free_bit(addr) set <=> addr is on free_list` and free-list `NoDup` across push, pop, major drain, and
  released-segment drain.
- `formal/rocq/gc/YoungMark.v` and `formal/lean/gc/YoungMark.lean`: no-old-to-young plus young-root marking and
  young-edge closure implies every reachable young node is marked and retained by a minor sweep. They also prove the
  stronger conservative-minor theorem used by the implementation now: if the marker traverses the whole reachable
  graph and marks every young node it sees, every reachable young node is retained without assuming old nodes have no
  young descendants.
- `formal/rocq/gc/StructuralRoots.v` and `formal/lean/gc/StructuralRoots.lean`: if future machine touches are
  inside the structural CESK-root closure and sweep frees only unmarked nodes, no future-touched node can be freed.
- `formal/rocq/gc/RendezvousWitness.v` and `formal/lean/gc/RendezvousWitness.lean`: if every occupied witness slot
  is published, publication buffers that slot's structural roots, and the driver drains the buffer, every occupied
  participant root is in the driver root set and cannot be freed after mark/sweep.
- `formal/rocq/gc/ThreadContribution.v` and `formal/lean/gc/ThreadContribution.lean`: if the canonical
  per-mutator contribution reader includes every component it claims (trampoline extra values, S/C/K, E0, global
  anchors, K-spine, deferred env roots; tier-leaf extra values plus env-less persistent roots), and publication/drain
  carries that contribution to the driver root set, mark/sweep cannot free any component root.
- `formal/rocq/gc/DriverCPublication.v` and `formal/lean/gc/DriverCPublication.lean`: if eval-entry publication
  maps every caller-held driver-C root into the driver/safepoint root set, root-complete mark and sweep safety retain
  every such driver-C root.
- `formal/rocq/gc/CESKCollectorSafety.v`: composes the rendezvous witness, collector-root closure, mark completeness,
  sweep-only-unmarked, driver-C publication, and young-minor obligations into explicit no-UAF theorems for participant
  roots, caller-held driver-C roots, future CESK touches, reachable young nodes under both the no-old-to-young and
  conservative-minor traversals, E2 snapshot-live nodes covered by initial roots, driver roots, SATB shades, or
  allocate-black publication, E2 freshly published allocate-black allocations, E2 final-rendezvous roots and
  abort-to-STW finalization, E2 snapshot-live values removed by value-bearing E0 cache capacity eviction, overwrite,
  and bulk clear, and E2 snapshot-live values removed from the pinned value-bearing E0 mutation categories.
- `formal/rocq/gc/SATB.v` and `formal/lean/gc/SATB.lean`: prove the E2 concurrent-mark SATB obligation: if
  snapshot-live values are covered by initial roots, final-rendezvous driver roots, shaded deletion pre-images, or
  allocate-black roots, sweep cannot free them. They also state the final-rendezvous driver-root theorem directly:
  a root captured by the final remark cannot be freed by the exclusive sweep.
- `formal/rocq/gc/AllocateBlack.v` and `formal/lean/gc/AllocateBlack.lean`: bridge the E2 allocate-black publication
  order into the SATB theorem. If a freshly published allocation is black before publication makes it visible, then it
  is a SATB root and cannot be freed by sweep; the direct mark-before-publish theorem mirrors the TLA discriminator.
- `formal/rocq/gc/SATBFinalization.v` and `formal/lean/gc/SATBFinalization.lean`: bridge the E2 finalization
  obligations into the SATB safety story. Final-rendezvous roots survive because they are re-marked before the
  exclusive sweep; if the final sweep gate is closed, the checked result must run the STW backstop; and an aborted SATB
  request is handled only after a freshly requested STW rendezvous runs.
- `formal/rocq/gc/E0MutationSites.v` and `formal/lean/gc/E0MutationSites.lean`: bridge the E2 value-bearing E0
  mutation-site enumeration into the SATB theorem. If every removed pre-image from the pinned space-local, rule-index,
  and environment/token/state categories is shaded, then any snapshot-live value removed through those E0 categories is
  a SATB root and cannot be freed by sweep.
- `formal/rocq/gc/E0EvictionBarriers.v` and `formal/lean/gc/E0EvictionBarriers.lean`: bridge the E2 value-bearing E0
  cache eviction and bulk-clear shapes into the SATB theorem. If capacity victims, same-key overwrite victims, and
  bulk-cleared entries are shaded before removal becomes invisible, then those removed snapshot-live values are SATB
  roots and cannot be freed by sweep.
- `tla/RendezvousWitness.tla`: checks the E1 witness gate predicate. The strict `published>=cur_gen OR
  acquired>cur_gen` model preserves root completeness at sweep; the negative `acquired>=cur_gen` model violates it.
- `tla/WitnessSlotLifecycle.tla`: checks the V4 slot lifecycle. Keeping the slot occupied across safepoint drop
  preserves live-machine visibility at sweep; the negative release-on-safepoint model violates it.
- `tla/DriverRootUnion.tla`: checks the E1 driver root-union channels. Including worker-buffer, safepoint,
  live-env/E0, and live-dispatch channels preserves root-union completeness; omitting live-env or live-dispatch
  violates it.
- `tla/DriverCPublication.tla`: checks that public eval entry publishes driver-C (`MettaState.source/output`) to
  the safepoint channel before midloop/rendezvous roots can be built. Omitting that publication violates
  `DriverCVisibleOnSweep`, matching a caller-held source/output value that can be freed while eval is still live.
- `tla/StartedCycleGate.tla`: checks the E5 straddle gate. Gating re-park on `GC_CYCLE_STARTED` avoids phantom
  re-parks during teardown; gating on `GC_CYCLE_GEN` violates `NoPhantomRepark`.
- `tla/WitnessOkReset.tla`: checks the cross-cycle witness flag reset. Clearing `CURRENT_WITNESS_OK` at cycle end
  prevents the previous cycle's true flag from admitting a next-cycle sweep before the next witness wait.
- `tla/CurSegReuseOrder.tla`: checks the historical skipped-old young-marker allocator premise. Cur-segment-only
  reuse preserves bump order; any-young reuse admits an old-parent to young-child edge after promotion. The live
  minor safety proof is `ConservativeMinorMark`, not this narrower premise.
- `tla/ConservativeMinorMark.tla`: checks the C1 first-class-space correction. Traversing old reachable containers
  during a minor preserves a young value reachable through an old `SpaceHandle`; the old skipped-old traversal violates
  `YoungReachableMarked`.
- `tla/SATBDeletionBarrier.tla`: checks the E2 Yuasa deletion-barrier obligation. Shading the removed pre-image
  preserves snapshot-live safety; omitting the barrier frees a snapshot-live value.
- `tla/SATBE0MutationSites.tla`: checks the E2 deletion-barrier obligation at the value-bearing E0 subcontainer
  level. Space-local roots, rule-index entries, and environment/token/state roots must each shade the removed
  pre-image; disabling any one category violates `NoE0SnapshotLiveFreed`.
- `tla/AllocateBlackPublish.tla`: checks the E2 allocate-black publication order. Marking before publication
  preserves safety; publishing first allows a visible allocation to be swept.
- `tla/SATBLRUEviction.tla`: checks the E2 LRU SATB barrier shape. Shading capacity-evicted victims preserves
  snapshot-live safety; a same-key put-return-only barrier misses capacity victims.
- `tla/SATBBulkClear.tla`: checks the E2 bulk-clear SATB barrier shape. Shading every removed pre-image before
  clearing a value-bearing E0 cache preserves snapshot-live safety; clearing without shading frees one.
- `tla/SATBPhaseGate.tla`: checks the E2 marker-start/deletion race. A read/write phase gate prevents marker
  start from straddling a deletion that observed "not marking"; without the gate a snapshot-live entry can be freed.
- `tla/SATBSweepGate.tla`: checks the E2 sweep/deletion race. Sweep must wait for in-flight deletion barriers
  after mark completion; otherwise a deletion can remove a snapshot-live E0 entry before shading it and sweep can free
  the pre-image before the shade becomes visible.
- `tla/SATBFinalRemark.tla`: checks the E2 final-rendezvous remark. Roots captured after the concurrent mark window
  must be re-marked before the exclusive sweep; omitting that remark frees a final root that was not in the initial
  snapshot.
- `tla/SATBFinalSweepResult.tla`: checks the E2 final-sweep result obligation. If the final sweep's rendezvous gate is
  unexpectedly closed, the driver must treat the false result as a SATB abort and run the STW backstop; ignoring the
  result lets the request finish without either sweeping or falling back.
- `tla/SATBAbortFallback.tla`: checks the E2 abort-to-STW backstop. If the SATB path aborts after cleanup, the
  driver must re-request and run a fresh STW rendezvous before treating the request as handled.

## Source coupling

`scripts/verify_cesk_gc_source_coupling.sh` is run by `scripts/verify_cesk_gc_formal.sh`. It pins the source-side
facts the proofs rely on:

- `ROOT_REGISTRY`, `RootProvider`, and `frame_chain` remain slab-only and are not index-root discovery channels.
- The E1 driver waits on `requestor_wait_for_all_reified_parked`, then sets `current_witness_ok`, builds the root
  union, runs the rendezvous-union oracle, and only then calls `run_collection_if_triggered_rendezvous`.
- `gate_open_rendezvous` is keyed by `current_witness_ok`, not the obsolete parked-count gate.
- Live envs and parallel fan-outs are registered through RAII handles, and the live-env/live-dispatch registry walks
  delegate to the structural `EnvRoots`/`DispatchRoots` readers used by the driver-root-union proof.
- Every self-root publication site routes through the canonical `collect_complete_thread_contribution` reader, whose
  source shape is pinned: trampoline participants publish extra hot values, live S/C/K, E0, global anchors, K-spine,
  and deferred env roots; tier leaves publish extra VM/JIT values plus the env-less persistent roots they can read.
- Both public eval boundaries (`eval` and `eval_with_tier`) publish `MettaState.source/output` into
  `SAFEPOINT_ROOTS` before the live transition begins, so midloop/rendezvous collection sees the caller's driver-C
  even when an outer CLI/REPL/conformance loop has not installed its own batch guard.
- OPERATOR_CACHE is guarded by `gc_sweep_epoch` in index mode before pointer-keyed lookup, so a parked worker
  self-invalidates after another thread completes a sweep.
- Value-bearing E0 LRU anchors (`EVAL_MEMO`, `MATCH_RESULT_CACHE`, and `BYTECODE_CACHE`) use `LruCache::push`
  rather than `put` on eviction-capable paths, and the surfaced victim is conservatively SATB-shaded in index mode.
- Value-bearing E0 bulk clears shade all cached roots before clearing during an active SATB mark, and the shading
  primitive is gated by `satb_marking_in_progress` so ordinary cycles cannot leave stale mark bits for a later mark.
- Value-bearing E0 deletion/eviction paths run under `with_satb_deletion_barrier`: marker start/end takes the write
  side while flipping `SATB_MARKING_DEPTH`, and cache deletion takes the read side around check, shade, and delete.
- The non-cache value-bearing E0 mutation sites are source-pinned too: symbol binding overwrite, mutable-state
  overwrite, named-space removal, type-vector removal, tokenizer remove/clear, ACT overlay variable clear,
  `SpaceHandle` variable-atom removal, `ModuleSpace` atom remove/clear, and `RuleIndex` rule remove/clear all shade
  the actual removed pre-image under the SATB phase gate.
- Fresh bump allocation realizes allocate-black for E2 SATB: `IndexArena` writes the claimed slot, marks it if
  `satb_marking_in_progress`, and only then publishes the slot through `len`.
- The E2 SATB marker path is source-coupled: `gc_driver_satb_rendezvous_cycle` arms `enter_satb_marking`, closes the
  initial rendezvous before `mark_concurrent_roots`, requests a final rendezvous, drops the SATB guard before
  `sweep_after_concurrent_mark`, asserts that the final sweep actually ran before dropping the final roots, and has
  RAII cleanup for an open rendezvous/request on panic.
- The E2 abort path is source-coupled: `gc_driver_rendezvous_cycle` checks the SATB `catch_unwind` result, and an
  abort calls `gc_driver_stw_rendezvous_cycle`, which re-issues `request_gc`, acquires a fresh rendezvous, prepares
  structural roots, runs the normal STW rendezvous collection, drops roots, and then closes the cycle.
- `mark_concurrent_roots` marks under `global_index_heap().read()` through `IndexHeap::mark_concurrent`; the final E2
  sweep takes `global_index_heap().write()`, re-marks the final rendezvous roots, runs a full `heap.sweep()`, then
  promotes and clears all mark bits. The FANOUT trigger is suppressed while `satb_marking_in_progress()`.
- The rooted global bytecode `MemoCache<MettaValue>` shades overwritten, LRU-evicted, and bulk-cleared cached results
  under the same SATB phase gate; non-`MettaValue` generic cache instantiations do not contribute index roots.
- The rooted global space registry shades the `SpaceHandle::collect_gc_values` roots for overwritten, removed, and
  bulk-cleared spaces under the same SATB phase gate.
- The rooted compiler atom statics are exactly three `OnceLock<MettaValue>` write-once anchors; the source-coupling
  gate asserts structural reads and no reset/take/set deletion path.
- The rooted tiered compilation cache shades pending bytecode source roots on overwrite, cancellation, task-drop, and
  guard drop, and shades pending roots plus compiled bytecode constants before full cache clear.
- The rooted thread-local subgoal and thunk tables shade cached result values on stale eviction, overwrite, explicit
  removal, invalidation, full clear, and thunk result replacement.
- R-FL source order keeps push guarded by `set_free_bit`, pop clearing the bit before reuse/discard, and released
  segments draining listed entries before dropping the segment bitmap.
- C1 source order keeps reuse current-segment-only, successful bump allocation guarded by the current segment, segment
  retargeting monotone, and promotion at `current_seg`. `IndexHeap::mark_young` marks only young nodes but traverses
  all reached nodes through `child_addrs_for_mark`, including `SpaceHandle::collect_gc_values`, so an old first-class
  space cannot hide a live young value from `sweep_young`.
- `published_gen` writes remain restricted to stale-stamp reset plus the genuine `note_reified_park` stamp, with
  worker root-buffer publication before the stamp.
- The V4 witness slot is acquired before `N_THREADS++`, released only after the true outermost `EvalGuard::drop`
  count decrement, never released by safepoint drops, and re-stamped before straddle re-park publication.
- The E5 straddle loop gates on `current_cycle_started()`, and the driver sets it after admission closes and before
  the witness wait.
- `end_rendezvous_cycle` clears `CURRENT_WITNESS_OK` after the gen bump and before the rendezvous notify; the driver
  runs that teardown before dropping `GC_IN_PROGRESS`.

## Harness

Run:

```bash
bash scripts/verify_cesk_gc_formal.sh
```

The harness derives paths from its own location, uses `target/tlc-formal-small` for small TLC logs/metadata by
default, runs Rocq under `systemd-run`, and includes the small TLC positive/negative discriminators.
