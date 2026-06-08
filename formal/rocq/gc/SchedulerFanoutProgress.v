(** Composed scheduler/FANOUT progress contract for the CESK GC.

    The component proofs cover individual obligations: admission closes before
    the snapshot, completion guards drop on normal and panic exits, failed
    concurrent triggers run the resume backstop, SATB aborts fall back to STW,
    and generation-based resume is independent of a back-to-back request.  This
    file composes those source-side facts into the GC-facing progress contract:
    every active participant is accounted, every parked active worker is
    resumed by either the trigger backstop or a closed generation, and the
    parent wait is not stranded by a missing completion drop.

    Temporal fairness and eventuality are checked by the companion TLA+ models
    in [tla/RendezvousProgress.tla], [tla/ConcurrentTriggerBackstop.tla],
    [tla/SATBAbortFallback.tla], and [tla/CollapseCompletion.tla].
*)

Module MeTTaTron_GC_SchedulerFanoutProgress.

Section SchedulerFanoutProgressModel.
  Variable Worker : Type.

  Inductive ExitPath : Type :=
  | NormalExit : ExitPath
  | PanicExit : ExitPath.

  Definition AllParticipantsContributed
      (Active Contributed : Worker -> Prop) : Prop :=
    forall w, Active w -> Contributed w.

  Definition ParticipantAccounted
      (Parked Finished : Worker -> Prop)
      (w : Worker) : Prop :=
    Parked w \/ Finished w.

  Definition WorkerExited
      (Exit : Worker -> ExitPath -> Prop)
      (w : Worker) : Prop :=
    Exit w NormalExit \/ Exit w PanicExit.

  Definition ParentWaitStranded
      (Spawned Dropped : Worker -> Prop) : Prop :=
    exists w, Spawned w /\ ~ Dropped w.

  Definition TriggerFailureBackstopped
      (TriggerFailed RequestCleared WorkersResumed : Prop) : Prop :=
    TriggerFailed -> RequestCleared /\ WorkersResumed.

  Definition SuccessfulTriggerHasDriver
      (TriggerSent DriverPosted : Prop) : Prop :=
    TriggerSent -> DriverPosted.

  Definition DriverCompletesOrAborts
      (DriverPosted CycleClosed SatbAbort : Prop) : Prop :=
    DriverPosted -> CycleClosed \/ SatbAbort.

  Theorem active_participants_accounted_by_contribution :
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

  Theorem completion_guard_rules_out_parent_strand :
    forall (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> Dropped w) ->
      (forall w, Exit w PanicExit -> Dropped w) ->
      ~ ParentWaitStranded Spawned Dropped.
  Proof.
    intros Spawned Dropped Exit Hexits Hnormal Hpanic Hstranded.
    destruct Hstranded as [w [Hspawned Hnot_dropped]].
    apply Hnot_dropped.
    destruct (Hexits w Hspawned) as [Hnormal_exit | Hpanic_exit].
    - apply Hnormal.
      exact Hnormal_exit.
    - apply Hpanic.
      exact Hpanic_exit.
  Qed.

  Theorem posted_driver_or_abort_fallback_closes_cycle :
    forall TriggerSent DriverPosted CycleClosed SatbAbort StwFallbackRan,
      SuccessfulTriggerHasDriver TriggerSent DriverPosted ->
      DriverCompletesOrAborts DriverPosted CycleClosed SatbAbort ->
      (SatbAbort -> StwFallbackRan) ->
      (StwFallbackRan -> CycleClosed) ->
      TriggerSent ->
      CycleClosed.
  Proof.
    intros TriggerSent DriverPosted CycleClosed SatbAbort StwFallbackRan
           Hsent_driver Hdriver_progress Habort_fallback Hfallback_closes Hsent.
    destruct (Hdriver_progress (Hsent_driver Hsent)) as [Hclosed | Habort].
    - exact Hclosed.
    - apply Hfallback_closes.
      apply Habort_fallback.
      exact Habort.
  Qed.

  Theorem scheduler_fanout_gc_progress_contract :
    forall (Active Contributed Parked Finished Resumed Spawned Dropped
              : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop)
           (TriggerSent TriggerFailed DriverPosted RequestCleared
              BackstopResumed CycleClosed GenerationAdvanced SatbAbort
              StwFallbackRan : Prop),
      AllParticipantsContributed Active Contributed ->
      (forall w, Contributed w -> ParticipantAccounted Parked Finished w) ->
      SuccessfulTriggerHasDriver TriggerSent DriverPosted ->
      TriggerFailureBackstopped TriggerFailed RequestCleared BackstopResumed ->
      (BackstopResumed -> forall w, Active w -> Parked w -> Resumed w) ->
      DriverCompletesOrAborts DriverPosted CycleClosed SatbAbort ->
      (SatbAbort -> StwFallbackRan) ->
      (StwFallbackRan -> CycleClosed) ->
      (CycleClosed -> GenerationAdvanced) ->
      (forall w, Active w -> Parked w -> GenerationAdvanced -> Resumed w) ->
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> Dropped w) ->
      (forall w, Exit w PanicExit -> Dropped w) ->
      (TriggerSent \/ TriggerFailed) ->
      (forall w, Active w -> ParticipantAccounted Parked Finished w) /\
      (forall w, Active w -> Parked w -> Resumed w) /\
      ~ ParentWaitStranded Spawned Dropped.
  Proof.
    intros Active Contributed Parked Finished Resumed Spawned Dropped Exit
           TriggerSent TriggerFailed DriverPosted RequestCleared BackstopResumed
           CycleClosed GenerationAdvanced SatbAbort StwFallbackRan
           Hall Haccounted Hsent_driver Hfailed_backstop Hbackstop_resumes
           Hdriver_progress Habort_fallback Hfallback_closes Hclosed_gen
           Hgeneration_resumes Hexits Hnormal Hpanic Htrigger.
    split.
    - apply active_participants_accounted_by_contribution with (Contributed := Contributed).
      + exact Hall.
      + exact Haccounted.
    - split.
      + intros w Hactive Hparked.
        destruct Htrigger as [Hsent | Hfailed].
        * apply Hgeneration_resumes.
          -- exact Hactive.
          -- exact Hparked.
          -- apply Hclosed_gen.
             apply (posted_driver_or_abort_fallback_closes_cycle
                      TriggerSent DriverPosted CycleClosed SatbAbort StwFallbackRan).
             ++ exact Hsent_driver.
             ++ exact Hdriver_progress.
             ++ exact Habort_fallback.
             ++ exact Hfallback_closes.
             ++ exact Hsent.
        * apply Hbackstop_resumes.
          -- destruct (Hfailed_backstop Hfailed) as [_ Hresumed].
             exact Hresumed.
          -- exact Hactive.
          -- exact Hparked.
      + apply completion_guard_rules_out_parent_strand with (Exit := Exit).
        * exact Hexits.
        * exact Hnormal.
        * exact Hpanic.
  Qed.
End SchedulerFanoutProgressModel.

End MeTTaTron_GC_SchedulerFanoutProgress.
