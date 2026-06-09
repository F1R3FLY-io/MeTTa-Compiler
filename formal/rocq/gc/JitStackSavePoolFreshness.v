(** E3 JIT stack-save-pool freshness.

    A live JIT choice point stores a stack-save-pool slot index. If allocation
    wraps while that choice point is still live, a later fork can overwrite the
    saved stack values that future backtracking will restore and that the GC
    must root. The implementation therefore uses each stack-save-pool slot at
    most once per JIT execution and falls back to bailout when no fresh slot is
    available.
*)

From Stdlib Require Import Arith Lia.

Module MeTTaTron_GC_JitStackSavePoolFreshness.

Section JitStackSavePoolFreshnessModel.
  Variable Choice Value : Type.

  Variable pool_size : nat.
  Variable LiveChoice : Choice -> Prop.
  Variable ChoiceSlot : Choice -> nat -> Prop.
  Variable PoolSlotValue : nat -> Value -> Prop.
  Variable RootedValue : Value -> Prop.

  Definition LiveSlotsBelowNext (next : nat) : Prop :=
    forall cp slot,
      LiveChoice cp ->
      ChoiceSlot cp slot ->
      slot < next.

  Definition FreshPoolAlloc (next slot next' : nat) : Prop :=
    slot = next /\ next < pool_size /\ next' = S next.

  Definition PoolWalkerComplete : Prop :=
    forall cp slot v,
      LiveChoice cp ->
      ChoiceSlot cp slot ->
      PoolSlotValue slot v ->
      RootedValue v.

  Theorem fresh_alloc_does_not_overwrite_live_choice_slot :
    forall next slot next' cp live_slot,
      LiveSlotsBelowNext next ->
      FreshPoolAlloc next slot next' ->
      LiveChoice cp ->
      ChoiceSlot cp live_slot ->
      live_slot <> slot.
  Proof.
    intros next slot next' cp live_slot Hbelow Halloc Hlive Hslot Heq.
    unfold FreshPoolAlloc in Halloc.
    destruct Halloc as [Hslot_eq [_ _]].
    subst slot.
    pose proof (Hbelow cp live_slot Hlive Hslot) as Hlt.
    lia.
  Qed.

  Theorem fresh_alloc_preserves_previous_live_pool_roots :
    forall next slot next' cp live_slot v,
      LiveSlotsBelowNext next ->
      FreshPoolAlloc next slot next' ->
      PoolWalkerComplete ->
      LiveChoice cp ->
      ChoiceSlot cp live_slot ->
      PoolSlotValue live_slot v ->
      RootedValue v.
  Proof.
    intros next slot next' cp live_slot v _ _ Hcomplete Hlive Hchoice Hvalue.
    apply (Hcomplete cp live_slot v Hlive Hchoice Hvalue).
  Qed.

  Theorem exhausted_pool_bails_before_choice_publication :
    forall (NeedsSavedStack Bailout PublishedChoicePoint : Prop) next,
      NeedsSavedStack ->
      pool_size <= next ->
      (NeedsSavedStack -> pool_size <= next -> Bailout /\ ~ PublishedChoicePoint) ->
      Bailout /\ ~ PublishedChoicePoint.
  Proof.
    intros NeedsSavedStack Bailout PublishedChoicePoint next Hneeds Hexhausted Himpl.
    apply Himpl; assumption.
  Qed.

  Theorem non_wrapping_allocation_or_bailout :
    forall (NeedsSavedStack Bailout PublishedChoicePoint : Prop) next slot next',
      NeedsSavedStack ->
      (next < pool_size -> FreshPoolAlloc next slot next') ->
      (pool_size <= next -> Bailout /\ ~ PublishedChoicePoint) ->
      (FreshPoolAlloc next slot next') \/ (Bailout /\ ~ PublishedChoicePoint).
  Proof.
    intros NeedsSavedStack Bailout PublishedChoicePoint next slot next'
           _ Hfresh Hbailout.
    destruct (Nat.lt_ge_cases next pool_size) as [Hlt | Hge].
    - left. apply Hfresh. exact Hlt.
    - right. apply Hbailout. exact Hge.
  Qed.
End JitStackSavePoolFreshnessModel.

End MeTTaTron_GC_JitStackSavePoolFreshness.
