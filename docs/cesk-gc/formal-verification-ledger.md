# CESK GC formal verification ledger

This ledger tracks the mechanically checked proof artifacts for the CESK-based `index-gc` collector. It is not about
the legacy slab mark-sweep collector.

## Checked obligations

- `formal/rocq/gc/FreeList.v` and `formal/lean/gc/FreeList.lean`: the R-FL free-list lifecycle preserves
  `free_bit(addr) set <=> addr is on free_list` and free-list `NoDup` across push, pop, major drain, and
  released-segment drain.
- `formal/rocq/gc/YoungMark.v` and `formal/lean/gc/YoungMark.lean`: no-old-to-young plus young-root marking and
  young-edge closure implies every reachable young node is marked and retained by a minor sweep.
- `formal/rocq/gc/StructuralRoots.v` and `formal/lean/gc/StructuralRoots.lean`: if future machine touches are
  inside the structural CESK-root closure and sweep frees only unmarked nodes, no future-touched node can be freed.
- `formal/rocq/gc/RendezvousWitness.v` and `formal/lean/gc/RendezvousWitness.lean`: if every occupied witness slot
  is published, publication buffers that slot's structural roots, and the driver drains the buffer, every occupied
  participant root is in the driver root set and cannot be freed after mark/sweep.
- `formal/rocq/gc/CESKCollectorSafety.v`: composes the rendezvous witness, collector-root closure, mark completeness,
  sweep-only-unmarked, and young-minor obligations into explicit no-UAF theorems for participant roots, future CESK
  touches, reachable young nodes, and E2 snapshot-live nodes covered by initial roots, driver roots, SATB shades, or
  allocate-black publication.
- `formal/rocq/gc/SATB.v` and `formal/lean/gc/SATB.lean`: prove the E2 concurrent-mark SATB obligation: if
  snapshot-live values are covered by initial roots, shaded deletion pre-images, or allocate-black roots, sweep cannot
  free them.
- `tla/RendezvousWitness.tla`: checks the E1 witness gate predicate. The strict `published>=cur_gen OR
  acquired>cur_gen` model preserves root completeness at sweep; the negative `acquired>=cur_gen` model violates it.
- `tla/WitnessSlotLifecycle.tla`: checks the V4 slot lifecycle. Keeping the slot occupied across safepoint drop
  preserves live-machine visibility at sweep; the negative release-on-safepoint model violates it.
- `tla/DriverRootUnion.tla`: checks the E1 driver root-union channels. Including worker-buffer, safepoint,
  live-env/E0, and live-dispatch channels preserves root-union completeness; omitting live-env or live-dispatch
  violates it.
- `tla/StartedCycleGate.tla`: checks the E5 straddle gate. Gating re-park on `GC_CYCLE_STARTED` avoids phantom
  re-parks during teardown; gating on `GC_CYCLE_GEN` violates `NoPhantomRepark`.
- `tla/WitnessOkReset.tla`: checks the cross-cycle witness flag reset. Clearing `CURRENT_WITNESS_OK` at cycle end
  prevents the previous cycle's true flag from admitting a next-cycle sweep before the next witness wait.
- `tla/CurSegReuseOrder.tla`: checks the C1 no-old-to-young premise for young-only minor marking. Cur-segment-only
  reuse preserves bump order; any-young reuse admits an old-parent to young-child edge after promotion.
- `tla/SATBDeletionBarrier.tla`: checks the E2 Yuasa deletion-barrier obligation. Shading the removed pre-image
  preserves snapshot-live safety; omitting the barrier frees a snapshot-live value.
- `tla/AllocateBlackPublish.tla`: checks the E2 allocate-black publication order. Marking before publication
  preserves safety; publishing first allows a visible allocation to be swept.
- `tla/SATBLRUEviction.tla`: checks the E2 LRU SATB barrier shape. Shading capacity-evicted victims preserves
  snapshot-live safety; a same-key put-return-only barrier misses capacity victims.
- `tla/SATBBulkClear.tla`: checks the E2 bulk-clear SATB barrier shape. Shading every removed pre-image before
  clearing a value-bearing E0 cache preserves snapshot-live safety; clearing without shading frees one.
- `tla/SATBPhaseGate.tla`: checks the E2 marker-start/deletion race. A read/write phase gate prevents marker
  start from straddling a deletion that observed "not marking"; without the gate a snapshot-live entry can be freed.

## Source coupling

`scripts/verify_cesk_gc_source_coupling.sh` is run by `scripts/verify_cesk_gc_formal.sh`. It pins the source-side
facts the proofs rely on:

- `ROOT_REGISTRY`, `RootProvider`, and `frame_chain` remain slab-only and are not index-root discovery channels.
- The E1 driver waits on `requestor_wait_for_all_reified_parked`, then sets `current_witness_ok`, builds the root
  union, runs the rendezvous-union oracle, and only then calls `run_collection_if_triggered_rendezvous`.
- `gate_open_rendezvous` is keyed by `current_witness_ok`, not the obsolete parked-count gate.
- Live envs and parallel fan-outs are registered through RAII handles, and the live-env/live-dispatch registry walks
  delegate to the structural `EnvRoots`/`DispatchRoots` readers used by the driver-root-union proof.
- OPERATOR_CACHE is guarded by `gc_sweep_epoch` in index mode before pointer-keyed lookup, so a parked worker
  self-invalidates after another thread completes a sweep.
- Value-bearing E0 LRU anchors (`EVAL_MEMO`, `MATCH_RESULT_CACHE`, and `BYTECODE_CACHE`) use `LruCache::push`
  rather than `put` on eviction-capable paths, and the surfaced victim is conservatively SATB-shaded in index mode.
- Value-bearing E0 bulk clears shade all cached roots before clearing during an active SATB mark, and the shading
  primitive is gated by `satb_marking_in_progress` so ordinary cycles cannot leave stale mark bits for a later mark.
- Value-bearing E0 deletion/eviction paths run under `with_satb_deletion_barrier`: marker start/end takes the write
  side while flipping `SATB_MARKING_DEPTH`, and cache deletion takes the read side around check, shade, and delete.
- The rooted global bytecode `MemoCache<MettaValue>` shades overwritten, LRU-evicted, and bulk-cleared cached results
  under the same SATB phase gate; non-`MettaValue` generic cache instantiations do not contribute index roots.
- The rooted global space registry shades the `SpaceHandle::collect_gc_values` roots for overwritten, removed, and
  bulk-cleared spaces under the same SATB phase gate.
- The rooted tiered compilation cache shades pending bytecode source roots on overwrite, cancellation, task-drop, and
  guard drop, and shades pending roots plus compiled bytecode constants before full cache clear.
- The rooted thread-local subgoal and thunk tables shade cached result values on stale eviction, overwrite, explicit
  removal, invalidation, full clear, and thunk result replacement.
- R-FL source order keeps push guarded by `set_free_bit`, pop clearing the bit before reuse/discard, and released
  segments draining listed entries before dropping the segment bitmap.
- C1 source order keeps reuse current-segment-only, successful bump allocation guarded by the current segment, segment
  retargeting monotone, promotion at `current_seg`, and young minor marking restricted to young roots/children.
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
