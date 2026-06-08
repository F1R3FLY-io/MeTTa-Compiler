(** E2 SATB trigger-suppression obligations.

    FANOUT workers may request a dedicated concurrent collection when the heap
    crosses a watermark, but not while an E2 SATB mark is already in progress.
    Otherwise a second request can overlap the snapshot being protected by the
    active SATB deletion barriers and final-sweep protocol.
*)

Module MeTTaTron_GC_SATBTriggerSuppression.

Section SATBTriggerSuppressionModel.
  Definition FanoutWatermarkTrigger
      (DedicatedCollector FanoutParticipantLive NoRequestPending
       SatbMarking WatermarkDue : Prop) : Prop :=
    DedicatedCollector /\
    FanoutParticipantLive /\
    NoRequestPending /\
    ~ SatbMarking /\
    WatermarkDue.

  Definition UngatedWatermarkTrigger
      (DedicatedCollector FanoutParticipantLive NoRequestPending
       WatermarkDue : Prop) : Prop :=
    DedicatedCollector /\
    FanoutParticipantLive /\
    NoRequestPending /\
    WatermarkDue.

  Theorem active_satb_suppresses_fanout_trigger :
    forall DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue,
      SatbMarking ->
      ~ FanoutWatermarkTrigger
          DedicatedCollector FanoutParticipantLive NoRequestPending
          SatbMarking WatermarkDue.
  Proof.
    intros DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue Hsatb Htrigger.
    destruct Htrigger as [_ [_ [_ [Hnot_satb _]]]].
    apply Hnot_satb.
    exact Hsatb.
  Qed.

  Theorem fanout_trigger_implies_satb_idle :
    forall DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue,
      FanoutWatermarkTrigger
        DedicatedCollector FanoutParticipantLive NoRequestPending
        SatbMarking WatermarkDue ->
      ~ SatbMarking.
  Proof.
    intros DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue Htrigger.
    destruct Htrigger as [_ [_ [_ [Hnot_satb _]]]].
    exact Hnot_satb.
  Qed.

  Theorem active_satb_blocks_even_when_watermark_due :
    forall DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue,
      DedicatedCollector ->
      FanoutParticipantLive ->
      NoRequestPending ->
      SatbMarking ->
      WatermarkDue ->
      ~ FanoutWatermarkTrigger
          DedicatedCollector FanoutParticipantLive NoRequestPending
          SatbMarking WatermarkDue.
  Proof.
    intros DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue _ _ _ Hsatb _.
    apply active_satb_suppresses_fanout_trigger.
    exact Hsatb.
  Qed.

  Theorem ungated_trigger_can_overlap_active_satb :
    forall DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue,
      DedicatedCollector ->
      FanoutParticipantLive ->
      NoRequestPending ->
      SatbMarking ->
      WatermarkDue ->
      UngatedWatermarkTrigger
        DedicatedCollector FanoutParticipantLive NoRequestPending WatermarkDue /\
      SatbMarking.
  Proof.
    intros DedicatedCollector FanoutParticipantLive NoRequestPending
           SatbMarking WatermarkDue Hdedicated Hother Hrequest Hsatb Hwatermark.
    split.
    - repeat split; assumption.
    - exact Hsatb.
  Qed.
End SATBTriggerSuppressionModel.

End MeTTaTron_GC_SATBTriggerSuppression.
