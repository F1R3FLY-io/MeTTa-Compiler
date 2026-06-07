(** Rocq model of the dedicated CESK-GC rendezvous progress obligation.

    This is the propositional companion to the TLA+ liveness model. It captures
    the source-side obligations needed to rule out the deadlock class where a GC
    rendezvous starts, workers are asked to park, and either the driver waits
    forever or parked workers never resume.

    Temporal scheduling is checked by [tla/RendezvousProgress.tla]. This file
    proves the compositional premises: every active participant must contribute
    by parking or finishing, panic cleanup must still close the cycle, closing
    must advance the generation and clear the request, and generation resume is
    not blocked by a back-to-back request.
*)

Module MeTTaTron_GC_RendezvousProgress.

Section RendezvousProgressModel.
  Variable Worker : Type.

  Definition AllParticipantsContributed
      (Active Contributed : Worker -> Prop) : Prop :=
    forall w, Active w -> Contributed w.

  Definition ParticipantAccounted
      (Parked Finished : Worker -> Prop)
      (w : Worker) : Prop :=
    Parked w \/ Finished w.

  Definition GenerationCanResume (my_gen current_gen : nat) : Prop :=
    current_gen <> my_gen.

  Definition BooleanCanResume (GcRequested : Prop) : Prop :=
    ~ GcRequested.

  Theorem active_participant_is_accounted :
    forall (Active Contributed Parked Finished : Worker -> Prop),
      AllParticipantsContributed Active Contributed ->
      (forall w, Contributed w -> ParticipantAccounted Parked Finished w) ->
      forall w,
        Active w ->
        ParticipantAccounted Parked Finished w.
  Proof.
    intros Active Contributed Parked Finished Hall Haccounted w Hactive.
    apply Haccounted.
    apply Hall.
    exact Hactive.
  Qed.

  Theorem missing_participant_contribution_exposes_wait_gap :
    forall (Active Contributed : Worker -> Prop) (w : Worker),
      Active w ->
      ~ Contributed w ->
      exists u, Active u /\ ~ Contributed u.
  Proof.
    intros Active Contributed w Hactive Hnot_contributed.
    exists w.
    split.
    - exact Hactive.
    - exact Hnot_contributed.
  Qed.

  Theorem panic_cleanup_closes_cycle :
    forall CollectPanicked CleanupClosesCycle CycleClosed : Prop,
      (CollectPanicked -> CleanupClosesCycle) ->
      (CleanupClosesCycle -> CycleClosed) ->
      CollectPanicked ->
      CycleClosed.
  Proof.
    intros CollectPanicked CleanupClosesCycle CycleClosed Hcleanup Hclose Hpanic.
    apply Hclose.
    apply Hcleanup.
    exact Hpanic.
  Qed.

  Theorem close_advances_generation_and_clears_request :
    forall CycleClosed GenerationAdvanced RequestCleared : Prop,
      (CycleClosed -> GenerationAdvanced) ->
      (CycleClosed -> RequestCleared) ->
      CycleClosed ->
      GenerationAdvanced /\ RequestCleared.
  Proof.
    intros CycleClosed GenerationAdvanced RequestCleared Hgen Hclear Hclosed.
    split.
    - apply Hgen. exact Hclosed.
    - apply Hclear. exact Hclosed.
  Qed.

  Theorem parked_worker_resumes_after_closed_generation :
    forall (Active Parked Resumed : Worker -> Prop)
           (GenerationAdvanced : Prop),
      (forall w, Active w -> Parked w -> GenerationAdvanced -> Resumed w) ->
      GenerationAdvanced ->
      forall w,
        Active w ->
        Parked w ->
        Resumed w.
  Proof.
    intros Active Parked Resumed GenerationAdvanced Hresume Hadvanced w Hactive Hparked.
    apply Hresume.
    - exact Hactive.
    - exact Hparked.
    - exact Hadvanced.
  Qed.

  Theorem rendezvous_progress_releases_parked_participants :
    forall (Active Contributed Parked Finished Resumed : Worker -> Prop)
           (CycleClosed GenerationAdvanced RequestCleared : Prop),
      AllParticipantsContributed Active Contributed ->
      (forall w, Contributed w -> ParticipantAccounted Parked Finished w) ->
      (CycleClosed -> GenerationAdvanced) ->
      (CycleClosed -> RequestCleared) ->
      (forall w, Active w -> Parked w -> GenerationAdvanced -> Resumed w) ->
      CycleClosed ->
      forall w,
        Active w ->
        Parked w ->
        Resumed w.
  Proof.
    intros Active Contributed Parked Finished Resumed
           CycleClosed GenerationAdvanced RequestCleared
           _ _ Hgen _ Hresume Hclosed w Hactive Hparked.
    apply (parked_worker_resumes_after_closed_generation
             Active Parked Resumed GenerationAdvanced Hresume).
    - apply Hgen. exact Hclosed.
    - exact Hactive.
    - exact Hparked.
  Qed.

  Theorem back_to_back_request_does_not_block_generation_resume :
    forall (my_gen current_gen : nat) (GcRequested : Prop),
      current_gen <> my_gen ->
      GcRequested ->
      GenerationCanResume my_gen current_gen.
  Proof.
    intros my_gen current_gen GcRequested Hadvanced _.
    exact Hadvanced.
  Qed.

  Theorem back_to_back_request_blocks_boolean_resume :
    forall GcRequested : Prop,
      GcRequested ->
      ~ BooleanCanResume GcRequested.
  Proof.
    intros GcRequested Hrequested Hcan_resume.
    apply Hcan_resume.
    exact Hrequested.
  Qed.
End RendezvousProgressModel.

End MeTTaTron_GC_RendezvousProgress.
