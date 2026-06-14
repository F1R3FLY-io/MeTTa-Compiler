(** Wavefront scheduler reordering contract.

    A wavefront schedule may batch tasks only when every dependency edge points
    to a strictly earlier wave.  This file captures the proof obligation used by
    the Rust scheduler: if all dependencies of a ready set are already in prior
    waves, every task in that ready set can share the next wave.  The maximal
    batching obligation additionally says that a ready task not already assigned
    to an earlier wave must not be deferred past the current wave.  If a cycle is
    collapsed into one wave, the independence claim is false.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerWavefrontParallelism.

Section WavefrontModel.
  Variable Task : Type.

  Definition dependencies_before
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop) : Prop :=
    forall task dep,
      depends_on dep task ->
      wave dep < wave task.

  Definition same_wave_independent
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop) : Prop :=
    forall left right,
      wave left = wave right ->
      ~ depends_on left right.

  Definition task_ready_for_wave
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop)
      (k : nat)
      (task : Task) : Prop :=
    forall dep,
      depends_on dep task ->
      wave dep < k.

  Definition ready_set_independent
      (ready : Task -> Prop)
      (depends_on : Task -> Task -> Prop) : Prop :=
    forall left right,
      ready left ->
      ready right ->
      ~ depends_on left right.

  Definition no_dependencies (depends_on : Task -> Task -> Prop) : Prop :=
    forall left right, ~ depends_on left right.

  Definition wave_maximal_for_ready
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop)
      (k : nat) : Prop :=
    forall task,
      task_ready_for_wave wave depends_on k task ->
      wave task <= k.

  Definition current_wave_ready_set
      (wave : Task -> nat)
      (depends_on : Task -> Task -> Prop)
      (k : nat)
      (ready : Task -> Prop) : Prop :=
    forall task,
      ready task <->
      task_ready_for_wave wave depends_on k task /\ ~ wave task < k.

  Theorem dependencies_before_implies_same_wave_independent :
    forall wave depends_on,
      dependencies_before wave depends_on ->
      same_wave_independent wave depends_on.
  Proof.
    intros wave depends_on Hbefore.
    unfold same_wave_independent.
    intros left right Hsame Hedge.
    specialize (Hbefore right left Hedge).
    lia.
  Qed.

  Theorem ready_set_can_share_wave :
    forall wave depends_on ready k,
      (forall task, ready task -> task_ready_for_wave wave depends_on k task) ->
      (forall task, ready task -> wave task = k) ->
      ready_set_independent ready depends_on.
  Proof.
    intros wave depends_on ready k Hready_before Hwave_eq.
    unfold ready_set_independent.
    intros left right Hleft Hright Hedge.
    specialize (Hready_before right Hright left Hedge).
    rewrite (Hwave_eq left Hleft) in Hready_before.
    lia.
  Qed.

  Theorem maximal_ready_wave_no_deferred_task :
    forall wave depends_on k task,
      wave_maximal_for_ready wave depends_on k ->
      task_ready_for_wave wave depends_on k task ->
      ~ wave task < k ->
      wave task = k.
  Proof.
    intros wave depends_on k task Hmaximal Hready Hnot_prior.
    specialize (Hmaximal task Hready).
    lia.
  Qed.

  Theorem maximal_ready_set_can_share_wave :
    forall wave depends_on ready k,
      wave_maximal_for_ready wave depends_on k ->
      current_wave_ready_set wave depends_on k ready ->
      ready_set_independent ready depends_on.
  Proof.
    intros wave depends_on ready k Hmaximal Hready_exact.
    apply (ready_set_can_share_wave wave depends_on ready k).
    - intros task Hready_member.
      destruct (Hready_exact task) as [Hready_to _].
      destruct (Hready_to Hready_member) as [Hready_for_wave _].
      exact Hready_for_wave.
    - intros task Hready_member.
      destruct (Hready_exact task) as [Hready_to _].
      destruct (Hready_to Hready_member) as [Hready_for_wave Hnot_prior].
      apply (maximal_ready_wave_no_deferred_task wave depends_on k task).
      + exact Hmaximal.
      + exact Hready_for_wave.
      + exact Hnot_prior.
  Qed.

  Theorem no_dependencies_single_wave_contract :
    forall wave depends_on,
      no_dependencies depends_on ->
      (forall task, wave task = 0) ->
      dependencies_before wave depends_on /\
      same_wave_independent wave depends_on.
  Proof.
    intros wave depends_on Hnone _.
    split.
    - unfold dependencies_before.
      intros task dep Hedge.
      exfalso.
      apply (Hnone dep task).
      exact Hedge.
    - unfold same_wave_independent.
      intros left right _.
      apply Hnone.
  Qed.

  Theorem two_cycle_rejected_by_dependency_order :
    forall wave depends_on left right,
      dependencies_before wave depends_on ->
      depends_on left right ->
      depends_on right left ->
      False.
  Proof.
    intros wave depends_on left right Hbefore Hleft_right Hright_left.
    pose proof (Hbefore right left Hleft_right) as Hleft_before_right.
    pose proof (Hbefore left right Hright_left) as Hright_before_left.
    lia.
  Qed.

  Theorem same_wave_edge_rejected :
    forall wave depends_on left right,
      wave left = wave right ->
      depends_on left right ->
      ~ same_wave_independent wave depends_on.
  Proof.
    intros wave depends_on left right Hsame Hedge Hindependent.
    unfold same_wave_independent in Hindependent.
    apply (Hindependent left right Hsame).
    exact Hedge.
  Qed.
End WavefrontModel.

End MeTTaTron_GC_SchedulerWavefrontParallelism.
