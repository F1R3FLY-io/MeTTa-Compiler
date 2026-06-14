(** Fanout admission completeness contract.

    The WFST transducer's [parallelism_degree] is an admission signal for the
    evaluator's branch fanout path: degree 1 means "keep this branch set
    sequential", while degree > 1 allows the branch set to attempt the ordinary
    purity/depth/pool/budget gates.  Once admitted, the stack-safe dispatcher
    must not trim the branch vector to the degree.  It dispatches the whole
    branch set and lets the fixed work pool, queue-pressure gate, per-depth
    quota, and completion/cancellation protocol bound execution.

    This distinction is load-bearing for correctness: capping the spawned
    slots to the degree would silently drop required branches for Demand::All
    fanouts, or convert the no-silent-drop checks into errors.  The theorem
    below pins the shape used by the Rust dispatcher: admitted fanout is
    complete exactly when every branch slot is represented in the spawned
    range.
*)

From Stdlib Require Import Arith Lia PeanoNat.

Module MeTTaTron_GC_SchedulerFanoutAdmissionCompleteness.

Definition degree_gate (parallelism_degree : nat) : Prop :=
  1 < parallelism_degree.

Definition fanout_admitted
    (branch_count min_branches parallelism_degree budget_granted : nat)
    (pure depth_ok pool_ok : Prop) : Prop :=
  min_branches <= branch_count /\
  degree_gate parallelism_degree /\
  pure /\
  depth_ok /\
  pool_ok /\
  0 < budget_granted.

Definition complete_dispatch (branch_count spawned_count : nat) : Prop :=
  spawned_count = branch_count.

Definition branch_slot_represented
    (spawned_count slot : nat) : Prop :=
  slot < spawned_count.

Definition degree_capped_spawn_count
    (branch_count parallelism_degree : nat) : nat :=
  Nat.min branch_count parallelism_degree.

Theorem admitted_fanout_has_degree_gate :
  forall branch_count min_branches parallelism_degree budget_granted
         pure depth_ok pool_ok,
    fanout_admitted
      branch_count min_branches parallelism_degree budget_granted
      pure depth_ok pool_ok ->
    degree_gate parallelism_degree.
Proof.
  intros branch_count min_branches parallelism_degree budget_granted
         pure depth_ok pool_ok Hadmitted.
  unfold fanout_admitted in Hadmitted.
  tauto.
Qed.

Theorem complete_dispatch_represents_every_branch :
  forall branch_count spawned_count slot,
    complete_dispatch branch_count spawned_count ->
    slot < branch_count ->
    branch_slot_represented spawned_count slot.
Proof.
  intros branch_count spawned_count slot Hcomplete Hslot.
  unfold complete_dispatch in Hcomplete.
  unfold branch_slot_represented.
  rewrite Hcomplete.
  exact Hslot.
Qed.

Theorem all_branch_slots_represented_implies_complete :
  forall branch_count spawned_count,
    spawned_count <= branch_count ->
    (forall slot, slot < branch_count -> branch_slot_represented spawned_count slot) ->
    complete_dispatch branch_count spawned_count.
Proof.
  intros branch_count spawned_count Hupper Hall.
  unfold complete_dispatch.
  destruct (Nat.eq_dec spawned_count branch_count) as [Heq | Hneq].
  - exact Heq.
  - assert (spawned_count < branch_count) by lia.
    specialize (Hall spawned_count H).
    unfold branch_slot_represented in Hall.
    lia.
Qed.

Theorem admitted_complete_fanout_represents_every_branch :
  forall branch_count min_branches parallelism_degree budget_granted
         pure depth_ok pool_ok spawned_count slot,
    fanout_admitted
      branch_count min_branches parallelism_degree budget_granted
      pure depth_ok pool_ok ->
    complete_dispatch branch_count spawned_count ->
    slot < branch_count ->
    branch_slot_represented spawned_count slot.
Proof.
  intros branch_count min_branches parallelism_degree budget_granted
         pure depth_ok pool_ok spawned_count slot _ Hcomplete Hslot.
  apply complete_dispatch_represents_every_branch
    with (branch_count := branch_count).
  - exact Hcomplete.
  - exact Hslot.
Qed.

Theorem degree_capped_partial_dispatch_drops_required_branch :
  forall branch_count parallelism_degree,
    1 < parallelism_degree ->
    parallelism_degree < branch_count ->
    ~ complete_dispatch
        branch_count
        (degree_capped_spawn_count branch_count parallelism_degree).
Proof.
  intros branch_count parallelism_degree Hdegree Hbelow Hcomplete.
  unfold complete_dispatch in Hcomplete.
  unfold degree_capped_spawn_count in Hcomplete.
  rewrite Nat.min_r in Hcomplete by lia.
  lia.
Qed.

Theorem degree_capped_partial_dispatch_misses_slot :
  forall branch_count parallelism_degree,
    1 < parallelism_degree ->
    parallelism_degree < branch_count ->
    exists slot,
      slot < branch_count /\
      ~ branch_slot_represented
          (degree_capped_spawn_count branch_count parallelism_degree)
          slot.
Proof.
  intros branch_count parallelism_degree Hdegree Hbelow.
  exists parallelism_degree.
  split.
  - exact Hbelow.
  - unfold branch_slot_represented, degree_capped_spawn_count.
    rewrite Nat.min_r by lia.
    lia.
Qed.

Theorem all_extra_branches_budget_request_is_maximal :
  forall branch_count request,
    0 < branch_count ->
    request = branch_count - 1 ->
    forall smaller,
      smaller < request ->
      smaller + 1 < branch_count.
Proof.
  intros branch_count request Hpositive Hrequest smaller Hsmaller.
  rewrite Hrequest in Hsmaller.
  lia.
Qed.

End MeTTaTron_GC_SchedulerFanoutAdmissionCompleteness.
