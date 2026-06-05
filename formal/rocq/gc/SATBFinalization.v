(** E2 SATB finalization obligations.

    The concurrent SATB path has three finalization duties after background
    marking: re-mark final-rendezvous roots before the exclusive sweep, treat a
    closed final sweep gate as an abort, and run a freshly requested STW
    rendezvous after any abort.  The TLA+ models discriminate the interleavings;
    these Rocq theorems discharge the abstract safety/control obligations that
    the source-coupling harness pins to the Rust driver.
*)

Module MeTTaTron_GC_SATBFinalization.

Section SATBFinalizationModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ FinalDriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition RequestHandled (SatbSwept StwFallbackRan : Prop) : Prop :=
    SatbSwept \/ StwFallbackRan.

  Theorem final_remark_root_is_satb_root :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            FinalRoot : Addr -> Prop),
      (forall a, FinalRoot a -> FinalDriverRoot a) ->
      forall a,
        FinalRoot a ->
        SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack FinalRoot
           Hremark a Hfinal.
    right; left.
    apply Hremark.
    exact Hfinal.
  Qed.

  Theorem final_remark_root_survives_collection :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            FinalRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, FinalRoot a -> FinalDriverRoot a) ->
      (forall a,
          Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, FinalRoot a -> ~ Freed a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack FinalRoot
           Edge Marked Freed Hremark Hmark Hsweep a Hfinal Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (final_remark_root_is_satb_root
             InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
             FinalRoot Hremark a).
    exact Hfinal.
  Qed.

  Theorem closed_final_sweep_uses_stw_backstop :
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

  Theorem aborted_satb_runs_requested_stw :
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

  Theorem completed_satb_request_has_collection :
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
End SATBFinalizationModel.

End MeTTaTron_GC_SATBFinalization.
