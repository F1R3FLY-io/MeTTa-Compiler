(** Collapse fanout admission completeness contract.

    Plain [collapse] and [collapse-bind] use a threshold-based admission gate
    rather than the WFST degree gate used by rule-match/amb/let fanout.  The
    correctness shape is the same once the parallel path is admitted: the
    dispatcher must represent every collapse result item.  The threshold is an
    admission threshold, not a cap on how many result items may be spawned.

    This proof complements [CollapseCompletion.v].  Completion proves that
    successful completion cannot silently omit a spawned result slot; this file
    proves the input side of the same contract, namely that every admitted
    collapse item is represented by a spawned slot in the first place.
*)

From Stdlib Require Import Arith Lia PeanoNat.

Module MeTTaTron_GC_CollapseFanoutAdmissionCompleteness.

Definition threshold_gate (item_count threshold : nat) : Prop :=
  threshold <= item_count.

Definition collapse_admitted
    (item_count threshold budget_granted : nat)
    (depth_ok pool_ok : Prop) : Prop :=
  threshold_gate item_count threshold /\
  depth_ok /\
  pool_ok /\
  0 < budget_granted.

Definition complete_dispatch (item_count spawned_count : nat) : Prop :=
  spawned_count = item_count.

Definition item_slot_represented
    (spawned_count slot : nat) : Prop :=
  slot < spawned_count.

Definition threshold_capped_spawn_count
    (item_count threshold : nat) : nat :=
  Nat.min item_count threshold.

Theorem admitted_collapse_has_threshold_gate :
  forall item_count threshold budget_granted depth_ok pool_ok,
    collapse_admitted item_count threshold budget_granted depth_ok pool_ok ->
    threshold_gate item_count threshold.
Proof.
  intros item_count threshold budget_granted depth_ok pool_ok Hadmitted.
  unfold collapse_admitted in Hadmitted.
  tauto.
Qed.

Theorem complete_dispatch_represents_every_item :
  forall item_count spawned_count slot,
    complete_dispatch item_count spawned_count ->
    slot < item_count ->
    item_slot_represented spawned_count slot.
Proof.
  intros item_count spawned_count slot Hcomplete Hslot.
  unfold complete_dispatch in Hcomplete.
  unfold item_slot_represented.
  rewrite Hcomplete.
  exact Hslot.
Qed.

Theorem all_item_slots_represented_implies_complete :
  forall item_count spawned_count,
    spawned_count <= item_count ->
    (forall slot, slot < item_count -> item_slot_represented spawned_count slot) ->
    complete_dispatch item_count spawned_count.
Proof.
  intros item_count spawned_count Hupper Hall.
  unfold complete_dispatch.
  destruct (Nat.eq_dec spawned_count item_count) as [Heq | Hneq].
  - exact Heq.
  - assert (spawned_count < item_count) by lia.
    specialize (Hall spawned_count H).
    unfold item_slot_represented in Hall.
    lia.
Qed.

Theorem admitted_complete_collapse_represents_every_item :
  forall item_count threshold budget_granted depth_ok pool_ok spawned_count slot,
    collapse_admitted item_count threshold budget_granted depth_ok pool_ok ->
    complete_dispatch item_count spawned_count ->
    slot < item_count ->
    item_slot_represented spawned_count slot.
Proof.
  intros item_count threshold budget_granted depth_ok pool_ok spawned_count slot
         _ Hcomplete Hslot.
  apply complete_dispatch_represents_every_item with (item_count := item_count).
  - exact Hcomplete.
  - exact Hslot.
Qed.

Theorem threshold_capped_partial_dispatch_drops_required_item :
  forall item_count threshold,
    threshold < item_count ->
    ~ complete_dispatch
        item_count
        (threshold_capped_spawn_count item_count threshold).
Proof.
  intros item_count threshold Hbelow Hcomplete.
  unfold complete_dispatch in Hcomplete.
  unfold threshold_capped_spawn_count in Hcomplete.
  rewrite Nat.min_r in Hcomplete by lia.
  lia.
Qed.

Theorem threshold_capped_partial_dispatch_misses_slot :
  forall item_count threshold,
    threshold < item_count ->
    exists slot,
      slot < item_count /\
      ~ item_slot_represented
          (threshold_capped_spawn_count item_count threshold)
          slot.
Proof.
  intros item_count threshold Hbelow.
  exists threshold.
  split.
  - exact Hbelow.
  - unfold item_slot_represented, threshold_capped_spawn_count.
    rewrite Nat.min_r by lia.
    lia.
Qed.

Theorem all_extra_items_budget_request_is_maximal :
  forall item_count request,
    0 < item_count ->
    request = item_count - 1 ->
    forall smaller,
      smaller < request ->
      smaller + 1 < item_count.
Proof.
  intros item_count request Hpositive Hrequest smaller Hsmaller.
  rewrite Hrequest in Hsmaller.
  lia.
Qed.

End MeTTaTron_GC_CollapseFanoutAdmissionCompleteness.
