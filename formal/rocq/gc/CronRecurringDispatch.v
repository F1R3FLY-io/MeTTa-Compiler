(** Rocq model of pooled cron recurring-task termination.

    Inline recurring tasks stop when their closure returns false or panics. For
    worker-pool dispatch, the worker must publish a terminal stop bit before it
    clears in_flight; otherwise the cron thread can observe an idle recurring
    task and dispatch it again.

    The pooled path also has a dispatch-side obligation: the cron thread must
    claim the shared in_flight bit before it submits work to the pool. If a due
    placeholder sees in_flight already set, it must requeue only and not dispatch
    a second worker for the same recurring closure.
*)

Module MeTTaTron_GC_CronRecurringDispatch.

Section CronRecurringModel.
  Inductive WorkerResult : Type :=
  | Continue : WorkerResult
  | Stop : WorkerResult
  | Panic : WorkerResult.

  Inductive CronDecision : Type :=
  | Dispatch : CronDecision
  | RequeueOnly : CronDecision
  | DropRecurring : CronDecision.

  Record DispatchState : Type := {
    in_flight : bool;
    stop_requested : bool
  }.

  Definition worker_complete (result : WorkerResult) (_ : DispatchState) : DispatchState :=
    match result with
    | Continue => {| in_flight := false; stop_requested := false |}
    | Stop => {| in_flight := false; stop_requested := true |}
    | Panic => {| in_flight := false; stop_requested := true |}
    end.

  Definition cron_due (s : DispatchState) : CronDecision :=
    if stop_requested s then DropRecurring
    else if in_flight s then RequeueOnly
    else Dispatch.

  Definition claim_for_dispatch (s : DispatchState) : DispatchState :=
    {| in_flight := true; stop_requested := stop_requested s |}.

  Theorem stop_result_next_due_drops :
    forall s,
      cron_due (worker_complete Stop s) = DropRecurring.
  Proof.
    intros s.
    reflexivity.
  Qed.

  Theorem panic_result_next_due_drops :
    forall s,
      cron_due (worker_complete Panic s) = DropRecurring.
  Proof.
    intros s.
    reflexivity.
  Qed.

  Theorem continue_result_next_due_dispatches :
    forall s,
      cron_due (worker_complete Continue s) = Dispatch.
  Proof.
    intros s.
    reflexivity.
  Qed.

  Theorem in_flight_recurring_is_not_overlapped :
    forall stop,
      cron_due {| in_flight := true; stop_requested := stop |} <> Dispatch.
  Proof.
    intros stop Hdispatch.
    destruct stop; discriminate Hdispatch.
  Qed.

  Theorem dispatch_claim_sets_in_flight :
    forall s,
      in_flight (claim_for_dispatch s) = true.
  Proof.
    intros s.
    reflexivity.
  Qed.

  Theorem due_after_claim_requeues_without_dispatch :
    forall s,
      stop_requested s = false ->
      cron_due (claim_for_dispatch s) = RequeueOnly.
  Proof.
    intros s Hstop.
    unfold cron_due, claim_for_dispatch.
    rewrite Hstop.
    reflexivity.
  Qed.

  Theorem due_without_claim_can_dispatch_again :
    cron_due {| in_flight := false; stop_requested := false |} = Dispatch.
  Proof.
    reflexivity.
  Qed.
End CronRecurringModel.

End MeTTaTron_GC_CronRecurringDispatch.
