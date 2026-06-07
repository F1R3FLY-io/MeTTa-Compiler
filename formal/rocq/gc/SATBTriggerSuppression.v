(** E2 SATB trigger-suppression obligations.

    FANOUT workers may request a dedicated concurrent collection when the heap
    crosses a watermark, but not while an E2 SATB mark is already in progress.
    Otherwise a second request can overlap the snapshot being protected by the
    active SATB deletion barriers and final-sweep protocol.
*)

Module MeTTaTron_GC_SATBTriggerSuppression.

Section SATBTriggerSuppressionModel.
  Definition FanoutWatermarkTrigger
      (DedicatedCollector OtherMutatorLive NoRequestPending
       SatbMarking WatermarkDue : Prop) : Prop :=
    DedicatedCollector /\
    OtherMutatorLive /\
    NoRequestPending /\
    ~ SatbMarking /\
    WatermarkDue.

  Definition UngatedWatermarkTrigger
      (DedicatedCollector OtherMutatorLive NoRequestPending
       WatermarkDue : Prop) : Prop :=
    DedicatedCollector /\
    OtherMutatorLive /\
    NoRequestPending /\
    WatermarkDue.

  Theorem active_satb_suppresses_fanout_trigger :
    forall DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue,
      SatbMarking ->
      ~ FanoutWatermarkTrigger
          DedicatedCollector OtherMutatorLive NoRequestPending
          SatbMarking WatermarkDue.
  Proof.
    intros DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue Hsatb Htrigger.
    destruct Htrigger as [_ [_ [_ [Hnot_satb _]]]].
    apply Hnot_satb.
    exact Hsatb.
  Qed.

  Theorem fanout_trigger_implies_satb_idle :
    forall DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue,
      FanoutWatermarkTrigger
        DedicatedCollector OtherMutatorLive NoRequestPending
        SatbMarking WatermarkDue ->
      ~ SatbMarking.
  Proof.
    intros DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue Htrigger.
    destruct Htrigger as [_ [_ [_ [Hnot_satb _]]]].
    exact Hnot_satb.
  Qed.

  Theorem active_satb_blocks_even_when_watermark_due :
    forall DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue,
      DedicatedCollector ->
      OtherMutatorLive ->
      NoRequestPending ->
      SatbMarking ->
      WatermarkDue ->
      ~ FanoutWatermarkTrigger
          DedicatedCollector OtherMutatorLive NoRequestPending
          SatbMarking WatermarkDue.
  Proof.
    intros DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue _ _ _ Hsatb _.
    apply active_satb_suppresses_fanout_trigger.
    exact Hsatb.
  Qed.

  Theorem ungated_trigger_can_overlap_active_satb :
    forall DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue,
      DedicatedCollector ->
      OtherMutatorLive ->
      NoRequestPending ->
      SatbMarking ->
      WatermarkDue ->
      UngatedWatermarkTrigger
        DedicatedCollector OtherMutatorLive NoRequestPending WatermarkDue /\
      SatbMarking.
  Proof.
    intros DedicatedCollector OtherMutatorLive NoRequestPending
           SatbMarking WatermarkDue Hdedicated Hother Hrequest Hsatb Hwatermark.
    split.
    - repeat split; assumption.
    - exact Hsatb.
  Qed.
End SATBTriggerSuppressionModel.

End MeTTaTron_GC_SATBTriggerSuppression.
