(** WorkPool panic-isolation obligation.

    WorkPool workers must survive panicking tasks and continue draining later
    queued work.  The production implementation has two catch layers:

    - [PriorityTask::execute] catches the task closure panic and returns runtime
      zero so runtime/WFST accounting is not updated from a failed task.
    - [work_pool_worker_loop] wraps the post-execute accounting path so an
      accounting/weight-update panic cannot kill the worker thread.

    This file abstracts the control-flow contract.  It intentionally does not
    model priority order; it proves that the failure edge preserves the worker
    needed to process the next queue item, and records the two historical
    negative shapes: missing inner catch loses the task-panic heartbeat path,
    while missing outer catch can kill the worker on accounting panic.
*)

Module MeTTaTron_GC_WorkPoolPanicIsolation.

Inductive FailureKind : Type :=
  | TaskClosurePanic : FailureKind
  | AccountingPanic : FailureKind.

Record StepResult : Type := {
  worker_alive : bool;
  runtime_recorded : bool;
  cpu_published : bool
}.

Definition dead_step : StepResult :=
  {| worker_alive := false; runtime_recorded := false; cpu_published := false |}.

Definition task_panic_inner_caught : StepResult :=
  {| worker_alive := true; runtime_recorded := false; cpu_published := true |}.

Definition task_panic_outer_only : StepResult :=
  {| worker_alive := true; runtime_recorded := false; cpu_published := false |}.

Definition accounting_panic_outer_caught : StepResult :=
  {| worker_alive := true; runtime_recorded := false; cpu_published := false |}.

Definition handle_failure
    (inner_catch outer_catch : bool)
    (failure : FailureKind) : StepResult :=
  match failure with
  | TaskClosurePanic =>
      if inner_catch then task_panic_inner_caught
      else if outer_catch then task_panic_outer_only
      else dead_step
  | AccountingPanic =>
      if outer_catch then accounting_panic_outer_caught
      else dead_step
  end.

Definition next_task_can_run (r : StepResult) : Prop :=
  worker_alive r = true.

Definition runtime_update_skipped (r : StepResult) : Prop :=
  runtime_recorded r = false.

Definition task_panic_heartbeat_published (r : StepResult) : Prop :=
  cpu_published r = true.

Theorem task_panic_inner_catch_skips_runtime_update :
  runtime_update_skipped (handle_failure true true TaskClosurePanic).
Proof.
  reflexivity.
Qed.

Theorem task_panic_inner_catch_publishes_cpu_state :
  task_panic_heartbeat_published (handle_failure true true TaskClosurePanic).
Proof.
  reflexivity.
Qed.

Theorem task_panic_inner_catch_keeps_worker_alive :
  next_task_can_run (handle_failure true true TaskClosurePanic).
Proof.
  reflexivity.
Qed.

Theorem accounting_panic_outer_catch_keeps_worker_alive :
  forall inner_catch,
    next_task_can_run (handle_failure inner_catch true AccountingPanic).
Proof.
  intros []; reflexivity.
Qed.

Theorem caught_failure_permits_subsequent_queue_drain :
  forall inner_catch outer_catch failure,
    (failure = TaskClosurePanic -> inner_catch = true \/ outer_catch = true) ->
    (failure = AccountingPanic -> outer_catch = true) ->
    next_task_can_run (handle_failure inner_catch outer_catch failure).
Proof.
  intros inner_catch outer_catch failure Htask Hacct.
  destruct failure; destruct inner_catch, outer_catch; simpl; try reflexivity.
  - destruct (Htask eq_refl) as [Hinner | Houter]; discriminate.
  - specialize (Hacct eq_refl). discriminate.
  - specialize (Hacct eq_refl). discriminate.
Qed.

Theorem missing_inner_catch_loses_task_panic_heartbeat :
  ~ task_panic_heartbeat_published (handle_failure false true TaskClosurePanic).
Proof.
  unfold task_panic_heartbeat_published.
  simpl.
  discriminate.
Qed.

Theorem missing_outer_catch_can_kill_worker_on_accounting_panic :
  ~ next_task_can_run (handle_failure true false AccountingPanic).
Proof.
  unfold next_task_can_run.
  simpl.
  discriminate.
Qed.

End MeTTaTron_GC_WorkPoolPanicIsolation.
