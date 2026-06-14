(** Rocq model of the E1 parallel completion-guard obligation.

    The parallel dispatch and collapse workers signal completion through a
    single RAII guard.  The guard's destructor is the only worker decrement and
    runs on both normal return and panic-unwind.  TLC checks the temporal
    fairness/liveness discriminator in [tla/CollapseCompletion.tla]; this file
    proves the compositional premise used by the source: once every spawned
    worker exits and every exit path drops the guard, the parent wait cannot be
    stranded by a skipped completion decrement.
*)

Module MeTTaTron_GC_CollapseCompletion.

Section CollapseCompletionModel.
  Variable Worker : Type.

  Inductive ExitPath : Type :=
  | NormalExit : ExitPath
  | PanicExit : ExitPath.

  Definition WorkerExited
      (Exit : Worker -> ExitPath -> Prop)
      (w : Worker) : Prop :=
    Exit w NormalExit \/ Exit w PanicExit.

  Definition ParentWaitStranded
      (Spawned Dropped : Worker -> Prop) : Prop :=
    exists w, Spawned w /\ ~ Dropped w.

  Definition SilentSuccessfulDrop
      (Spawned SlotStored : Worker -> Prop)
      (ParentSucceeded : Prop) : Prop :=
    ParentSucceeded /\ exists w, Spawned w /\ ~ SlotStored w.

  Theorem guard_drop_covers_worker_exit :
    forall (Dropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Exit w NormalExit -> Dropped w) ->
      (forall w, Exit w PanicExit -> Dropped w) ->
      forall w,
        WorkerExited Exit w -> Dropped w.
  Proof.
    intros Dropped Exit Hnormal Hpanic w Hexited.
    destruct Hexited as [Hnormal_exit | Hpanic_exit].
    - apply Hnormal. exact Hnormal_exit.
    - apply Hpanic. exact Hpanic_exit.
  Qed.

  Theorem every_spawned_worker_drops_on_exit :
    forall (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> Dropped w) ->
      (forall w, Exit w PanicExit -> Dropped w) ->
      forall w,
        Spawned w -> Dropped w.
  Proof.
    intros Spawned Dropped Exit Hevery_exits Hnormal Hpanic w Hspawned.
    apply (guard_drop_covers_worker_exit Dropped Exit Hnormal Hpanic).
    apply Hevery_exits.
    exact Hspawned.
  Qed.

  Theorem no_stranded_parent_wait_when_all_workers_drop :
    forall (Spawned Dropped : Worker -> Prop),
      (forall w, Spawned w -> Dropped w) ->
      ~ ParentWaitStranded Spawned Dropped.
  Proof.
    intros Spawned Dropped Hall Hstranded.
    destruct Hstranded as [w [Hspawned Hnot_dropped]].
    apply Hnot_dropped.
    apply Hall.
    exact Hspawned.
  Qed.

  Theorem completion_guard_prevents_panic_strand :
    forall (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop),
      (forall w, Spawned w -> WorkerExited Exit w) ->
      (forall w, Exit w NormalExit -> Dropped w) ->
      (forall w, Exit w PanicExit -> Dropped w) ->
      ~ ParentWaitStranded Spawned Dropped.
  Proof.
    intros Spawned Dropped Exit Hevery_exits Hnormal Hpanic.
    apply no_stranded_parent_wait_when_all_workers_drop.
    intros w Hspawned.
    apply (every_spawned_worker_drops_on_exit
             Spawned Dropped Exit Hevery_exits Hnormal Hpanic).
    exact Hspawned.
  Qed.

  Theorem panic_skip_completion_can_strand_parent_observation :
    forall (Spawned Dropped : Worker -> Prop)
           (Exit : Worker -> ExitPath -> Prop)
           (ParentObserved : Prop)
           (w : Worker),
      Spawned w ->
      Exit w PanicExit ->
      ~ Dropped w ->
      (ParentObserved -> forall u, Spawned u -> Exit u PanicExit -> Dropped u) ->
      ~ ParentObserved.
  Proof.
    intros Spawned Dropped Exit ParentObserved w Hspawned Hpanic Hnot_dropped Hparent_complete Hobserved.
    apply Hnot_dropped.
    apply Hparent_complete.
    - exact Hobserved.
    - exact Hspawned.
    - exact Hpanic.
  Qed.

  Theorem strict_success_requires_all_slots_prevents_silent_drop :
    forall (Spawned SlotStored : Worker -> Prop)
           (ParentSucceeded : Prop),
      (ParentSucceeded -> forall w, Spawned w -> SlotStored w) ->
      ~ SilentSuccessfulDrop Spawned SlotStored ParentSucceeded.
  Proof.
    intros Spawned SlotStored ParentSucceeded Hstrict Hsilent.
    destruct Hsilent as [Hsucceeded [w [Hspawned Hnot_stored]]].
    apply Hnot_stored.
    apply Hstrict.
    - exact Hsucceeded.
    - exact Hspawned.
  Qed.

  Theorem weak_success_with_missing_slot_witnesses_silent_drop :
    forall (Spawned SlotStored : Worker -> Prop)
           (ParentSucceeded : Prop)
           (w : Worker),
      ParentSucceeded ->
      Spawned w ->
      ~ SlotStored w ->
      SilentSuccessfulDrop Spawned SlotStored ParentSucceeded.
  Proof.
    intros Spawned SlotStored ParentSucceeded w Hsucceeded Hspawned Hnot_stored.
    split.
    - exact Hsucceeded.
    - exists w. split.
      + exact Hspawned.
      + exact Hnot_stored.
  Qed.

  Theorem missing_slot_forces_error_prevents_silent_success :
    forall (Spawned SlotStored : Worker -> Prop)
           (ParentSucceeded : Prop),
      (forall w, Spawned w -> ~ SlotStored w -> ~ ParentSucceeded) ->
      ~ SilentSuccessfulDrop Spawned SlotStored ParentSucceeded.
  Proof.
    intros Spawned SlotStored ParentSucceeded Hmissing_forces_error Hsilent.
    destruct Hsilent as [Hsucceeded [w [Hspawned Hnot_stored]]].
    apply (Hmissing_forces_error w Hspawned Hnot_stored).
    exact Hsucceeded.
  Qed.
End CollapseCompletionModel.

End MeTTaTron_GC_CollapseCompletion.
