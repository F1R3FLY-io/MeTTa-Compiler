(** E3 production JIT choice-point restore through an ABI spine owner.

    JIT-generated code requires [JitContext.choice_points] to be a contiguous
    repr(C) pointer. The production E3 carrier is therefore a verified owner:
    a [JitChoicePointSpineOwner] owns the contiguous buffer, [JitContext]
    receives only a transient pointer view, root walking bridges the live prefix
    to ContinuationAddr-backed nodes, and fail/restore reads the same owner slot
    before either advancing the alternative index or popping the live prefix.

    Source coupling pins the abstract premises to the Rust owner/reset/context
    construction sites and the runtime fail/root-walk mutation order.
*)

Module MeTTaTron_GC_JitChoicePointProductionRestore.

Section ProductionRestoreModel.
  Variable Slot Addr Value : Type.

  Variable OwnerSlotLive : Slot -> Prop.
  Variable PointerViewSlot : Slot -> Prop.
  Variable FutureTouch : Slot -> Value -> Prop.
  Variable BridgeHandle : Slot -> Addr -> Prop.
  Variable RootedByBridge : Addr -> Value -> Prop.
  Variable RestoresFromSlot : Slot -> Prop.
  Variable SlotLiveAfterPop : Slot -> Prop.

  Definition PointerViewCoversOwnerLivePrefix : Prop :=
    forall s,
      OwnerSlotLive s ->
      PointerViewSlot s.

  Definition BridgeWalkComplete : Prop :=
    forall s a v,
      OwnerSlotLive s ->
      PointerViewSlot s ->
      BridgeHandle s a ->
      FutureTouch s v ->
      RootedByBridge a v.

  Definition RestoreUsesOwnerSlot : Prop :=
    forall s,
      OwnerSlotLive s ->
      PointerViewSlot s ->
      RestoresFromSlot s.

  Definition PopRemovesLivePrefixSlot : Prop :=
    forall s,
      OwnerSlotLive s ->
      RestoresFromSlot s ->
      ~ SlotLiveAfterPop s.

  Theorem production_jit_choice_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      PointerViewCoversOwnerLivePrefix ->
      BridgeWalkComplete ->
      (forall s, OwnerSlotLive s -> exists a, BridgeHandle s a) ->
      (forall a v, RootedByBridge a v -> RootedValue v) ->
      forall s v,
        OwnerSlotLive s ->
        FutureTouch s v ->
        RootedValue v.
  Proof.
    intros RootedValue Hview Hbridge Hhandle Hroot s v Hlive Hfuture.
    destruct (Hhandle s Hlive) as [a Haddr].
    apply (Hroot a v).
    apply (Hbridge s a v Hlive (Hview s Hlive) Haddr Hfuture).
  Qed.

  Theorem production_jit_choice_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      PointerViewCoversOwnerLivePrefix ->
      BridgeWalkComplete ->
      (forall s, OwnerSlotLive s -> exists a, BridgeHandle s a) ->
      (forall a v, RootedByBridge a v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall s v,
        OwnerSlotLive s ->
        FutureTouch s v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hview Hbridge Hhandle Hroot Hfreed
           s v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (production_jit_choice_roots_future_touch
             RootedValue Hview Hbridge Hhandle Hroot s v Hlive Hfuture).
  Qed.

  Theorem production_jit_choice_restore_reads_owner_slot :
    PointerViewCoversOwnerLivePrefix ->
    RestoreUsesOwnerSlot ->
    forall s,
      OwnerSlotLive s ->
      RestoresFromSlot s.
  Proof.
    intros Hview Hrestore s Hlive.
    apply (Hrestore s Hlive (Hview s Hlive)).
  Qed.

  Theorem production_jit_choice_pop_removes_stale_live_slot :
    PointerViewCoversOwnerLivePrefix ->
    RestoreUsesOwnerSlot ->
    PopRemovesLivePrefixSlot ->
    forall s,
      OwnerSlotLive s ->
      ~ SlotLiveAfterPop s.
  Proof.
    intros Hview Hrestore Hpop s Hlive.
    apply (Hpop s Hlive).
    apply (production_jit_choice_restore_reads_owner_slot
             Hview Hrestore s Hlive).
  Qed.

  Theorem production_jit_choice_root_then_restore_safe :
    forall (RootedValue FreedValue : Value -> Prop),
      PointerViewCoversOwnerLivePrefix ->
      BridgeWalkComplete ->
      RestoreUsesOwnerSlot ->
      PopRemovesLivePrefixSlot ->
      (forall s, OwnerSlotLive s -> exists a, BridgeHandle s a) ->
      (forall a v, RootedByBridge a v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall s v,
        OwnerSlotLive s ->
        FutureTouch s v ->
        exists a,
          BridgeHandle s a /\
          RootedValue v /\
          RestoresFromSlot s /\
          ~ SlotLiveAfterPop s /\
          ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hview Hbridge Hrestore Hpop
           Hhandle Hroot Hfreed s v Hlive Hfuture.
    destruct (Hhandle s Hlive) as [a Haddr].
    exists a.
    repeat split.
    - exact Haddr.
    - apply (Hroot a v).
      apply (Hbridge s a v Hlive (Hview s Hlive) Haddr Hfuture).
    - apply (Hrestore s Hlive (Hview s Hlive)).
    - apply (Hpop s Hlive).
      apply (Hrestore s Hlive (Hview s Hlive)).
    - intros Hfreed_v.
      apply (Hfreed v Hfreed_v).
      apply (Hroot a v).
      apply (Hbridge s a v Hlive (Hview s Hlive) Haddr Hfuture).
  Qed.

End ProductionRestoreModel.

End MeTTaTron_GC_JitChoicePointProductionRestore.
