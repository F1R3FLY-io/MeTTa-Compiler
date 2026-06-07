# CESK GC formal verification ledger

This ledger tracks the mechanically checked proof artifacts for the CESK-based `index-gc` collector. It is not about
the legacy slab mark-sweep collector.

## Verified implementation boundary

The default live collector verified here is the CESK-based generational `index-gc` collector's E1 path:
single-threaded quiescence and rendezvous collection compute structural roots, then mark and sweep while holding the
index heap write lock. Mid-loop collection remains opt-in (`METTATRON_INDEX_GC_MIDLOOP=1`): the mid-loop root-union
theorem, TLC discriminator, and source-coupling checks pin its live S/C/K, E0/global/K-spine, deferred-env, and driver-C
root vector, but the default verified boundary does not claim that opt-in path is shipped-on until its separate
ASAN/root-completeness gate is green. There is also an opt-in E2 SATB major path
(`METTATRON_INDEX_GC_SATB=1`): the dedicated GC thread
uses the same witness/root-union rendezvous to capture the initial structural roots, arms SATB deletion barriers and
allocate-black, releases workers while it marks under a shared heap read lock, then requests a second rendezvous,
waits out in-flight deletion barriers by dropping the SATB guard, and performs a full-major final mark/sweep under the
heap write lock. If that SATB rendezvous aborts, or if the final sweep reports that its rendezvous witness gate closed,
RAII closes any open rendezvous/request and the driver immediately runs a fresh normal STW rendezvous. E2 SATB
currently sweeps as a full major only. The formal/source-coupling gate pins that boundary: a final SATB sweep uses
`heap.sweep()`, not `sweep_young`, and the full sweep clears every SATB mark before promotion. The formal harness also
contains a stale-old-mark discriminator: a young-only SATB final sweep violates `NoStaleOldMark` when an old SATB mark
exists. A future young-only SATB path must therefore prove old SATB marks are absent or explicitly cleared before it
can replace the full-major final sweep.

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
- `formal/rocq/gc/NodeEdgeCompleteness.v` and `formal/lean/gc/NodeEdgeCompleteness.lean`: prove the node-edge
  completeness obligation. If the marker's concrete reader covers every semantic index-node edge class
  (inline handle fields, side-arena `SExpr`/`Conjunction` children, and first-class `SpaceHandle` contents), then
  ordinary mark/sweep cannot free anything reachable through those semantic node edges.
- `formal/rocq/gc/MidloopRootUnion.v` and `formal/lean/gc/MidloopRootUnion.lean`: prove the opt-in single-threaded
  mid-loop root-union obligation. If live S/C/K, E0, global anchors, K-spine, deferred environment drops, and driver-C
  safepoint roots are included in the mid-loop root vector, mark/sweep cannot free any channel root, and future touches
  reachable from that union survive collection.
- `formal/rocq/gc/RendezvousWitness.v` and `formal/lean/gc/RendezvousWitness.lean`: if every occupied witness slot
  is published, publication buffers that slot's structural roots, and the driver drains the buffer, every occupied
  participant root is in the driver root set and cannot be freed after mark/sweep.
- `formal/rocq/gc/WitnessSlotLifecycle.v` and `formal/lean/gc/WitnessSlotLifecycle.lean`: prove the V4 witness-slot
  lifecycle obligation. If live machines remain occupied or buffered, and sweep can proceed only for unoccupied or
  buffered machines, then a swept live machine must have buffered roots. Safepoint drops preserve visibility only when
  they do not release the occupied witness slot.
- `formal/rocq/gc/WitnessOkReset.v` and `formal/lean/gc/WitnessOkReset.lean`: prove the cross-cycle witness-ok reset
  obligation. Clearing the non-generational witness gate blocks collection before the next fresh witness proof, and a
  collection allowed by a fresh same-cycle witness cannot be a stale-witness collection.
- `formal/rocq/gc/StartedCycleGate.v` and `formal/lean/gc/StartedCycleGate.lean`: prove the E5 started-cycle
  straddle-gate obligation. If re-park is gated by `GC_CYCLE_STARTED > my_reparked_gen`, a teardown-only generation
  bump cannot cause a phantom re-park before the next driver starts.
- `formal/rocq/gc/CollapseCompletion.v` and `formal/lean/gc/CollapseCompletion.lean`: prove the E1 parallel
  dispatch/collapse completion obligation. If every spawned worker exits and the RAII completion guard drops on both
  normal and panic-unwind exits, then the parent wait cannot be stranded by a skipped worker decrement; the companion
  theorem also captures the historical negative shape where a panic edge skips completion and parent observation is
  impossible.
- `formal/rocq/gc/WorkerAdmission.v` and `formal/lean/gc/WorkerAdmission.lean`: prove the E1 worker-admission
  obligation. If collection admission is closed before the participant snapshot and no worker can join during the
  collection window, then every worker live at sweep was in the snapshot and is retained by ordinary mark/sweep.
- `formal/rocq/gc/ThreadContribution.v` and `formal/lean/gc/ThreadContribution.lean`: if the canonical
  per-mutator contribution reader includes every component it claims (trampoline extra values, S/C/K, E0, global
  anchors, K-spine, deferred env roots; tier-leaf extra values plus env-less persistent roots), and publication/drain
  carries that contribution to the driver root set, mark/sweep cannot free any component root.
- `formal/rocq/gc/FrameEnvRoots.v` and `formal/lean/gc/FrameEnvRoots.lean`: prove the forked-env frame-root
  obligation. If the frame-local reader includes bindings, type assertions, state cells, named-space atoms, and
  inferred function type roots from a live forked environment, and that frame root is published through the normal
  thread contribution, mark/sweep cannot free those fork-local values.
- `formal/rocq/gc/TierLeafExtraRoots.v` and `formal/lean/gc/TierLeafExtraRoots.lean`: prove the tier-leaf
  register-root obligation. If VM/JIT register-file values are included in the `extra` part of the worker's tier-leaf
  contribution before the worker parks, and that contribution is published/drained/marked, mark/sweep cannot free
  those tier-local values.
- `formal/rocq/gc/IndexArenaPublication.v` and `formal/lean/gc/IndexArenaPublication.lean`: prove the fixed-size
  index-arena publication obligation. If a segment is initialized before directory publication, a claimed slot is
  written before `len` publication, and allocation returns an `Addr` only after that slot publication, a later read of
  the returned `Addr` observes initialized segment and node bytes.
- `formal/rocq/gc/SideArenaPublication.v` and `formal/lean/gc/SideArenaPublication.lean`: prove the side-arena
  publication obligation. If side pages are published before chunks, chunks before entries, and entry writes before
  entry publication, a reader observing a published side entry sees initialized page/chunk/entry payloads.
- `formal/rocq/gc/SideFreeQuiescence.v` and `formal/lean/gc/SideFreeQuiescence.lean`: prove the side-payload
  lifetime obligation. If side payload boxes are freed only on the true-quiescence arm, stack laundered references
  imply a non-quiescent evaluator, and the materialization shadow is cleared before any future dereference, then
  dropping reclaimed side boxes cannot create a dangling future dereference. Non-quiescent collections therefore defer
  side-box freeing.
- `formal/rocq/gc/HashConsSweepRetain.v` and `formal/lean/gc/HashConsSweepRetain.lean`: prove the Addr-valued
  hash-cons retain obligation. A major hash-cons hit cannot return a freed address when retained entries imply marked
  entries and sweep frees only unmarked entries; a minor hash-cons hit cannot return a freed address when retained
  entries are either old or marked and minor sweep frees only young unmarked entries.
- `formal/rocq/gc/DriverRootUnion.v` and `formal/lean/gc/DriverRootUnion.lean`: prove the driver root-union
  obligation. If worker-buffer roots, safepoint roots, live environment anchors, and live dispatch anchors are all
  included in the driver root set, mark/sweep cannot free any live channel root.
- `formal/rocq/gc/DriverCPublication.v` and `formal/lean/gc/DriverCPublication.lean`: if eval-entry publication
  maps every caller-held driver-C root into the driver/safepoint root set, root-complete mark and sweep safety retain
  every such driver-C root.
- `formal/rocq/gc/BatchHandoff.v` and `formal/lean/gc/BatchHandoff.lean`: prove the async rholang batch-result
  handoff obligation. A worker result survives while protected by its persistent safepoint handle, survives after the
  caller copies it into `MettaState.output`, and dropping the handle is safe only after that output copy.
- `formal/rocq/gc/OperatorCacheEpoch.v` and `formal/lean/gc/OperatorCacheEpoch.lean`: prove the pointer-keyed
  operator-cache sweep-epoch obligation. A returned cache entry is current if the local sweep-epoch guard runs before
  lookup; if the local epoch is stale, the guarded lookup misses after clearing the cache.
- `formal/rocq/gc/EpochProtectedCaches.v` and `formal/lean/gc/EpochProtectedCaches.lean`: prove the shared
  sweep-epoch obligation for worker-local caches that can key by, or return values containing, reusable index `Addr`s.
  If a reclaiming index sweep advances `gc_sweep_epoch` and every such cache validates that epoch before lookup, then
  no lookup can return an entry that is stale for the current sweep epoch. This proof found and fixed a source bug in
  the MORK ground-fragment cache: its index-mode lookup path still assumed that no index sweep ran and skipped
  validation. The live implementation now clears the MORK ground-fragment cache when `gc_sweep_epoch` advances under
  index-gc, while slab mode keeps per-slot allocation-epoch validation.
- `formal/rocq/gc/WriteOnceAnchors.v` and `formal/lean/gc/WriteOnceAnchors.lean`: prove the write-once global-anchor
  obligation used by the compiler atom statics. If initialized anchors cannot be deleted and the structural reader
  scans each initialized anchor, ordinary root-complete mark/sweep retains the anchored values.
- `formal/rocq/gc/SpaceRegistryBarriers.v` and `formal/lean/gc/SpaceRegistryBarriers.lean`: prove the global
  space-registry obligation. Registered `SpaceHandle` values survive as structural E0 global anchors, and values
  reachable from overwritten, removed, or bulk-cleared old handles survive E2 SATB collection when those old handles
  are shaded.
- `formal/rocq/gc/TieredCacheBarriers.v` and `formal/lean/gc/TieredCacheBarriers.lean`: prove the global tiered
  compilation cache obligation. Pending bytecode source roots and ready bytecode constants survive as structural E0
  global anchors, removed pending/compiled values survive E2 SATB collection when shaded, and ownership-token-checked
  pending-root guards cannot unregister newer same-hash pending roots.
- `formal/rocq/gc/ThreadLocalTablesBarriers.v` and `formal/lean/gc/ThreadLocalTablesBarriers.lean`: prove the
  thread-local value-table obligation. Cached eval memo, match-result, subgoal, and thunk values survive while scanned
  as structural roots, and stale-evicted, overwritten, explicitly removed, cleared/invalidated, and thunk-replaced
  subgoal/thunk cached results survive E2 SATB collection when shaded. Eval/match eviction and clear removal shapes
  are discharged by the E0 cache-eviction proof.
- `formal/rocq/gc/CESKCollectorSafety.v`: composes the rendezvous witness, witness-slot lifecycle, witness-ok reset,
  started-cycle straddle gate, four-channel driver-root union, collector-root closure, mark completeness,
  sweep-only-unmarked, driver-C publication, and young-minor obligations into explicit no-UAF theorems for participant
  roots, live witness-slot visibility, cross-cycle witness-gate freshness, no-phantom straddle re-park, driver channel
  roots, opt-in mid-loop channel roots and future touches, caller-held driver-C roots, async batch-result handoff values, pointer-keyed operator-cache and
  shared worker-local Addr-cache sweep-epoch coherence, write-once global anchors, global space-registry roots and removed-handle SATB shades, global tiered-cache
  roots and removed-value SATB shades, thread-local table roots and removed-result SATB shades, future CESK touches,
  reachable young nodes under both the no-old-to-young and conservative-minor traversals, E2 snapshot-live nodes
  covered by initial roots, driver roots, SATB shades, or allocate-black publication, E2 freshly published
  allocate-black allocations, E2 final-rendezvous roots and abort-to-STW finalization, E2 full-major SATB mark
  lifecycle, E2 snapshot-live values removed by value-bearing E0 cache capacity eviction, overwrite, and bulk clear, and
  E2 snapshot-live values removed from the pinned value-bearing E0 mutation categories.
- `formal/rocq/gc/SATB.v` and `formal/lean/gc/SATB.lean`: prove the E2 concurrent-mark SATB obligation: if
  snapshot-live values are covered by initial roots, final-rendezvous driver roots, shaded deletion pre-images, or
  allocate-black roots, sweep cannot free them. They also state the final-rendezvous driver-root theorem directly:
  a root captured by the final remark cannot be freed by the exclusive sweep.
- `formal/rocq/gc/SATBGates.v` and `formal/lean/gc/SATBGates.lean`: prove the E2 SATB phase/sweep gate
  obligations. If marker start cannot pass an open deletion that saw "not marking", a removed snapshot-live pre-image
  must have been removed after marker start and therefore shaded; if sweep cannot pass an open deletion barrier, a
  snapshot-live value at sweep is either still visible or already shaded, so ordinary mark/sweep cannot free it.
- `formal/rocq/gc/AllocateBlack.v` and `formal/lean/gc/AllocateBlack.lean`: bridge the E2 allocate-black publication
  order into the SATB theorem. If a freshly published allocation is black before publication makes it visible, then it
  is a SATB root and cannot be freed by sweep; the direct mark-before-publish theorem mirrors the TLA discriminator.
- `formal/rocq/gc/SATBFinalization.v` and `formal/lean/gc/SATBFinalization.lean`: bridge the E2 finalization
  obligations into the SATB safety story. Final-rendezvous roots survive because they are re-marked before the
  exclusive sweep; if the final sweep gate is closed, the checked result must run the STW backstop; and an aborted SATB
  request is handled only after a freshly requested STW rendezvous runs.
- `formal/rocq/gc/FullMajorSweep.v` and `formal/lean/gc/FullMajorSweep.lean`: pin the E2 full-major-only mark
  lifecycle. If every SATB-marked address is in the full-major swept range and every swept address is cleared before
  promotion, no SATB mark can remain stale for a later cycle. They also prove the negative young-only obligation:
  if an old SATB mark survives a young-only final sweep, safety requires that no such old SATB mark exist. The
  source-coupling harness rejects `sweep_young` inside the final SATB sweep path.
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
- `tla/MidloopRootUnion.tla`: checks the opt-in single-threaded mid-loop root vector. Including live S/C/K,
  E0/global/K-spine, deferred env drops, and driver-C preserves `MidloopRootUnionComplete`; omitting the machine,
  deferred-env, or driver-C channel violates it.
- `tla/DriverCPublication.tla`: checks that public eval entry publishes driver-C (`MettaState.source/output`) to
  the safepoint channel before midloop/rendezvous roots can be built. Omitting that publication violates
  `DriverCVisibleOnSweep`, matching a caller-held source/output value that can be freed while eval is still live.
- `tla/BatchHandoff.tla`: checks the async rholang batch-result handoff. Holding a persistent handle until the caller
  copies worker results into `MettaState.output` preserves safety; omitting the handle or dropping it before the copy
  violates `NoPublishedBatchResultFreed`.
- `tla/OperatorCacheEpoch.tla`: checks the pointer-keyed operator-cache sweep-epoch guard. Checking the local
  `gc_sweep_epoch` before lookup clears another worker's stale cache entry after sweep; skipping the check violates
  `NoStaleOperatorCacheHit`.
- `tla/EpochProtectedCaches.tla`: checks the shared worker-local cache sweep-epoch guard for value hash, MORK
  ground-fragment, hash-cons, eval memo, match-result, and operator caches. Checking every modeled cache preserves
  `NoStaleAddrCacheHit`; omitting any one cache admits a stale hit after a reclaiming sweep bumps the heap epoch.
- `tla/WriteOnceAnchors.tla`: checks the write-once compiler atom anchors. Scanning all three initialized anchors and
  forbidding deletion preserves `LiveAnchorsScanned`; omitting `ATOM_IF` or allowing a reset/take-style deletion
  violates the corresponding invariant.
- `tla/SpaceRegistryBarriers.tla`: checks the global space-registry obligation. Scanning registered spaces and shading
  overwritten, removed, and bulk-cleared old `SpaceHandle` values preserves `NoSpaceRegistryValueFreed`; omitting the
  scan, remove shade, or clear shade violates it.
- `tla/TieredCacheBarriers.tla`: checks the global tiered-cache obligation. Scanning pending bytecode source roots and
  compiled bytecode constants, shading overwritten/cancelled/guard-dropped/cleared old roots, and token-checking guard
  drops preserves `NoTieredCacheValueFreed`; omitting compiled-constant scan, cancellation shade, compiled clear shade,
  or token ownership violates it.
- `tla/ThreadLocalTablesBarriers.tla`: checks the thread-local subgoal/thunk table obligation. Scanning cached subgoal
  and thunk results and shading stale-evicted, overwritten, removed, cleared/invalidated, and thunk-replaced cached
  results preserves `NoThreadLocalTableValueFreed`; omitting thunk scan, subgoal stale-eviction shade, thunk clear shade,
  or thunk replacement shade violates it.
- `tla/EvalTablesRegisteredRoots.tla`: checks the thread-local eval memo and match-result table registered-root
  obligation. Scanning both tables preserves `NoEvalTableValueFreed`; omitting either eval memo or match-result scan
  violates it.
- `tla/StartedCycleGate.tla`: checks the E5 straddle gate. Gating re-park on `GC_CYCLE_STARTED` avoids phantom
  re-parks during teardown; gating on `GC_CYCLE_GEN` violates `NoPhantomRepark`.
- `tla/CollapseCompletion.tla`: checks the E1 parallel collapse completion liveness obligation. With the RAII
  completion guard, normal and panic exits both decrement the worker counter and `<>(parentDone)` holds; without the
  panic-edge decrement, a panic can leave `remaining > 0` forever and violates the temporal property.
- `tla/WorkerAdmission.tla`: checks the E1 WorkerEnter admission race. Closing admission before the participant
  snapshot prevents a new worker from joining during collection; disabling the admission gate frees a live worker that
  entered after the snapshot.
- `tla/WitnessOkReset.tla`: checks the cross-cycle witness flag reset. Clearing `CURRENT_WITNESS_OK` at cycle end
  prevents the previous cycle's true flag from admitting a next-cycle sweep before the next witness wait.
- `tla/CurSegReuseOrder.tla`: checks the historical skipped-old young-marker allocator premise. Cur-segment-only
  reuse preserves bump order; any-young reuse admits an old-parent to young-child edge after promotion. The live
  minor safety proof is `ConservativeMinorMark`, not this narrower premise.
- `tla/StoreCentricGC_Generational.tla`: checks the C1 generational full-mark minor/major collector. Full marking
  before either sweep preserves reachable nodes even with old-to-young edges; minor sweep reclaims only young
  unmarked nodes; major sweep reclaims globally; segment release is safe only when no reachable node remains in the
  released segment. The default harness uses `MC_StoreCentricGC_Generational_small.cfg` as a disk-light gate.
- `tla/StoreCentricGC_GenerationalYoungMark.tla`: checks the rejected skipped-old young-only marker premise. The
  positive small config proves cur-segment-only reuse preserves the premise in the reduced state space; the negative
  small config demonstrates that any-young reuse violates `YoungOnlyMarkReachesLiveYoung` with an old parent pointing
  at a young child.
- `tla/ConservativeMinorMark.tla`: checks the C1 first-class-space correction. Traversing old reachable containers
  during a minor preserves a young value reachable through an old `SpaceHandle`; the old skipped-old traversal violates
  `YoungReachableMarked`.
- `tla/NodeEdgeCompleteness.tla`: checks the marker edge-reader completeness obligation. Including inline handle
  fields, side-arena child slices, and `SpaceHandle` contents preserves `NoReachableFreed`; omitting any one class
  admits a reachable child that is swept unmarked.
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
- `tla/SATBYoungSweepStaleOldMark.tla`: checks the stale-old-mark obligation behind the full-major E2 SATB final
  sweep. Clearing old marks passes; a young-only final sweep leaves an old SATB mark stale and violates
  `NoStaleOldMark`.
- `tla/SATBAbortFallback.tla`: checks the E2 abort-to-STW backstop. If the SATB path aborts after cleanup, the
  driver must re-request and run a fresh STW rendezvous before treating the request as handled.
- `tla/RegistryIsolation.tla`: checks the A5/E1 root-source boundary. Index mode may build roots from structural CESK
  readers and explicit driver transport roots only; enabling the legacy `RootProvider` registry as an index root source
  violates `NoRegistryInIndex`.
- `tla/FrameEnvRoots.tla`: checks the forked-env frame-root channel. Including the five fork-local Addr-bearing maps
  preserves `FrameEnvRootsComplete`; omitting inferred function type roots violates it.
- `tla/TierLeafExtraRoots.tla`: checks the VM/JIT tier-leaf extra-root channel. Including every VM/JIT tier-local
  value-bearing field preserves `TierLeafExtraRootsComplete`; omitting VM dispatch-memo roots or JIT state-cache roots
  violates it.
- `tla/IndexArenaPublication.tla`: checks the fixed-size arena publication order. Segment write-before-publish,
  slot-after-segment, slot write-before-publish, and return-after-slot-publish preserve `ReturnedAddrReady`; violating
  any one of those orders lets a reader observe an uninitialized segment/slot path.
- `tla/SideArenaPublication.tla`: checks the variable-length side-arena publication order. Page-before-chunk,
  chunk-before-entry, and write-before-entry-publish preserve `PublishedEntryReady`; violating any one of those orders
  produces a published read of an uninitialized component.
- `tla/SideFreeQuiescence.tla`: checks the side-payload free lifetime obligation. Quiescent side-free with shadow
  clearing and non-quiescent deferral preserve `NoDanglingSideUse`; freeing on a non-quiescent arm or skipping the
  shadow clear admits a dangling side-payload dereference.
- `tla/HashConsSweepRetain.tla`: checks the Addr-valued hash-cons retain obligation. Major sweep dropping unmarked
  entries and minor sweep retaining only old or marked-young entries preserve `NoReturnedFreed`; retaining a dead major
  entry or a dead young minor entry admits a later hash-cons hit returning a freed address.

## Source coupling

`scripts/verify_cesk_gc_source_coupling.sh` is run by `scripts/verify_cesk_gc_formal.sh`. It pins the source-side
facts the proofs rely on:

- `ROOT_REGISTRY`, `RootProvider`, `frame_chain`, `current_iter_root`, every bridge-period `RootProvider` impl, and
  every bridge-period root-provider registration function remain slab-only. The index variants of those registration
  functions are no-ops, and index roots are read through named structural readers or typed live-env/live-dispatch driver
  channels.
- `IndexHeap::child_addrs_for_mark` first delegates to `Node::child_addrs` for inline handle fields, then explicitly
  resolves side-arena `SExpr`/`Conjunction` children, then traverses first-class `SpaceHandle` contents. The harness
  also pins the premises that `State` payloads are rooted by `GenericEnvironmentShared::collect_roots_into` and
  `MemoHandle` entries store serialized bytes, not live `MettaValue` handles.
- `IndexHeap` frees reclaimed side payload boxes only through the two `phase == "quiescence"` guarded calls to
  `free_reclaimed_side_slots`; rendezvous and midloop collection pass non-quiescence phase strings, and both side-free
  paths clear `INNER_SHADOW` before returning to code that can materialize/dereference values again.
- `IndexHeap::intern_ground_sexpr` revalidates a hash-cons hit against the current node children before returning it;
  `IndexHeap::sweep` retains only marked hash-cons entries before full sweep, and `IndexHeap::sweep_young` retains old
  entries unconditionally while retaining young entries only when marked before young sweep.
- The E1 driver waits on `requestor_wait_for_all_reified_parked`, then sets `current_witness_ok`, builds the root
  union, runs the rendezvous-union oracle, and only then calls `run_collection_if_triggered_rendezvous`.
- `gate_open_rendezvous` is keyed by `current_witness_ok`, not the obsolete parked-count gate.
- Live envs and parallel fan-outs are registered through RAII handles, and the live-env/live-dispatch registry walks
  delegate to the structural `EnvRoots`/`DispatchRoots` readers used by the driver-root-union proof.
- Every self-root publication site routes through the canonical `collect_complete_thread_contribution` reader, whose
  source shape is pinned: trampoline participants publish extra hot values, live S/C/K, E0, global anchors, K-spine,
  and deferred env roots; tier leaves publish extra VM/JIT values plus the env-less persistent roots they can read.
- Live work item and continuation frames include fork-local environment roots in index mode. The source-coupling gate
  pins `fork_for_nondeterminism`'s five CoW-local Addr-bearing maps and the corresponding `collect_fork_local_roots`
  reader, asserts the `frame_env` classifiers remain exhaustive, and checks the three narrowed live-K arms re-read the
  frame env before skipping post-cut-dead iterator fields.
- The opt-in single-threaded mid-loop branch is pinned to build `midloop_roots` from `collect_machine_roots_live`
  (live S/C/K plus persistent E0/global/K-spine), then deferred environment drops, then `collect_safepoint_roots`, and
  `run_collection_if_triggered_midloop` must consume that same vector. Its gate remains index-only, explicit opt-in,
  single-evaluator, and pre-worker-spawned.
- Collapse-bind binding-capture frames are pinned as metadata-only: the old empty capture/root shims are absent, the
  frame contains only `tracked_vars` and `collapse_fork_depth`, and it cannot hold `MettaValue`, `BoundValue`, or
  `GenericBindings` roots outside the canonical work-item / continuation readers.
- Both public eval boundaries (`eval` and `eval_with_tier`) publish `MettaState.source/output` into
  `SAFEPOINT_ROOTS` before the live transition begins, so midloop/rendezvous collection sees the caller's driver-C
  even when an outer CLI/REPL/conformance loop has not installed its own batch guard.
- Async rholang batch results carry a `SafepointRootHandle` inside `BatchOutcome`: the worker projects
  `BoundValue` results to their `MettaValue` component, registers that value vector before publishing the outcome into
  the gather slot, the handle rides through sorting and return to `run_state_async`, and each caller loop pushes values
  into `MettaState.output` before the outcome drops.
- Reclaiming index sweeps are source-pinned to bump `gc_sweep_epoch` before eager sweeping-thread cache clears.
  VALUE_HASH_CACHE, hash-cons, EVAL_MEMO, MATCH_RESULT_CACHE, OPERATOR_CACHE, and the MORK ground-fragment cache
  are pinned to validate that epoch before returning hits. MORK ground fragments clear under index-gc on epoch advance
  because index `Addr` keys cannot be checked through slab slot allocation epochs.
- `collect_global_anchors` is source-pinned to scan the thread-local value-bearing evaluation tables in order:
  eval memo roots, match-result roots, subgoal roots, then thunk roots. That keeps the formal `Global` component tied
  to the actual E0 root reader, not only to the SATB deletion-barrier paths for those tables.
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
- Fixed-size arena publication is source-pinned: `open_segment` writes the segment cell before `seg_count` publication,
  `alloc_bump`/`try_bump_in` write and publish a claimed node slot before returning its `Addr`, and `get` checks the
  published slot prefix before calling `node_at`.
- Side-arena publication is source-pinned: segment side cells are written before `sides_count` publication,
  `SideColumn` pages are published before chunks, chunks before entries, entry payloads before entry publication, and
  variable-length node allocation interns side payloads before publishing the owning node.
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
  guard drop, and shades pending roots plus compiled bytecode constants before full cache clear. Pending bytecode root
  entries carry non-wrapping ownership tokens; guard-drop and backpressure cancellation use token-checked removal so an
  old guard cannot unregister a newer same-hash pending root.
- The rooted thread-local value tables scan eval memo entries, match-result RHS templates/bindings/RHS types, subgoal
  results, and thunk results as structural roots. The subgoal and thunk tables additionally shade cached result values
  on stale eviction, overwrite, explicit removal, invalidation, full clear, and thunk result replacement.
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
- Parallel dispatch/collapse worker completion is source-pinned: both worker closures construct exactly one
  `CompletionGuard` before `EvalGuard::enter()`, the only executable `remaining.fetch_sub(1, ...)` is inside the
  guard's `Drop`, and the wait arms observe completion from `remaining == 0`.
- Worker admission is source-pinned: the dedicated driver obtains `GcInProgressGuard` before preparing rendezvous
  roots, `EvalGuard::enter()` backs out while `GC_IN_PROGRESS` is set before joining `N_THREADS`, and the
  dispatch/collapse worker closures run the early dedicated-GC `worker_wait_for_resume()` admission wait before
  `EvalGuard::enter()`.

## Harness

Run:

```bash
bash scripts/verify_cesk_gc_formal.sh
```

The harness derives paths from its own location, uses `target/tlc-formal-small` for small TLC logs/metadata by
default, runs Rocq under `systemd-run`, and includes the small TLC positive/negative discriminators. The adjacent GC
gate scripts likewise default log/build scratch to repo-derived `target/...` directories (`target/gc-logs` or a
script-specific subdirectory) while preserving caller overrides such as `LOG_DIR`, `LOG_ROOT`, `SCRATCH_ROOT`,
`OUT`, `P`, `PGO_DIR`, and `CARGO_TARGET_DIR`; large gates must not spill into `/tmp` unless the caller explicitly
chooses that.

Before compiling proofs, `scripts/verify_cesk_gc_proof_hygiene.sh` rejects Lean proof shortcuts (`sorry`, `admit`,
`axiom`, `constant`, `opaque`, `unsafe`) and Rocq proof shortcuts (`Admitted`, `admit`, `Axiom`, `Parameter`,
`Conjecture`, `Abort`) in the CESK GC proof directories, and verifies every `formal/lean/gc/*.lean` and
`formal/rocq/gc/*.v` file is enumerated by the formal harness.

The formal harness also runs `scripts/verify_cesk_gc_tlc_hygiene.sh` before compiling proofs. That check parses every
`run_tlc` entry, verifies labels/configs are unique, requires every referenced TLA+ module and config to exist, requires
negative TLC runs to carry a discriminator pattern, rejects any tracked `tla/*.cfg` or `tla/MC_*.tla` wrapper that is
neither run nor explicitly classified, and rejects any tracked `tla/*.tla` module that is not either run directly,
imported by a run/classified wrapper, or explicitly classified. The only classified exclusions are legacy slab
mark-sweep models, the older non-generational store-centric mark-sweep wrapper/configs, and larger CESK generational
discriminator configs whose disk-light small counterparts are the default gate.
