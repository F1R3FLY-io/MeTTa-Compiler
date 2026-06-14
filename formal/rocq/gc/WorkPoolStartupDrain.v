(** WorkPool startup-drain obligation.

    The eval WorkPool is allocated before OS worker threads are necessarily
    available. Eval tasks submitted during that startup window must be retained
    in the priority queue, and once workers start the retained queue must drain.

    This proof abstracts away priority order: for the startup-drain safety
    property only the count of retained tasks matters. A lossy enqueue, or a
    startup path with zero workers, is enough to refute full completion.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_WorkPoolStartupDrain.

Definition prestart_queue_after_submissions (submitted : nat) : nat := submitted.

Fixpoint drain_steps (steps queue completed : nat) : nat * nat :=
  match steps with
  | O => (queue, completed)
  | S steps' =>
      match queue with
      | O => drain_steps steps' O completed
      | S queue' => drain_steps steps' queue' (S completed)
      end
  end.

Definition drain_tick (active queue completed : nat) : nat * nat :=
  match active, queue with
  | O, _ => (queue, completed)
  | S _, O => (O, completed)
  | S _, S queue' => (queue', S completed)
  end.

Fixpoint drain_ticks (active steps queue completed : nat) : nat * nat :=
  match steps with
  | O => (queue, completed)
  | S steps' =>
      let '(queue', completed') := drain_tick active queue completed in
      drain_ticks active steps' queue' completed'
  end.

Theorem prestart_enqueue_retains_all :
  forall submitted,
    prestart_queue_after_submissions submitted = submitted.
Proof.
  intros; reflexivity.
Qed.

Lemma drain_steps_exact_aux :
  forall queued completed,
    drain_steps queued queued completed = (O, completed + queued).
Proof.
  induction queued as [| queued IH]; intros completed.
  - simpl. rewrite Nat.add_0_r. reflexivity.
  - simpl. rewrite IH. f_equal. lia.
Qed.

Theorem queued_before_start_drains_after_workers_start :
  forall submitted,
    drain_steps submitted (prestart_queue_after_submissions submitted) O =
      (O, submitted).
Proof.
  intros submitted.
  unfold prestart_queue_after_submissions.
  rewrite drain_steps_exact_aux.
  simpl.
  reflexivity.
Qed.

Theorem no_worker_start_makes_no_drain_progress :
  forall steps queue completed,
    drain_ticks O steps queue completed = (queue, completed).
Proof.
  induction steps as [| steps IH]; intros queue completed.
  - reflexivity.
  - simpl. apply IH.
Qed.

Theorem missing_workers_leave_prestart_queue_undrained :
  forall submitted steps,
    O < submitted ->
    drain_ticks O steps (prestart_queue_after_submissions submitted) O =
      (submitted, O).
Proof.
  intros submitted steps _Hpositive.
  unfold prestart_queue_after_submissions.
  apply no_worker_start_makes_no_drain_progress.
Qed.

Theorem lossy_prestart_enqueue_prevents_full_completion :
  forall submitted retained completed_after,
    retained < submitted ->
    drain_steps retained retained O = (O, completed_after) ->
    completed_after < submitted.
Proof.
  intros submitted retained completed_after Hlost Hdrained.
  rewrite drain_steps_exact_aux in Hdrained.
  simpl in Hdrained.
  inversion Hdrained; subst.
  exact Hlost.
Qed.

Theorem full_startup_completion_requires_all_tasks_retained :
  forall submitted retained,
    (forall completed_after,
        drain_steps retained retained O = (O, completed_after) ->
        completed_after = submitted) ->
    retained = submitted.
Proof.
  intros submitted retained Hcomplete.
  pose proof (drain_steps_exact_aux retained O) as Hdrain.
  simpl in Hdrain.
  specialize (Hcomplete retained Hdrain).
  lia.
Qed.

End MeTTaTron_GC_WorkPoolStartupDrain.
