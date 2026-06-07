(** C1.c major/minor scheduler obligations.

    A level-3 young-nursery pressure signal may let the scheduler choose a
    cheap minor instead of a coincident live-growth major for one cycle.  The
    deferral is deliberately narrow: cap-forced and cadence-forced majors must
    still run, and any deferred major is therefore a live-growth major under an
    acute young-minor trigger.
*)

Module MeTTaTron_GC_MajorMinorScheduler.

Section MajorMinorSchedulerModel.
  Definition MajorDue (LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    LiveMajor \/ CapMajor \/ CadenceMajor.

  Definition MinorDue (YoungOverBudget NurseryPending : Prop) : Prop :=
    YoungOverBudget \/ NurseryPending.

  Definition DoMajor
      (Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    MajorDue LiveMajor CapMajor CadenceMajor /\
    ~ (Level3 /\ MinorDueNow /\ ~ CapMajor /\ ~ CadenceMajor).

  Definition DeferredMajor
      (Level3 MinorDueNow LiveMajor CapMajor CadenceMajor : Prop) : Prop :=
    MajorDue LiveMajor CapMajor CadenceMajor /\
    Level3 /\ MinorDueNow /\ ~ CapMajor /\ ~ CadenceMajor.

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

  Theorem level3_young_pressure_requests_minor :
    forall Level3 YoungOverBudget NurseryPending : Prop,
      Level3 ->
      (Level3 -> YoungOverBudget) ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros Level3 YoungOverBudget NurseryPending Hlevel3 Hlevel3_over_budget.
    left.
    apply Hlevel3_over_budget.
    exact Hlevel3.
  Qed.
End MajorMinorSchedulerModel.

End MeTTaTron_GC_MajorMinorScheduler.
