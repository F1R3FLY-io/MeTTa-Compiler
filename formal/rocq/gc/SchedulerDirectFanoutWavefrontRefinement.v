(** Direct production fanout as the independent-wavefront refinement.

    The production evaluator does not currently call the general wavefront DAG
    scheduler.  Its rule-match branch fanout is still a valid implementation of
    the all-independent wavefront special case: every pure branch is ready in
    wave 0, direct dispatch represents every branch slot, and the resulting
    parallelism is maximal for that independent branch set.

    Conversely, this model makes the boundary explicit: a dependency-bearing
    instruction DAG cannot be justified by direct single-wave fanout.  It needs
    the general wavefront dependency construction and conflict coverage proof.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerDirectFanoutWavefrontRefinement.

Section DirectFanoutWavefrontRefinement.
  Definition independent_branch_set (dependency_edges : nat) : Prop :=
    dependency_edges = 0.

  Definition wavefront_single_wave
      (branch_count dependency_edges wave_count max_wave_size : nat) : Prop :=
    0 < branch_count /\
    independent_branch_set dependency_edges /\
    wave_count = 1 /\
    max_wave_size = branch_count.

  Definition direct_fanout_complete
      (branch_count dispatched_count : nat) : Prop :=
    dispatched_count = branch_count.

  Definition direct_fanout_refines_independent_wavefront
      (branch_count dependency_edges wave_count max_wave_size
       dispatched_count : nat) : Prop :=
    wavefront_single_wave
      branch_count dependency_edges wave_count max_wave_size /\
    direct_fanout_complete branch_count dispatched_count.

  Theorem direct_refinement_requires_independence :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      direct_fanout_refines_independent_wavefront
        branch_count dependency_edges wave_count max_wave_size dispatched_count ->
      independent_branch_set dependency_edges.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hrefines.
    unfold direct_fanout_refines_independent_wavefront in Hrefines.
    unfold wavefront_single_wave in Hrefines.
    tauto.
  Qed.

  Theorem direct_refinement_dispatches_every_branch :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      direct_fanout_refines_independent_wavefront
        branch_count dependency_edges wave_count max_wave_size dispatched_count ->
      dispatched_count = branch_count.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hrefines.
    unfold direct_fanout_refines_independent_wavefront in Hrefines.
    unfold direct_fanout_complete in Hrefines.
    tauto.
  Qed.

  Theorem direct_refinement_matches_wavefront_max_parallelism :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      direct_fanout_refines_independent_wavefront
        branch_count dependency_edges wave_count max_wave_size dispatched_count ->
      dispatched_count = max_wave_size.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hrefines.
    unfold direct_fanout_refines_independent_wavefront in Hrefines.
    unfold wavefront_single_wave in Hrefines.
    unfold direct_fanout_complete in Hrefines.
    lia.
  Qed.

  Theorem direct_refinement_uses_wave_zero_only :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      direct_fanout_refines_independent_wavefront
        branch_count dependency_edges wave_count max_wave_size dispatched_count ->
      wave_count = 1.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hrefines.
    unfold direct_fanout_refines_independent_wavefront in Hrefines.
    unfold wavefront_single_wave in Hrefines.
    tauto.
  Qed.

  Theorem independent_complete_direct_fanout_refines_single_wave :
    forall branch_count dispatched_count,
      0 < branch_count ->
      direct_fanout_complete branch_count dispatched_count ->
      direct_fanout_refines_independent_wavefront
        branch_count 0 1 branch_count dispatched_count.
  Proof.
    intros branch_count dispatched_count Hnonempty Hcomplete.
    unfold direct_fanout_refines_independent_wavefront.
    split.
    - unfold wavefront_single_wave.
      unfold independent_branch_set.
      repeat split; try lia.
    - exact Hcomplete.
  Qed.

  Theorem dependent_dag_rejects_direct_single_wave_refinement :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      0 < dependency_edges ->
      ~ direct_fanout_refines_independent_wavefront
          branch_count dependency_edges wave_count max_wave_size dispatched_count.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hdeps Hrefines.
    pose proof
      (direct_refinement_requires_independence
         branch_count dependency_edges wave_count max_wave_size
         dispatched_count Hrefines) as Hindependent.
    unfold independent_branch_set in Hindependent.
    lia.
  Qed.

  Theorem partial_direct_dispatch_rejects_refinement :
    forall branch_count dependency_edges wave_count max_wave_size dispatched_count,
      dispatched_count < branch_count ->
      ~ direct_fanout_refines_independent_wavefront
          branch_count dependency_edges wave_count max_wave_size dispatched_count.
  Proof.
    intros branch_count dependency_edges wave_count max_wave_size
           dispatched_count Hpartial Hrefines.
    pose proof
      (direct_refinement_dispatches_every_branch
         branch_count dependency_edges wave_count max_wave_size
         dispatched_count Hrefines) as Hcomplete.
    lia.
  Qed.
End DirectFanoutWavefrontRefinement.

End MeTTaTron_GC_SchedulerDirectFanoutWavefrontRefinement.
