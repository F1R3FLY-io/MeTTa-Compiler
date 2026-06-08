(** Deep-branching FANOUT collection progress for the CESK/index collector.

    The old collector could wait for global evaluator quiescence under highly
    branching workloads.  The CESK/index collector must instead make collection
    progress through structural participant contribution, witness publication,
    SATB/rendezvous collection, and generational backpressure triggers.  This
    file composes those source-side premises; the companion TLA+
    [RendezvousQuiescenceIndependence] model checks the temporal negative case
    where the old [active == 0] gate is reintroduced.
*)

Module MeTTaTron_GC_DeepBranchingCollectionProgress.

Section DeepBranchingCollectionProgressModel.
  Variable Addr : Type.

  Definition MinorDue (YoungOverBudget NurseryPending : Prop) : Prop :=
    YoungOverBudget \/ NurseryPending.

  Definition MajorDue (LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    LiveMajor \/ CapMajor \/ CadenceMajor.

  Definition RendezvousReady
      (ParticipantRootsPublished WitnessOk : Prop) : Prop :=
    ParticipantRootsPublished /\ WitnessOk.

  Definition CollectionEnabled
      (RendezvousSweep MinorSweep MajorSweep : Prop) : Prop :=
    RendezvousSweep \/ MinorSweep \/ MajorSweep.

  Definition ProgressWithoutGlobalQuiescence
      (CollectionEnabled GlobalQuiescent : Prop) : Prop :=
    CollectionEnabled /\ ~ GlobalQuiescent.

  Definition DeepBranchingPressure
      (RendezvousReady YoungOverBudget NurseryPending CapMajor CadenceMajor
       : Prop) : Prop :=
    RendezvousReady \/ YoungOverBudget \/ NurseryPending \/ CapMajor \/
      CadenceMajor.

  Theorem rendezvous_ready_enables_nonquiescent_sweep :
    forall ParticipantRootsPublished WitnessOk RendezvousSweep,
      (RendezvousReady ParticipantRootsPublished WitnessOk -> RendezvousSweep) ->
      ParticipantRootsPublished ->
      WitnessOk ->
      RendezvousSweep.
  Proof.
    intros ParticipantRootsPublished WitnessOk RendezvousSweep Hready_sweeps
           Hpublished Hwitness.
    apply Hready_sweeps.
    unfold RendezvousReady.
    split.
    - exact Hpublished.
    - exact Hwitness.
  Qed.

  Theorem old_global_quiescence_gate_blocks_ready_fanout :
    forall RendezvousSweep GlobalQuiescent : Prop,
      (RendezvousSweep -> GlobalQuiescent) ->
      ~ GlobalQuiescent ->
      ~ RendezvousSweep.
  Proof.
    intros RendezvousSweep GlobalQuiescent Hrequires_quiescence
           Hnot_quiescent Hsweep.
    apply Hnot_quiescent.
    apply Hrequires_quiescence.
    exact Hsweep.
  Qed.

  Theorem deep_branching_pressure_requests_collection :
    forall (RendezvousSweep MinorSweep MajorSweep ParticipantRootsPublished
              WitnessOk YoungOverBudget NurseryPending LiveMajor CapMajor
              CadenceMajor : Prop),
      (RendezvousReady ParticipantRootsPublished WitnessOk -> RendezvousSweep) ->
      (MinorDue YoungOverBudget NurseryPending -> MinorSweep) ->
      (MajorDue LiveMajor CapMajor CadenceMajor -> MajorSweep) ->
      DeepBranchingPressure
        (RendezvousReady ParticipantRootsPublished WitnessOk)
        YoungOverBudget NurseryPending CapMajor CadenceMajor ->
      CollectionEnabled RendezvousSweep MinorSweep MajorSweep.
  Proof.
    intros RendezvousSweep MinorSweep MajorSweep ParticipantRootsPublished
           WitnessOk YoungOverBudget NurseryPending LiveMajor CapMajor
           CadenceMajor Hready_sweep Hminor Hmajor Hpressure.
    unfold DeepBranchingPressure in Hpressure.
    destruct Hpressure as
        [Hready | [Hyoung | [Hnursery | [Hcap | Hcadence]]]].
    - left.
      apply Hready_sweep.
      exact Hready.
    - right. left.
      apply Hminor.
      unfold MinorDue.
      left.
      exact Hyoung.
    - right. left.
      apply Hminor.
      unfold MinorDue.
      right.
      exact Hnursery.
    - right. right.
      apply Hmajor.
      unfold MajorDue.
      right. left.
      exact Hcap.
    - right. right.
      apply Hmajor.
      unfold MajorDue.
      right. right.
      exact Hcadence.
  Qed.

  Theorem deep_branching_collection_progress_without_global_quiescence :
    forall (RendezvousSweep MinorSweep MajorSweep ParticipantRootsPublished
              WitnessOk YoungOverBudget NurseryPending LiveMajor CapMajor
              CadenceMajor FanoutActive ActiveWorkersLive GlobalQuiescent
              : Prop)
           (FutureTouch RootCovered Marked Freed : Addr -> Prop),
      FanoutActive ->
      (FanoutActive -> ActiveWorkersLive) ->
      (ActiveWorkersLive -> ~ GlobalQuiescent) ->
      (RendezvousReady ParticipantRootsPublished WitnessOk -> RendezvousSweep) ->
      (MinorDue YoungOverBudget NurseryPending -> MinorSweep) ->
      (MajorDue LiveMajor CapMajor CadenceMajor -> MajorSweep) ->
      DeepBranchingPressure
        (RendezvousReady ParticipantRootsPublished WitnessOk)
        YoungOverBudget NurseryPending CapMajor CadenceMajor ->
      (forall a, FutureTouch a -> RootCovered a) ->
      (forall a, RootCovered a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      ProgressWithoutGlobalQuiescence
        (CollectionEnabled RendezvousSweep MinorSweep MajorSweep)
        GlobalQuiescent /\
      (forall a, FutureTouch a -> ~ Freed a).
  Proof.
    intros RendezvousSweep MinorSweep MajorSweep ParticipantRootsPublished
           WitnessOk YoungOverBudget NurseryPending LiveMajor CapMajor
           CadenceMajor FanoutActive ActiveWorkersLive GlobalQuiescent
           FutureTouch RootCovered Marked Freed Hfanout Hactive
           Hnot_quiescent Hready_sweep Hminor Hmajor Hpressure Hcovered Hmarked
           Hsweep.
    split.
    - unfold ProgressWithoutGlobalQuiescence.
      split.
      + apply (deep_branching_pressure_requests_collection
                 RendezvousSweep MinorSweep MajorSweep ParticipantRootsPublished
                 WitnessOk YoungOverBudget NurseryPending LiveMajor CapMajor
                 CadenceMajor).
        * exact Hready_sweep.
        * exact Hminor.
        * exact Hmajor.
        * exact Hpressure.
      + apply Hnot_quiescent.
        apply Hactive.
        exact Hfanout.
    - intros a Htouch Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmarked.
      apply Hcovered.
      exact Htouch.
  Qed.
End DeepBranchingCollectionProgressModel.

End MeTTaTron_GC_DeepBranchingCollectionProgress.
