(** Async Rholang batch worker completion obligation.

    `evaluate_batch_parallel_arena` submits one WorkPool eval task per batch
    item and waits on a shared `remaining` counter.  A tail-position decrement
    is not enough: if `eval_trampoline` panics, the WorkPool catches the unwind
    after the closure exits, and a skipped decrement can strand the async caller
    forever.  The completion decrement must therefore live in an RAII guard that
    drops on both normal return and panic unwind.

    Separately, a completion guard may let the parent observe completion even
    when the panicking worker never stored its result slot.  That must not be a
    successful smaller batch: parent success requires every spawned slot to be
    stored; a missing slot must be an error/panic, not a valid subset.
*)

Module MeTTaTron_GC_RholangBatchCompletion.

Section RholangBatchCompletionModel.
  Variable Worker : Type.

  Inductive ExitPath : Type :=
  | NormalExit : ExitPath
  | PanicExit : ExitPath.

  Definition WorkerExited
      (Exit : Worker -> ExitPath -> Prop)
      (w : Worker) : Prop :=
    Exit w NormalExit \/ Exit w PanicExit.

  Definition ParentWaitStranded
      (Spawned GuardDropped : Worker -> Prop) : Prop :=
    exists w, Spawned w /\ ~ GuardDropped w.

  Definition SilentSuccessfulBatchDrop
      (Spawned SlotStored : Worker -> Prop)
      (ParentSucceeded : Prop) : Prop :=
    ParentSucceeded /\ exists w, Spawned w /\ ~ SlotStored w.

  Theorem batch_guard_drop_covers_worker_exit :
    forall (GuardDropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Exit w NormalExit -> GuardDropped w) ->
      (forall w, Exit w PanicExit -> GuardDropped w) ->
      forall w,
        WorkerExited Exit w -> GuardDropped w.
  Proof.
    intros GuardDropped Exit Hnormal Hpanic w Hexited.
    destruct Hexited as [Hnormal_exit | Hpanic_exit].
    - apply Hnormal. exact Hnormal_exit.
    - apply Hpanic. exact Hpanic_exit.
  Qed.

  Theorem every_spawned_batch_worker_drops_on_exit :
    forall (Spawned GuardDropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> GuardDropped w) ->
      (forall w, Exit w PanicExit -> GuardDropped w) ->
      forall w,
        Spawned w -> GuardDropped w.
  Proof.
    intros Spawned GuardDropped Exit Hevery_exits Hnormal Hpanic w Hspawned.
    apply (batch_guard_drop_covers_worker_exit GuardDropped Exit Hnormal Hpanic).
    apply Hevery_exits.
    exact Hspawned.
  Qed.

  Theorem no_stranded_batch_wait_when_all_workers_drop :
    forall (Spawned GuardDropped : Worker -> Prop),
      (forall w, Spawned w -> GuardDropped w) ->
      ~ ParentWaitStranded Spawned GuardDropped.
  Proof.
    intros Spawned GuardDropped Hall Hstranded.
    destruct Hstranded as [w [Hspawned Hnot_dropped]].
    apply Hnot_dropped.
    apply Hall.
    exact Hspawned.
  Qed.

  Theorem completion_guard_prevents_batch_panic_strand :
    forall (Spawned GuardDropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> GuardDropped w) ->
      (forall w, Exit w PanicExit -> GuardDropped w) ->
      ~ ParentWaitStranded Spawned GuardDropped.
  Proof.
    intros Spawned GuardDropped Exit Hevery_exits Hnormal Hpanic.
    apply no_stranded_batch_wait_when_all_workers_drop.
    intros w Hspawned.
    apply (every_spawned_batch_worker_drops_on_exit
             Spawned GuardDropped Exit Hevery_exits Hnormal Hpanic).
    exact Hspawned.
  Qed.

  Theorem panic_skip_completion_can_strand_batch_wait :
    forall (Spawned GuardDropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop)
           (ParentObserved : Prop)
           (w : Worker),
      Spawned w ->
      Exit w PanicExit ->
      ~ GuardDropped w ->
      (ParentObserved -> forall u, Spawned u -> Exit u PanicExit -> GuardDropped u) ->
      ~ ParentObserved.
  Proof.
    intros Spawned GuardDropped Exit ParentObserved w
           Hspawned Hpanic Hnot_dropped Hparent_complete Hobserved.
    apply Hnot_dropped.
    apply Hparent_complete.
    - exact Hobserved.
    - exact Hspawned.
    - exact Hpanic.
  Qed.

  Theorem strict_batch_success_requires_all_slots :
    forall (Spawned SlotStored : Worker -> Prop)
           (ParentSucceeded : Prop),
      (ParentSucceeded -> forall w, Spawned w -> SlotStored w) ->
      ~ SilentSuccessfulBatchDrop Spawned SlotStored ParentSucceeded.
  Proof.
    intros Spawned SlotStored ParentSucceeded Hstrict Hsilent.
    destruct Hsilent as [Hsucceeded [w [Hspawned Hnot_stored]]].
    apply Hnot_stored.
    apply Hstrict.
    - exact Hsucceeded.
    - exact Hspawned.
  Qed.

  Theorem missing_batch_slot_forces_error_prevents_silent_success :
    forall (Spawned SlotStored : Worker -> Prop)
           (ParentSucceeded : Prop),
      (forall w, Spawned w -> ~ SlotStored w -> ~ ParentSucceeded) ->
      ~ SilentSuccessfulBatchDrop Spawned SlotStored ParentSucceeded.
  Proof.
    intros Spawned SlotStored ParentSucceeded Hmissing_forces_error Hsilent.
    destruct Hsilent as [Hsucceeded [w [Hspawned Hnot_stored]]].
    apply (Hmissing_forces_error w Hspawned Hnot_stored).
    exact Hsucceeded.
  Qed.
End RholangBatchCompletionModel.

End MeTTaTron_GC_RholangBatchCompletion.
