(** Rocq model of pooled cron recurring-task termination.

    Inline recurring tasks stop when their closure returns false or panics. For
    worker-pool dispatch, the worker must publish a terminal stop bit before it
    clears in_flight; otherwise the cron thread can observe an idle recurring
    task and dispatch it again.
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
End CronRecurringModel.

End MeTTaTron_GC_CronRecurringDispatch.
