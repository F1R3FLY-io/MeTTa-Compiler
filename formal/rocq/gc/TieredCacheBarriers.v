(** Tiered compilation cache rooting and SATB deletion obligations.

    The global tiered compilation cache is persistent E0.  It contributes two
    index-GC root classes: source expressions held by pending bytecode compile
    tasks and constants reachable from ready bytecode chunks.  Removed pending
    roots and cleared compiled constants must be shaded during an active E2
    SATB mark before they become unreachable from the cache.

    Pending root guards also carry an ownership token.  A guard may unregister
    only the map entry carrying its token, so an older guard cannot remove a
    newer pending root registered for the same expression hash.
*)

Module MeTTaTron_GC_TieredCacheBarriers.

Section TieredCacheBarrierModel.
  Variables Addr Entry Token : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition ConcurrentCollectorRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition TieredCacheRegisteredValue
      (PendingRoot CompiledConstant : Addr -> Prop)
      (a : Addr) : Prop :=
    PendingRoot a \/ CompiledConstant a.

  Definition TieredCacheRemovedValue
      (PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
       ClearPendingVictim ClearCompiledConstant : Addr -> Prop)
      (a : Addr) : Prop :=
    PendingOverwriteVictim a \/
    PendingCancelVictim a \/
    PendingGuardDropVictim a \/
    ClearPendingVictim a \/
    ClearCompiledConstant a.

  Definition TokenCheckedRemove
      (EntryToken : Entry -> Token)
      (guard : Token)
      (entry : Entry) : Prop :=
    EntryToken entry = guard.

  Theorem old_pending_guard_cannot_remove_newer_root :
    forall (EntryToken : Entry -> Token)
           (old_entry new_entry : Entry)
           (old_guard : Token),
      EntryToken old_entry = old_guard ->
      EntryToken new_entry <> old_guard ->
      ~ TokenCheckedRemove EntryToken old_guard new_entry.
  Proof.
    intros EntryToken old_entry new_entry old_guard Hold Hnew Hremove.
    unfold TokenCheckedRemove in Hremove.
    apply Hnew.
    exact Hremove.
  Qed.

  Theorem tiered_cache_value_is_structural_root :
    forall (PendingRoot CompiledConstant PendingScanned CompiledScanned
            StructuralRoot : Addr -> Prop),
      (forall a, PendingRoot a -> PendingScanned a) ->
      (forall a, PendingScanned a -> StructuralRoot a) ->
      (forall a, CompiledConstant a -> CompiledScanned a) ->
      (forall a, CompiledScanned a -> StructuralRoot a) ->
      forall a,
        TieredCacheRegisteredValue PendingRoot CompiledConstant a ->
        StructuralRoot a.
  Proof.
    intros PendingRoot CompiledConstant PendingScanned CompiledScanned StructuralRoot
           Hscan_pending Hroot_pending Hscan_compiled Hroot_compiled a Hregistered.
    destruct Hregistered as [Hpending | Hcompiled].
    - apply Hroot_pending.
      apply Hscan_pending.
      exact Hpending.
    - apply Hroot_compiled.
      apply Hscan_compiled.
      exact Hcompiled.
  Qed.

  Theorem tiered_cache_value_survives_collection :
    forall (PendingRoot CompiledConstant PendingScanned CompiledScanned
            StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, PendingRoot a -> PendingScanned a) ->
      (forall a, PendingScanned a -> StructuralRoot a) ->
      (forall a, CompiledConstant a -> CompiledScanned a) ->
      (forall a, CompiledScanned a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        TieredCacheRegisteredValue PendingRoot CompiledConstant a ->
        ~ Freed a.
  Proof.
    intros PendingRoot CompiledConstant PendingScanned CompiledScanned
           StructuralRoot Marked Freed Edge Hscan_pending Hroot_pending
           Hscan_compiled Hroot_compiled Hmark Hsweep a Hregistered Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    destruct Hregistered as [Hpending | Hcompiled].
    - apply Hroot_pending.
      apply Hscan_pending.
      exact Hpending.
    - apply Hroot_compiled.
      apply Hscan_compiled.
      exact Hcompiled.
  Qed.

  Theorem removed_tiered_cache_value_shaded :
    forall (PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
            ClearPendingVictim ClearCompiledConstant ShadedDeletion : Addr -> Prop),
      (forall a, PendingOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, PendingCancelVictim a -> ShadedDeletion a) ->
      (forall a, PendingGuardDropVictim a -> ShadedDeletion a) ->
      (forall a, ClearPendingVictim a -> ShadedDeletion a) ->
      (forall a, ClearCompiledConstant a -> ShadedDeletion a) ->
      forall a,
        TieredCacheRemovedValue
          PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
          ClearPendingVictim ClearCompiledConstant a ->
        ShadedDeletion a.
  Proof.
    intros PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
           ClearPendingVictim ClearCompiledConstant ShadedDeletion
           Hoverwrite Hcancel Hguard Hclear_pending Hclear_compiled a Hremoved.
    destruct Hremoved as
        [Hoverwrite_a | [Hcancel_a | [Hguard_a | [Hclear_pending_a | Hclear_compiled_a]]]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hcancel. exact Hcancel_a.
    - apply Hguard. exact Hguard_a.
    - apply Hclear_pending. exact Hclear_pending_a.
    - apply Hclear_compiled. exact Hclear_compiled_a.
  Qed.

  Theorem removed_tiered_cache_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
            ClearPendingVictim ClearCompiledConstant SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, PendingOverwriteVictim a -> ShadedDeletion a) ->
      (forall a, PendingCancelVictim a -> ShadedDeletion a) ->
      (forall a, PendingGuardDropVictim a -> ShadedDeletion a) ->
      (forall a, ClearPendingVictim a -> ShadedDeletion a) ->
      (forall a, ClearCompiledConstant a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          TieredCacheRemovedValue
            PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
            ClearPendingVictim ClearCompiledConstant a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           PendingOverwriteVictim PendingCancelVictim PendingGuardDropVictim
           ClearPendingVictim ClearCompiledConstant SnapshotLive Edge Marked Freed
           Hoverwrite Hcancel Hguard Hclear_pending Hclear_compiled Hremoved Hmark Hsweep
           a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as
        [Hoverwrite_a | [Hcancel_a | [Hguard_a | [Hclear_pending_a | Hclear_compiled_a]]]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hcancel. exact Hcancel_a.
    - apply Hguard. exact Hguard_a.
    - apply Hclear_pending. exact Hclear_pending_a.
    - apply Hclear_compiled. exact Hclear_compiled_a.
  Qed.
End TieredCacheBarrierModel.

End MeTTaTron_GC_TieredCacheBarriers.
