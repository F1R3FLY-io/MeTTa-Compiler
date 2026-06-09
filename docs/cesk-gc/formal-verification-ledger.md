# CESK GC formal verification ledger

This ledger tracks the mechanically checked proof artifacts for the CESK-based `index-gc` collector. It is not about
the legacy slab mark-sweep collector.

Rocq is the load-bearing proof assistant for this gate, paired with TLA+ model checking and source-coupling checks.
Existing Lean files are supplemental mirrors only; the default formal harness does not require or maintain a second
mandatory proof track.

## Verified implementation boundary

The default live collector verified here is the CESK-based generational `index-gc` collector's E1 path:
single-threaded quiescence, default-on single-evaluator mid-loop collection, and rendezvous collection compute
structural roots, then mark and sweep while holding the index heap write lock. The mid-loop root-union theorem, TLC
discriminator, source-coupling checks, and focused forced-ASAN gate pin its live S/C/K, E0/global/K-spine,
deferred-env, and driver-C root vector. The K-spine proof and source-coupling gate also pin the native-stack
current-work component of C, so a nested evaluator cannot collect while an outer trampoline has popped a live work
item that is absent from the pending work stack. The focused ASAN run on 2026-06-07 forced one default mid-loop minor
with 0 UAF and result `[done]`. The default-on release conformance gate on 2026-06-07 passed 483/0/0/0 with
`INDEX_GC_CYCLES_RUN=798` under the existing committed-cap trigger, so the semantic oracle was non-vacuous.
The default dedicated FANOUT=8 discriminator on 2026-06-08 passed `Robot.metta` Arm A (`MIN=131072`) 0/16 failures
and Arm C (`MIN=4294967295`) 0/16 failures; the Arm A non-vacuity witness reported rendezvous minor cycles with
reclaimed slots and segment releases.
The focused loom gate on 2026-06-08 passed the live rendezvous, straddle, arena bump/publish, and side-node ordering
models under `RUSTFLAGS="--cfg loom -C target-cpu=native"` and model-specific `LOOM_MAX_PREEMPTIONS=2` or `3`;
the two expected-fail straddle discriminator variants remained ignored.
The focused TSan gate on 2026-06-08 passed
`backend::eval::cesk::index_heap::tsan_concurrent_factory::concurrent_read_path_allocations_are_race_free`
under `RUSTFLAGS="-Zsanitizer=thread -C target-cpu=native"` and `-Zbuild-std`, with no ThreadSanitizer warning.
The E1-FLIP V4 ASAN gate on 2026-06-08 passed the default dedicated `index-gc` collector under `FANOUT=8`.
`Robot.metta` reported 82 index cycles (81 rendezvous, 1 quiescence), `FlyingRaven.metta` reported 72 index cycles
(70 rendezvous, 2 quiescence), and `examples/cesk-gc/stress_multidir.metta` reported 1142 index cycles
(15 rendezvous, 1127 quiescence). All three arms reported 0 ASAN/UAF hits, 0 mid-loop cycles under FANOUT, 0
unexpected non-rendezvous cycles, and no `Error`/`StackOverflow`.
After making SATB the default rendezvous path, the full `scripts/verify_cesk_gc_formal.sh` harness passed:
proof hygiene, TLC hygiene, the mandatory Rocq corpus, source coupling, and the full positive/negative TLC
discriminator suite.
The E2 SATB major path is now the production rendezvous collector path: the dedicated GC thread
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
- `formal/rocq/gc/NurseryBackpressure.v`: proves the C1.c allocator-to-GC nursery trigger obligation. A subsequent
  segment-open pending signal, or a young-allocation budget overflow, is enough to request a minor collection; after
  promotion, resetting the young odometer and clearing the pending signal prevents the same stale event from
  immediately re-firing a minor when the young budget is not over.
- `formal/rocq/gc/DeepBranchingCollectionProgress.v`: proves the high-branching FANOUT progress composition. Under
  fanout-active/nonquiescent workers, rendezvous witness readiness, nursery/young pressure, or cap/cadence major
  pressure enables a collection path without requiring global evaluator quiescence, while structural/SATB root coverage
  still prevents any future touch from being freed. The TLA discriminator rejects reintroducing the old `active == 0`
  rendezvous gate.
- `formal/rocq/gc/YoungAllocationOdometer.v`: proves the C1.c young-allocation odometer obligation. Reused young
  slots and fresh bump allocations advance the odometer by a positive node-size quantum, promotion resets it, and
  crossing the budget entails the minor trigger.
- `formal/rocq/gc/MajorMinorScheduler.v`: proves the C1.c major/minor scheduler obligation. Cap-forced and
  cadence-forced majors cannot be deferred by acute young pressure; below level-3 young pressure a due major runs; and
  any deferred major is therefore a live-growth major under a level-3 minor trigger.
- `formal/rocq/gc/CapFloorAntiThrash.v`: proves the B.5 cap-floor anti-thrash obligation. Raising the cap floor to
  the current committed bytes after a futile cap-triggered major prevents an immediate cap re-fire unless committed
  grows, and clearing the floor after a segment-releasing major restores the ordinary base-cap predicate.
- `formal/rocq/gc/MajorWatermarkRearm.v`: proves the B.4 old-live major watermark rearm obligation. Rearming the
  major watermark from `max(old_live_after * 2, min_threshold)` prevents an unchanged old generation from immediately
  re-firing the live-growth major clause and requires future old-live bytes to exceed the doubled post-major old-live
  metric and the minimum threshold.
- `formal/rocq/gc/StructuralRoots.v` and `formal/lean/gc/StructuralRoots.lean`: if future machine touches are
  inside the structural CESK-root closure and sweep frees only unmarked nodes, no future-touched node can be freed.
  Rocq also states the manual-registration boundary explicitly: a registry-only value is not an index collector root,
  and no-UAF follows from machine completeness rather than from auxiliary root registration.
- `formal/rocq/gc/StructuralRootSourceAudit.v`: closes the source-coupling audit over the no-registry root
  architecture. It enumerates the source-coupled live root families (S/C/K, frame-local environments, E0/global
  anchors, typed K-spine, VM/JIT leaves, selective choice points, deferred env drops, driver-C, worker/safepoint
  publications, live env/dispatch anchors, and batch handoff roots) and proves that future-touch safety depends only
  on those structural/driver roots, not on a RootProvider/root-registry/frame-chain side channel.
- `formal/rocq/gc/RegistryIsolation.v`: proves the dedicated A5/E1 index-mode root-source isolation companion to
  `tla/RegistryIsolation.tla`. Structural CESK roots and driver transport roots are valid index collector sources;
  the legacy `RootProvider` registry is not. A registry-only value is therefore not an index root, and future-touch
  safety is independent of auxiliary manual registration.
- `formal/rocq/gc/NodeEdgeCompleteness.v` and `formal/lean/gc/NodeEdgeCompleteness.lean`: prove the node-edge
  completeness obligation. If the marker's concrete reader covers every semantic index-node edge class
  (inline handle fields, side-arena `SExpr`/`Conjunction` children, and first-class `SpaceHandle` contents), then
  ordinary mark/sweep cannot free anything reachable through those semantic node edges.
- `formal/rocq/gc/AbstractGCLiveNarrowing.v` and `formal/lean/gc/AbstractGCLiveNarrowing.lean`: prove the C2
  abstract-GC K-frame narrowing obligation. If every future transition touch is a full K-frame root and is not in the
  frame's dead-for-next-transition field set, then marking only the narrowed live root set cannot free a future-touched
  value. The proof also states the no-dead-fields equality case and rejects the contradictory shape where a skipped
  field is still future-live.
- `formal/rocq/gc/MidloopRootUnion.v` and `formal/lean/gc/MidloopRootUnion.lean`: prove the default single-threaded
  mid-loop root-union obligation. If live S/C/K, E0, global anchors, K-spine, deferred environment drops, and driver-C
  safepoint roots are included in the mid-loop root vector, mark/sweep cannot free any channel root, and future touches
  reachable from that union survive collection.
- `formal/rocq/gc/KSpineCurrentWork.v`: proves the typed K-spine current-work obligation. If a suspended trampoline
  activation contributes the in-flight current work item, pending work stack, and continuation stack to its K-spine
  root reader, ordinary mark/sweep cannot free any live suspended control component. The companion TLA+ discriminator
  (`KSpineCurrentWork.tla`) rejects the historical shape where the current-work component is omitted.
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
- `formal/rocq/gc/GenerationResume.v` and `formal/lean/gc/GenerationResume.lean`: prove the E1/E5
  generation-gated resume obligation. A parked worker can resume once `GC_CYCLE_GEN != my_gen`, even if a back-to-back
  request reasserts `GC_REQUESTED`; if the generation never advances, the worker remains parked, and boolean
  `GC_REQUESTED` resume can be re-blocked by the next request.
- `formal/rocq/gc/RendezvousProgress.v`: proves the compositional premises for the dedicated rendezvous progress
  obligation. If every active participant contributes by park or finish, collection panic cleanup closes the cycle,
  close advances the generation and clears the request, and parked workers resume from the advanced generation, then
  a posted rendezvous cannot strand a parked participant. The temporal scheduler obligation is checked in TLA+.
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
- `formal/rocq/gc/SelectiveChoicePointRoots.v`: proves the E3 selective-CESK re-enterable continuation obligation.
  If trampoline coroutine/choice state, VM choice points, JIT choice points, and captured/suspended spines are all
  included in the structural K root contribution, mark/sweep cannot free a re-enterable continuation address that a
  future resume can touch. The companion TLC discriminator rejects omitting the VM, JIT, or trampoline choice family.
- `formal/rocq/gc/StoredBranchCoroutineSpine.v`: proves the first implementation-level E3 lowering. A live
  `StoredBranchCoroutine` carries a `ContinuationAddr` into the continuation-spine store; if that address resolves to
  a stored branch-coroutine node and the node walker includes remaining RHS values, remaining branch binding values,
  and yielded results, then every value a future lazy resume can touch is rooted and cannot be swept as unrooted. The
  source-coupling gate pins the Rust lowering from `ProcessRuleMatchesLazy` to `StoredBranchCoroutine` and rejects a
  return to `Box<BranchCoroutine<MettaValue>>`.
- `formal/rocq/gc/VmChoicePointSpine.v`: proves the VM implementation-level E3 lowering. A live VM choice-point stack
  entry is now a `ContinuationAddr` into the typed continuation-spine store; if that address resolves to a stored
  choice-point node and the node walker includes the continuation chunk constants, alternatives, rule-match bindings,
  bound values/bindings, and saved current bindings, then every value a future VM fail/backtrack transition can touch is
  rooted and cannot be swept. The source-coupling gate rejects restoring `GenericBytecodeVM.choice_points` to a raw
  `Vec<GenericChoicePoint<...>>` and pins `push`, `pop`, `truncate`, `clear`, and `iter` to the address-backed stack.
- `formal/rocq/gc/JitChoicePointSpineBridge.v`: proves the JIT implementation-level E3 bridge. The repr(C) native JIT
  choice-point buffer remains the execution ABI, but the collector materializes the live prefix into
  `ContinuationAddr`-backed spine nodes before walking it. If the bridge resolves every live native choice point and
  the node walker includes saved chunk constants, saved stack-pool values, inline value/space-match alternatives, and
  chunk/rule-match constants, then every value a future JIT fail/backtrack transition can touch is rooted.
- `formal/rocq/gc/JitStackSavePoolFreshness.v`: proves the stack-save-pool freshness obligation discovered during the
  JIT bridge proof. Native JIT fork now allocates stack-save-pool slots monotonically within one execution and bails out
  before publishing a choice point if no fresh slot is available. Therefore a later fork cannot wrap and overwrite a
  live choice point's saved stack values before the root walker or backtracking restore reads them.
- `formal/rocq/gc/JitChoicePointProductionRestore.v`: proves the production E3 JIT choice-point restore carrier. The
  generated-code ABI still receives a contiguous `repr(C)` pointer, but the backing buffer is owned by
  `JitChoicePointSpineOwner`; `JitContext.choice_points` is only a transient pointer view. The theorem composes
  owner-live-prefix coverage, ContinuationAddr bridge root walking, restore-from-owner-slot, and pop-removes-live-slot
  premises to show every value a future JIT fail/backtrack transition can touch is rooted and cannot be freed.
- `formal/rocq/gc/TrampolineFanoutSpineBridge.v`: proves the trampoline fan-out/collapse implementation-level E3
  bridge. Collector-facing root walking materializes `ProcessRuleMatches`, `ProcessAmb`, `ProcessMatchTemplates`,
  `ProcessCollapseEvalResults`,
  `WaitForParallel`, and `WaitForParallelCollapse` as `ContinuationAddr`-backed bridge nodes. If the bridge resolves a
  live frame and its node walker includes the future-touch fields, then every future resume/collapse value is rooted;
  the proof models the three cut-pruned remaining families as future touches only when their cut barrier has not fired.
- `formal/rocq/gc/TrampolineFanoutProductionRestore.v`: proves the production E3 trampoline fan-out restore carrier.
  The trampoline loop now normalizes live re-enterable fan-out frames into a thread-local
  `SpineStore<Continuation>` and leaves `Continuation::TrampolineFanoutSpine { handle }` on K. Root walking resolves
  the address-backed handle; `process_continuation` removes/resolves the same address before executing the payload; and
  handle drop removes abandoned nodes. The theorem composes normalization, root walk, resolve-before-execute, and
  post-resolve no-stale-node premises to show every future-touch value is rooted and cannot be freed.
- `formal/rocq/gc/UnifiedChoicePointRestore.v`: proves the final E3 aggregate restore obligation for Selective
  CESK*. Stored lazy branch coroutines, VM choice-point spine handles, JIT choice-point ABI spine owner slots, and
  trampoline fan-out spine handles are the four production re-enterable families. If each family roots every
  future-touch value and restores from its carrier without leaving a stale live node, then the unified re-enterable
  continuation relation roots every future resume/fail/backtrack touch and prevents sweep from freeing it.
- `formal/rocq/gc/SerializableContinuationSlice.v`: proves the E4 serialized-continuation slice obligation. If a
  serialized suspended state includes its control, environment, and continuation roots and is closed under store
  edges, every address a restored transition can touch is in the serialized slice and cannot be reclaimed as outside
  that slice. The companion TLC discriminator rejects omitting either the K root or a reachable store child.
- `formal/rocq/gc/IndexArenaPublication.v` and `formal/lean/gc/IndexArenaPublication.lean`: prove the fixed-size
  index-arena publication obligation. If a segment is initialized before directory publication, a claimed slot is
  written before `len` publication, and allocation returns an `Addr` only after that slot publication, a later read of
  the returned `Addr` observes initialized segment and node bytes.
- `formal/rocq/gc/SideArenaPublication.v` and `formal/lean/gc/SideArenaPublication.lean`: prove the side-arena
  publication and co-location obligation. If side pages are published before chunks, chunks before entries, entry
  writes before entry publication, and the owning node is published only after a same-segment side payload exists, a
  reader observing a published side-bearing node sees initialized side payloads in the segment selected by the node
  address.
- `formal/rocq/gc/ConcurrentBumpFreshOnly.v`: proves the D-RLOCK/B2 shared-allocation separation obligation.
  Concurrent allocation returns only fresh bump slots, fresh slots are separated from the free list, and free-list reuse
  is reserved for the exclusive path; therefore a concurrent allocation cannot return a free-list slot or alias a
  reuse return.
- `formal/rocq/gc/IndexAllocatorRefinement.v`: composes the Rust source-coupled allocator facts into the abstract
  contract consumed by `CESKCollectorSafety.v`: fixed-slot reads observe published segment/slot bytes, side-payload
  reads observe same-segment published side entries, published allocations satisfy allocate-black, shared concurrent
  allocation is disjoint from exclusive free-list reuse, and `free_bit` tracks a duplicate-free free list.
- `formal/rocq/gc/SideFreeQuiescence.v` and `formal/lean/gc/SideFreeQuiescence.lean`: prove the side-payload
  lifetime obligation. If side payload boxes are freed only on the true-quiescence arm, stack laundered references
  imply a non-quiescent evaluator, and the materialization shadow is cleared before any future dereference, then
  dropping reclaimed side boxes cannot create a dangling future dereference. Non-quiescent collections therefore defer
  side-box freeing.
- `formal/rocq/gc/SideReclaimRefinement.v`: composes the source-coupled side-reclaim ownership protocol. Side reads
  observe published same-segment payloads, future side reads cannot target freed pending snapshots, freed side boxes
  have an owner snapshot plus quiescent full-mark drain and shadow clear, marked owners that still own the side are
  retained, released segment resets drop pending snapshots, and duplicate reports require fresh free-owner acquisition.
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
- `formal/rocq/gc/SchedulerGcBoundary.v`: proves the GC-facing scheduler/thread-pool boundary. If active workers,
  live dispatch/collapse fan-outs, and async batch handoff values are all mapped into driver root channels, and
  collection admission prevents newly joined workers during the sweep window, mark/sweep cannot free a scheduler-held
  live address. This intentionally does not claim general scheduler fairness or work-stealing correctness.
- `formal/rocq/gc/SchedulerFanoutProgress.v`: composes the FANOUT scheduler progress obligations. Given the existing
  trigger backstop, posted-driver or SATB-abort-to-STW fallback, generation-based resume, participant contribution,
  and completion-guard premises, every active parked worker is resumed and the parent wait cannot be stranded by a
  missing worker completion drop. The temporal eventuality/discriminator layer remains the paired TLC suite.
- `formal/rocq/gc/DedicatedHandoff.v`: proves the E1 dedicated-thread handoff ownership rule. Once a root vector has
  been successfully sent to the GC thread, response-channel failure cannot justify an inline fallback because the
  mutator no longer owns those roots; failed sends still return the roots for inline fallback. It also proves the
  channel-liveness obligation from the static channel audit: a successful `Collect` handoff carries a per-request
  response sender and the driver attempts a reply after catching the collection result.
- `formal/rocq/gc/DedicatedSingleRegime.v`: proves the E1 dedicated-thread single-regime rule. With the dedicated
  collector enabled, default/session/parallel/cron legacy producers are suppressed, so any request in that regime must
  be paired with a dedicated driver request.
- `formal/rocq/gc/DepthZeroSafepoint.v`: proves the E1 cooperative-safepoint depth-zero rule. A caller that is not
  inside an EvalGuard is not in the dedicated collector's participant snapshot and therefore must return without
  parking or dropping an EvalGuard; depth-positive callers may use the ordinary park path.
- `formal/rocq/gc/PollEdgeContribution.v`: proves the E1 GC-pending poll-edge contribution rule. Once a
  depth-positive worker reaches a GC-pending poll edge, it must collect structural roots, drop its guard, publish the
  roots, and only then wait; a published poll-edge root then survives ordinary mark/sweep through the driver root set.
- `formal/rocq/gc/ConcurrentTriggerBackstop.v`: proves the E1 FANOUT rendezvous-trigger backstop rule. If a worker
  trigger cannot hand `CollectRendezvous` to the dedicated GC thread, the resume backstop must clear the pending
  request and wake workers; otherwise the trigger must have posted a driver request.
- `formal/rocq/gc/E1DefaultConcurrentFlip.v`: composes the E1 default-flip boundary. In index mode the dedicated
  collector is the active regime; legacy request producers are suppressed; a FANOUT watermark trigger either posts
  `CollectRendezvous` or runs the request-clear/resume backstop; and a posted driver cycle closes, advances the
  generation, clears the request, and resumes parked workers. The source-coupling harness pins this theorem to
  `dedicated_gc_enabled()`, `request_concurrent_collection`, and the no-`METTATRON_INDEX_GC_DEDICATED` production
  invariant.
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
- `formal/rocq/gc/E0CacheBarrierCompleteness.v`: closes the end-to-end Rocq audit for value-bearing E0/cache
  mutation coverage. The proof enumerates the source-coupled value-dropping categories (symbol/state/named-space/type
  vector/token/ACT/module/rule mutations, space-registry replacement/removal/clear, bytecode and tiered cache
  evictions/clears, eval/match cache evictions/clears, and subgoal/thunk stale/overwrite/remove/clear/replace paths)
  and proves that once the corresponding source assertion shades the removed pre-image, the value cannot be freed by
  SATB sweep. It also records the persistent-root and epoch-protected-cache cases used for write-once anchors,
  operator/value-hash/MORK/inner-shadow caches, and hash-cons hits.
- `formal/rocq/gc/CESKCollectorSafety.v`: composes the rendezvous witness, witness-slot lifecycle, witness-ok reset,
  started-cycle straddle gate, rendezvous progress, scheduler/thread-pool root boundary, four-channel driver-root
  union, collector-root closure, mark completeness, sweep-only-unmarked, driver-C publication, and young-minor
  obligations into explicit no-UAF/progress theorems for participant roots, live witness-slot visibility,
  cross-cycle witness-gate freshness, no-phantom straddle re-park, parked-participant release after a closed
  rendezvous, scheduler-held live roots, driver channel roots, C2 abstract-GC live-K narrowing, default mid-loop
  channel roots and future touches, caller-held driver-C roots, async batch-result handoff values, pointer-keyed
  operator-cache and shared worker-local Addr-cache sweep-epoch coherence, write-once global anchors, global
  space-registry roots and removed-handle SATB shades, global tiered-cache
  roots and removed-value SATB shades, thread-local table roots and removed-result SATB shades, future CESK touches,
  reachable young nodes under both the no-old-to-young and conservative-minor traversals, E2 snapshot-live nodes
  covered by initial roots, driver roots, SATB shades, or allocate-black publication, E2 freshly published
  allocate-black allocations, E2 final-rendezvous roots and abort-to-STW finalization, E2 full-major SATB mark
  lifecycle, E2 snapshot-live values removed by value-bearing E0 cache capacity eviction, overwrite, and bulk clear, and
  E2 snapshot-live values removed from the pinned value-bearing E0 mutation categories. It also composes the explicit
  "machine completeness displaces manual registration" theorem into the top-level no-UAF story, and states that
  rendezvous live roots survive from the witness/root-coverage premise even when the old global-quiescence gate is
  false. The `end_to_end_cesk_index_gc_safety` theorem now ties those proof families into one checked boundary:
  future CESK touches covered by structural roots, driver channels, scheduler-held roots, SATB roots, or
  allocate-black publication are not freed; observed fixed-arena reads see fully published segment/slot state; and
  shared concurrent allocation cannot alias exclusive free-list reuse.
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
- `formal/rocq/gc/SATBFinalization.v`: bridges the E2 finalization obligations into the SATB safety story.
  Final-rendezvous roots survive because they are re-marked before the exclusive sweep; premarked allocate-black final
  roots must still be revisited so their reachable children are marked; if the final sweep gate is closed, the checked
  result must run the STW backstop; and an aborted SATB request is handled only after a freshly requested STW
  rendezvous runs. Lean mirrors may exist for older obligations, but the mandatory proof for this rung is Rocq.
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
- `formal/rocq/gc/SATBSubmodelClosure.v`: closes the Rocq audit over the SATB submodel split. It gives one mandatory
  Rocq theorem whose fields cover the named TLA submodels that carry source-level safety premises: deletion barrier,
  E0 mutation-site shading, LRU eviction, bulk clear, phase gate, sweep gate, allocate-black publication, final
  remark, premarked final-root revisit, final-sweep backstop, abort-to-STW handling, full-major mark clearing, and the
  young-only stale-old-mark negative. Lean mirrors remain supplemental only; this closure is Rocq because Rocq is the
  preferred mandatory proof lane for this workstream.
- `tla/RendezvousWitness.tla`: checks the E1 witness gate predicate. The strict `published>=cur_gen OR
  acquired>cur_gen` model preserves root completeness at sweep; the negative `acquired>=cur_gen` model violates it.
  A second negative config makes a non-reified finisher stamp `published_gen`, which also violates root completeness.
- `tla/WitnessSlotLifecycle.tla`: checks the V4 slot lifecycle. Keeping the slot occupied across safepoint drop
  preserves live-machine visibility at sweep; the negative release-on-safepoint model violates it.
- `tla/DriverRootUnion.tla`: checks the E1 driver root-union channels. Including worker-buffer, safepoint,
  live-env/E0, and live-dispatch channels preserves root-union completeness; omitting live-env or live-dispatch
  violates it.
- `tla/MidloopRootUnion.tla`: checks the default single-threaded mid-loop root vector. Including live S/C/K,
  E0/global/K-spine, deferred env drops, and driver-C preserves `MidloopRootUnionComplete`; omitting the machine,
  deferred-env, or driver-C channel violates it.
- `tla/AbstractGCLiveNarrowing.tla`: checks the C2 abstract-GC live-field narrowing discriminator. When all three
  fields omitted by `collect_live_values` are dead for the next transition, `NoFutureTouchFreed` holds; making any
  one of `remaining_matches`, `remaining_alts`, or `remaining_templates` future-live while still omitted violates it.
- `tla/DriverCPublication.tla`: checks that public eval entry publishes driver-C (`MettaState.source/output`) to
  the safepoint channel before midloop/rendezvous roots can be built. Omitting that publication violates
  `DriverCVisibleOnSweep`, matching a caller-held source/output value that can be freed while eval is still live.
- `tla/BatchHandoff.tla`: checks the async rholang batch-result handoff. Holding a persistent handle until the caller
  copies worker results into `MettaState.output` preserves safety; omitting the handle or dropping it before the copy
  violates `NoPublishedBatchResultFreed`.
- `tla/SchedulerGcBoundary.tla`: checks the GC-facing scheduler/thread-pool boundary. Including active worker roots,
  live dispatch/collapse fan-out anchors, and async batch handoff roots while closing admission preserves
  `SchedulerBoundaryComplete`; omitting any modeled channel, or admitting a new worker after the root snapshot,
  violates it.
- `tla/DedicatedHandoff.tla`: checks the E1 dedicated-thread root-vector handoff. Failed send before consumption may
  run inline with the returned roots; response failure after successful send must skip, and a consumed request must
  have a reply attempt. Falling back inline after a consumed handoff violates `NoInlineWithoutRoots`; omitting the
  reply attempt violates `ConsumedRequestGetsReplyAttempt`.
- `tla/DedicatedSingleRegime.tla`: checks the E1 dedicated-thread single-regime rule. Suppressing all legacy
  cooperative producers preserves `NoDriverlessRequest`; leaving default, session, parallel, or cron ungated violates it.
- `tla/DepthZeroSafepoint.tla`: checks the E1 cooperative-safepoint depth-zero rule. Guarding depth zero preserves
  `DepthZeroDoesNotPark` and `DepthZeroDoesNotDropGuard`; omitting the guard lets a non-participant park/drop path run.
- `tla/PollEdgeContribution.tla`: checks the E1 GC-pending poll-edge contribution order. Collecting roots, dropping
  the guard, publishing roots, and then waiting preserves `WaitAfterContribution`; waiting with publication omitted
  violates it.
- `tla/ConcurrentTriggerBackstop.tla`: checks the E1 FANOUT rendezvous-trigger handoff. A successful trigger leaves
  a posted driver request; spawn/send failure must run the resume backstop. Omitting either failure backstop leaves
  `GC_REQUESTED` pending without a driver.
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
- `tla/SelectiveChoicePointRoots.tla`: checks the E3 selective-CESK choice-point root obligation. Including all
  re-enterable choice families preserves `NoReenterableChoiceFreed`; omitting VM, JIT, or trampoline choice roots
  violates the invariant.
- `tla/SerializableContinuationSlice.tla`: checks the E4 serializable-continuation slice obligation. Including S/E/K
  and the reachable store child preserves `NoRestoredFutureTouchFreed`; omitting the continuation root or a reachable
  child violates the invariant.
- `tla/StartedCycleGate.tla`: checks the E5 straddle gate. Gating re-park on `GC_CYCLE_STARTED` avoids phantom
  re-parks during teardown; gating on `GC_CYCLE_GEN` violates `NoPhantomRepark`.
- `tla/GenerationResume.tla`: checks the E1/E5 worker-resume rule. Generation-gated resume with an end-of-cycle
  bump preserves `EndedCycleCanResume` even after a back-to-back request; boolean `GC_REQUESTED` resume and
  generation resume without the end bump both violate it.
- `tla/RendezvousProgress.tla`: checks the combined dedicated rendezvous liveness obligation. With participant
  contribution, panic cleanup, generation bump, generation resume, and resume notification, every posted rendezvous
  reaches `EventuallyRendezvousResumed`; omitting participant contribution, cleanup-on-panic, the generation bump,
  generation resume under a back-to-back request, or the resume notification violates the temporal property.
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
- `tla/NurseryBackpressure.tla`: checks the C1.c nursery-backpressure trigger. Opening a subsequent segment sets
  `nursery_full_pending`, the driver folds that pending flag into `minor_due`, and promotion clears the stale trigger;
  disabling the signal, the fold, or the clear violates the named discriminator invariant.
- `tla/YoungAllocationOdometer.tla`: checks the C1.c young-allocation odometer. Production accounting counts reused
  young slots and fresh bump allocation and resets on promotion; omitting reuse count, bump count, or reset violates
  the corresponding invariant.
- `tla/MajorMinorScheduler.tla`: checks the C1.c scheduler choice. Production guards pass; allowing live-major
  deferral below level 3, allowing cap-major deferral, or allowing cadence-major deferral violates the corresponding
  scheduler invariant.
- `tla/CapFloorAntiThrash.tla`: checks the B.5 cap-floor update. Production rules prevent a futile cap major from
  immediately re-firing and clear stale floor state after release; omitting the raise or the clear violates the named
  discriminator invariant.
- `tla/MajorWatermarkRearm.tla`: checks the B.4 major watermark rearm. Production rearm from post-promote old-live
  with `GROWTH = 2` prevents immediate live-major re-fire and requires doubled old-live growth; omitting the rearm or
  the growth factor violates the corresponding discriminator invariant.
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
- `tla/SATBFinalRemarkPremarked.tla`: checks the E2 allocate-black/final-remark interaction. If a final-rendezvous
  root was already marked by allocate-black, the final remark must still traverse through it with a separate visited
  set; a newly-marked-only traversal leaves its white child sweepable.
- `tla/SATBFinalSweepResult.tla`: checks the E2 final-sweep result obligation. If the final sweep's rendezvous gate is
  unexpectedly closed, the driver must treat the false result as a SATB abort and run the STW backstop; ignoring the
  result lets the request finish without either sweeping or falling back.
- `tla/SATBYoungSweepStaleOldMark.tla`: checks the stale-old-mark obligation behind the full-major E2 SATB final
  sweep. Clearing old marks passes; a young-only final sweep leaves an old SATB mark stale and violates
  `NoStaleOldMark`.
- `tla/SATBAbortFallback.tla`: checks the E2 abort-to-STW backstop. If the SATB path aborts after cleanup, the
  driver must re-request and run a fresh STW rendezvous before treating the request as handled.
- `formal/rocq/gc/SATBAbortFallback.v`: proves the abstract SATB abort-control obligation paired with the TLA+
  discriminator. A completed request after SATB abort has a collection only if the abort posts and runs the requested
  STW fallback; missing fallback or running a fallback without a request exposes a driver gap.
- `tla/RegistryIsolation.tla`: checks the A5/E1 root-source boundary. Index mode may build roots from structural CESK
  readers and explicit driver transport roots only; enabling the legacy `RootProvider` registry as an index root source
  violates `NoRegistryInIndex`.
- `tla/RendezvousQuiescenceIndependence.tla`: checks that a FANOUT rendezvous sweep is enabled by participant
  contribution and witness publication while active workers remain nonzero. Reintroducing the old `active == 0`
  global-quiescence gate violates `ReadyCanSweep`.
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
- `tla/SideArenaCoLocation.tla`: checks the variable-length node co-location obligation. Interning a side payload in
  the same segment before publishing the owning node preserves `NoBadSideRead`; publishing with a wrong-segment side
  payload or publishing the node before the side payload is ready violates it.
- `tla/ConcurrentBumpFreshOnly.tla`: checks the shared-allocation/free-list split. Production fresh-bump-only
  concurrent allocation and exclusive reuse pass; allowing concurrent free-list consumption or nonexclusive reuse
  violates the named discriminator invariant.
- `tla/SideFreeQuiescence.tla`: checks the side-payload free lifetime obligation. Quiescent side-free with shadow
  clearing and non-quiescent deferral preserve `NoDanglingSideUse`; freeing on a non-quiescent arm or skipping the
  shadow clear admits a dangling side-payload dereference.
- `tla/SideReclaimSnapshot.tla`: checks the side-payload ownership obligation exposed by the E1 ASAN stress arm.
  Reclaim-time `(segment, side-column, index)` snapshots plus dropping pending snapshots on whole-segment reset
  preserve `NoLiveSideFreed`; rereading a reused node slot at drain time or keeping reset-segment snapshots admits a
  live side-payload free.
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
- `IndexHeap` snapshots reclaimed side payload owners at sweep time, drops pending snapshots for whole-released
  segments, and frees pending side payload boxes only through the two `phase == "quiescence"` guarded calls to
  `free_pending_side_reclaims`; rendezvous and midloop collection pass non-quiescence phase strings, and both
  side-free paths clear `INNER_SHADOW` before returning to code that can materialize/dereference values again.
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
- The typed native K-spine source shape is pinned to `current_work`, `work_stack`, and `continuations`. The
  trampoline publishes the popped `WorkItem` into `current_work` before running it, and `collect_k_spine` reads that
  in-flight work item before the pending work stack and continuation stack.
- Live work item and continuation frames include fork-local environment roots in index mode. The source-coupling gate
  pins `fork_for_nondeterminism`'s five CoW-local Addr-bearing maps and the corresponding `collect_fork_local_roots`
  reader, asserts the `frame_env` classifiers remain exhaustive, and checks the three narrowed live-K arms re-read the
  frame env before skipping post-cut-dead iterator fields.
- C2 abstract-GC narrowing is source-pinned to exactly the three post-cut iterator fields:
  `remaining_matches`, `remaining_alts`, and `remaining_templates`. Each narrowed field has a debug assertion at the
  next transition read site proving it is not read on the cut-fired path, plus a differential unit test showing
  `collect_live_values` equals `collect_values` when no cut fired and drops only the dead iterator field after cut.
- The default single-threaded mid-loop branch is pinned to build `midloop_roots` from `collect_machine_roots_live`
  (live S/C/K plus persistent E0/global/K-spine), then deferred environment drops, then `collect_safepoint_roots`, and
  `run_collection_if_triggered_midloop` must consume that same vector. Its gate remains index-only,
  single-evaluator, and pre-worker-spawned, with no separate mid-loop feature switch.
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
  every side-bearing reuse, bump, and concurrent bump allocation interns side payloads into the owning node's segment
  before publishing that node.
- D-RLOCK/B2 shared allocation is source-pinned as fresh-bump-only: `IndexArena::alloc_bump`,
  `IndexArena::try_bump_in`, and every `alloc_*_concurrent` heap entry are checked to avoid `pop_young_free_slot` and
  `write_reused`; free-list reuse remains in the exclusive `&mut` allocation path.
- The E2 SATB marker path is source-coupled: `gc_driver_satb_rendezvous_cycle` arms `enter_satb_marking`, closes the
  initial rendezvous before `mark_concurrent_roots`, requests a final rendezvous, drops the SATB guard before
  `sweep_after_concurrent_mark`, asserts that the final sweep actually ran before dropping the final roots, and has
  RAII cleanup for an open rendezvous/request on panic.
- The E2 abort path is source-coupled: `gc_driver_rendezvous_cycle` checks the SATB `catch_unwind` result, and an
  abort calls `gc_driver_stw_rendezvous_cycle`, which re-issues `request_gc`, acquires a fresh rendezvous, prepares
  structural roots, runs the normal STW rendezvous collection, drops roots, and then closes the cycle.
- `mark_concurrent_roots` marks under `global_index_heap().read()` through `IndexHeap::mark_concurrent`; the final E2
  sweep takes `global_index_heap().write()`, re-marks and revisits the final rendezvous roots through
  `IndexHeap::mark_revisit`, runs a full `heap.sweep()`, then
  promotes and clears all mark bits. The FANOUT trigger is suppressed while `satb_marking_in_progress()`.
- The scheduler/thread-pool boundary is source-coupled at the GC-facing edges: dispatch/collapse workers wait for an
  in-progress dedicated rendezvous before `EvalGuard::enter`, live fan-outs are registered through
  `register_live_dispatch`, async batch results carry a persistent safepoint handle until the caller copies them into
  `MettaState.output`, active workers publish `ThreadContribution` roots into `WORKER_ROOT_BUFFER`, and the driver
  root union drains worker, safepoint, live-env, and live-dispatch channels before sweep.
- The dedicated rendezvous progress proof is source-coupled: `request_concurrent_collection` sets `GC_REQUESTED`
  before posting `CollectRendezvous` and runs `resume_workers` on handoff failure; worker park and finish paths both
  publish/bump/notify; `EvalGuard::drop` finish-bumps on panic/non-finisher exit while a request is pending;
  `run_open_stw_rendezvous_cycle` closes the cycle after `catch_unwind`; `SatbRendezvousCleanup::drop` closes any
  open SATB cycle; `end_rendezvous_cycle` bumps `GC_CYCLE_GEN` and notifies gen-waiters; `resume_workers` clears
  `GC_REQUESTED` under the resume mutex before notifying.
- `SATBTriggerSuppression.v` and `SATBTriggerSuppression.tla` pin that FANOUT trigger guard: a worker watermark
  trigger requires the dedicated collector gate, another live mutator, no pending request, `!satb_marking_in_progress`,
  and `watermark_due_for_concurrent`; the TLC negative config removes the SATB guard and reaches an overlapping trigger.
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
- E3 selective-CESK choice-point source order is pinned for the current partially unified implementation: VM
  `collect_roots_into` walks the `GenericChoicePointStack` through `ContinuationAddr` handles; JIT
  `collect_jit_roots_into` bridges the live native `choice_points[..]` prefix into a `SpineStore<JitChoicePoint>`
  before walking saved chunk constants, saved stack-pool values, and value/space-match/chunk/rule-match alternatives;
  production JIT execution owns the contiguous ABI buffer through `JitChoicePointSpineOwner` and passes
  `JitContext.choice_points` only as a transient pointer view at the executor/backtracking/arena entry points; native
  JIT stack-save-pool allocation is non-wrapping and bails out before publishing a choice point when no fresh slot
  exists; `StoredBranchCoroutine::collect_values` walks remaining branches, branch bindings, and yielded values;
  trampoline fan-out/collapse K entries are normalized at the loop boundary into
  `Continuation::TrampolineFanoutSpine` handles backed by `SpineStore<Continuation>`, root walking resolves those
  handles through the bridge node walker for rule-match, amb, match-template, collapse-eval, parallel-dispatch, and
  parallel-collapse frames, `process_continuation` resolves/removes the handle before executing the payload, and
  `collect_live_values` sets `include_remaining: !cut_fired_peek(*cut_barrier)` for the three cut-pruned remaining
  iterators. `UnifiedChoicePointRestore.v` is mandatory in the formal harness and composes these four production
  carriers into the aggregate Selective CESK* root-then-restore safety theorem.
- R-FL source order keeps push guarded by `set_free_bit`, pop clearing the bit before reuse/discard, and released
  segments draining listed entries before dropping the segment bitmap.
- C1 source order keeps reuse current-segment-only, successful bump allocation guarded by the current segment, segment
  retargeting monotone, and promotion at `current_seg`. `IndexHeap::mark_young` marks only young nodes but traverses
  all reached nodes through `child_addrs_for_mark`, including `SpaceHandle::collect_gc_values`, so an old first-class
  space cannot hide a live young value from `sweep_young`.
- C1 nursery-backpressure source order pins the allocator-to-GC trigger: opening any segment after the initial segment
  sets `nursery_full_pending`, every collection-scheduling probe folds `nursery_pending` into the minor trigger, and
  promotion resets the young-allocation odometer before clearing the pending signal.
- C1 young-allocation odometer source order is pinned: the only three `young_alloc_bytes` increments are
  `write_reused`, `alloc_bump`, and `try_bump_in`, each by `size_of::<N>()`, and `promote_young` resets the odometer
  before clearing `nursery_full_pending`.
- C1 scheduler source order pins the bounded level-3 major/minor inversion: level 3 means at least `2 * YOUNG_BUDGET`
  young pressure, `minor_due` is computed before the no-due return, `do_major` may suppress a due live-growth major
  only when `level == 3 && minor_due && !cap_major && !cadence_major`, and the cadence counter resets on major but
  increments on minor.
- B.5 cap-floor source order is pinned in both full-major paths: the cap predicate uses
  `max_bytes().max(CAP_FLOOR)`, a cap-triggered major that releases no segment stores current `committed` into
  `CAP_FLOOR`, and a segment-releasing major clears `CAP_FLOOR` to zero.
- B.4 major-watermark source order is pinned in both full-major paths: `GROWTH` is exactly 2, the live-major trigger
  uses `old_live > WATERMARK.max(min_threshold())`, promotion runs before measuring `old_live_after`, and the rearm
  stores `old_live_after.saturating_mul(GROWTH).max(min_threshold())`.
- `published_gen` writes remain restricted to stale-stamp reset plus the genuine `note_reified_park` stamp, with
  worker root-buffer publication before the stamp. `worker_finish_into_buffer` is source-coupled to avoid
  `note_reified_park`, so non-reified finishers cannot satisfy the rendezvous witness.
- The V4 witness slot is acquired before `N_THREADS++`, released only after the true outermost `EvalGuard::drop`
  count decrement, never released by safepoint drops, and re-stamped before straddle re-park publication.
- `worker_cooperative_safepoint` returns at EvalGuard depth zero before building park roots or calling
  `drop_eval_guard_for_safepoint_full`, so depth-zero MORK/type-fixpoint callers cannot become uncounted rendezvous
  participants or trip the depth assertion.
- `request_concurrent_collection` sets `GC_REQUESTED` before attempting the `CollectRendezvous` handoff, but both
  driver-spawn failure and send failure call `resume_workers`; `resume_workers` clears `GC_REQUESTED` under
  `RESUME_MUTEX` before notifying workers.
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
- The obsolete D1/D2 `METTATRON_INDEX_GC_PARALLEL` / `rendezvous_enabled` production gate is retired. The live
  FANOUT>0 rendezvous path is governed by `dedicated_gc_enabled()`, which now follows index mode directly; source
  coupling rejects reintroducing either retired rendezvous env switch in `gc_allocator.rs` or the active GC scripts.
- The index collector has no production environment-variable off switch. The quiescence, mid-loop, and rendezvous
  gates are safety predicates only; source coupling rejects reintroducing the retired index-GC disable hook.
- The R-FL no-recycle/swept-slot diagnostic and env-gated free-list shadow checker are retired from production source.
  Source coupling now rejects the behavior-changing oracle, collector-read bypass, and `METTATRON_INDEX_GC_FREELIST_CHECK`;
  the live R-FL obligation is the persistent free-bit invariant plus its Rocq/TLA/source-coupled checks.
- The reclaim-time side-owner bug is modeled as a side-reclaim snapshot obligation: side payload frees drain the
  saved owner snapshot captured before node-slot reuse, and segment reset drops any pending snapshot before side
  indices can be reused. `SideFreeQuiescence.v` and `SideReclaimSnapshot.tla` cover the positive and negative cases.
- The K-spine current-work gap is modeled separately from the pending work-stack: a suspended trampoline activation
  roots its in-flight work item before a nested evaluator can collect. `KSpineCurrentWork.v` and its TLC discriminator
  fail if `current_work` is omitted.
- VM native locals live across nested CESK evaluation are now a formal K-spine leaf obligation. `VmNestedLocals.v`
  proves that pre-eval locals, dispatch RHS locals, rule-match vectors, saved bindings, combo vectors, and accumulated
  outcomes survive collection when published through `VmLeaf::ValueVec`; `VmNestedLocals.tla` fails if either the
  pre-eval locals or rule-match vector class is omitted.
- The E1 stress ASAN counterexample showed a pre-spawn FANOUT watermark could still run the non-rendezvous midloop
  collector (`worker_ever_spawned()==false`) before the first worker existed. The corrected obligation is split:
  FANOUT closes midloop non-rendezvous collection, but true quiescence (`active_evaluator_count()==0 && n_threads()==0`)
  remains fanout-independent because no evaluator/native stack is live. `NonRendezvousFanoutGate.v` and
  `NonRendezvousFanoutGate.tla` pin that split while preserving the `FANOUT=0` single-threaded midloop gate.
- The follow-on liveness case is a one-participant rendezvous: with fanout enabled and exactly one active mutator,
  the watermark trigger posts `CollectRendezvous`, the mutator self-roots at the next safepoint, and the driver waits
  on the same reified-witness protocol. `MC_RendezvousProgress_one_participant.cfg` covers this `N=1` progress path.

## Harness

Run:

```bash
bash scripts/verify_cesk_gc_formal.sh
```

The harness derives paths from its own location, uses `target/tlc-formal-small` for small TLC logs/metadata by
default, runs Rocq under `systemd-run`, skips supplemental Lean mirrors unless `RUN_LEAN_MIRRORS=1` is set, and
includes the small TLC positive/negative discriminators. The adjacent GC
gate scripts likewise default log/build scratch to repo-derived `target/...` directories (`target/gc-logs` or a
script-specific subdirectory) while preserving caller overrides such as `LOG_DIR`, `LOG_ROOT`, `SCRATCH_ROOT`,
`OUT`, `P`, `PGO_DIR`, and `CARGO_TARGET_DIR`; large gates must not spill into `/tmp` unless the caller explicitly
chooses that.

Before compiling proofs, `scripts/verify_cesk_gc_proof_hygiene.sh` rejects Rocq proof shortcuts (`Admitted`, `admit`,
`Axiom`, `Parameter`, `Conjecture`, `Abort`) in the mandatory CESK GC proof directory and verifies every
`formal/rocq/gc/*.v` file is enumerated by the formal harness. If supplemental Lean mirrors exist, the same hygiene
pass scans them for Lean proof shortcuts (`sorry`, `admit`, `axiom`, `constant`, `opaque`, `unsafe`) without making
them part of the mandatory gate.

The formal harness also runs `scripts/verify_cesk_gc_tlc_hygiene.sh` before compiling proofs. That check parses every
`run_tlc` entry, verifies labels/configs are unique, requires every referenced TLA+ module and config to exist, requires
negative TLC runs to carry a discriminator pattern, rejects any tracked `tla/*.cfg` or `tla/MC_*.tla` wrapper that is
neither run nor explicitly classified, and rejects any tracked `tla/*.tla` module that is not either run directly,
imported by a run/classified wrapper, or explicitly classified. The only classified exclusions are legacy slab
mark-sweep models, the older non-generational store-centric mark-sweep wrapper/configs, and larger CESK generational
discriminator configs whose disk-light small counterparts are the default gate.
