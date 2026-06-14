(** Effect-conflict completeness for wavefront scheduler reordering.

    [SchedulerWavefrontParallelism] proves that dependency-safe tasks can share
    a wave.  This file makes the hidden precondition explicit: any pair of tasks
    that conflict through effects, shared state, allocator/GC safepoints, or
    other non-commuting behavior must be represented by a dependency edge in at
    least one direction before wavefront batching can claim same-wave safety.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerEffectConflictCompleteness.

Section ConflictCompletenessModel.
  Variable Task : Type.

  Definition dependencies_before
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop) : Prop :=
    forall task dep,
      depends_on dep task ->
      wave dep < wave task.

  Definition conflict_edges_covered
      (depends_on : Task -> Task -> Prop)
      (conflicts : Task -> Task -> Prop) : Prop :=
    forall left right,
      conflicts left right ->
      depends_on left right \/ depends_on right left.

  Definition same_wave_conflict_free
      (wave : Task -> nat)
      (conflicts : Task -> Task -> Prop) : Prop :=
    forall left right,
      wave left = wave right ->
      ~ conflicts left right.

  Definition no_conflicts (conflicts : Task -> Task -> Prop) : Prop :=
    forall left right,
      ~ conflicts left right.

  Theorem dependency_order_and_conflict_coverage_imply_same_wave_conflict_free :
    forall wave depends_on conflicts,
      dependencies_before wave depends_on ->
      conflict_edges_covered depends_on conflicts ->
      same_wave_conflict_free wave conflicts.
  Proof.
    intros wave depends_on conflicts Hbefore Hcovered.
    unfold same_wave_conflict_free.
    intros left right Hsame Hconflict.
    specialize (Hcovered left right Hconflict) as [Hleft_right | Hright_left].
    - specialize (Hbefore right left Hleft_right).
      lia.
    - specialize (Hbefore left right Hright_left).
      lia.
  Qed.

  Theorem conflict_edge_coverage_forces_distinct_waves :
    forall wave depends_on conflicts left right,
      dependencies_before wave depends_on ->
      conflict_edges_covered depends_on conflicts ->
      conflicts left right ->
      wave left <> wave right.
  Proof.
    intros wave depends_on conflicts left right Hbefore Hcovered Hconflict Hsame.
    pose proof
      (dependency_order_and_conflict_coverage_imply_same_wave_conflict_free
         wave depends_on conflicts Hbefore Hcovered)
      as Hfree.
    unfold same_wave_conflict_free in Hfree.
    exact (Hfree left right Hsame Hconflict).
  Qed.

  Theorem same_wave_conflict_rejects_complete_dependency_order :
    forall wave depends_on conflicts left right,
      dependencies_before wave depends_on ->
      wave left = wave right ->
      conflicts left right ->
      ~ conflict_edges_covered depends_on conflicts.
  Proof.
    intros wave depends_on conflicts left right Hbefore Hsame Hconflict Hcovered.
    pose proof
      (conflict_edge_coverage_forces_distinct_waves
         wave depends_on conflicts left right Hbefore Hcovered Hconflict)
      as Hdistinct.
    exact (Hdistinct Hsame).
  Qed.

  Theorem no_conflicts_single_wave_conflict_free :
    forall wave conflicts,
      no_conflicts conflicts ->
      same_wave_conflict_free wave conflicts.
  Proof.
    intros wave conflicts Hnone.
    unfold same_wave_conflict_free.
    intros left right _.
    exact (Hnone left right).
  Qed.

  Theorem single_wave_conflict_free_implies_no_conflicts :
    forall wave conflicts k,
      (forall task, wave task = k) ->
      same_wave_conflict_free wave conflicts ->
      no_conflicts conflicts.
  Proof.
    intros wave conflicts k Hsingle Hfree.
    unfold no_conflicts.
    intros left right Hconflict.
    unfold same_wave_conflict_free in Hfree.
    apply (Hfree left right).
    - rewrite Hsingle.
      rewrite Hsingle.
      reflexivity.
    - exact Hconflict.
  Qed.
End ConflictCompletenessModel.

End MeTTaTron_GC_SchedulerEffectConflictCompleteness.
