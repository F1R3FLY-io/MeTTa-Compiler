(** End-to-end CESK collector safety obligations in Rocq.

    This file composes the local proof obligations used by the MeTTaTron
    generational index collector:

    - rendezvous witness publication puts participant roots in the driver roots;
    - witness-slot lifecycle keeps live frozen machines visible until their
      roots are buffered by a genuine park or the true outermost guard drops;
    - cross-cycle witness-ok reset prevents a previous cycle's non-generational
      true flag from allowing the next cycle's sweep before a fresh wait;
    - generation-gated resume releases a worker after its own rendezvous cycle's
      end bump even if a back-to-back request reasserts `GC_REQUESTED`;
    - started-cycle straddle gating prevents a teardown generation bump from
      causing a phantom worker re-park before the next driver starts;
    - parallel dispatch/collapse completion guards prevent a panic-unwind from
      stranding the parent wait with a nonzero remaining count;
    - worker admission gating prevents a new evaluator from joining after the
      driver has closed admission and snapshotted participants;
    - the driver root union includes worker-buffer, safepoint, live-env, and
      live-dispatch channel roots;
    - the index collector source set excludes the old `RootProvider` registry,
      so registry roots cannot justify future touches in the CESK proof;
    - the single-threaded mid-loop root union includes live S/C/K, E0,
      globals, K-spine, deferred env roots, and driver-C safepoint roots;
    - C2 abstract-GC K-frame narrowing keeps every future-touched value rooted
      when omitted continuation fields are dead for the next transition;
    - fork-local environment roots carried by live work items and
      continuations are included in the published frame contribution;
    - VM/JIT tier-leaf register roots are included in the worker's extra
      publication before the worker parks;
    - eval-entry driver-C publication puts caller-held source/output roots in
      the driver roots;
    - async batch-result handoff roots worker results in driver-C until the
      caller copies them into MettaState.output;
    - the collector marks the reachability closure of structural CESK roots plus
      driver roots;
    - the marker's concrete node-edge reader covers every semantic heap edge
      class used by index nodes: inline handle fields, side-arena children, and
      first-class SpaceHandle contents;
    - sweep frees only unmarked addresses;
    - young-only minor marking retains every reachable young address when there
      is no old-to-young edge;
    - conservative minor marking retains every reachable young address when the
      marker traverses the whole reachable graph but only marks young nodes;
    - C1.c nursery-backpressure segment-open signals request a minor and
      promotion clears stale nursery triggers before the next scheduling check;
    - C1.c scheduler choice never defers cap-forced or cadence-forced majors,
      and any deferred major is a live-growth major under level-3 young pressure;
    - B.5 cap-floor anti-thrash prevents a futile cap-triggered major from
      immediately re-firing without committed growth and clears stale floors on
      segment-releasing majors;
    - E2 concurrent marking retains every snapshot-live address covered by
      initial roots, rendezvous driver roots, SATB deletion shades, or
      allocate-black publication;
    - E2 SATB phase/sweep gates force snapshot-live removed pre-images to be
      visible or shaded before sweep can free them;
    - E2 freshly published allocations survive when publication implies
      allocate-black marking;
    - fixed-size arena slots are initialized and published before their
      addresses can be returned and read;
    - variable-length side-arena payloads are initialized before their
      page/chunk/entry publication chain can be observed by a reader, and
      side-bearing nodes publish only after same-segment side payloads exist;
    - variable-length side-arena payload boxes are dropped only on the
      true-quiescence arm, and the materialization shadow is cleared before a
      future dereference can observe a freed side payload;
    - Addr-valued ground-SExpr hash-cons entries retained across sweep cannot
      return a reclaimed slot on a later lookup;
    - E2 final-rendezvous roots survive the exclusive sweep, and a completed
      SATB request is backed by either the final SATB sweep or the abort-to-STW
      backstop;
    - E2 SATB mark bits cannot leak into later cycles when the final sweep is a
      full major that clears every swept address before promotion;
    - E2 value-bearing E0 cache capacity-eviction, overwrite, and bulk-clear
      pre-images compose into SATB coverage when those removed values are
      shaded;
    - E2 value-bearing E0 deletion categories compose into that SATB coverage
      when removed space-local, rule-index, and environment/token/state
      pre-images are shaded;
    - pointer-keyed operator-cache lookup and the other epoch-protected
      worker-local Addr caches cannot return stale post-sweep entries when
      their local sweep-epoch guard runs before lookup;
    - write-once global anchors that are scanned structurally and cannot be
      deleted survive ordinary structural-root collection;
    - global space-registry values survive while registered as structural roots,
      and replaced/removed/cleared space values survive SATB collection when the
      old `SpaceHandle` roots are shaded;
    - global tiered-cache pending bytecode source roots and compiled bytecode
      constants survive while registered as structural roots, removed old cache
      values survive SATB collection when shaded, and pending-root guard drops
      are ownership-token checked;
    - thread-local eval memo, match-result, subgoal, and thunk cached result
      values survive while scanned as structural roots, and stale/overwritten/
      removed/cleared/replaced subgoal/thunk cached results survive SATB
      collection when shaded.

    These are parametric theorems over the store graph and do not assume a finite
    TLC state space.
*)

From Stdlib Require Import Arith.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_CESKCollectorSafety.

Section CESKCollectorSafetyModel.
  Variables Addr Slot : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SemanticNodeEdge
      (InlineEdge SideEdge SpaceEdge : Addr -> Addr -> Prop)
      (parent child : Addr) : Prop :=
    InlineEdge parent child \/ SideEdge parent child \/ SpaceEdge parent child.

  Definition CollectorRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Inductive RootSource : Type :=
  | StructuralSource : RootSource
  | DriverSource : RootSource
  | RegistrySource : RootSource.

  Definition IndexRootSource (source : RootSource) : Prop :=
    match source with
    | StructuralSource => True
    | DriverSource => True
    | RegistrySource => False
    end.

  Definition DriverRootUnion
      (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor : Addr -> Prop)
      (a : Addr) : Prop :=
    WorkerRoot a \/ SafepointRoot a \/ EnvAnchor a \/ DispatchAnchor a.

  Definition MidloopRootUnion
      (LiveSCK Env0 Global KSpine Deferred DriverC : Addr -> Prop)
      (a : Addr) : Prop :=
    LiveSCK a \/ Env0 a \/ Global a \/ KSpine a \/ Deferred a \/ DriverC a.

  Definition ForkLocalRoot
      (Binding TypeAssertion StateCell NamedSpace InferredType : Addr -> Prop)
      (a : Addr) : Prop :=
    Binding a \/ TypeAssertion a \/ StateCell a \/ NamedSpace a \/ InferredType a.

  Definition TierLeafExtraRoot
      (VmRegisterRoot JitRegisterRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    VmRegisterRoot a \/ JitRegisterRoot a.

  Definition PublishedEntryReady
      (PageReady ChunkReady EntryWritten EntryPublished : Slot -> Prop)
      (slot : Slot) : Prop :=
    PageReady slot /\ ChunkReady slot /\ EntryWritten slot /\ EntryPublished slot.

  Definition PublishedNodeSideReady
      (node_seg side_seg : nat)
      (SideReady NodePublished : Prop) : Prop :=
    NodePublished /\ SideReady /\ side_seg = node_seg.

  Definition PublishedSlotReady
      (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned :
          Addr -> Prop)
      (a : Addr) : Prop :=
    SegmentWritten a /\ SegmentPublished a /\ SlotWritten a /\ SlotPublished a /\
      AddrReturned a.

  Definition LiveMachineVisible
      (Machine : Type)
      (Live Occupied Buffered : Machine -> Prop)
      (m : Machine) : Prop :=
    Live m -> Occupied m \/ Buffered m.

  Definition GenerationCanResume (my_gen current_gen : nat) : Prop :=
    current_gen <> my_gen.

  Definition BooleanCanResume (gc_requested : Prop) : Prop :=
    ~ gc_requested.

  Definition MinorDue (YoungOverBudget NurseryPending : Prop) : Prop :=
    YoungOverBudget \/ NurseryPending.

  Definition MajorDue (LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    LiveMajor \/ CapMajor \/ CadenceMajor.

  Definition DoMajor
      (Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    MajorDue LiveMajor CapMajor CadenceMajor /\
    ~ (Level3 /\ MinorDueNow /\ ~ CapMajor /\ ~ CadenceMajor).

  Definition DeferredMajor
      (Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    MajorDue LiveMajor CapMajor CadenceMajor /\
    Level3 /\ MinorDueNow /\ ~ CapMajor /\ ~ CadenceMajor.

  Definition CapDue (Committed BaseCap CapFloor : nat) : Prop :=
    Committed > Nat.max BaseCap CapFloor.

  Definition FutileCapMajor
      (Committed BaseCap CapFloor ReleasedSegments : nat) : Prop :=
    CapDue Committed BaseCap CapFloor /\ ReleasedSegments = 0.

  Definition PromotionRelaxed
      (YoungOdometerReset NurseryPendingCleared : Prop) : Prop :=
    YoungOdometerReset /\ NurseryPendingCleared.

  Definition ConcurrentCollectorRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition RequestHandled (SatbSwept StwFallbackRan : Prop) : Prop :=
    SatbSwept \/ StwFallbackRan.

  Definition BatchHandoffRoot
      (HandleRoot OutputRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    HandleRoot a \/ OutputRoot a.

  Definition E0RemovedPreimage
      (SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
      (a : Addr) : Prop :=
    SpacePreimage a \/ RulePreimage a \/ EnvPreimage a.

  Definition E0CacheRemovedPreimage
      (CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
      (a : Addr) : Prop :=
    CapacityVictim a \/ OverwriteVictim a \/ BulkClearedEntry a.

  Definition AbstractGCRoot
      (FullRoot DeadForNext : Addr -> Prop)
      (a : Addr) : Prop :=
    FullRoot a /\ ~ DeadForNext a.

  Definition OperatorCacheEnsurePost
      (Entry Epoch : Type)
      (heap_epoch local_epoch : Epoch)
      (CacheBefore CacheAfter : Entry -> Prop) : Prop :=
    (local_epoch = heap_epoch /\ (forall e, CacheAfter e -> CacheBefore e)) \/
    (local_epoch <> heap_epoch /\ (forall e, ~ CacheAfter e)).

  Definition EpochProtectedCacheValidatePost
      (Entry Epoch : Type)
      (heap_epoch local_epoch : Epoch)
      (CacheBefore CacheAfter : Entry -> Prop)
      (Stale : Epoch -> Entry -> Prop) : Prop :=
    (local_epoch = heap_epoch /\ (forall e, CacheAfter e -> CacheBefore e)) \/
    (local_epoch <> heap_epoch /\
      (forall e, CacheAfter e -> CacheBefore e) /\
      (forall e, CacheAfter e -> ~ Stale heap_epoch e)).

  Definition WriteOnceAnchorLive
      (Anchor : Type)
      (Initialized Deleted : Anchor -> Prop)
      (slot : Anchor) : Prop :=
    Initialized slot /\ ~ Deleted slot.

  Definition SpaceRegistryRemovedValue
      (OverwriteVictim RemoveVictim ClearVictim : Addr -> Prop)
      (a : Addr) : Prop :=
    OverwriteVictim a \/ RemoveVictim a \/ ClearVictim a.

  Definition TieredCacheRegisteredValue
      (PendingRoot CompiledConstant : Addr -> Prop)
      (a : Addr) : Prop :=
    PendingRoot a \/ CompiledConstant a.

  Definition TieredCacheRemovedValue
      (PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
       ClearPendingVictim ClearCompiledConstant : Addr -> Prop)
      (a : Addr) : Prop :=
    PendingOverwriteVictim a \/
    PendingCancelVictim a \/
    PendingGuardDropVictim a \/
    ClearPendingVictim a \/
    ClearCompiledConstant a.

  Definition TokenCheckedRemove
      (EntryToken : Slot -> Addr)
      (guard : Addr)
      (entry : Slot) : Prop :=
    EntryToken entry = guard.

  Definition ThreadLocalTableRegisteredValue
      (EvalMemoResult MatchResult SubgoalResult ThunkResult : Addr -> Prop)
      (a : Addr) : Prop :=
    EvalMemoResult a \/ MatchResult a \/ SubgoalResult a \/ ThunkResult a.

  Definition ThreadLocalTableRemovedValue
      (SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
       SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
       ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim : Addr -> Prop)
      (a : Addr) : Prop :=
    SubgoalStaleVictim a \/
    SubgoalOverwriteVictim a \/
    SubgoalRemoveVictim a \/
    SubgoalClearVictim a \/
    ThunkStaleVictim a \/
    ThunkOverwriteVictim a \/
    ThunkRemoveVictim a \/
    ThunkClearVictim a \/
    ThunkReplaceVictim a.

  Inductive CompletionExit : Type :=
  | CompletionNormal : CompletionExit
  | CompletionPanic : CompletionExit.

  Definition CompletionWorkerExited
      (Worker : Type)
      (Exit : Worker -> CompletionExit -> Prop)
      (w : Worker) : Prop :=
    Exit w CompletionNormal \/ Exit w CompletionPanic.

  Definition CompletionParentWaitStranded
      (Worker : Type)
      (Spawned Dropped : Worker -> Prop) : Prop :=
    exists w, Spawned w /\ ~ Dropped w.

  Theorem index_collector_excludes_registry_source :
    forall source,
      IndexRootSource source ->
      source <> RegistrySource.
  Proof.
    intros source Hsource Hregistry.
    destruct source.
    - discriminate Hregistry.
    - discriminate Hregistry.
    - exact Hsource.
  Qed.

  Theorem registry_independent_no_future_touch_uaf :
    forall (StructuralRoot DriverRoot RegistryRoot Marked Freed FutureTouch : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, FutureTouch a -> Reach (CollectorRoot StructuralRoot DriverRoot) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot RegistryRoot Marked Freed FutureTouch Edge
           Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem fork_local_env_root_survives_collection :
    forall (Binding TypeAssertion StateCell NamedSpace InferredType
            FrameRoot ThreadRoot BufferRoot DriverRoot Marked Freed : Addr -> Prop),
      (forall a, Binding a -> FrameRoot a) ->
      (forall a, TypeAssertion a -> FrameRoot a) ->
      (forall a, StateCell a -> FrameRoot a) ->
      (forall a, NamedSpace a -> FrameRoot a) ->
      (forall a, InferredType a -> FrameRoot a) ->
      (forall a, FrameRoot a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ForkLocalRoot Binding TypeAssertion StateCell NamedSpace InferredType a ->
        ~ Freed a.
  Proof.
    intros Binding TypeAssertion StateCell NamedSpace InferredType
           FrameRoot ThreadRoot BufferRoot DriverRoot Marked Freed
           Hbinding Htype Hstate Hnamed Hinferred
           Hframe Hpublish Hdrain Hmark Hsweep a Hfork Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hdrain.
    apply Hpublish.
    apply Hframe.
    destruct Hfork as [Hbinding_a | [Htype_a | [Hstate_a | [Hnamed_a | Hinferred_a]]]].
    - apply Hbinding. exact Hbinding_a.
    - apply Htype. exact Htype_a.
    - apply Hstate. exact Hstate_a.
    - apply Hnamed. exact Hnamed_a.
    - apply Hinferred. exact Hinferred_a.
  Qed.

  Theorem tier_leaf_extra_root_survives_collection :
    forall (VmRegisterRoot JitRegisterRoot Extra ThreadRoot BufferRoot DriverRoot
            Marked Freed : Addr -> Prop),
      (forall a, VmRegisterRoot a -> Extra a) ->
      (forall a, JitRegisterRoot a -> Extra a) ->
      (forall a, Extra a -> ThreadRoot a) ->
      (forall a, ThreadRoot a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        TierLeafExtraRoot VmRegisterRoot JitRegisterRoot a ->
        ~ Freed a.
  Proof.
    intros VmRegisterRoot JitRegisterRoot Extra ThreadRoot BufferRoot DriverRoot
           Marked Freed Hvm Hjit Hextra Hpublish Hdrain Hmark Hsweep
           a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hdrain.
    apply Hpublish.
    apply Hextra.
    destruct Hroot as [Hvm_root | Hjit_root].
    - apply Hvm. exact Hvm_root.
    - apply Hjit. exact Hjit_root.
  Qed.

  Theorem rendezvous_participant_root_survives_collection :
    forall (Occupied Published : Slot -> Prop)
           (SlotRoot : Slot -> Addr -> Prop)
           (BufferRoot DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall s, Occupied s -> Published s) ->
      (forall s a, Published s -> SlotRoot s a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall s a, Occupied s -> SlotRoot s a -> ~ Freed a.
  Proof.
    intros Occupied Published SlotRoot BufferRoot DriverRoot StructuralRoot
           Edge Marked Freed Hwait Hbuffer Hdrain Hmark Hsweep s a Hoccupied Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Hdrain.
    apply (Hbuffer s a).
    - apply Hwait.
      exact Hoccupied.
    - exact Hroot.
  Qed.

  Theorem witness_live_machine_visible_on_sweep :
    forall (Machine : Type)
           (Live Occupied Buffered Swept : Machine -> Prop),
      (forall m, LiveMachineVisible Machine Live Occupied Buffered m) ->
      (forall m, Swept m -> ~ Occupied m \/ Buffered m) ->
      forall m,
        Swept m -> Live m -> Buffered m.
  Proof.
    intros Machine Live Occupied Buffered Swept Hvisible Hgate m Hswept Hlive.
    destruct (Hvisible m Hlive) as [Hoccupied | Hbuffered].
    - destruct (Hgate m Hswept) as [Hnot_occupied | Hbuffered].
      + exfalso. apply Hnot_occupied. exact Hoccupied.
      + exact Hbuffered.
    - exact Hbuffered.
  Qed.

  Theorem cleared_witness_ok_blocks_collection :
    forall (WitnessOk Collect : Prop),
      ~ WitnessOk ->
      (Collect -> WitnessOk) ->
      ~ Collect.
  Proof.
    intros WitnessOk Collect Hcleared Hgate Hcollect.
    apply Hcleared.
    apply Hgate.
    exact Hcollect.
  Qed.

  Theorem fresh_witness_ok_prevents_stale_collect :
    forall (WitnessOk FreshWitness Collect StaleCollect : Prop),
      (Collect -> WitnessOk) ->
      (WitnessOk -> FreshWitness) ->
      (FreshWitness -> ~ StaleCollect) ->
      Collect ->
      ~ StaleCollect.
  Proof.
    intros WitnessOk FreshWitness Collect StaleCollect
           Hgate Hfresh Hfresh_not_stale Hcollect.
    apply Hfresh_not_stale.
    apply Hfresh.
    apply Hgate.
    exact Hcollect.
  Qed.

  Theorem generation_end_bump_releases_worker :
    forall my_gen current_gen : nat,
      current_gen <> my_gen ->
      GenerationCanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen Hadvanced.
    exact Hadvanced.
  Qed.

  Theorem generation_resume_ignores_back_to_back_request :
    forall (my_gen current_gen : nat) (GcRequested : Prop),
      current_gen <> my_gen ->
      GcRequested ->
      GenerationCanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen GcRequested Hadvanced _.
    exact Hadvanced.
  Qed.

  Theorem same_generation_keeps_worker_parked :
    forall my_gen current_gen : nat,
      current_gen = my_gen ->
      ~ GenerationCanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen Hsame Hcan_resume.
    apply Hcan_resume.
    exact Hsame.
  Qed.

  Theorem boolean_resume_reasserted_request_blocks :
    forall GcRequested : Prop,
      GcRequested ->
      ~ BooleanCanResume GcRequested.
  Proof.
    intros GcRequested Hrequested Hcan_resume.
    apply Hcan_resume.
    exact Hrequested.
  Qed.

  Theorem started_gate_prevents_phantom_repark :
    forall (StartedAfterMy Repark Phantom : Prop),
      (Repark -> StartedAfterMy) ->
      (Phantom -> Repark) ->
      (Phantom -> ~ StartedAfterMy) ->
      ~ Phantom.
  Proof.
    intros StartedAfterMy Repark Phantom Hgate Hphantom_repark Hphantom_stale Hphantom.
    apply (Hphantom_stale Hphantom).
    apply Hgate.
    apply Hphantom_repark.
    exact Hphantom.
  Qed.

  Theorem completion_guard_prevents_panic_strand :
    forall (Worker : Type)
           (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> CompletionExit -> Prop),
      (forall w, Spawned w -> CompletionWorkerExited Worker Exit w) ->
      (forall w, Exit w CompletionNormal -> Dropped w) ->
      (forall w, Exit w CompletionPanic -> Dropped w) ->
      ~ CompletionParentWaitStranded Worker Spawned Dropped.
  Proof.
    intros Worker Spawned Dropped Exit Hevery_exits Hnormal Hpanic Hstranded.
    destruct Hstranded as [w [Hspawned Hnot_dropped]].
    apply Hnot_dropped.
    destruct (Hevery_exits w Hspawned) as [Hnormal_exit | Hpanic_exit].
    - apply Hnormal. exact Hnormal_exit.
    - apply Hpanic. exact Hpanic_exit.
  Qed.

  Theorem panic_skip_completion_can_strand_parent_observation :
    forall (Worker : Type)
           (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> CompletionExit -> Prop)
           (ParentObserved : Prop)
           (w : Worker),
      Spawned w ->
      Exit w CompletionPanic ->
      ~ Dropped w ->
      (ParentObserved -> forall u, Spawned u -> Exit u CompletionPanic -> Dropped u) ->
      ~ ParentObserved.
  Proof.
    intros Worker Spawned Dropped Exit ParentObserved w
           Hspawned Hpanic Hnot_dropped Hparent_complete Hobserved.
    apply Hnot_dropped.
    apply Hparent_complete.
    - exact Hobserved.
    - exact Hspawned.
    - exact Hpanic.
  Qed.

  Theorem worker_admission_snapshot_complete :
    forall (Worker : Type)
           (JoinedAtSweep InSnapshot JoinedDuringCollection : Worker -> Prop),
      (forall w, JoinedAtSweep w -> InSnapshot w \/ JoinedDuringCollection w) ->
      (forall w, ~ JoinedDuringCollection w) ->
      forall w,
        JoinedAtSweep w -> InSnapshot w.
  Proof.
    intros Worker JoinedAtSweep InSnapshot JoinedDuringCollection
           Hjoined_shape Hadmission_closed w Hjoined.
    destruct (Hjoined_shape w Hjoined) as [Hsnapshot | Hduring].
    - exact Hsnapshot.
    - exfalso.
      apply (Hadmission_closed w).
      exact Hduring.
  Qed.

  Theorem admitted_worker_survives_sweep :
    forall (Worker : Type)
           (JoinedAtSweep InSnapshot JoinedDuringCollection
            Marked Freed : Worker -> Prop),
      (forall w, JoinedAtSweep w -> InSnapshot w \/ JoinedDuringCollection w) ->
      (forall w, ~ JoinedDuringCollection w) ->
      (forall w, InSnapshot w -> Marked w) ->
      (forall w, Freed w -> ~ Marked w) ->
      forall w,
        JoinedAtSweep w -> ~ Freed w.
  Proof.
    intros Worker JoinedAtSweep InSnapshot JoinedDuringCollection Marked Freed
           Hjoined_shape Hadmission_closed Hsnapshot_marked Hsweep w Hjoined Hfreed.
    apply (Hsweep w Hfreed).
    apply Hsnapshot_marked.
    apply (worker_admission_snapshot_complete
             Worker JoinedAtSweep InSnapshot JoinedDuringCollection).
    - exact Hjoined_shape.
    - exact Hadmission_closed.
    - exact Hjoined.
  Qed.

  Theorem driver_root_union_channel_survives_collection :
    forall (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor
            DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, WorkerRoot a -> DriverRoot a) ->
      (forall a, SafepointRoot a -> DriverRoot a) ->
      (forall a, EnvAnchor a -> DriverRoot a) ->
      (forall a, DispatchAnchor a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        DriverRootUnion WorkerRoot SafepointRoot EnvAnchor DispatchAnchor a ->
        ~ Freed a.
  Proof.
    intros WorkerRoot SafepointRoot EnvAnchor DispatchAnchor DriverRoot StructuralRoot
           Edge Marked Freed Hworker Hsafepoint Henv Hdispatch Hmark Hsweep
           a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    destruct Hroot as [Hworker_a | [Hsafepoint_a | [Henv_a | Hdispatch_a]]].
    - apply Hworker. exact Hworker_a.
    - apply Hsafepoint. exact Hsafepoint_a.
    - apply Henv. exact Henv_a.
    - apply Hdispatch. exact Hdispatch_a.
  Qed.

  Theorem midloop_root_union_channel_survives_collection :
    forall (LiveSCK Env0 Global KSpine Deferred DriverC
            MidloopRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, LiveSCK a -> MidloopRoot a) ->
      (forall a, Env0 a -> MidloopRoot a) ->
      (forall a, Global a -> MidloopRoot a) ->
      (forall a, KSpine a -> MidloopRoot a) ->
      (forall a, Deferred a -> MidloopRoot a) ->
      (forall a, DriverC a -> MidloopRoot a) ->
      (forall a, Reach MidloopRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC a ->
        ~ Freed a.
  Proof.
    intros LiveSCK Env0 Global KSpine Deferred DriverC MidloopRoot
           Edge Marked Freed HliveSCK Henv0 Hglobal HkSpine Hdeferred HdriverC
           Hmark Hsweep a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    destruct Hroot as [HliveSCK_a | [Henv0_a | [Hglobal_a | [HkSpine_a | [Hdeferred_a | HdriverC_a]]]]].
    - apply HliveSCK. exact HliveSCK_a.
    - apply Henv0. exact Henv0_a.
    - apply Hglobal. exact Hglobal_a.
    - apply HkSpine. exact HkSpine_a.
    - apply Hdeferred. exact Hdeferred_a.
    - apply HdriverC. exact HdriverC_a.
  Qed.

  Theorem midloop_future_touch_survives_collection :
    forall (LiveSCK Env0 Global KSpine Deferred DriverC
            Marked Freed FutureTouch : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a,
          Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a,
          FutureTouch a ->
          Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros LiveSCK Env0 Global KSpine Deferred DriverC
           Marked Freed FutureTouch Edge Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem structural_future_touch_survives_collection :
    forall (StructuralRoot DriverRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed FutureTouch : Addr -> Prop),
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, FutureTouch a -> Reach (CollectorRoot StructuralRoot DriverRoot) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot Edge Marked Freed FutureTouch Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem complete_node_edge_reader_retains_semantic_reachability :
    forall (StructuralRoot DriverRoot Marked Freed : Addr -> Prop)
           (InlineEdge SideEdge SpaceEdge ReaderEdge : Addr -> Addr -> Prop),
      (forall a, CollectorRoot StructuralRoot DriverRoot a -> Marked a) ->
      (forall parent child, Marked parent -> ReaderEdge parent child -> Marked child) ->
      (forall parent child, InlineEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SideEdge parent child -> ReaderEdge parent child) ->
      (forall parent child, SpaceEdge parent child -> ReaderEdge parent child) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        Reach (CollectorRoot StructuralRoot DriverRoot)
          (SemanticNodeEdge InlineEdge SideEdge SpaceEdge) a ->
        ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot Marked Freed InlineEdge SideEdge SpaceEdge ReaderEdge
           Hroot Hclosed Hinline Hside Hspace Hsweep a Hreach.
    assert (Marked a) as Hmarked.
    {
      induction Hreach as [a Hroot_a | parent child _ IH Hedge].
      - apply Hroot. exact Hroot_a.
      - eapply Hclosed.
        + exact IH.
        + destruct Hedge as [Hinline_edge | [Hside_edge | Hspace_edge]].
          * apply Hinline. exact Hinline_edge.
          * apply Hside. exact Hside_edge.
          * apply Hspace. exact Hspace_edge.
    }
    intro Hfreed.
    apply (Hsweep a Hfreed).
    exact Hmarked.
  Qed.

  Theorem published_driver_c_survives_collection :
    forall (DriverC DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, DriverC a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, DriverC a -> ~ Freed a.
  Proof.
    intros DriverC DriverRoot StructuralRoot Edge Marked Freed
           Hpublished Hmark Hsweep a Hdriver Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Hpublished.
    exact Hdriver.
  Qed.

  Theorem batch_result_handoff_survives_collection :
    forall (StructuralRoot DriverRoot BatchResult
            HandleRoot OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> BatchHandoffRoot HandleRoot OutputRoot a) ->
      (forall a, HandleRoot a -> DriverRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult HandleRoot OutputRoot
           Edge Marked Freed Hcovered Hhandle_driver Houtput_driver Hmark Hsweep
           a Hresult Hfreed.
    destruct (Hcovered a Hresult) as [Hhandle | Houtput].
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Hhandle_driver.
      exact Hhandle.
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Houtput_driver.
      exact Houtput.
  Qed.

  Theorem batch_handle_drop_after_output_copy_survives_collection :
    forall (StructuralRoot DriverRoot BatchResult OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> OutputRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult OutputRoot Edge Marked Freed
           Hcopied Houtput_driver Hmark Hsweep a Hresult Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Houtput_driver.
    apply Hcopied.
    exact Hresult.
  Qed.

  Theorem nursery_open_signal_requests_minor :
    forall YoungOverBudget NurseryPending : Prop,
      NurseryPending ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending Hpending.
    right.
    exact Hpending.
  Qed.

  Theorem young_budget_requests_minor :
    forall YoungOverBudget NurseryPending : Prop,
      YoungOverBudget ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending Hover_budget.
    left.
    exact Hover_budget.
  Qed.

  Theorem promotion_clears_stale_nursery_trigger :
    forall YoungOverBudget NurseryPending
           YoungOdometerReset NurseryPendingCleared : Prop,
      ~ YoungOverBudget ->
      (NurseryPendingCleared -> ~ NurseryPending) ->
      PromotionRelaxed YoungOdometerReset NurseryPendingCleared ->
      ~ MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending YoungOdometerReset NurseryPendingCleared
           Hnot_over Hcleared_not_pending Hrelaxed Hminor.
    destruct Hrelaxed as [_ Hcleared].
    destruct Hminor as [Hover | Hpending].
    - apply Hnot_over. exact Hover.
    - apply (Hcleared_not_pending Hcleared). exact Hpending.
  Qed.

  Theorem cap_major_not_deferred :
    forall Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop,
      CapMajor ->
      DoMajor Level3 MinorDueNow LiveMajor CapMajor CadenceMajor.
  Proof.
    intros Level3 MinorDueNow LiveMajor CapMajor CadenceMajor Hcap.
    split.
    - right. left. exact Hcap.
    - intros Hdefer.
      destruct Hdefer as [_ [_ [Hnot_cap _]]].
      apply Hnot_cap. exact Hcap.
  Qed.

  Theorem cadence_major_not_deferred :
    forall Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop,
      CadenceMajor ->
      DoMajor Level3 MinorDueNow LiveMajor CapMajor CadenceMajor.
  Proof.
    intros Level3 MinorDueNow LiveMajor CapMajor CadenceMajor Hcadence.
    split.
    - right. right. exact Hcadence.
    - intros Hdefer.
      destruct Hdefer as [_ [_ [_ Hnot_cadence]]].
      apply Hnot_cadence. exact Hcadence.
  Qed.

  Theorem non_level3_major_runs :
    forall Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop,
      MajorDue LiveMajor CapMajor CadenceMajor ->
      ~ Level3 ->
      DoMajor Level3 MinorDueNow LiveMajor CapMajor CadenceMajor.
  Proof.
    intros Level3 MinorDueNow LiveMajor CapMajor CadenceMajor Hmajor Hnot_level3.
    split.
    - exact Hmajor.
    - intros Hdefer.
      destruct Hdefer as [Hlevel3 _].
      apply Hnot_level3. exact Hlevel3.
  Qed.

  Theorem live_major_level3_minor_can_be_deferred :
    forall Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop,
      LiveMajor ->
      Level3 ->
      MinorDueNow ->
      ~ CapMajor ->
      ~ CadenceMajor ->
      DeferredMajor Level3 MinorDueNow LiveMajor CapMajor CadenceMajor.
  Proof.
    intros Level3 MinorDueNow LiveMajor CapMajor CadenceMajor
           Hlive Hlevel3 Hminor Hnot_cap Hnot_cadence.
    repeat split.
    - left. exact Hlive.
    - exact Hlevel3.
    - exact Hminor.
    - exact Hnot_cap.
    - exact Hnot_cadence.
  Qed.

  Theorem deferred_major_is_live_growth_only :
    forall Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop,
      DeferredMajor Level3 MinorDueNow LiveMajor CapMajor CadenceMajor ->
      LiveMajor /\ ~ CapMajor /\ ~ CadenceMajor.
  Proof.
    intros Level3 MinorDueNow LiveMajor CapMajor CadenceMajor Hdeferred.
    destruct Hdeferred as [Hmajor [_ [_ [Hnot_cap Hnot_cadence]]]].
    split.
    - destruct Hmajor as [Hlive | [Hcap | Hcadence]].
      + exact Hlive.
      + exfalso. apply Hnot_cap. exact Hcap.
      + exfalso. apply Hnot_cadence. exact Hcadence.
    - split.
      + exact Hnot_cap.
      + exact Hnot_cadence.
  Qed.

  Theorem raised_floor_blocks_immediate_cap_refire :
    forall Committed BaseCap NextFloor : nat,
      NextFloor = Committed ->
      ~ CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap NextFloor Hfloor Hcap_due.
    subst NextFloor.
    unfold CapDue in Hcap_due.
    assert (Committed <= Nat.max BaseCap Committed) by apply Nat.le_max_r.
    lia.
  Qed.

  Theorem cap_refire_after_raised_floor_requires_growth :
    forall CommittedAfterSweep CommittedNext BaseCap NextFloor : nat,
      NextFloor = CommittedAfterSweep ->
      CapDue CommittedNext BaseCap NextFloor ->
      CommittedNext > CommittedAfterSweep.
  Proof.
    intros CommittedAfterSweep CommittedNext BaseCap NextFloor Hfloor Hcap_due.
    subst NextFloor.
    unfold CapDue in Hcap_due.
    assert (CommittedAfterSweep <= Nat.max BaseCap CommittedAfterSweep)
      by apply Nat.le_max_r.
    lia.
  Qed.

  Theorem futile_cap_major_raise_prevents_same_committed_refire :
    forall Committed BaseCap OldFloor ReleasedSegments NextFloor : nat,
      FutileCapMajor Committed BaseCap OldFloor ReleasedSegments ->
      NextFloor = Committed ->
      ~ CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap OldFloor ReleasedSegments NextFloor _ Hfloor.
    apply raised_floor_blocks_immediate_cap_refire.
    exact Hfloor.
  Qed.

  Theorem cleared_floor_restores_base_cap_trigger :
    forall Committed BaseCap NextFloor : nat,
      NextFloor = 0 ->
      Committed > BaseCap ->
      CapDue Committed BaseCap NextFloor.
  Proof.
    intros Committed BaseCap NextFloor Hfloor Hover_base.
    subst NextFloor.
    unfold CapDue.
    rewrite Nat.max_0_r.
    exact Hover_base.
  Qed.

  Theorem young_reachable_marked :
    forall (Root Young Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      forall a, Reach Root Edge a -> Young a -> Marked a.
  Proof.
    intros Root Young Marked Edge Hroot Hno_old_to_young Hclosed a Hreach.
    induction Hreach as [a Hroot_a | parent child Hparent IH Hedge].
    - intro Hyoung.
      apply Hroot; assumption.
    - intro Hchild_young.
      pose proof (Hno_old_to_young parent child Hedge Hchild_young) as Hparent_young.
      apply Hclosed with (parent := parent); auto.
  Qed.

  Theorem young_minor_reachable_survives_sweep :
    forall (Root Young Marked Freed : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Reach Root Edge a -> Young a -> ~ Freed a.
  Proof.
    intros Root Young Marked Freed Edge Hroot Hno_old_to_young Hclosed Hsweep a Hreach Hyoung Hfreed.
    apply (Hsweep a Hfreed).
    eapply young_reachable_marked; eauto.
  Qed.

  Theorem reachable_seen :
    forall (Root Seen : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      forall a, Reach Root Edge a -> Seen a.
  Proof.
    intros Root Seen Edge Hroot_seen Hseen_closed a Hreach.
    induction Hreach as [a Hroot | parent child _ IH Hedge].
    - apply Hroot_seen.
      exact Hroot.
    - eapply Hseen_closed; eauto.
  Qed.

  Theorem conservative_young_reachable_marked :
    forall (Root Young Seen Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      (forall a, Seen a -> Young a -> Marked a) ->
      forall a, Reach Root Edge a -> Young a -> Marked a.
  Proof.
    intros Root Young Seen Marked Edge Hroot_seen Hseen_closed Hseen_young_marked
      a Hreach Hyoung.
    apply Hseen_young_marked; [| exact Hyoung].
    eapply reachable_seen; eauto.
  Qed.

  Theorem conservative_young_minor_reachable_survives_sweep :
    forall (Root Young Seen Marked Freed : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      (forall a, Seen a -> Young a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Reach Root Edge a -> Young a -> ~ Freed a.
  Proof.
    intros Root Young Seen Marked Freed Edge Hroot_seen Hseen_closed Hseen_young_marked
      Hsweep a Hreach Hyoung Hfreed.
    apply (Hsweep a Hfreed).
    eapply conservative_young_reachable_marked; eauto.
  Qed.

  Theorem e2_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a,
          SnapshotLive a ->
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked Freed SnapshotLive
           Hmark Hsweep Hcovered a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hcovered.
    exact Hlive.
  Qed.

  Theorem e2_phase_gate_removed_snapshot_preimage_shaded :
    forall (InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
            AfterStartDeletion RemovedBySweep Shaded SnapshotLive : Addr -> Prop),
      (forall a, SnapshotLive a -> InCacheAtStart a) ->
      (forall a, BeganBeforeStart a -> CommittedBeforeStart a \/ OpenAtStart a) ->
      (forall a, CommittedBeforeStart a -> ~ InCacheAtStart a) ->
      (forall a, ~ OpenAtStart a) ->
      (forall a, RemovedBySweep a -> BeganBeforeStart a \/ AfterStartDeletion a) ->
      (forall a, AfterStartDeletion a -> Shaded a) ->
      forall a,
        SnapshotLive a -> RemovedBySweep a -> Shaded a.
  Proof.
    intros InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
           AfterStartDeletion RemovedBySweep Shaded SnapshotLive
           Hsnapshot_visible Hpre_start_state Hcommitted_invisible Hphase_gate
           Hremoved_shape Hafter_start_shaded a Hsnapshot Hremoved.
    destruct (Hremoved_shape a Hremoved) as [Hbefore | Hafter].
    - destruct (Hpre_start_state a Hbefore) as [Hcommitted | Hopen].
      + exfalso.
        apply (Hcommitted_invisible a Hcommitted).
        apply Hsnapshot_visible.
        exact Hsnapshot.
      + exfalso.
        apply (Hphase_gate a).
        exact Hopen.
    - apply Hafter_start_shaded.
      exact Hafter.
  Qed.

  Theorem e2_sweep_gate_snapshot_live_survives_collection :
    forall (Sweep : Prop)
           (DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
            InCacheAtSweep Shaded SnapshotLive Marked Freed : Addr -> Prop),
      (Sweep -> forall a, ~ DeleteOpenAtSweep a) ->
      (forall a, RemovedBeforeSweep a ->
        DeleteOpenAtSweep a \/ CommittedBeforeSweep a) ->
      (forall a, CommittedBeforeSweep a -> Shaded a) ->
      (forall a, SnapshotLive a -> RemovedBeforeSweep a \/ InCacheAtSweep a) ->
      (forall a, InCacheAtSweep a -> Marked a) ->
      (forall a, Shaded a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      Sweep ->
      forall a,
        SnapshotLive a -> ~ Freed a.
  Proof.
    intros Sweep DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
           InCacheAtSweep Shaded SnapshotLive Marked Freed
           Hsweep_gate Hremoved_state Hcommitted_shaded Hsnapshot_shape
           Hvisible_marked Hshaded_marked Hsweep_only_unmarked Hsweep
           a Hsnapshot Hfreed.
    apply (Hsweep_only_unmarked a Hfreed).
    destruct (Hsnapshot_shape a Hsnapshot) as [Hremoved | Hvisible].
    - destruct (Hremoved_state a Hremoved) as [Hopen | Hcommitted].
      + exfalso.
        apply (Hsweep_gate Hsweep a).
        exact Hopen.
      + apply Hshaded_marked.
        apply Hcommitted_shaded.
        exact Hcommitted.
    - apply Hvisible_marked.
      exact Hvisible.
  Qed.

  Theorem e2_published_allocate_black_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PublishedAlloc : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, PublishedAlloc a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc
           Edge Marked Freed Hpublished_black Hmark Hsweep a Hpublished Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; right.
    apply Hpublished_black.
    exact Hpublished.
  Qed.

  Theorem index_arena_returned_addr_read_ready :
    forall (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
            ReadObserved : Addr -> Prop),
      (forall a, ReadObserved a -> AddrReturned a) ->
      (forall a, AddrReturned a -> SlotPublished a) ->
      (forall a, SlotPublished a -> SlotWritten a) ->
      (forall a, SlotPublished a -> SegmentPublished a) ->
      (forall a, SegmentPublished a -> SegmentWritten a) ->
      forall a,
        ReadObserved a ->
        PublishedSlotReady
          SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned a.
  Proof.
    intros SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
           ReadObserved Hread Hreturn Hslot_write Hslot_segment Hsegment_write
           a Hread_observed.
    split.
    - apply Hsegment_write. apply Hslot_segment. apply Hreturn. apply Hread.
      exact Hread_observed.
    - split.
      + apply Hslot_segment. apply Hreturn. apply Hread. exact Hread_observed.
      + split.
        * apply Hslot_write. apply Hreturn. apply Hread. exact Hread_observed.
        * split.
          -- apply Hreturn. apply Hread. exact Hread_observed.
          -- apply Hread. exact Hread_observed.
  Qed.

  Theorem side_arena_read_published_entry_ready :
    forall (PagePublished ChunkPublished PageReady ChunkReady EntryWritten
            EntryPublished ReadObserved : Slot -> Prop),
      (forall s, ReadObserved s -> EntryPublished s) ->
      (forall s, EntryPublished s -> EntryWritten s) ->
      (forall s, EntryPublished s -> ChunkPublished s) ->
      (forall s, ChunkPublished s -> PagePublished s) ->
      (forall s, PagePublished s -> PageReady s) ->
      (forall s, ChunkPublished s -> ChunkReady s) ->
      forall s,
        ReadObserved s ->
        PublishedEntryReady PageReady ChunkReady EntryWritten EntryPublished s.
  Proof.
    intros PagePublished ChunkPublished PageReady ChunkReady EntryWritten
           EntryPublished ReadObserved Hread Hwrite Hchunk Hpage
           Hpage_ready Hchunk_ready s Hread_observed.
    split.
    - apply Hpage_ready. apply Hpage. apply Hchunk. apply Hread.
      exact Hread_observed.
    - split.
      + apply Hchunk_ready. apply Hchunk. apply Hread.
        exact Hread_observed.
      + split.
        * apply Hwrite. apply Hread. exact Hread_observed.
        * apply Hread. exact Hread_observed.
  Qed.

  Theorem published_side_bearing_node_reads_ready_payload :
    forall (node_seg side_seg : nat) (SideReady NodePublished : Prop),
      (NodePublished -> SideReady) ->
      (NodePublished -> side_seg = node_seg) ->
      NodePublished ->
      PublishedNodeSideReady node_seg side_seg SideReady NodePublished.
  Proof.
    intros node_seg side_seg SideReady NodePublished Hside Hsame Hpublished.
    split.
    - exact Hpublished.
    - split.
      + apply Hside. exact Hpublished.
      + apply Hsame. exact Hpublished.
  Qed.

  Theorem wrong_segment_cannot_satisfy_published_node_side_ready :
    forall (node_seg side_seg : nat) (SideReady NodePublished : Prop),
      side_seg <> node_seg ->
      ~ PublishedNodeSideReady node_seg side_seg SideReady NodePublished.
  Proof.
    intros node_seg side_seg SideReady NodePublished Hwrong Hready.
    destruct Hready as [_ [_ Hsame]].
    apply Hwrong.
    exact Hsame.
  Qed.

  Theorem side_free_shadow_clear_blocks_future_deref :
    forall (Quiescent SideFreed StackLaunderLive ShadowCleared FutureDeref : Prop),
      (SideFreed -> Quiescent) ->
      (StackLaunderLive -> ~ Quiescent) ->
      (SideFreed -> ShadowCleared) ->
      (FutureDeref -> StackLaunderLive \/ ~ ShadowCleared) ->
      SideFreed ->
      ~ FutureDeref.
  Proof.
    intros Quiescent SideFreed StackLaunderLive ShadowCleared FutureDeref
           Hfree_quiescent Hstack_nonquiescent Hclear Hderef_shape Hfreed Hderef.
    destruct (Hderef_shape Hderef) as [Hstack | Hnot_cleared].
    - apply (Hstack_nonquiescent Hstack).
      exact (Hfree_quiescent Hfreed).
    - apply Hnot_cleared.
      exact (Hclear Hfreed).
  Qed.

  Theorem major_hash_cons_hit_not_freed :
    forall (Retained Marked Freed Returned : Addr -> Prop),
      (forall a, Returned a -> Retained a) ->
      (forall a, Retained a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Returned a -> ~ Freed a.
  Proof.
    intros Retained Marked Freed Returned Hreturned_retained Hretained_marked
           Hsweep a Hreturned Hfreed.
    apply (Hsweep a Hfreed).
    apply Hretained_marked.
    apply Hreturned_retained.
    exact Hreturned.
  Qed.

  Theorem minor_hash_cons_hit_not_freed :
    forall (Retained Young Marked Freed Returned : Addr -> Prop),
      (forall a, Returned a -> Retained a) ->
      (forall a, Retained a -> ~ Young a \/ Marked a) ->
      (forall a, Freed a -> Young a /\ ~ Marked a) ->
      forall a, Returned a -> ~ Freed a.
  Proof.
    intros Retained Young Marked Freed Returned Hreturned_retained
           Hretained_old_or_marked Hminor_frees a Hreturned Hfreed.
    destruct (Hretained_old_or_marked a (Hreturned_retained a Hreturned))
      as [Hold | Hmarked].
    - destruct (Hminor_frees a Hfreed) as [Hyoung _].
      apply Hold.
      exact Hyoung.
    - destruct (Hminor_frees a Hfreed) as [_ Hunmarked].
      apply Hunmarked.
      exact Hmarked.
  Qed.

  Theorem e2_final_remark_root_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            FinalRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, FinalRoot a -> DriverRoot a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, FinalRoot a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack FinalRoot
           Edge Marked Freed Hremark Hmark Hsweep a Hfinal Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; left.
    apply Hremark.
    exact Hfinal.
  Qed.

  Theorem e2_closed_final_sweep_uses_stw_backstop :
    forall (FinalSweepReturned SatbSwept SatbAbort StwFallbackRan : Prop),
      (FinalSweepReturned -> ~ SatbSwept -> SatbAbort) ->
      (SatbAbort -> StwFallbackRan) ->
      FinalSweepReturned ->
      ~ SatbSwept ->
      StwFallbackRan.
  Proof.
    intros FinalSweepReturned SatbSwept SatbAbort StwFallbackRan
           Hclosed_abort Habort_stw Hreturned Hnot_swept.
    apply Habort_stw.
    apply Hclosed_abort; assumption.
  Qed.

  Theorem e2_aborted_satb_runs_requested_stw :
    forall (SatbAbort StwRequested StwRan : Prop),
      (SatbAbort -> StwRequested) ->
      (StwRequested -> StwRan) ->
      SatbAbort ->
      StwRequested /\ StwRan.
  Proof.
    intros SatbAbort StwRequested StwRan Hrequest Hrun Habort.
    split.
    - apply Hrequest; exact Habort.
    - apply Hrun.
      apply Hrequest.
      exact Habort.
  Qed.

  Theorem e2_completed_satb_request_has_collection :
    forall (SatbSuccess SatbAbort SatbSwept StwFallbackRan RequestDone : Prop),
      (SatbSuccess -> SatbSwept) ->
      (SatbAbort -> StwFallbackRan) ->
      (RequestDone -> SatbSuccess \/ SatbAbort) ->
      RequestDone ->
      RequestHandled SatbSwept StwFallbackRan.
  Proof.
    intros SatbSuccess SatbAbort SatbSwept StwFallbackRan RequestDone
           Hsuccess_swept Habort_stw Hdone_case Hdone.
    destruct (Hdone_case Hdone) as [Hsuccess | Habort].
    - left. apply Hsuccess_swept. exact Hsuccess.
    - right. apply Habort_stw. exact Habort.
  Qed.

  Theorem e2_full_major_clears_all_satb_marks :
    forall (SATBMarked Swept Cleared : Addr -> Prop),
      (forall a, SATBMarked a -> Swept a) ->
      (forall a, Swept a -> Cleared a) ->
      forall a, SATBMarked a -> Cleared a.
  Proof.
    intros SATBMarked Swept Cleared Hsatb_swept Hswept_cleared a Hmarked.
    apply Hswept_cleared.
    apply Hsatb_swept.
    exact Hmarked.
  Qed.

  Theorem e2_full_major_leaves_no_stale_mark_after_promotion :
    forall (Swept MarkedAfterSweep MarkedAfterPromotion : Addr -> Prop),
      (forall a, Swept a) ->
      (forall a, Swept a -> ~ MarkedAfterSweep a) ->
      (forall a, MarkedAfterPromotion a -> MarkedAfterSweep a) ->
      forall a, ~ MarkedAfterPromotion a.
  Proof.
    intros Swept MarkedAfterSweep MarkedAfterPromotion
           Hfull Hcleared Hpromotion_no_set a Hmarked_after_promotion.
    apply (Hcleared a).
    - apply Hfull.
    - apply Hpromotion_no_set.
      exact Hmarked_after_promotion.
  Qed.

  Theorem e2_young_only_sweep_safe_requires_no_surviving_old_satb_mark :
    forall (SATBMarked Old MarkedAfter : Addr -> Prop),
      (forall a, SATBMarked a -> Old a -> MarkedAfter a) ->
      (forall a, ~ MarkedAfter a) ->
      forall a, SATBMarked a -> Old a -> False.
  Proof.
    intros SATBMarked Old MarkedAfter Hold_survives Hno_stale a Hsatb Hold.
    apply (Hno_stale a).
    apply Hold_survives; assumption.
  Qed.

  Theorem e2_e0_cache_removed_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           CapacityVictim OverwriteVictim BulkClearedEntry Edge Marked Freed SnapshotLive
           Hcapacity Hoverwrite Hbulk Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hcapacity_a | [Hoverwrite_a | Hbulk_a]].
    - apply Hcapacity; exact Hcapacity_a.
    - apply Hoverwrite; exact Hoverwrite_a.
    - apply Hbulk; exact Hbulk_a.
  Qed.

  Theorem e2_e0_removed_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, SpacePreimage a -> ShadedDeletion a) ->
      (forall a, RulePreimage a -> ShadedDeletion a) ->
      (forall a, EnvPreimage a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           SpacePreimage RulePreimage EnvPreimage Edge Marked Freed SnapshotLive
           Hspace Hrule Henv Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hspace_a | [Hrule_a | Henv_a]].
    - apply Hspace; exact Hspace_a.
    - apply Hrule; exact Hrule_a.
    - apply Henv; exact Henv_a.
  Qed.

  Theorem abstract_gc_future_touch_survives_live_narrowing :
    forall (FullRoot DeadForNext FutureTouch Marked Freed : Addr -> Prop),
      (forall a, FutureTouch a -> FullRoot a) ->
      (forall a, FutureTouch a -> ~ DeadForNext a) ->
      (forall a, AbstractGCRoot FullRoot DeadForNext a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros FullRoot DeadForNext FutureTouch Marked Freed
           Hfuture_full Hfuture_not_dead Hmark Hsweep a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    split.
    - apply Hfuture_full.
      exact Htouch.
    - apply Hfuture_not_dead.
      exact Htouch.
  Qed.

  Theorem operator_cache_returned_entry_current_after_ensure :
    forall (Entry Epoch : Type)
           (entry_epoch : Entry -> Epoch)
           (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      (forall e, CacheBefore e -> entry_epoch e = local_epoch) ->
      OperatorCacheEnsurePost Entry Epoch heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, Returned e -> entry_epoch e = heap_epoch.
  Proof.
    intros Entry Epoch entry_epoch heap_epoch local_epoch
           CacheBefore CacheAfter Returned Hstamped Hensure Hlookup e Hreturned.
    destruct Hensure as [[Hcurrent Hpreserved] | [_ Hcleared]].
    - rewrite (Hstamped e (Hpreserved e (Hlookup e Hreturned))).
      exact Hcurrent.
    - exfalso.
      apply (Hcleared e).
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem epoch_protected_cache_lookup_not_stale_after_validate :
    forall (Entry Epoch : Type)
           (Stale : Epoch -> Entry -> Prop)
           (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      (local_epoch = heap_epoch ->
        forall e, CacheBefore e -> ~ Stale heap_epoch e) ->
      EpochProtectedCacheValidatePost Entry Epoch heap_epoch local_epoch
        CacheBefore CacheAfter Stale ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, Returned e -> ~ Stale heap_epoch e.
  Proof.
    intros Entry Epoch Stale heap_epoch local_epoch
           CacheBefore CacheAfter Returned
           Hcurrent_safe Hvalidated Hlookup e Hreturned.
    destruct Hvalidated as [[Hcurrent Hsubset] | [_ [_ Hnot_stale]]].
    - apply Hcurrent_safe.
      + exact Hcurrent.
      + apply Hsubset.
        apply Hlookup.
        exact Hreturned.
    - apply Hnot_stale.
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem write_once_anchor_survives_collection :
    forall (Anchor : Type)
           (Initialized Deleted Scanned : Anchor -> Prop)
           (AnchorValue : Anchor -> Addr -> Prop)
           (StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall slot, Initialized slot -> ~ Deleted slot) ->
      (forall slot, WriteOnceAnchorLive Anchor Initialized Deleted slot -> Scanned slot) ->
      (forall slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall slot a,
        Initialized slot ->
        AnchorValue slot a ->
        ~ Freed a.
  Proof.
    intros Anchor Initialized Deleted Scanned AnchorValue StructuralRoot Marked Freed Edge
           Hnot_deleted Hscanned Hroot Hmark Hsweep slot a Hinitialized Hvalue Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (Hroot slot a).
    - apply Hscanned.
      split.
      + exact Hinitialized.
      + apply Hnot_deleted.
        exact Hinitialized.
    - exact Hvalue.
  Qed.

  Theorem registered_space_value_survives_collection :
    forall (Space : Type)
           (Registered Scanned : Space -> Prop)
           (SpaceValue : Space -> Addr -> Prop)
           (StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall s, Registered s -> Scanned s) ->
      (forall s a, Scanned s -> SpaceValue s a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall s a,
        Registered s ->
        SpaceValue s a ->
        ~ Freed a.
  Proof.
    intros Space Registered Scanned SpaceValue StructuralRoot Marked Freed Edge
           Hscan Hroot Hmark Hsweep s a Hregistered Hvalue Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (Hroot s a).
    - apply Hscan.
      exact Hregistered.
    - exact Hvalue.
  Qed.

  Theorem removed_space_registry_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            OverwriteVictim RemoveVictim ClearVictim SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, RemoveVictim a -> ShadedDeletion a) ->
      (forall a, ClearVictim a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          SpaceRegistryRemovedValue OverwriteVictim RemoveVictim ClearVictim a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           OverwriteVictim RemoveVictim ClearVictim SnapshotLive Edge Marked Freed
           Hoverwrite Hremove Hclear Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hoverwrite_a | [Hremove_a | Hclear_a]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hremove. exact Hremove_a.
    - apply Hclear. exact Hclear_a.
  Qed.

  Theorem old_pending_guard_cannot_remove_newer_tiered_root :
    forall (EntryToken : Slot -> Addr)
           (old_entry new_entry : Slot)
           (old_guard : Addr),
      EntryToken old_entry = old_guard ->
      EntryToken new_entry <> old_guard ->
      ~ TokenCheckedRemove EntryToken old_guard new_entry.
  Proof.
    intros EntryToken old_entry new_entry old_guard Hold Hnew Hremove.
    unfold TokenCheckedRemove in Hremove.
    apply Hnew.
    exact Hremove.
  Qed.

  Theorem registered_tiered_cache_value_survives_collection :
    forall (PendingRoot CompiledConstant PendingScanned CompiledScanned
            StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, PendingRoot a -> PendingScanned a) ->
      (forall a, PendingScanned a -> StructuralRoot a) ->
      (forall a, CompiledConstant a -> CompiledScanned a) ->
      (forall a, CompiledScanned a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        TieredCacheRegisteredValue PendingRoot CompiledConstant a ->
        ~ Freed a.
  Proof.
    intros PendingRoot CompiledConstant PendingScanned CompiledScanned
           StructuralRoot Marked Freed Edge Hscan_pending Hroot_pending
           Hscan_compiled Hroot_compiled Hmark Hsweep a Hregistered Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    destruct Hregistered as [Hpending | Hcompiled].
    - apply Hroot_pending.
      apply Hscan_pending.
      exact Hpending.
    - apply Hroot_compiled.
      apply Hscan_compiled.
      exact Hcompiled.
  Qed.

  Theorem removed_tiered_cache_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
            ClearPendingVictim ClearCompiledConstant SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, PendingOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, PendingCancelVictim a -> ShadedDeletion a) ->
      (forall a, PendingGuardDropVictim a -> ShadedDeletion a) ->
      (forall a, ClearPendingVictim a -> ShadedDeletion a) ->
      (forall a, ClearCompiledConstant a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          TieredCacheRemovedValue
            PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
            ClearPendingVictim ClearCompiledConstant a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
           ClearPendingVictim ClearCompiledConstant SnapshotLive Edge Marked Freed
           Hoverwrite Hcancel Hguard Hclear_pending Hclear_compiled Hremoved Hmark Hsweep
           a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as
        [Hoverwrite_a | [Hcancel_a | [Hguard_a | [Hclear_pending_a | Hclear_compiled_a]]]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hcancel. exact Hcancel_a.
    - apply Hguard. exact Hguard_a.
    - apply Hclear_pending. exact Hclear_pending_a.
    - apply Hclear_compiled. exact Hclear_compiled_a.
  Qed.

  Theorem registered_thread_local_table_value_survives_collection :
    forall (EvalMemoResult MatchResult SubgoalResult ThunkResult
            EvalMemoScanned MatchResultScanned SubgoalScanned ThunkScanned
            StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, EvalMemoResult a -> EvalMemoScanned a) ->
      (forall a, EvalMemoScanned a -> StructuralRoot a) ->
      (forall a, MatchResult a -> MatchResultScanned a) ->
      (forall a, MatchResultScanned a -> StructuralRoot a) ->
      (forall a, SubgoalResult a -> SubgoalScanned a) ->
      (forall a, SubgoalScanned a -> StructuralRoot a) ->
      (forall a, ThunkResult a -> ThunkScanned a) ->
      (forall a, ThunkScanned a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ThreadLocalTableRegisteredValue
          EvalMemoResult MatchResult SubgoalResult ThunkResult a ->
        ~ Freed a.
  Proof.
    intros EvalMemoResult MatchResult SubgoalResult ThunkResult
           EvalMemoScanned MatchResultScanned SubgoalScanned ThunkScanned
           StructuralRoot Marked Freed Edge Hscan_eval Hroot_eval
           Hscan_match Hroot_match Hscan_subgoal Hroot_subgoal
           Hscan_thunk Hroot_thunk Hmark Hsweep a Hregistered Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    destruct Hregistered as [Heval | [Hmatch | [Hsubgoal | Hthunk]]].
    - apply Hroot_eval.
      apply Hscan_eval.
      exact Heval.
    - apply Hroot_match.
      apply Hscan_match.
      exact Hmatch.
    - apply Hroot_subgoal.
      apply Hscan_subgoal.
      exact Hsubgoal.
    - apply Hroot_thunk.
      apply Hscan_thunk.
      exact Hthunk.
  Qed.

  Theorem removed_thread_local_table_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
            SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
            ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim
            SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, SubgoalStaleVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalRemoveVictim a -> ShadedDeletion a) ->
      (forall a, SubgoalClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkStaleVictim a -> ShadedDeletion a) ->
      (forall a, ThunkOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, ThunkRemoveVictim a -> ShadedDeletion a) ->
      (forall a, ThunkClearVictim a -> ShadedDeletion a) ->
      (forall a, ThunkReplaceVictim a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          ThreadLocalTableRemovedValue
            SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
            SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
            ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           SubgoalStaleVictim SubgoalOverwriteVictim SubgoalRemoveVictim
           SubgoalClearVictim ThunkStaleVictim ThunkOverwriteVictim
           ThunkRemoveVictim ThunkClearVictim ThunkReplaceVictim SnapshotLive
           Edge Marked Freed Hsubgoal_stale Hsubgoal_overwrite Hsubgoal_remove
           Hsubgoal_clear Hthunk_stale Hthunk_overwrite Hthunk_remove Hthunk_clear
           Hthunk_replace Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as
        [Hsubgoal_stale_a |
         [Hsubgoal_overwrite_a |
          [Hsubgoal_remove_a |
           [Hsubgoal_clear_a |
            [Hthunk_stale_a |
             [Hthunk_overwrite_a |
              [Hthunk_remove_a |
               [Hthunk_clear_a | Hthunk_replace_a]]]]]]]].
    - apply Hsubgoal_stale. exact Hsubgoal_stale_a.
    - apply Hsubgoal_overwrite. exact Hsubgoal_overwrite_a.
    - apply Hsubgoal_remove. exact Hsubgoal_remove_a.
    - apply Hsubgoal_clear. exact Hsubgoal_clear_a.
    - apply Hthunk_stale. exact Hthunk_stale_a.
    - apply Hthunk_overwrite. exact Hthunk_overwrite_a.
    - apply Hthunk_remove. exact Hthunk_remove_a.
    - apply Hthunk_clear. exact Hthunk_clear_a.
    - apply Hthunk_replace. exact Hthunk_replace_a.
  Qed.
End CESKCollectorSafetyModel.

End MeTTaTron_GC_CESKCollectorSafety.
