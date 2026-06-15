# CESK GC formal verification ledger

This ledger tracks the mechanically checked proof artifacts for the CESK-based `index-gc` collector. It is not about
the legacy slab mark-sweep collector.

Rocq is the load-bearing proof assistant for this gate, paired with TLA+ model checking and source-coupling checks.
Existing Lean files are supplemental mirrors, but the default formal harness still compiles every tracked
`formal/lean/gc/*.lean` mirror and proof hygiene rejects Lean proof shortcuts before the Rocq/TLA wall runs.

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
The E5 system-level TSan gate (`scripts/e5_satb_tsan.sh`) on 2026-06-10 extended that focused unit check to the
WHOLE concurrent SATB collector running under load at HEAD `a5996d1f`: a `-Zsanitizer=thread -Zbuild-std`
`--features index-gc` release binary ran `Robot.metta` and `examples/cesk-gc/stress_alloc.metta` at `FANOUT=8`
with `MIN_BYTES=131072`. `Robot.metta` reported 89 index cycles (88 rendezvous SATB-major, 1 quiescence) with the
dedicated GC thread marking under the shared read lock while 8 worker threads allocated through
`alloc_*_concurrent(&self)` (roots 39426..69181, reclaimed_slots 84208..132372 per cycle), produced the correct
detection answer, and ThreadSanitizer reported 0 data races and 0 warnings; `stress_alloc.metta` reported the same
rendezvous path race-free. This is the design-doc E5 TSan target — the read-locked concurrent mark racing the
concurrent allocator — exercised end-to-end at scale with no race.
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
On 2026-06-14, the parallel completion proof was strengthened from liveness-only to liveness plus no-silent-drop:
after `WaitForParallel` or `WaitForParallelCollapse` observes completion for a required-complete group, successful
output requires every spawned slot to have stored a result slot. A missing dispatch slot now returns
`ParallelDispatchMissingResults`; a missing collapse slot returns `ParallelCollapseMissingResults`; an impossible
required-complete completion with `remaining != 0` returns the matching `Parallel*Incomplete` error. These are error
values, not valid smaller result sets. TLC now checks the positive rule and rejects the old successful-drop behavior
with `CollapseCompletion_slot_bug.cfg`.
Validation evidence from the same day: the capped full `scripts/verify_cesk_gc_formal.sh` harness passed after this
change; a debug FANOUT=8 `Robot.metta` smoke that hit the pre-existing `ProcessRuleMatches` freshened-key canary now
returned `(Error ParallelDispatchMissingResults (WaitForParallel 4))` instead of a silent empty result; the capped
release FANOUT=8 `Robot.metta` smoke produced the expected frisbee+orange detections with `INDEX_GC_CYCLES_RUN=54`
and `INDEX_GC_MIDLOOP_CYCLES=0`.
The follow-up proof obligation is `ProcessRuleMatches` binding-sidecar projection. A capped debug Robot repro on
2026-06-14 (`systemd-run --user --scope`, `MemoryMax=12G`, `MemorySwapMax=0`, `CPUQuota=400%`) showed worker panics
at `eval_loop.rs:9397` with 1039/1043 freshened binding keys; the prior no-silent-drop correction correctly converted
those panics into `(Error ParallelDispatchMissingResults (WaitForParallel 4))`. A diagnostic rerun showed
`tracked_vars_hint_len=0`, so the failure was an over-eager debug assertion at a boundary with no tracked/consumer
liveness context, not a failed projection. The corrected invariant is conditional: a tracked branch-result boundary
must retain every result/tracked live key and any bound freshened dependency reachable from those visible values while
dropping stale freshened rule-epoch keys, but a no-context boundary deliberately preserves the full sidecar and defers
projection to a later consumer because fold/progn continuations may use bindings not syntactically live in the
immediate value. This is modeled by `formal/rocq/gc/BindingProjection.v` and `tla/BindingProjection.tla`; TLC checks
both the positive tracked projection and the positive no-context deferral, and rejects both missing-closure and
tracked-no-projection variants. Source coupling pins the Rust projection before the freshened-key canary in both eager
and lazy `ProcessRuleMatches` paths and pins the canary/release warning behind `tracked_vars_hint.is_some()`. The
capped debug Robot replay after the conditional canary produced the expected frisbee+orange detections with no
`ParallelDispatchMissingResults`.

## Checked obligations

- `formal/rocq/gc/BindingProjection.v` and `tla/BindingProjection.tla`: prove and model-check the conditional
  branch-result binding projection obligation. With tracked/consumer context, projection is a narrowing of the original
  sidecar; it retains all live keys, retains bound freshened dependencies reachable from visible values, drops stale
  freshened keys at tracked boundaries, and rejects a projection that would leave a visible binding dangling on a
  dropped bound freshened key. With no context, projection is explicitly deferred and the full sidecar is preserved.
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
  crossing any positive young budget entails the minor trigger.
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
  (`KSpineCurrentWork.tla`) rejects any omitted control-root component: current work, pending work stack, or
  continuation stack.
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
- `formal/rocq/gc/ParkedPhantomCycleGate.v`: proves the #309 phantom-future-park gate obligation (the branch-B/pump
  park path's twin of the E5 straddle gate). If parking requires a REAL cycle (`current_cycle_started() == my_gen`
  or `is_gc_requested()`), every park resumes and a phantom (post-close) generation can never park — closing the
  masked-parker missed-root hazard by construction; the companion theorem captures the ungated negative shape where
  a phantom park strands forever (the captured wedge: autopsy rep 17).
- `formal/rocq/gc/RequestReassertAtOpen.v`: proves the #309 coalesced-request witness-starvation fix obligation.
  `request_concurrent_collection` posts two effects (flag + channel message); two coalesced requests leave two
  messages and cycle 1's close clears the flag, so cycle 2 opens with the flag false and no safepoint ever stamps.
  If the driver re-asserts `request_gc()` at every rendezvous open, then `open ⇒ requested` holds and every opened
  cycle closes; the companion theorem captures the unreasserted starved-open negative shape (the captured wedge:
  validate rep 36).
- `formal/rocq/gc/CollapseCompletion.v` and `formal/lean/gc/CollapseCompletion.lean`: prove the E1 parallel
  dispatch/collapse completion obligation. If every spawned worker exits and the RAII completion guard drops on both
  normal and panic-unwind exits, then the parent wait cannot be stranded by a skipped worker decrement; the companion
  theorem also captures the historical negative shape where a panic edge skips completion and parent observation is
  impossible. The same artifact now proves the release-mode no-silent-drop obligation: parent success must imply every
  spawned required-complete dispatch/collapse result slot was stored, and any missing slot must force an error rather
  than a successful subset.
- `formal/rocq/gc/WorkerAdmission.v` and `formal/lean/gc/WorkerAdmission.lean`: prove the E1 worker-admission
  obligation. If collection admission is closed before the participant snapshot and no worker can join during the
  collection window, then every worker live at sweep was in the snapshot and is retained by ordinary mark/sweep.
- `formal/rocq/gc/SchedulerSpawnLatch.v` and `tla/SchedulerSpawnLatch.tla`: prove and model-check the eval-worker
  spawn-latch ordering obligation. If `note_worker_spawned()` is stored before an eval worker is submitted to the
  pool, then any later mid-loop collection check that can observe the worker must also observe the sticky latch, so
  `gate_open_midloop` is closed by `!worker_ever_spawned()`. The TLC negative configs reject both spawn-before-latch
  and missing-latch handoff shapes with `NoWorkerWithMidloopGateOpen`. The proof forced a source correction in the
  Rholang async batch evaluator: API-level batch parallelism can exist even when `FANOUT_DEPTH=0`, so the
  `index-gc` latch is no longer gated on the dedicated collector.
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
- `formal/rocq/gc/ConcurrentReusePressureProgress.v`: proves the E1 allocator-progress refinement added after the
  default FANOUT ASAN gate exposed reclaim pressure without enough reuse. If `try_write` loses while a current-segment
  free slot is observed, the factory policy chooses the exclusive allocation path rather than the shared fresh-bump
  path; the existing exclusive reuse proof can then consume the reclaimed slot, while the concurrent path remains
  fresh-only and free-list-separated.
- `formal/rocq/gc/IndexAllocatorRefinement.v`: composes the Rust source-coupled allocator facts into the abstract
  contract consumed by `CESKCollectorSafety.v`: fixed-slot reads observe published segment/slot bytes, side-payload
  reads observe same-segment published side entries, published allocations satisfy allocate-black, shared concurrent
  allocation is disjoint from exclusive free-list reuse, and the full `FreeBitTracksFreeList` invariant is preserved:
  `free_bit(addr)`, `OnFreeList addr`, concrete list membership, and `NoDup` all agree.
- `formal/rocq/gc/SideFreeQuiescence.v` and `formal/lean/gc/SideFreeQuiescence.lean`: prove the side-payload
  lifetime obligation. If side payload boxes are freed only on the true-quiescence arm, stack laundered references
  imply a non-quiescent evaluator, and the materialization shadow is cleared before any future dereference, then
  dropping reclaimed side boxes cannot create a dangling future dereference. Non-quiescent collections therefore defer
  side-box freeing.
- `formal/rocq/gc/SideReclaimRefinement.v`: composes the source-coupled side-reclaim ownership protocol. Side reads
  observe published same-segment payloads, future side reads cannot target freed pending snapshots, freed side boxes
  have an owner snapshot plus quiescent full-mark drain and shadow clear, marked owners that still own the side are
  retained, released segment resets drop pending snapshots, and duplicate reports require fresh free-owner acquisition.
- `formal/rocq/gc/QuiescentSideIndexReuse.v`: proves the side-column allocator-progress refinement discovered by the
  E1 V4 ASAN stress timeout. A side index may enter the reusable stack only after a full true-quiescence drain has
  freed it and consumed the reclaim snapshot; non-quiescent or deferred-pending indices cannot be reused. When
  reusable pressure exists, `SideColumn::push` consumes a reusable index without increasing the side-column
  high-water, so a long-lived current segment is not forced into append-only side-page growth after quiescent drains.
  The `GenerationGuardSafety` section certifies the SAFETY half (added with the side-index ABA fix, commit
  `f6dd7a76`): because `266d19d` made side indices RECYCLABLE, a stale `SideReclaim` snapshot `{dead owner, idx}`
  could free a cell a LIVE node had reused — the `live Spanned slot` use-after-free at `index_heap.rs:876`
  (gdb-confirmed: dead owner `Addr(2)` vs live reuser `Addr(220)` on span idx 43, freed by the guard-less
  `free_pending_side_reclaims` drain; only tripped under the greenwall `MAX_BYTES=1MiB` config, which forces a full
  major + side-drain every cycle). The earlier `owner_still_owns` drain guard FAILED (the stale owner bytes still
  name `idx`, so value-equality cannot tell the dead occupant from the live one). The fix stamps each
  `SideColumn::push` with a fresh, strictly-increasing per-cell generation (stored in `SpanRef`/`ChildRef`/`ByteRef`,
  captured in `SideReclaim`); `SideColumn::free(idx, gen)` drops the cell ONLY when the cell's current generation
  equals the snapshot's captured one. `gen_guard_never_frees_live` proves a dead-owner snapshot whose generation
  matches the current cell forces — by generation injectivity (each `push` strictly bumps it) — `owner = current`, so
  the current occupant is that dead owner, hence not live; the non-vacuity `idx_only_free_drops_live_reuser` shows the
  pre-fix idx-only free drops the live reuser while the guard does not. Rocq-only (a deductive safety obligation with
  no temporal component ⇒ no TLA mirror, per write-each-obligation-once). Source-coupled on the refs' `gen` field,
  `push`'s gen stamp, the snapshot's gen capture, and `free`'s gen check. Verified: 84-fixture cross-fixture repro
  EXIT 0 (was the panic), greenwall 483/0 both modes + debug-oracle 0-panics, slab byte-identical (4387/0).
- `formal/rocq/gc/RendezvousSideReclaimProgress.v` + `tla/RendezvousSideReclaimProgress.tla`: prove the #273 BOUNDED
  side-payload reclaim PROGRESS obligation under the default dedicated (rendezvous) collector. A rendezvous/minor sweep
  that reclaims an owner slot only APPENDS a `SideReclaim` snapshot (`append_pending_side_reclaims`) — the payload Box
  stays committed, deferred — while a true-quiescence MAJOR drains the ENTIRE pending vec exhaustively
  (`free_pending_side_reclaims`, `std::mem::take` then iterate every entry). The trigger `pending_side_major`
  (`phase == "quiescence" && pending_side_reclaims > 0`) FORCES that major the moment a quiescence point has pending
  reclaims, so `pending` is emptied every quiescence and committed side storage cannot grow unboundedly across cycles.
  `side_committed_bounded` proves `committed_side <= live_side + pending` under the committed invariant;
  `forced_drain_reduces_committed_to_live` proves the forced major reduces committed storage to exactly the live
  high-water. Non-vacuity `without_pending_trigger_side_grows_unbounded` exhibits the pre-`266d19d` rendezvous-only
  world where, with no forced drain, `pending` grows past any bound. The TLA discriminator `PendingBounded` PASSES with
  `PendingSideTrigger = TRUE` and is VIOLATED with `FALSE`. Admit-free + axiom-free; source-coupled on
  `pending_side_major`, the exhaustive `mem::take` drain, and `append_pending_side_reclaims`. Design B (defer the side
  free to the next quiescence) is what is implemented; design A (drain side payloads every rendezvous) is deferred to
  Finding 2's `'static` confinement, since a parked worker may otherwise hold a laundered side reference across the
  park (the `index_heap.rs` rendezvous-vs-quiescence safety comment). No `src/*.rs` logic change ⇒ byte-identical.
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
- `formal/rocq/gc/RholangBatchCompletion.v` and `tla/RholangBatchCompletion.tla`: prove and model-check the async
  rholang batch completion obligation. Each spawned batch worker owns an RAII completion guard whose Drop is the sole
  decrement/notify site, so a panic-unwind from `eval_trampoline` cannot strand the async caller with `remaining > 0`.
  The negative TLC model without that guard violates `EventuallyParentDone`; the missing-slot model rejects successful
  completion with an unstored batch slot via `NoSilentBatchSuccess`.
- `formal/rocq/gc/SchedulerGcBoundary.v`: proves the GC-facing scheduler/thread-pool boundary. If active workers,
  live dispatch/collapse fan-outs, and async batch handoff values are all mapped into driver root channels, and
  collection admission prevents newly joined workers during the sweep window, mark/sweep cannot free a scheduler-held
  live address. This intentionally does not claim general scheduler fairness or work-stealing correctness.
- `formal/rocq/gc/SchedulerFanoutProgress.v`: composes the FANOUT scheduler progress obligations. Given the existing
  trigger backstop, posted-driver or SATB-abort-to-STW fallback, generation-based resume, participant contribution,
  and completion-guard premises, every active parked worker is resumed and the parent wait cannot be stranded by a
  missing worker completion drop. The temporal eventuality/discriminator layer remains the paired TLC suite, and the
  end-to-end threading envelope imports this contract so participant accounting, parked-worker resume, and completion
  guard drops are checked alongside scheduler, cron, WorkPool, and GC-boundary interleavings.
- `formal/rocq/gc/SchedulerFanoutAdmissionCompleteness.v` and
  `tla/SchedulerFanoutAdmissionCompleteness.tla`: prove and model-check the live fanout admission contract. The WFST
  `parallelism_degree` is an admission gate (`1` sequential, `>1` eligible for the purity/depth/pool/budget gates),
  not a partial-spawn cap. Once admitted, stack-safe fanout represents every branch slot; the fixed work pool,
  queue-pressure gate, per-depth quota, completion guard, and cancellation protocol bound execution. TLC rejects both
  degree-capped partial fanout (which drops required branches) and admission with the degree gate removed.
- `formal/rocq/gc/SchedulerActiveFanoutGate.v` and `tla/SchedulerActiveFanoutGate.tla`: compose the production
  rule-match fanout gates on the active direct-dispatch path. A dispatch implies the branch threshold, WFST degree,
  purity/dynamic-eval, depth, active-worker pool, and budget gates; once dispatched, every admitted branch slot is
  represented. The E2E threading proof consumes the standalone purity, budget, and dispatch-slot representation
  lemmas for active fanout counterexamples. TLC rejects missing-purity, missing-budget, and partial-dispatch variants.
- `formal/rocq/gc/CollapseFanoutAdmissionCompleteness.v` and
  `tla/CollapseFanoutAdmissionCompleteness.tla`: prove and model-check the matching input-completeness contract for
  `collapse` and `collapse-bind`. The collapse threshold is an admission threshold, not a spawn cap; once admitted,
  `parallel_collapse_dispatch` represents every collapse result item. This complements `CollapseCompletion.v`, which
  proves successful completion cannot silently omit a spawned result slot. TLC rejects both threshold-capped partial
  collapse fanout and admission with the threshold gate removed.
- `formal/rocq/gc/SchedulerTransducerParallelism.v` and `tla/SchedulerTransducerParallelism.tla`: prove and
  model-check the WFST transducer parallelism contract used by the evaluator's `parallelism_degree > 1` gate. The
  default table never constructs degree 0; only branch-parallel classes can cross the default fanout gate; branch-aware
  transduction uses every available branch before the cap and respects the cap; and a zero cap degrades to sequential
  degree 1 instead of an invalid degree 0. The E2E threading proof consumes the standalone branch-degree maximality,
  gate-soundness, and zero-cap theorems rather than reproving those facts by local simplification. TLC rejects the old
  zero-cap shape, an underutilized-before-cap shape, and a forced non-branch degree > 1.
- `formal/rocq/gc/DedicatedHandoff.v`: proves the E1 dedicated-thread handoff ownership rule. Once a root vector has
  been successfully sent to the GC thread, response-channel failure cannot justify an inline fallback because the
  mutator no longer owns those roots; failed sends still return the roots for inline fallback. It also proves the
  channel-liveness obligation from the static channel audit: a successful `Collect` handoff carries a per-request
  response sender and the driver attempts a reply after catching the collection result. The threading E2E envelope
  now imports this theorem directly through `DedicatedHandoffSafe`, rejecting both inline fallback after consumed
  roots and consumed requests that omit a reply attempt.
- `formal/rocq/gc/GcDriverChannelProtocol.v` plus `tla/GcDriverChannelProtocol.tla`: discharges the target-scope pgmcp
  channel audit findings for `gc_driver`. The proof/model split the channel obligation from root ownership: the
  spawn-created request sender is paired with the driver receiver; a successful synchronous `Collect` carries a
  per-request response sender while the caller owns and waits on the paired receiver; the driver attempts that reply
  after handling `Collect`; and fire-and-forget requests (`CollectRendezvous`, `Shutdown`) create no response wait.
  TLC discriminators cover missing request sender, missing reply attempt, and orphan reply send variants. The
  threading E2E envelope now imports this theorem through `DriverChannelProtocolSafe`, carrying those request/response
  pairing facts and the fire-and-forget no-wait fact into the composed interleaving proof.
- `scripts/verify_cesk_gc_all.sh` determinism gate: hashes Robot FANOUT=8 output after sorting and alpha-normalizing
  generated `$__fr_<epoch>_` prefixes to `$__fr_E_`. The whole-wall DETERM failure on 2026-06-13 showed multiple raw
  hashes whose normalized outputs were byte-identical; the epoch number is a per-invocation freshening artifact from
  parallel rule matching, not semantic content. Source-coupling pins the normalizer before the hash. Runtime replays
  now run through `systemd-run` with `MemoryMax`, `MemorySwapMax=0`, CPU quota, and timeout caps, and force
  `METTATRON_INDEX_GC_MIN_BYTES=131072` so the index collector is non-vacuous during the replay. An oversized Robot
  replay must fail the gate instead of consuming host memory unboundedly.
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
- `formal/rocq/gc/E1SatbStwDriverProgress.v`: refines the E1 posted-driver progress premise to the shipped E2
  default path. A posted `CollectRendezvous` either completes the SATB initial/final rendezvous sequence and releases
  workers, or a SATB panic / closed final-sweep result becomes an abort that posts and runs a fresh STW rendezvous
  backstop. In both cases the request is cleared, parked workers resume, witness-ok is reset, and GC-in-progress is
  released. The threading E2E envelope now imports this theorem directly: its
  `E1SatbStwDriverSafe` component derives posted-driver release and rejects
  both a SATB success path that leaves witness/driver state uncleared and an
  abort path that omits the fresh STW rendezvous request.
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
  false. The `end_to_end_cesk_index_gc_safety` theorem ties the core no-UAF story into one checked boundary:
  future CESK touches covered by structural roots, driver channels, scheduler-held roots, SATB roots, or
  allocate-black publication are not freed; observed fixed-arena reads see fully published segment/slot state; and
  shared concurrent allocation cannot alias exclusive free-list reuse. The stronger
  `end_to_end_cesk_index_gc_allocator_cache_safety` capstone composes that theorem with the allocator/cache
  refinements: same-segment side-payload publication, allocate-black survival, concurrent/exclusive reuse
  disjointness, the full free-bit/free-list/no-duplicate invariant, epoch-protected `INNER_SHADOW` stale-entry
  exclusion, and the side-free quiescence/shadow-clear no-dangling-deref obligation.
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
- `tla/ParkedPhantomCycleGate.tla`: checks the #309 phantom-future-park gate on the branch-B/pump park path. With
  the cycle-reality gate (`started == my_gen ∨ requested`), `ParkedEventuallyResumes` and `NoPhantomCountBump`
  hold; the ungated config deadlocks at exactly the captured phantom-parked state (autopsy rep 17: `gen 95,
  started 94, requested F, park 2`).
- `tla/RequestReassertAtOpen.tla`: checks the #309 coalesced-request re-assert. With `ReassertAtOpen`, both queued
  messages' cycles close (`EveryCycleCloses`) and `OpenImpliesRequested` holds; the lost config deadlocks at
  exactly the captured starved-open state (validate rep 36: cycle open, `requested F`, witness unstamped).
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
- `tla/YoungAllocationOdometer.tla`: checks the C1.c young-allocation odometer over a finite positive budget domain
  chosen once at init, matching the runtime `METTATRON_INDEX_GC_YOUNG_BYTES`/default `OnceLock` budget. Production
  accounting counts reused young slots and fresh bump allocation and resets on promotion; omitting reuse count, bump
  count, or reset violates the corresponding invariant. The paired Rocq theorem proves the arithmetic obligation for
  every positive budget, so changing the cached runtime default preserves the proof shape. The default is 4 MiB as of
  pgmcp experiment #64 (`5f469201`): 51 Robot FANOUT=0 samples per arm accepted the 4 MiB treatment over the former
  2 MiB default (Welch p=1.54e-24, 95% CI for treatment-control [-349.68, -265.31] ms), after a GC-report diagnostic
  confirmed non-vacuous collection.
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
- `formal/rocq/gc/SchedulerPriorityFairness.v` and `tla/PriorityQueueAging.tla`: prove and model-check the
  priority-queue fairness obligation exposed by the scheduler audit. Dequeue must recompute age-adjusted scores before
  selecting work, and equal scores must break ties FIFO by sequence number. The positive TLC config preserves
  `OldPopsAfterAging`; the stale-score discriminator violates it by popping the younger high-base-priority task after
  age should have made the older task runnable first.
- `formal/rocq/gc/SchedulerClassificationLookup.v` and `tla/SchedulerClassificationLookup.tla`: prove and
  model-check the L1/L2 classifier-table insertion obligation used by MeTTa instruction reordering. Inserting a new
  head into a class must extend that class's contiguous L2 range and shift later L1 starts; failing to shift later
  starts violates range disjointness. This pins the lookup structure that separates pure parallelizable heads from
  state-mutating heads before the scheduler maximizes parallelism. The end-to-end threading envelope now imports this
  proof as `SchedulerClassificationRangesDisjoint`; the E2E no-shift discriminator keeps every other scheduler,
  WorkPool, cron, and GC-boundary setting safe and fails specifically on that classification invariant.
- `formal/rocq/gc/SchedulerWavefrontParallelism.v` and `tla/SchedulerWavefrontParallelism.tla`: prove and
  model-check the wavefront instruction-reordering obligation. A task may share a wave only when all dependency edges
  point to earlier waves; a ready task whose dependencies are already in prior waves and that has not already run must
  be placed in the current wave rather than deferred; and all-independent tasks may use one full-width wave. The cyclic
  same-wave discriminator violates `SameWaveIndependent`, while the deferred-ready discriminator preserves dependency
  safety but violates `NoReadyTaskDeferred`. These checks pin both sides of the Rust Kahn loop: malformed
  task indices/dependencies and cyclic unresolved suffixes degrade to sequential waves, while valid DAGs enumerate every
  initially-ready task and every newly-ready dependent into the earliest possible wave.
- `formal/rocq/gc/SchedulerEffectConflictCompleteness.v` and
  `tla/SchedulerEffectConflictCompleteness.tla`: prove and model-check the dependency-construction precondition behind
  wavefront instruction reordering. If two tasks conflict through effects, shared state, allocator/GC safepoints, or
  other non-commuting behavior, the dependency relation must contain an edge in at least one direction; dependency order
  plus that conflict-edge coverage implies same-wave conflict freedom. The complete-conflict and no-conflict TLC
  configs pass, while the missing-edge discriminator keeps dependency order vacuously true but violates
  `ConflictEdgesCovered`, exposing exactly the latent bug class where a caller forgets to encode an effect conflict.
- `formal/rocq/gc/SchedulerDirectFanoutWavefrontRefinement.v` and
  `tla/SchedulerDirectFanoutWavefrontRefinement.tla`: prove and model-check the production/direct-fanout refinement of
  the wavefront model. The active rule-match fanout path implements the all-independent single-wave case without
  calling `compute_wavefront`; a dependency-bearing instruction DAG cannot be justified by direct fanout and must use
  the general wavefront builder with complete dependency/effect-conflict edges.
- `formal/rocq/gc/SchedulerDynamicEvalGate.v` and `tla/SchedulerDynamicEvalGate.tla`: prove and model-check the
  dynamic-evaluation parallel-dispatch obligation. Dynamic heads (`eval`, `!`, `evalc`) can execute code supplied by a
  variable or user expression, so absence of a visible mutating head is not enough to admit the no-budget parallel
  path. The E2E threading proof consumes the standalone blocker lemmas for active dynamic eval, state mutation,
  strict IO, and gated no-budget exclusion. The fixed TLC config preserves `NoDynamicEvalParallelBypass`; the
  missing-gate discriminator violates it,
  matching the source correction that removes dynamic eval from known-pure classification and makes the scheduler's
  parallel-dispatch blocker reject dynamic-eval bodies before strict I/O filtering.
- `formal/rocq/gc/CronRecurringDispatch.v` and `tla/CronRecurringDispatch.tla`: prove and model-check pooled cron
  recurring-dispatch control. A recurring task that returns `false` or panics must set a durable stop flag before
  clearing `in_flight`, so the next due tick drops the recurrence instead of redispatching it. The model also proves
  the cron thread must claim `in_flight` before submitting pooled work; a due placeholder that observes an already
  claimed recurring task must requeue only and return without spawning a second worker. The no-stop discriminator
  violates `StopPreventsRedispatch`; the no-claim discriminator preserves stop-before-idle but violates
  `NoOverlapDispatch`.
- `formal/rocq/gc/WorkPoolOverflowCap.v` and `tla/WorkPoolOverflowCap.tla`: prove and model-check the adaptive
  work-pool overflow cap. The live helper computes
  `min(requested, max_overflow - live_overflow)`, so spawning overflow workers preserves
  `live_overflow <= max_overflow`. The uncapped discriminator reproduces the old behavior where a direct
  `spawn_overflow(1)` after reaching the cap lets `live` grow beyond `MaxOverflow`.
- `formal/rocq/gc/CounterFlushExclusion.v` and `tla/CounterFlushExclusion.tla`: prove and model-check the
  cron counter-sync / GC free-phase exclusion. Periodic counter sync may scan live value slots while GC response
  processing and session release may free slots; the shared `COUNTER_FLUSH_LOCK` makes the sync scanner and GC
  freer mutually exclusive. The unlocked TLC discriminator violates `NoCounterSyncFreeOverlap`.
- 2026-06-12 scheduler/threading formal increment: `scripts/verify_cesk_gc_formal.sh` passed proof hygiene, TLC
  hygiene, source coupling, 90 mandatory Rocq files, and the full positive/negative TLC discriminator suite. Focused
  runtime gates passed under `systemd-run` caps: `cargo test --lib priority_queue`, `cargo test --lib
  interleaved_table`, `cargo test --lib random_heads`, and `cargo test --lib pooled_recurring_task_stops_on_false`.
  The slab release gate `cargo nextest run --release` then ran 4400 tests with 4400 passed. All checks were run with
  memory caps and no swap, preserving the project heavy-op mandate.
- 2026-06-12 WorkPool overflow-cap increment: `scripts/verify_cesk_gc_formal.sh` passed proof hygiene, TLC hygiene,
  source coupling, 91 mandatory Rocq files, and 229 TLC configs after adding the overflow cap proof/model. Focused
  gates passed under `systemd-run` caps: `rocq c ... WorkPoolOverflowCap.v`, capped/uncapped TLC runs for
  `WorkPoolOverflowCap.tla`, `bash scripts/verify_cesk_gc_source_coupling.sh`, and `cargo test --lib overflow`.
  The slab release gate `cargo nextest run --release` then ran 4401 tests with 4401 passed.
- 2026-06-12 CounterFlush exclusion increment: focused gates passed under `systemd-run` caps:
  `rocq c ... CounterFlushExclusion.v`, `CounterFlushExclusion.tla` locked positive TLC, and the unlocked negative
  discriminator which violates `NoCounterSyncFreeOverlap`. Source coupling was also updated for the committed
  inner-column allocation shape: the reuse-pressure branch must still take the write lock before `exclusive(&mut h)`,
  populate the shared column, and only then return the address. The full real-worktree formal wall then passed:
  proof hygiene found 92 mandatory Rocq files, TLC hygiene found 231 TLC configs, source coupling passed, and
  `scripts/verify_cesk_gc_formal.sh` completed successfully.
- 2026-06-12 Wavefront parallelism increment: focused gates passed under `systemd-run` caps:
  `rocq c ... SchedulerWavefrontParallelism.v`, TLC diamond and all-independent positive configs, the cyclic
  same-wave negative discriminator, `cargo test --lib scheduler::wavefront`, proof hygiene, TLC hygiene, and source
  coupling. The source correction is proof-driven: valid DAGs keep Kahn level grouping and full independent hot-path
  parallelism, while malformed inputs and cyclic unresolved suffixes fall back to sequential waves so the scheduler no
  longer violates its same-wave independence contract. The full capped formal harness then passed with 94 mandatory GC
  Rocq files, 5 WorkPool Rocq files, 36 mandatory Lean mirrors, 237 TLC configs, and source coupling. The slab release
  gate `cargo nextest run --release` passed 4406/4406 tests.
- 2026-06-14 Wavefront maximality tightening: `SchedulerWavefrontParallelism.v` now includes the
  `maximal_ready_wave_no_deferred_task` and `maximal_ready_set_can_share_wave` obligations, and
  `SchedulerWavefrontParallelism.tla` now includes a `DeferReady` model variant that keeps dependency order and
  same-wave independence but leaves a ready diamond branch out of its earliest wave. The regular formal harness runs
  that variant as `scheduler_wavefront_deferred_ready` and expects `NoReadyTaskDeferred` to fail; source coupling pins
  the initial in-degree-zero scan and the `in_degree == 0` dependent insertion that realize the maximal ready-set
  property in Rust.
- 2026-06-15 Wavefront production activation: the production direct-fanout gates now construct independent
  `WavefrontTask`s from the classified branch/item cost classes, call `compute_wavefront()`, and admit direct
  `parallel_dispatch()` only when the verified scheduler returns a full-width single wave. This closes the prior
  activation audit gap without claiming dependency-bearing instruction-DAG reordering; those workloads still require a
  complete data/effect-edge builder before entering the general wavefront path.
- 2026-06-14 Effect-conflict completeness increment: `SchedulerEffectConflictCompleteness.v` now proves that
  dependency order plus conflict-edge coverage implies same-wave conflict freedom, and that a same-wave conflict rejects
  a complete dependency/order pair. `SchedulerEffectConflictCompleteness.tla` adds positive complete-conflict and
  no-conflict hot-path configs plus a missing-edge negative discriminator that violates `ConflictEdgesCovered`. Source
  coupling pins the `WavefrontTask.dependencies` contract: callers must include both data dependencies and
  effect-conflict edges because `compute_wavefront` treats missing edges as safe commutativity evidence.
- 2026-06-14 Active fanout gate-composition increment: `SchedulerActiveFanoutGate.v` proves that active rule-match
  fanout dispatch requires the branch threshold, WFST degree, purity/dynamic-eval, depth, pool, and budget gates, and
  that complete dispatch represents every admitted slot. `SchedulerActiveFanoutGate.tla` adds a positive all-gates
  config plus missing-purity, missing-budget, and partial-dispatch negative discriminators. Source coupling pins the
  active `eval_loop.rs` rule-match path so `try_acquire_budget()` is reached only after the WFST/purity, depth, and
  active-worker gates and `parallel_dispatch()` is under `budget > 0`.
- 2026-06-14 Direct-fanout/wavefront refinement increment: `SchedulerDirectFanoutWavefrontRefinement.v` proves that
  production direct fanout refines the wavefront model only for the all-independent single-wave branch case, dispatches
  every branch, and matches wavefront maximal parallelism. `SchedulerDirectFanoutWavefrontRefinement.tla` adds the
  independent positive config plus dependent-DAG and partial-dispatch negative discriminators. Source coupling pins
  `wavefront.rs` so it states the active production boundary: dependency-bearing instruction DAGs must call the
  general wavefront builder with complete edges before claiming wavefront reordering.
- 2026-06-12 Dynamic-eval dispatch gate increment: focused gates passed under `systemd-run` caps:
  `rocq c ... SchedulerDynamicEvalGate.v`, the fixed dynamic-eval TLC config, the missing-gate negative discriminator
  which violates `NoDynamicEvalParallelBypass`, `cargo test --lib dynamic_eval`, `cargo test --lib branch_analysis`,
  proof hygiene, TLC hygiene, and source coupling. The source correction is proof-driven: dynamic evaluation heads are
  no longer classified as known-pure, are marked impure/sequential for branch analysis, and block the scheduler's
  no-budget parallel-dispatch path even when a hidden mutating head is carried through a variable expression. The full
  capped formal harness then passed with 95 mandatory GC Rocq files, 5 WorkPool Rocq files, 36 mandatory Lean mirrors,
  239 TLC configs, and source coupling. The slab release gate `cargo nextest run --release` passed 4408/4408 tests.
- 2026-06-14 Cron pooled-recurring overlap tightening: `CronRecurringDispatch.v` now proves that dispatch claiming sets
  `in_flight`, a due tick after that claim requeues without dispatch, and a missing claim can dispatch again.
  `CronRecurringDispatch.tla` now models two due ticks before worker completion; the regular formal harness runs
  `cron_recurring_dispatch_no_claim` and expects `NoOverlapDispatch` to fail. Source coupling pins the
  `compare_exchange(false, true, ...)` claim before `pool.spawn_eval`, and pins the failed-CAS path to push the
  placeholder back into the cron queue and return before any worker submission.

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
- The index-mode `INNER_SHADOW` materialization cache is source-pinned to the accepted paged `RefCell` shape:
  `ensure_inner_shadow_epoch_current` reads `gc_sweep_epoch`, drops allocated pages on epoch mismatch, and only then
  publishes `INNER_SHADOW_EPOCH`; `inner_ref_index` validates the epoch before entering `INNER_SHADOW.with` and
  materializing from the heap. The source-coupling gate also rejects reintroducing the rejected experiment #15
  `with_shadow` / release-`UnsafeCell` chokepoint without a new proof+benchmark gate.
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
- The E1/SATB driver-progress refinement is source-coupled: the formal harness requires
  `E1SatbStwDriverProgress.v`; `gc_driver_satb_rendezvous_cycle` closes the initial rendezvous before concurrent
  marking, requests and opens the final rendezvous before taking final roots, checks the final sweep result before
  closing, and `SatbRendezvousCleanup` closes any open cycle or clears an open request on panic. The STW fallback
  closes through `close_open_rendezvous_cycle`, whose order is generation/witness reset, drop GC-in-progress, then
  `resume_workers`.
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
- C1 scheduler source order pins the bounded level-3 major/minor inversion: level 3 means at least twice the effective
  young budget (`young_budget()`/default `YOUNG_BUDGET`) in young pressure, `minor_due` is computed before the no-due
  return, `do_major` may suppress a due live-growth major only when
  `level == 3 && minor_due && !cap_major && !cadence_major`, and the cadence counter resets on major but increments on
  minor.
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
  roots its in-flight work item before a nested evaluator can collect. `KSpineCurrentWork.v` and its TLC discriminators
  fail if `current_work`, the pending `work_stack`, or the continuation stack is omitted.
- VM native locals live across nested CESK evaluation are now a formal K-spine leaf obligation. `VmNestedLocals.v`
  proves that pre-eval locals, dispatch RHS locals, rule-match vectors, saved bindings, combo vectors, and accumulated
  outcomes survive collection when published through `VmLeaf::ValueVec`; `VmNestedLocals.tla` now fails independently
  if any of the pre-eval, dispatch RHS, rule-match, saved-binding, combo, or accumulated-outcome local classes is
  omitted.
- The E1 stress ASAN counterexample showed a pre-spawn FANOUT watermark could still run the non-rendezvous midloop
  collector (`worker_ever_spawned()==false`) before the first worker existed. The corrected obligation is split:
  FANOUT closes midloop non-rendezvous collection, but true quiescence (`active_evaluator_count()==0 && n_threads()==0`)
  remains fanout-independent because no evaluator/native stack is live. `NonRendezvousFanoutGate.v` and
  `NonRendezvousFanoutGate.tla` pin that split while preserving the `FANOUT=0` single-threaded midloop gate.
- The follow-on liveness case is a one-participant rendezvous: with fanout enabled and exactly one active mutator,
  the watermark trigger posts `CollectRendezvous`, the mutator self-roots at the next safepoint, and the driver waits
  on the same reified-witness protocol. `MC_RendezvousProgress_one_participant.cfg` covers this `N=1` progress path.
- #275 (observability prerequisite for the E1 evaluator-progress liveness proof) — the SIGUSR1 diagnostic dump is now
  source-coupled to the active GC mode. `render_gc_state` (`src/backend/diagnostics.rs`) branches on
  `gc_mode_is_index()`: in index mode it reports the CESK `IndexHeap` allocator (committed / live / old-live /
  young-alloc / nursery backpressure, read with `try_read()` so the diagnostic watcher thread NEVER blocks on the heap
  lock during a suspected hang) and the dedicated collector's rendezvous CYCLE state (`cycle_gen` / `cycle_started` /
  `witness_ok` plus the witness-slot occupancy summary `witness_directory_summary`), returning BEFORE the legacy
  "Slab Pages" block; in slab mode the output is byte-identical to before. The `occupied_unpublished` witness count
  (occupied slots not yet re-rooted for the current cycle while the GC is idle) is the direct lever for diagnosing the
  E1 evaluator-progress hang. `verify_cesk_gc_source_coupling.sh` pins the mode branch, the feature-gated index helper,
  the `try_read`→witness-summary order, and the index-gc gate on `witness_directory_summary`; the focused
  `index_mode_dump_reports_index_heap_not_slab_pages` test asserts index mode does not present slab pages as the
  authoritative GC state. This is a diagnostics source-coupling (no new proof obligation) that makes the subsequent
  liveness counterexample evidence trustworthy.

- TrampolineFanoutSpineProgress (`formal/rocq/gc/TrampolineFanoutSpineProgress.v` +
  `tla/TrampolineFanoutSpineProgress.tla`) — the PROGRESS/termination companion to the spine
  SAFETY proofs. `TrampolineFanoutSpineBridge` / `TrampolineFanoutProductionRestore` /
  `UnifiedChoicePointRestore` / `VmChoicePointSpine` / `JitChoicePointSpineBridge` prove
  root/value/restore SAFETY of the continuation-spine round-trip; NONE proved that the fan-out
  exploration makes progress. The CESK trampoline fan-out continuations (`ProcessRuleMatches` /
  `ProcessAmb` / `ProcessMatchTemplates` / `ProcessCollapseEvalResults`, eval_loop.rs
  process_continuation arms) each consume EXACTLY ONE `remaining_*` element per visit and
  re-push the strictly-smaller tail, with a terminal base case (push Resume, not self) when
  empty; the spine lowering (`into_/resolve_trampoline_fanout_spine`, types.rs) is a FAITHFUL,
  idempotent round-trip — `resolve(persist c) = c` via SpineStore `alloc(cont)`/`remove(addr)`,
  a no-op `other => other` on an already-lowered frame — invoked INCREMENTALLY from the
  trampoline loop top by `persist_trampoline_fanout_spines_from` (low-water mark). Rocq
  `faithful_lowering_preserves_termination` proves
  (admit-free, fuel-bounded well-foundedness; no library WF lemma relied upon) that a faithful
  round-trip preserves the fan-out progress measure, so the LOWERED fan-out succession relation
  is `Acc` (well-founded) — the spine lowering introduces NO divergence of its own, and the
  lowered trampoline terminates exactly when the un-lowered (program) machine does.
  `nonfaithful_reset_breaks_progress` (Rocq) and the TLA `_reset.cfg` (`LoweringFaithful=FALSE`:
  resolve RESETS `remaining`) are the non-vacuity witnesses — both reproduce the never-
  terminating lasso (`EventuallyDone` VIOLATED), while `_faithful.cfg` holds it. Source-coupled
  in `verify_cesk_gc_source_coupling.sh` (the `alloc(cont)`/`remove(addr)` round-trip + the
  incremental `persist_trampoline_fanout_spines_from` call site + watermark + Resume-arm clamp).
  CONSEQUENCE (formal-verification-driven debugging result): because the lowering provably
  TERMINATES, the observed `FlyingRaven.metta` slowdown was NOT a non-termination but a COST
  regression — the progress proof redirected the diagnosis, and `git bisect` then pinned commit
  `8c29d4c3`, which re-lowered the WHOLE K stack on every trampoline tick (O(depth·ticks),
  quadratic; `FANOUT_DEPTH=0` reproduced it with no rendezvous straddle, refuting the
  SATB-straddle-liveness hypothesis). Fixed in `8b74952e` by incremental low-water-mark
  persistence.

- IncrementalSpinePersistEquivalence (`formal/rocq/gc/IncrementalSpinePersistEquivalence.v`) —
  the CORRECTNESS companion to that COST fix (Rocq-only: a deductive functional-equivalence
  obligation with no temporal/liveness component, so no paired TLA+ model — written once, in
  Rocq). Models `lower` = `into_trampoline_fanout_spine` (idempotent: the `other => other` arm),
  the incremental `persist_from s from = firstn from s ++ map lower (skipn from s)` =
  `persist_trampoline_fanout_spines_from`, and the trampoline loop as a state machine over
  `(stack, watermark)`: `loop_top` (persist + set mark = len), `op_push` (append raw frames,
  mark unchanged), `op_pop` (drop top + `min`-clamp the mark — the Resume arm). `incremental_eq_full`
  proves that a lowered persisted prefix makes the incremental persist EQUAL the whole-stack
  `persist_full`; `inv_preserved` + `reachable_inv` carry the invariant (persisted prefix lowered
  ∧ mark ≤ len) along ANY push/pop sequence, so `incremental_eq_full_at_every_loop_top` gives
  observational equivalence at EVERY reachable loop top — the fix changes only COST, never the
  lowered K-stack state. Non-vacuity `clamp_is_necessary`/`clamp_restores_equivalence`: dropping
  the Resume-arm clamp leaves a frame pushed after a pop un-lowered (`[false] ≠ [true]`), breaking
  the equivalence — so the clamp is load-bearing, not incidental. Admit-free; source-coupled on
  the same pins as the progress proof (idempotence `other => other`, `persist_from` suffix-lower,
  the loop-top persist + watermark-set, the Resume-arm clamp).

- InnerColumnReadRefinement (`formal/rocq/gc/InnerColumnReadRefinement.v`,
  `tla/InnerColumnReadRefinement.tla`) — exp46 formal retirement of the per-thread
  `INNER_SHADOW` reader. Rocq proves the source refinement: `Space`/`Memo` handles route to the
  append-only id store (`space_memo_reads_id_store`), every other variant routes to the shared
  column (`non_space_memo_reads_shared_column`), and a POD column rewrite before handle escape is
  sufficient to prevent stale reads (`pod_rewrite_before_escape_prevents_stale_read`). The explicit
  counterexample `missing_pod_rewrite_has_stale_counterexample` witnesses why the rewrite is
  load-bearing. TLC checks the same interleaving surface: `_all.cfg` passes with
  `RewriteBeforeEscape=TRUE` and `UseIdStoreForSpaceMemo=TRUE`; `_no_rewrite.cfg` violates
  `NoStaleRead` when a reused POD Addr can escape before its cell is rewritten; and
  `_space_memo_column.cfg` violates `NoStaleRead` when Arc-backed Space/Memo are incorrectly routed
  through the POD column. Source coupling pins the live implementation to the proof: `inner_ref_index`
  reads `inner_column::column_read(addr)` without heap-lock/TLS materialization, the Space/Memo arm
  calls `prebuilt_space_memo_inner(addr)`, all routed factory paths plus continuation-slice restore
  call `populate_column` post-alloc/pre-escape, and the retired `INNER_SHADOW` functions are absent
  from `metta_value.rs`. Follow-up debug instrumentation adds a `written` mark to each column cell in
  debug builds: `column_write` marks after the payload write, `column_read` checks the mark before
  `assume_init_ref`, and segment release resets marks. Rocq theorem
  `debug_tripwire_rejects_unwritten_cell` captures the instrumentation precondition; source coupling
  pins write-before-mark, mark-before-read, and reset-on-release.

- WorkPoolStability assumption-free conversion (`formal/rocq/work_pool_stability/theories/*.v`) —
  removes the WorkPool scaling proof package's trusted global declarations from the threading/model
  verification lane. `Prelude.v` now exposes `WorkPoolParams`, a proof-carrying record for USL inputs
  (`T1`, `sigma`, `kappa`, `lambda`) and their range evidence; `USL.v` is parametric over that record;
  `ObjectiveFunction.v` introduces `WorkPoolSignals`, a proof-carrying record for slab pressure, RSS
  pressure, and queue-depth signal contracts; and `LyapunovConvergence.v` takes `N_opt` as an explicit
  theorem argument. `WeightDominance.v` remains concrete and assumption-free. The formal harness now
  compiles all five WorkPool Rocq files, and proof hygiene scans them for `Admitted`/`admit`/`Axiom`/
  `Parameter`/`Conjecture`/`Abort` alongside the CESK GC proofs. Source coupling pins the harness entries
  plus the `WorkPoolParams`, `WorkPoolSignals`, and explicit-`N_opt` Lyapunov shapes.

- Mandatory Lean mirror compilation (`formal/lean/gc/*.lean`) — closes the remaining proof-harness
  opt-in gap. The 36 Lean GC mirrors were already scanned for Lean proof shortcuts; they now compile
  unconditionally in `scripts/verify_cesk_gc_formal.sh`, and proof hygiene fails if the mirror set is
  missing. Source coupling pins the default harness to the `formal/lean/gc` scan and rejects the old
  `RUN_LEAN_MIRRORS` opt-in gate.

- WorkPool lifecycle accounting (`formal/rocq/gc/WorkPoolLifecycle.v`,
  `tla/WorkPoolLifecycle.tla`, 2026-06-13) — discharges the active-count/parking/respawn
  obligation for `WorkPool` and `AdaptiveGcPool`. Rocq proves that pool aggregate accounting is
  driven exactly by successful worker-state transitions: repeated `try_unpark` or `try_park` calls
  are idempotent, parking preserves the capacity invariant only when the active count decrements,
  and respawning a previously parked replacement as unparked must increment the active count. TLC
  runs the matching fixed model plus two negative discriminators: the old separate-check double
  unpark shape violates `CapacityConsistent`, and the old respawn-without-increment shape violates
  the same invariant. The source fix adds transition-returning `WorkerPark::{try_park,try_unpark}`,
  serializes scale operations with pool-level `scale_lock`s, updates respawn accounting from the
  transition result, and replaces overflow-worker raw pointer counter sharing with `Arc<AtomicUsize>`.
  Source coupling pins the proof/model harness entries and the implementation shapes. Verified by
  `systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 -p CPUQuota=600% --quiet bash
  scripts/verify_cesk_gc_formal.sh` at the implementation increment.

- WorkPool startup drain (`formal/rocq/gc/WorkPoolStartupDrain.v`,
  `tla/WorkPoolStartupDrain.tla`, 2026-06-14) — discharges the first-access/startup-window
  obligation for eval tasks submitted before workers are available. Rocq proves retained pre-start
  eval tasks drain after workers start, that zero started workers make no drain progress, and that
  lossy pre-start enqueue prevents full completion. TLC runs the matching fixed model plus two
  negative discriminators: no worker start violates the eventual-drain property, and lossy enqueue
  violates `AllSubmittedComplete`. Source coupling pins enqueue-before-notify, non-lossy
  `spawn_eval` queue insertion, `start_init()` worker startup under `WORK_POOL_INIT`,
  `global_eval_pool()` startup before monitor start, and worker-loop wait-then-pop order. The public
  threading documentation now reflects the verified WorkPool path rather than the removed
  Tokio-blocking eval narrative.

- WorkPool panic isolation (`formal/rocq/gc/WorkPoolPanicIsolation.v`,
  `tla/WorkPoolPanicIsolation.tla`, 2026-06-14) — discharges the worker-survival obligation for
  panicking eval tasks and post-execute accounting failures. Rocq proves that the inner
  `PriorityTask::execute` catch converts a task panic into zero runtime, skips runtime/WFST
  accounting, publishes the worker heartbeat, and leaves the worker available for the next queued
  task; it also proves the outer worker-loop catch keeps accounting/weight-update panics from
  killing the worker. TLC runs two positive models plus two negative discriminators: missing the
  inner catch violates `TaskPanicPublishesHeartbeat`, and missing the outer catch violates
  eventual completion of the next queued task. Source coupling pins the inner `catch_unwind`, zero
  runtime on panic, runtime-update gate, core/overflow outer catches, and the existing panic-survival
  tests.

- Struct channel pairing (`formal/rocq/gc/StructChannelPairing.v`,
  `tla/StructChannelPairing.tla`, 2026-06-13) — discharges the field-insensitive
  pgmcp channel audit findings for struct-stored endpoints in `gc_pool`,
  `task_scheduler`, and the dormant `priority_scheduler::ResultReceiver`
  wrapper. Rocq proves that a used receiver implies a constructor-created pair
  plus stored sender/receiver, worker receives imply a cloned receiver, worker
  response sends imply a caller-visible response receiver, ready waits imply the
  ready sender/receiver/signal path, and an unconstructed private wrapper cannot
  wait. TLC runs one positive model plus four negative discriminators for missing
  sender, missing worker clone, missing response receiver, and missing ready
  sender. Source coupling pins the concrete Rust constructor, clone, send, recv,
  and return sites, plus the absence of `ResultReceiver` construction.

- JIT cache-entry thread safety (`formal/rocq/gc/JitCacheEntryThreadSafety.v`,
  2026-06-13) — closes the pgmcp Send/Sync audit finding on
  `bytecode::jit::tiered::CacheEntry`. The runtime fix removes manual
  `unsafe impl Send/Sync for CacheEntry` by replacing the raw cached native-code
  data pointer with a typed `NativeCodeFn`; Rust now derives the cache entry's
  Send/Sync status from the function pointer, integer fields, atomic
  `Arc<JitProfile>`, tier enum, and `Instant` under the `RwLock<HashMap<...>>`.
  Source coupling pins the typed field, the two compile-site conversions, the
  absence of the manual impls, and the removal of a non-runtime scanner test
  fixture string that pgmcp had correctly flagged as text.

- Threading/scheduler/cron/GC proof wall (`a2a35d59`, 2026-06-14) — records the
  committed proof-first audit for the end-to-end threading path. The checked
  scope includes priority-queue fairness, WorkPool lifecycle/startup/panic
  isolation, scheduler classification/dynamic-eval gating, wavefront
  instruction reordering, transducer/fanout admission, pooled recurring cron
  dispatch, Rholang batch completion/handoff, driver-root union, scheduler/GC
  boundary roots, worker spawn latches, and the composing
  `CESKCollectorSafety.v` theorem. The full formal harness passed on that
  committed HEAD under `systemd-run --user --scope` with `MemoryMax=24G`,
  `MemorySwapMax=0`, and `CPUQuota=600%`, ending with
  `CESK GC formal checks passed`. Its preflight hygiene made 107 GC Rocq files,
  5 WorkPool Rocq files, 36 Lean mirrors, and 280 TLC configs mandatory; source
  coupling then tied the proof boundary to live env/dispatch anchors,
  persistent batch handoff handles, closed worker admission,
  spawn-latch-before-worker handoff, and active worker structural-root
  publication.

- Threading end-to-end interleaving envelope (`formal/rocq/gc/ThreadingEndToEndInterleaving.v`,
  `tla/ThreadingEndToEndInterleaving.tla`, 2026-06-14) — composes scheduler
  dependency waves, effect-conflict edge coverage, direct fanout, active-worker
  root publication, closed worker admission, eval-worker spawn latching, active
  direct-fanout gates, sweep, recurring-cron dispatch, cron startup delivery,
  WorkPool startup drain, WorkPool panic isolation, and the GC-facing scheduler
  boundary for active workers, live-dispatch fanout, async batch roots, and
  closed admission into one TLC state machine. It now composes
  `SchedulerClassificationLookup.v`, `SchedulerWavefrontParallelism.v`,
  `SchedulerEffectConflictCompleteness.v`,
  `SchedulerDirectFanoutWavefrontRefinement.v`,
  `SchedulerFanoutAdmissionCompleteness.v`,
  `CollapseFanoutAdmissionCompleteness.v`, `SchedulerFanoutProgress.v`, and
  `SchedulerGcBoundary.v`, plus `DedicatedHandoff.v`,
  `GcDriverChannelProtocol.v`, `KSpineCurrentWork.v`, `VmNestedLocals.v`,
  `E1DefaultConcurrentFlip.v` and
  `E1SatbStwDriverProgress.v`: classification-table
  range disjointness, wavefront edge coverage, direct-fanout independent-wavefront
  refinement, admitted branch/collapse slot representation, FANOUT participant
  accounting, parked-worker resume, worker completion-drop accounting, the
  GC-facing scheduler root/admission boundary, and the E1 dedicated default-flip
  request/backstop boundary plus the SATB-success-or-fresh-STW driver release
  boundary, dedicated root-vector ownership handoff, driver request/response
  channel liveness, and K-spine structural control roots are part of the same
  end-to-end proof boundary. Bytecode-VM native locals held across nested CESK
  evaluation are also part of that boundary: pre-eval locals, dispatch RHS
  locals, rule-match vectors, saved bindings, combo vectors, and accumulated
  outcomes must be represented by typed K-spine leaves before collection.
  The Rocq envelope now also imports `CESKCollectorSafety.v` and proves
  `end_to_end_safe_feeds_cesk_index_gc_safety`: from `EndToEndSafe` it extracts
  the `gc_window_safe` scheduler-root premises, feeds them into
  `end_to_end_cesk_index_gc_safety`, and carries the threading proof through to
  future-touch no-UAF, published-slot readiness, and concurrent-allocation versus
  exclusive-reuse disjointness. This bridge composes existing Rocq capstones and
  adds no new TLA state-machine surface.
  Positive dependency-bearing and independent configs preserve `EndToEndSafe`.
  Negative discriminators violate it for missing dependency edges, active
  direct-fanout without purity or budget gates, partial direct-fanout dispatch,
  active direct-fanout that bypasses the WFST transducer's zero-cap clamp,
  maximal-before-cap use of available branches, or branch-parallel class gate,
  dynamic eval that bypasses the dynamic-eval blocker, state mutation through
  the pure no-budget path, strict I/O through the pure no-budget path, worker
  spawn before the sticky latch, missing worker-spawn latch, missing FANOUT
  participant contribution, missing parked FANOUT worker resume, missing worker
  completion-drop accounting, unclaimed recurring cron dispatch, and a pooled recurring
  worker that clears `in_flight` without first publishing the terminal stop
  state. The no-shift classification E2E discriminator fails specifically on
  `SchedulerClassificationRangesDisjoint`, matching the standalone
  `SchedulerClassificationLookup` model inside the composed envelope. The E1
  legacy-default and trigger-backstop E2E discriminators fail specifically on
  `E1LegacyProducersSuppressed` and `E1FailedTriggerBackstopped`, matching the
  default dedicated-regime and FANOUT rendezvous-trigger premises composed by
  `E1DefaultConcurrentFlip`. The E1 SATB/STW E2E discriminators fail
  specifically on `E1FinalCycleRelease` when the success path does not clear
  witness/driver state, and on `E1SatbAbortPostsFreshStw` when abort does not
  post the fresh STW backstop, matching the shipped driver progress theorem
  composed by `E1SatbStwDriverProgress`. The dedicated-handoff E2E
  discriminators fail specifically on `DedicatedInlineFallbackHasRoots` when a
  sent/consumed root vector attempts inline fallback, and on
  `DedicatedCollectReplyProducerSafe` when a consumed request omits the reply
  attempt, matching the ownership/reply-producer theorem composed by
  `DedicatedHandoff`. The driver-channel E2E discriminators fail specifically
  on `DriverRequestReceiveHasProducer`, `DriverResponseWaitHasProducer`,
  `DriverNoOrphanReplySend`, and `DriverFireAndForgetDoesNotWait` for missing
  request sender, missing reply attempt, orphan reply, and fire-and-forget wait
  shapes, matching the channel-liveness theorem composed by
  `GcDriverChannelProtocol`. The K-spine E2E discriminators fail specifically
  on `KSpineCurrentWorkRooted`, `KSpineWorkStackRooted`, and `KSpineKontRooted`
  for omitted current work, pending work stack, and continuation roots, matching
  the typed K-spine theorem composed by `KSpineCurrentWork`. The VM nested-local
  E2E discriminators fail specifically on `VmNestedPreEvalRooted`,
  `VmNestedDispatchRhsRooted`, `VmNestedRuleMatchesRooted`,
  `VmNestedSavedBindingsRooted`, `VmNestedCombosRooted`, and
  `VmNestedOutcomesRooted` for omitted VM native-local classes, matching the
  typed K-spine leaf theorem composed by `VmNestedLocals`. The missing
  dependency-edge and effect-conflict-edge
  E2E discriminators now fail specifically on `SchedulerWavefrontEdgesComplete`,
  and the partial
  direct-dispatch discriminator fails on
  `SchedulerDirectFanoutRefinesWavefront`, matching the standalone scheduler
  wavefront/effect/direct-refinement proofs inside the composed model. The
  degree-as-spawn-cap and threshold-as-spawn-cap E2E discriminators fail
  specifically on `ActiveFanoutAdmissionComplete` and
  `CollapseFanoutAdmissionComplete`, matching the standalone fanout/collapse
  admission-completeness proofs inside the composed model. The
  missing active-worker-root, dispatch-root, batch-root, and
  open-admission E2E discriminators now fail specifically on
  `SchedulerBoundaryComplete`, matching the standalone scheduler/GC boundary
  model inside the composed interleaving envelope. The composed recurring-cron
  proof now calls the standalone `CronRecurringDispatch` theorems for
  dispatch-claim publication, requeue-after-claim, stop-before-idle, and
  continue-without-stop redispatch; the missing-stop E2E discriminator therefore
  depends on the same transition contract as the standalone cron model. The
  composed cron startup discriminator separately rejects a submitted startup
  task when neither `CheckEvents` nor `DrainChannel` polls the cron task channel.
  The composed spawn-latch proof now calls the standalone
  `SchedulerSpawnLatch` latch-before-spawn theorem for the positive path and a
  concrete standalone spawn-before-latch bad trace for the E2E negative
  discriminator, so the mid-loop gate closure obligation is pinned to the same
  proof surface as the standalone latch model.  Composed WorkPool discriminators
  reject lossy startup enqueue, missing inner task-panic heartbeat publication,
  and missing outer accounting-panic catch.  The envelope now also composes
  WorkPool overflow and lifecycle accounting: the E2E model rejects uncapped
  overflow spawning, double-unpark overcounting, and parked-worker respawn
  without the active-count increment, using the same cap/capacity invariants as
  the standalone `WorkPoolOverflowCap` and `WorkPoolLifecycle` models.  It also
  composes WorkPool priority-aging fairness: pop-time score recomputation is
  required before dequeue, and the stale-priority discriminator violates
  `WorkPoolOldPopsAfterAging` by popping newer high-priority work before the
  aged older task.  The E2E Rocq proof now exposes named WorkPool bridge lemmas
  whose proof terms call the standalone startup, panic-isolation, overflow-cap,
  lifecycle, and priority-aging theorems directly; source coupling pins those
  qualified theorem calls so the composed envelope cannot silently regress to
  local arithmetic-only obligations.  The composed spawn-latch discriminators violate
  `WorkerSpawnLatchPrecedesWorker`, matching either a pool handoff that spawns
  before storing `worker_ever_spawned` or a handoff site that omits the latch.
- JIT Long boxing store selection (`formal/rocq/gc/JitLongBoxStoreSelection.v`,
  `tla/JitLongBoxStoreSelection.tla`, 2026-06-14) — proves out-of-inline-range
  JIT Long boxing selects the compiled store: index builds allocate through the
  index factory, legacy slab builds keep the slab fallback, and inline Longs do
  not allocate. TLC positives preserve the selection invariants; negative
  discriminators reject both an index build that boxes through the slab and an
  index build that still compiles a slab fallback after the index branch.
- JIT get-type store selection (`formal/rocq/gc/JitTypeOpsStoreSelection.v`,
  `tla/JitTypeOpsStoreSelection.tla`, 2026-06-15) — proves
  `jit_runtime_get_type` selects its factory from the compiled store: index
  builds allocate type-name atoms through the active index factory, legacy slab
  builds keep the slab factory, and the JIT context arena pointer is never a
  store selector in index builds. TLC positives preserve the selection
  invariants; negative discriminators reject both an index build that uses a
  slab factory and an index build that treats the arena pointer as a store
  selector.
- JIT `is-function` TAG_PTR decode (`formal/rocq/gc/JitIsFunctionPointerDecode.v`,
  `tla/JitIsFunctionPointerDecode.tla`, 2026-06-14) — proves heap inspection is
  selected by the compiled store: index builds reconstruct TAG_PTR payloads as
  arena handles before structural classification, legacy slab builds may
  dereference slab pointers, and non-pointer values perform no heap inspection.
  TLC positives preserve the decode policy; negative discriminators reject an
  index build that slab-dereferences a TAG_PTR payload and any non-pointer path
  that performs a dereference.
- Cron startup delivery (`formal/rocq/gc/CronStartupDelivery.v`,
  `tla/CronStartupDelivery.tla`, 2026-06-14) — proves the cron ready-channel
  startup contract beyond generic endpoint pairing: after the caller observes
  the returned ready receiver, a task submitted through the returned
  `CronHandle` is reachable by the cron event loop only if the ready signal is
  sent from inside `CronStateMachine::run()` and the event loop polls the task
  channel in `CheckEvents` or `DrainChannel`. TLC accepts the complete startup
  path and rejects missing ready signal, missing handle sender, and missing
  poll-path variants.

## Harness

Run:

```bash
bash scripts/verify_cesk_gc_formal.sh
```

The harness derives paths from its own location, uses `target/tlc-formal-small` for small TLC logs/metadata by
default, runs Rocq under `systemd-run`, compiles the Lean GC mirrors by default, and includes the small TLC
positive/negative discriminators. The adjacent GC
gate scripts likewise default log/build scratch to repo-derived `target/...` directories (`target/gc-logs` or a
script-specific subdirectory) while preserving caller overrides such as `LOG_DIR`, `LOG_ROOT`, `SCRATCH_ROOT`,
`OUT`, `P`, `PGO_DIR`, and `CARGO_TARGET_DIR`; large gates must not spill into `/tmp` unless the caller explicitly
chooses that.

Before compiling proofs, `scripts/verify_cesk_gc_proof_hygiene.sh` rejects Rocq proof shortcuts (`Admitted`, `admit`,
`Axiom`, `Parameter`, `Conjecture`, `Abort`) in the mandatory CESK GC proof directory and the WorkPool stability
proof directory, then verifies every `formal/rocq/gc/*.v` and `formal/rocq/work_pool_stability/theories/*.v` file is
enumerated by the formal harness. It also requires the Lean GC mirror set to exist and scans it for Lean proof
shortcuts (`sorry`, `admit`, `axiom`, `constant`, `opaque`, `unsafe`) before the formal harness compiles those mirrors.

The formal harness also runs `scripts/verify_cesk_gc_tlc_hygiene.sh` before compiling proofs. That check parses every
`run_tlc` entry, verifies labels/configs are unique, requires every referenced TLA+ module and config to exist, requires
negative TLC runs to carry a discriminator pattern, rejects any tracked `tla/*.cfg` or `tla/MC_*.tla` wrapper that is
neither run nor explicitly classified, and rejects any tracked `tla/*.tla` module that is not either run directly,
imported by a run/classified wrapper, or explicitly classified. The only classified exclusions are legacy slab
mark-sweep models, the older non-generational store-centric mark-sweep wrapper/configs, and larger CESK generational
discriminator configs whose disk-light small counterparts are the default gate.

## Whole-system verification wall (capstone, 2026-06-11)

`scripts/verify_cesk_gc_all.sh` is the single-entrypoint capstone harness (pgmcp #149/#22): it runs
every standing gate in dependency order — proof hygiene, TLC hygiene, source coupling, the full
Rocq corpus + TLC discriminator suite, the greenwall (both stores: nextest, conformance 483
release, 49/49 warnings, the DEBUG machine-equivalence oracle), the forced-cycle FANOUT=8 ASAN
gate (3 arms, 0-UAF + rendezvous non-vacuity), the loom concurrency models, mmverify "Correct
proof!" on both store binaries, and the 20-run Robot FANOUT=8 sorted-content determinism check
(order-insensitive: the unsorted output ORDER is pre-existing nondeterministic at HEAD; content
must be invariant) — each stage hard-gating the next, ending in a single verdict table. It pairs
with the end-to-end composition theorem `formal/rocq/gc/CESKCollectorSafety.v` (#152): the theorem
composes the local obligations; the wall re-checks every obligation's mechanized artifact and the
runtime evidence on the live tree. First full run (label `capstone`, HEAD 8cfe9cdd):
ALL 9 STAGES GREEN (logs `target/gc-logs/allwall_capstone.fTdf7jEl`).

## Phase F1 Robot gate evidence (2026-06-13, `3b157137`)

After the F1 harness was repaired to emit pgmcp-ready raw samples and to reject any statistically
and materially worse index arm, the final Robot-only Phase F1 run at `3b157137` passed both the local
harness and pgmcp experiment #6. Protocol: 51 measured reps per arm after 3 warmups, interleaved
measurement order, `taskset 8-15`, `FANOUT=0`, and every measured invocation in a capped
`systemd-run` scope (`MemoryMax=24G`, `MemorySwapMax=0`, `CPUQuota=800%`). Local harness output:
`pln_robot_wall_ms` slab mean 5518.6 ms, index mean 5311.0 ms, index/slab 0.962x, ACCEPT;
`pln_robot_peak_rss_mib` slab mean 620.1 MiB, index mean 414.2 MiB, index/slab 0.668x, ACCEPT.

The raw samples were recorded into pgmcp experiment #6 under commit-specific arms
`slab_3b157137_robot` and `index_3b157137_robot`. pgmcp's pre-registered Welch decision accepted
the `pln_robot_wall_ms` hypothesis (p=7.2675e-17, Cohen's d=-2.0574, 95% CI for index-minus-slab
[-247.4140, -167.8801] ms). This does not replace the whole-system wall; it closes the F1
performance predicate that gates any later Phase F3 default flip. The source/proof side at the same
commit was checked by `scripts/verify_cesk_gc_source_coupling.sh`,
`scripts/verify_cesk_gc_proof_hygiene.sh`, `scripts/verify_cesk_gc_tlc_hygiene.sh`, the full
`scripts/verify_cesk_gc_formal.sh` Rocq+TLC harness under a 16 GiB cap, and a capped
`--features index-gc` release build.
