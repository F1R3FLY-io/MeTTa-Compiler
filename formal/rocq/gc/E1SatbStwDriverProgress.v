(** E1 default driver progress through the SATB/STW rendezvous split.

    [E1DefaultConcurrentFlip] uses the premise "a posted driver request closes
    the rendezvous cycle".  After E2, the production default driver no longer has
    a single straight-line STW body: it enters an initial SATB rendezvous, closes
    it, runs concurrent marking, opens a final rendezvous, and either completes
    the final sweep or aborts to a freshly requested STW rendezvous.

    This file proves the abstract composition obligation for that shipped shape.
    Source coupling pins the propositions below to [gc_driver.rs]:

      - [SatbSuccess] is the path ending in [cleanup.close_cycle].
      - [FinalSweepClosed] is the checked false return from the SATB final sweep.
      - [SatbPanic] is the outer [catch_unwind] error.
      - every abort posts and runs [gc_driver_stw_rendezvous_cycle].
      - every closed open cycle advances the generation, clears the request,
        clears witness-ok, drops GC-in-progress, and resumes workers.
*)

Module MeTTaTron_GC_E1SatbStwDriverProgress.

Section DriverProgressModel.
  Variable PostedDriver : Prop.
  Variable SatbSuccess SatbPanic FinalSweepClosed SatbAbort : Prop.
  Variable StwRequested StwRan : Prop.
  Variable InitialCycleClosed FinalCycleClosed StwCycleClosed : Prop.
  Variable GenerationAdvanced RequestCleared WorkersResumed : Prop.
  Variable WitnessCleared GcInProgressReleased : Prop.

  Definition CycleRelease (Closed : Prop) : Prop :=
    Closed ->
      GenerationAdvanced /\
      RequestCleared /\
      WorkersResumed /\
      WitnessCleared /\
      GcInProgressReleased.

  Definition DriverReleased : Prop :=
    GenerationAdvanced /\
    RequestCleared /\
    WorkersResumed /\
    WitnessCleared /\
    GcInProgressReleased.

  Definition SatbAbortDetected : Prop :=
    SatbPanic \/ FinalSweepClosed.

  Definition AbortPostsFreshStw : Prop :=
    SatbAbort -> StwRequested.

  Definition StwBackstopCloses : Prop :=
    StwRequested -> StwRan /\ StwCycleClosed.

  Theorem satb_success_final_close_releases_driver :
    SatbSuccess ->
    InitialCycleClosed ->
    FinalCycleClosed ->
    CycleRelease FinalCycleClosed ->
    DriverReleased.
  Proof.
    intros _ _ Hfinal_closed Hrelease.
    apply Hrelease.
    exact Hfinal_closed.
  Qed.

  Theorem panic_or_closed_final_sweep_is_satb_abort :
    (SatbPanic -> SatbAbort) ->
    (FinalSweepClosed -> SatbAbort) ->
    SatbAbortDetected ->
    SatbAbort.
  Proof.
    intros Hpanic Hclosed Habortish.
    destruct Habortish as [Hpanic_case | Hclosed_case].
    - apply Hpanic. exact Hpanic_case.
    - apply Hclosed. exact Hclosed_case.
  Qed.

  Theorem satb_abort_fresh_stw_releases_driver :
    AbortPostsFreshStw ->
    StwBackstopCloses ->
    CycleRelease StwCycleClosed ->
    SatbAbort ->
    StwRequested /\ StwRan /\ StwCycleClosed /\ DriverReleased.
  Proof.
    intros Hpost Hbackstop Hrelease Habort.
    pose proof (Hpost Habort) as Hrequested.
    destruct (Hbackstop Hrequested) as [Hran Hclosed].
    pose proof (Hrelease Hclosed) as Hdriver.
    split.
    - exact Hrequested.
    - split.
      + exact Hran.
      + split.
        * exact Hclosed.
        * exact Hdriver.
  Qed.

  Theorem posted_driver_satb_or_stw_releases :
    (PostedDriver -> SatbSuccess \/ SatbAbort) ->
    (SatbSuccess -> InitialCycleClosed /\ FinalCycleClosed) ->
    CycleRelease FinalCycleClosed ->
    AbortPostsFreshStw ->
    StwBackstopCloses ->
    CycleRelease StwCycleClosed ->
    PostedDriver ->
    DriverReleased.
  Proof.
    intros Hcase Hsatb_closes Hsatb_release Hpost Hbackstop Hstw_release Hposted.
    destruct (Hcase Hposted) as [Hsuccess | Habort].
    - destruct (Hsatb_closes Hsuccess) as [Hinitial Hfinal].
      apply (satb_success_final_close_releases_driver
               Hsuccess Hinitial Hfinal Hsatb_release).
    - destruct (satb_abort_fresh_stw_releases_driver
                  Hpost Hbackstop Hstw_release Habort)
        as [_ [_ [_ Hreleased]]].
      exact Hreleased.
  Qed.

  Theorem posted_driver_no_sticky_request_or_witness :
    (PostedDriver -> SatbSuccess \/ SatbAbort) ->
    (SatbSuccess -> InitialCycleClosed /\ FinalCycleClosed) ->
    CycleRelease FinalCycleClosed ->
    AbortPostsFreshStw ->
    StwBackstopCloses ->
    CycleRelease StwCycleClosed ->
    PostedDriver ->
    RequestCleared /\ WorkersResumed /\ WitnessCleared /\ GcInProgressReleased.
  Proof.
    intros Hcase Hsatb_closes Hsatb_release Hpost Hbackstop Hstw_release Hposted.
    pose proof (posted_driver_satb_or_stw_releases
                  Hcase Hsatb_closes Hsatb_release
                  Hpost Hbackstop Hstw_release Hposted)
      as [_ [Hcleared [Hresumed [Hwitness Hgip]]]].
    repeat split; assumption.
  Qed.

End DriverProgressModel.

End MeTTaTron_GC_E1SatbStwDriverProgress.
