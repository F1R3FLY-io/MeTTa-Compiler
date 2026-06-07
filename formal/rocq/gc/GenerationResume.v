(** Rocq model of the E1/E5 generation-gated worker resume obligation.

    A worker parked for rendezvous generation [my_gen] may resume exactly when
    [GC_CYCLE_GEN] has advanced.  This makes resume independent of
    [GC_REQUESTED], so a back-to-back request cannot re-block a worker whose
    own cycle ended.
*)

Module MeTTaTron_GC_GenerationResume.

Section GenerationResumeModel.
  Definition CanResume (my_gen current_gen : nat) : Prop :=
    current_gen <> my_gen.

  Definition BooleanCanResume (gc_requested : Prop) : Prop :=
    ~ gc_requested.

  Theorem generation_end_bump_releases_worker :
    forall my_gen current_gen : nat,
      current_gen <> my_gen ->
      CanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen Hadvanced.
    exact Hadvanced.
  Qed.

  Theorem back_to_back_request_does_not_block_generation_resume :
    forall (my_gen current_gen : nat) (GcRequested : Prop),
      current_gen <> my_gen ->
      GcRequested ->
      CanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen GcRequested Hadvanced _.
    exact Hadvanced.
  Qed.

  Theorem same_generation_keeps_worker_parked :
    forall my_gen current_gen : nat,
      current_gen = my_gen ->
      ~ CanResume my_gen current_gen.
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
End GenerationResumeModel.

End MeTTaTron_GC_GenerationResume.
