(** Rocq companion for [tla/SATBAbortFallback.tla].

    The concurrent SATB rendezvous is allowed to abort, but it is not allowed to
    silently consume a GC request.  Once SATB aborts, the driver must post a
    fresh stop-the-world rendezvous request and that requested fallback must run
    before the original request is considered handled.
*)

Module MeTTaTron_GC_SATBAbortFallback.

Section SATBAbortFallbackModel.
  Definition RequestHandled
      (SatbSwept StwFallbackRan : Prop) : Prop :=
    SatbSwept \/ StwFallbackRan.

  Theorem abort_has_stw_backstop :
    forall SatbAbort StwFallbackRan : Prop,
      (SatbAbort -> StwFallbackRan) ->
      SatbAbort ->
      StwFallbackRan.
  Proof.
    intros SatbAbort StwFallbackRan Hbackstop Habort.
    apply Hbackstop.
    exact Habort.
  Qed.

  Theorem fallback_stw_was_requested :
    forall StwRequested StwFallbackRan : Prop,
      (StwFallbackRan -> StwRequested) ->
      StwFallbackRan ->
      StwRequested.
  Proof.
    intros StwRequested StwFallbackRan Hrequested Hran.
    apply Hrequested.
    exact Hran.
  Qed.

  Theorem aborted_satb_runs_requested_stw :
    forall SatbAbort StwRequested StwFallbackRan : Prop,
      (SatbAbort -> StwRequested) ->
      (StwRequested -> StwFallbackRan) ->
      SatbAbort ->
      StwRequested /\ StwFallbackRan.
  Proof.
    intros SatbAbort StwRequested StwFallbackRan Hrequest Hrun Habort.
    split.
    - apply Hrequest.
      exact Habort.
    - apply Hrun.
      apply Hrequest.
      exact Habort.
  Qed.

  Theorem completed_request_after_abort_has_collection :
    forall SatbSwept SatbAbort StwRequested StwFallbackRan RequestDone : Prop,
      (RequestDone -> SatbSwept \/ SatbAbort) ->
      (SatbAbort -> StwRequested) ->
      (StwRequested -> StwFallbackRan) ->
      RequestDone ->
      RequestHandled SatbSwept StwFallbackRan.
  Proof.
    intros SatbSwept SatbAbort StwRequested StwFallbackRan RequestDone
           Hdone_shape Hrequest Hrun Hdone.
    destruct (Hdone_shape Hdone) as [Hswept | Habort].
    - left.
      exact Hswept.
    - right.
      apply Hrun.
      apply Hrequest.
      exact Habort.
  Qed.

  Theorem missing_fallback_exposes_abort_gap :
    forall SatbAbort StwFallbackRan : Prop,
      SatbAbort ->
      ~ StwFallbackRan ->
      exists AbortGap : Prop,
        AbortGap /\ ~ StwFallbackRan.
  Proof.
    intros SatbAbort StwFallbackRan Habort Hno_fallback.
    exists SatbAbort.
    split.
    - exact Habort.
    - exact Hno_fallback.
  Qed.

  Theorem unrequested_fallback_exposes_driver_gap :
    forall StwRequested StwFallbackRan : Prop,
      StwFallbackRan ->
      ~ StwRequested ->
      exists DriverGap : Prop,
        DriverGap /\ ~ StwRequested.
  Proof.
    intros StwRequested StwFallbackRan Hran Hnot_requested.
    exists StwFallbackRan.
    split.
    - exact Hran.
    - exact Hnot_requested.
  Qed.
End SATBAbortFallbackModel.

End MeTTaTron_GC_SATBAbortFallback.
