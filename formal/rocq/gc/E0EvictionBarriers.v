(** E2 SATB E0 cache eviction and bulk-clear obligations.

    Value-bearing E0 caches can remove snapshot-live values through capacity
    eviction, same-key overwrite, or bulk clear.  The source-coupling harness
    pins each concrete cache path to expose the removed pre-image and shade it
    while the SATB phase gate is held.  This proof discharges the abstract
    safety shape consumed by the concurrent SATB theorem.
*)

Module MeTTaTron_GC_E0EvictionBarriers.

Section E0EvictionBarrierModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ FinalDriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition E0CacheRemovedPreimage
      (CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
      (a : Addr) : Prop :=
    CapacityVictim a \/ OverwriteVictim a \/ BulkClearedEntry a.

  Theorem e0_cache_removed_preimage_shaded :
    forall (CapacityVictim OverwriteVictim BulkClearedEntry
            ShadedDeletion : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      forall a,
        E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a ->
        ShadedDeletion a.
  Proof.
    intros CapacityVictim OverwriteVictim BulkClearedEntry ShadedDeletion
           Hcapacity Hoverwrite Hbulk a Hremoved.
    destruct Hremoved as [Hcapacity_a | [Hoverwrite_a | Hbulk_a]].
    - apply Hcapacity; exact Hcapacity_a.
    - apply Hoverwrite; exact Hoverwrite_a.
    - apply Hbulk; exact Hbulk_a.
  Qed.

  Theorem capacity_evicted_victim_is_satb_root :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            CapacityVictim : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      forall a,
        CapacityVictim a ->
        SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           CapacityVictim Hcapacity a Hvictim.
    right; right; left.
    apply Hcapacity.
    exact Hvictim.
  Qed.

  Theorem bulk_cleared_entry_is_satb_root :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            BulkClearedEntry : Addr -> Prop),
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      forall a,
        BulkClearedEntry a ->
        SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           BulkClearedEntry Hbulk a Hentry.
    right; right; left.
    apply Hbulk.
    exact Hentry.
  Qed.

  Theorem e0_cache_removed_preimage_is_satb_root :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      forall a,
        E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a ->
        SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           CapacityVictim OverwriteVictim BulkClearedEntry
           Hcapacity Hoverwrite Hbulk a Hremoved.
    right; right; left.
    apply (e0_cache_removed_preimage_shaded
             CapacityVictim OverwriteVictim BulkClearedEntry ShadedDeletion
             Hcapacity Hoverwrite Hbulk a).
    exact Hremoved.
  Qed.

  Theorem capacity_evicted_victim_survives_collection :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            CapacityVictim : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a,
          Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, CapacityVictim a -> ~ Freed a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           CapacityVictim Edge Marked Freed Hcapacity Hmark Hsweep a Hvictim Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (capacity_evicted_victim_is_satb_root
             InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
             CapacityVictim Hcapacity a).
    exact Hvictim.
  Qed.

  Theorem bulk_cleared_entry_survives_collection :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            BulkClearedEntry : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      (forall a,
          Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BulkClearedEntry a -> ~ Freed a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           BulkClearedEntry Edge Marked Freed Hbulk Hmark Hsweep a Hentry Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (bulk_cleared_entry_is_satb_root
             InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
             BulkClearedEntry Hbulk a).
    exact Hentry.
  Qed.

  Theorem e0_cache_removed_snapshot_live_survives_collection :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a) ->
      (forall a,
          Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           CapacityVictim OverwriteVictim BulkClearedEntry Edge Marked Freed SnapshotLive
           Hcapacity Hoverwrite Hbulk Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (e0_cache_removed_preimage_is_satb_root
             InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
             CapacityVictim OverwriteVictim BulkClearedEntry
             Hcapacity Hoverwrite Hbulk a).
    apply Hremoved.
    exact Hlive.
  Qed.
End E0EvictionBarrierModel.

End MeTTaTron_GC_E0EvictionBarriers.
