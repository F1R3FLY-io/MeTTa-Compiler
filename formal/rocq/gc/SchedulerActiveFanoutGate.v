(** Active production fanout gate composition.

    The production evaluator does not currently call the wavefront DAG builder.
    Its active parallel branch/collapse path is direct fanout: the WFST degree
    gate, purity/dynamic-eval blocker, depth gate, pool gate, and budget gate
    must all pass before [parallel_dispatch] or [parallel_collapse_dispatch]
    runs.  Once it runs, every admitted slot must be represented.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_SchedulerActiveFanoutGate.

Section ActiveFanoutGateModel.
  Definition degree_gate (parallelism_degree : nat) : Prop :=
    1 < parallelism_degree.

  Definition budget_gate (budget_granted : nat) : Prop :=
    0 < budget_granted.

  Definition active_fanout_allowed
      (branch_count min_branches parallelism_degree budget_granted : nat)
      (pure depth_ok pool_ok : Prop) : Prop :=
    min_branches <= branch_count /\
    degree_gate parallelism_degree /\
    pure /\
    depth_ok /\
    pool_ok /\
    budget_gate budget_granted.

  Definition complete_dispatch (slot_count dispatched_count : nat) : Prop :=
    dispatched_count = slot_count.

  Definition slot_represented (dispatched_count slot : nat) : Prop :=
    slot < dispatched_count.

  Theorem active_fanout_requires_degree_gate :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      degree_gate parallelism_degree.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok Hallowed.
    unfold active_fanout_allowed in Hallowed.
    tauto.
  Qed.

  Theorem active_fanout_requires_purity_gate :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      pure.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok Hallowed.
    unfold active_fanout_allowed in Hallowed.
    tauto.
  Qed.

  Theorem active_fanout_requires_depth_gate :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      depth_ok.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok Hallowed.
    unfold active_fanout_allowed in Hallowed.
    tauto.
  Qed.

  Theorem active_fanout_requires_pool_gate :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      pool_ok.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok Hallowed.
    unfold active_fanout_allowed in Hallowed.
    tauto.
  Qed.

  Theorem active_fanout_requires_budget_gate :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      budget_gate budget_granted.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok Hallowed.
    unfold active_fanout_allowed in Hallowed.
    tauto.
  Qed.

  Theorem active_complete_dispatch_represents_every_slot :
    forall branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok dispatched_count slot,
      active_fanout_allowed
        branch_count min_branches parallelism_degree budget_granted
        pure depth_ok pool_ok ->
      complete_dispatch branch_count dispatched_count ->
      slot < branch_count ->
      slot_represented dispatched_count slot.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           pure depth_ok pool_ok dispatched_count slot _ Hcomplete Hslot.
    unfold complete_dispatch in Hcomplete.
    unfold slot_represented.
    rewrite Hcomplete.
    exact Hslot.
  Qed.

  Theorem all_slots_represented_with_upper_bound_implies_complete :
    forall branch_count dispatched_count,
      dispatched_count <= branch_count ->
      (forall slot, slot < branch_count -> slot_represented dispatched_count slot) ->
      complete_dispatch branch_count dispatched_count.
  Proof.
    intros branch_count dispatched_count Hupper Hall.
    unfold complete_dispatch.
    destruct (Nat.eq_dec dispatched_count branch_count) as [Heq | Hneq].
    - exact Heq.
    - assert (dispatched_count < branch_count) by lia.
      specialize (Hall dispatched_count H).
      unfold slot_represented in Hall.
      lia.
  Qed.

  Theorem missing_purity_gate_rejects_active_fanout :
    forall branch_count min_branches parallelism_degree budget_granted
           depth_ok pool_ok,
      ~ active_fanout_allowed
          branch_count min_branches parallelism_degree budget_granted
          False depth_ok pool_ok.
  Proof.
    intros branch_count min_branches parallelism_degree budget_granted
           depth_ok pool_ok Hallowed.
    exact (active_fanout_requires_purity_gate
             branch_count min_branches parallelism_degree budget_granted
             False depth_ok pool_ok Hallowed).
  Qed.

  Theorem missing_budget_gate_rejects_active_fanout :
    forall branch_count min_branches parallelism_degree
           pure depth_ok pool_ok,
      ~ active_fanout_allowed
          branch_count min_branches parallelism_degree 0
          pure depth_ok pool_ok.
  Proof.
    intros branch_count min_branches parallelism_degree
           pure depth_ok pool_ok Hallowed.
    pose proof
      (active_fanout_requires_budget_gate
         branch_count min_branches parallelism_degree 0
         pure depth_ok pool_ok Hallowed) as Hbudget.
    unfold budget_gate in Hbudget.
    lia.
  Qed.
End ActiveFanoutGateModel.

End MeTTaTron_GC_SchedulerActiveFanoutGate.
