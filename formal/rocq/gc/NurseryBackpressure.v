(** C1.c nursery-backpressure trigger obligations.

    The generational index collector receives an allocator signal when a
    subsequent segment opens: [nursery_full_pending].  The driver must fold that
    signal into the minor trigger, and promotion must clear the signal and reset
    the young-allocation odometer so the same stale event cannot immediately
    re-fire a minor.
*)

Module MeTTaTron_GC_NurseryBackpressure.

Section NurseryBackpressureModel.
  Definition MinorDue (YoungOverBudget NurseryPending : Prop) : Prop :=
    YoungOverBudget \/ NurseryPending.

  Definition PromotionRelaxed
      (YoungOdometerReset NurseryPendingCleared : Prop) : Prop :=
    YoungOdometerReset /\ NurseryPendingCleared.

  Theorem nursery_open_signal_requests_minor :
    forall YoungOverBudget NurseryPending : Prop,
      NurseryPending ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending Hpending.
    right.
    exact Hpending.
  Qed.

  Theorem young_budget_requests_minor :
    forall YoungOverBudget NurseryPending : Prop,
      YoungOverBudget ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending Hover_budget.
    left.
    exact Hover_budget.
  Qed.

  Theorem promotion_clears_stale_nursery_trigger :
    forall YoungOverBudget NurseryPending
           YoungOdometerReset NurseryPendingCleared : Prop,
      ~ YoungOverBudget ->
      (NurseryPendingCleared -> ~ NurseryPending) ->
      PromotionRelaxed YoungOdometerReset NurseryPendingCleared ->
      ~ MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending YoungOdometerReset NurseryPendingCleared
           Hnot_over Hcleared_not_pending Hrelaxed Hminor.
    destruct Hrelaxed as [_ Hcleared].
    destruct Hminor as [Hover | Hpending].
    - apply Hnot_over. exact Hover.
    - apply (Hcleared_not_pending Hcleared). exact Hpending.
  Qed.

  Theorem stale_nursery_pending_refires_minor :
    forall YoungOverBudget NurseryPending : Prop,
      NurseryPending ->
      MinorDue YoungOverBudget NurseryPending.
  Proof.
    intros YoungOverBudget NurseryPending Hpending.
    right.
    exact Hpending.
  Qed.
End NurseryBackpressureModel.

End MeTTaTron_GC_NurseryBackpressure.
