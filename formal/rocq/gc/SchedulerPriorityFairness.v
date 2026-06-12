(** Rocq model of the priority-queue aging obligation.

    The Rust priority queue stores tasks in a BinaryHeap, so enqueue-time scores
    are not enough to prove starvation prevention: the age component must be
    recomputed immediately before dequeue. This model proves the arithmetic
    obligation used by the source fix: once age has grown enough, recomputation
    makes an older task strictly outrank a newer task, and equal recomputed
    scores fall back to FIFO sequence order.
*)

From Stdlib Require Import ZArith Lia.

Open Scope Z_scope.

Module MeTTaTron_GC_SchedulerPriorityFairness.

Section PriorityAgingModel.
  Record TaskScore : Type := {
    base_priority : Z;
    age_ticks : Z;
    sequence : nat
  }.

  Definition score (t : TaskScore) : Z :=
    base_priority t - age_ticks t.

  Definition StrictlyBefore (a b : TaskScore) : Prop :=
    score a < score b \/
    (score a = score b /\ (sequence a < sequence b)%nat).

  Theorem older_task_eventually_preempts :
    forall old_base new_base old_age new_age old_seq new_seq,
      old_age > old_base - new_base + new_age ->
      StrictlyBefore
        {| base_priority := old_base; age_ticks := old_age; sequence := old_seq |}
        {| base_priority := new_base; age_ticks := new_age; sequence := new_seq |}.
  Proof.
    intros old_base new_base old_age new_age old_seq new_seq Hage.
    left.
    unfold score; simpl.
    lia.
  Qed.

  Theorem recomputed_score_reflects_increased_age :
    forall base old_age new_age seq,
      new_age > old_age ->
      score {| base_priority := base; age_ticks := new_age; sequence := seq |} <
      score {| base_priority := base; age_ticks := old_age; sequence := seq |}.
  Proof.
    intros base old_age new_age seq Hage.
    unfold score; simpl.
    lia.
  Qed.

  Theorem fifo_tie_prefers_lower_sequence :
    forall base age older_seq newer_seq,
      (older_seq < newer_seq)%nat ->
      StrictlyBefore
        {| base_priority := base; age_ticks := age; sequence := older_seq |}
        {| base_priority := base; age_ticks := age; sequence := newer_seq |}.
  Proof.
    intros base age older_seq newer_seq Hseq.
    right.
    split.
    - unfold score; simpl. reflexivity.
    - exact Hseq.
  Qed.
End PriorityAgingModel.

End MeTTaTron_GC_SchedulerPriorityFairness.
