(** C1.c young-allocation odometer obligations.

    The generational minor trigger uses [young_alloc_bytes]: bytes of young
    node-slot allocation since the last promotion.  Reused young slots and
    fresh bump allocation both add one positive node-size quantum to the
    odometer.  Promotion resets the odometer, so the next minor cannot be
    requested by stale young-allocation accounting.
*)

From Stdlib Require Import Arith.
From Stdlib Require Import Lia.

Module MeTTaTron_GC_YoungAllocationOdometer.

Section YoungAllocationOdometerModel.
  Definition YoungBudgetDue (Budget Odometer : nat) : Prop :=
    Odometer > Budget.

  Definition AllocationCounted
      (Before After NodeSize : nat) : Prop :=
    NodeSize > 0 /\ After = Before + NodeSize.

  Definition PromotionReset (After : nat) : Prop :=
    After = 0.

  Theorem reused_young_slot_advances_odometer :
    forall Before After NodeSize : nat,
      AllocationCounted Before After NodeSize ->
      After > Before.
  Proof.
    intros Before After NodeSize Hcounted.
    destruct Hcounted as [Hpositive Hafter].
    subst After.
    lia.
  Qed.

  Theorem fresh_bump_advances_odometer :
    forall Before After NodeSize : nat,
      AllocationCounted Before After NodeSize ->
      After > Before.
  Proof.
    intros Before After NodeSize Hcounted.
    apply reused_young_slot_advances_odometer with (NodeSize := NodeSize).
    exact Hcounted.
  Qed.

  Theorem promotion_reset_clears_budget_due :
    forall Budget After : nat,
      PromotionReset After ->
      ~ YoungBudgetDue Budget After.
  Proof.
    intros Budget After Hreset Hdue.
    unfold PromotionReset in Hreset.
    subst After.
    unfold YoungBudgetDue in Hdue.
    lia.
  Qed.

  Theorem budget_crossing_requests_minor :
    forall Budget Odometer : nat,
      Odometer > Budget ->
      YoungBudgetDue Budget Odometer.
  Proof.
    intros Budget Odometer Hover.
    unfold YoungBudgetDue.
    exact Hover.
  Qed.

  Theorem counted_allocation_preserves_or_creates_budget_due :
    forall Budget Before After NodeSize : nat,
      AllocationCounted Before After NodeSize ->
      YoungBudgetDue Budget Before \/ After > Budget ->
      YoungBudgetDue Budget After.
  Proof.
    intros Budget Before After NodeSize Hcounted Hcase.
    destruct Hcase as [Hbefore_due | Hafter_over].
    - unfold YoungBudgetDue in *.
      destruct Hcounted as [Hpositive Hafter].
      subst After.
      lia.
    - unfold YoungBudgetDue.
      exact Hafter_over.
  Qed.
End YoungAllocationOdometerModel.

End MeTTaTron_GC_YoungAllocationOdometer.
