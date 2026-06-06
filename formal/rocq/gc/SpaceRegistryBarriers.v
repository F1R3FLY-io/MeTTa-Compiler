(** Global space-registry rooting and SATB deletion obligations.

    `GLOBAL_SPACE_REGISTRY` is part of persistent E0.  Registered spaces are
    read structurally by `collect_global_anchors` through
    `collect_all_gc_values`.  During E2 SATB marking, replaced, removed, and
    bulk-cleared `SpaceHandle`s must shade the values reachable from their old
    handles while the SATB phase gate is held.  This proof captures those two
    source-coupled shapes.
*)

Module MeTTaTron_GC_SpaceRegistryBarriers.

Section SpaceRegistryBarrierModel.
  Variables Addr Space : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition ConcurrentCollectorRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition SpaceRegistryRemovedValue
      (OverwriteVictim RemoveVictim ClearVictim : Addr -> Prop)
      (a : Addr) : Prop :=
    OverwriteVictim a \/ RemoveVictim a \/ ClearVictim a.

  Theorem registered_space_value_is_structural_root :
    forall (Registered Scanned : Space -> Prop)
           (SpaceValue : Space -> Addr -> Prop)
           (StructuralRoot : Addr -> Prop),
      (forall s, Registered s -> Scanned s) ->
      (forall s a, Scanned s -> SpaceValue s a -> StructuralRoot a) ->
      forall s a,
        Registered s ->
        SpaceValue s a ->
        StructuralRoot a.
  Proof.
    intros Registered Scanned SpaceValue StructuralRoot Hscan Hroot s a Hregistered Hvalue.
    apply (Hroot s a).
    - apply Hscan.
      exact Hregistered.
    - exact Hvalue.
  Qed.

  Theorem registered_space_value_survives_collection :
    forall (Registered Scanned : Space -> Prop)
           (SpaceValue : Space -> Addr -> Prop)
           (StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall s, Registered s -> Scanned s) ->
      (forall s a, Scanned s -> SpaceValue s a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall s a,
        Registered s ->
        SpaceValue s a ->
        ~ Freed a.
  Proof.
    intros Registered Scanned SpaceValue StructuralRoot Marked Freed Edge
           Hscan Hroot Hmark Hsweep s a Hregistered Hvalue Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    eapply registered_space_value_is_structural_root; eauto.
  Qed.

  Theorem removed_space_registry_value_shaded :
    forall (OverwriteVictim RemoveVictim ClearVictim
            ShadedDeletion : Addr -> Prop),
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, RemoveVictim a -> ShadedDeletion a) ->
      (forall a, ClearVictim a -> ShadedDeletion a) ->
      forall a,
        SpaceRegistryRemovedValue OverwriteVictim RemoveVictim ClearVictim a ->
        ShadedDeletion a.
  Proof.
    intros OverwriteVictim RemoveVictim ClearVictim ShadedDeletion
           Hoverwrite Hremove Hclear a Hremoved.
    destruct Hremoved as [Hoverwrite_a | [Hremove_a | Hclear_a]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hremove. exact Hremove_a.
    - apply Hclear. exact Hclear_a.
  Qed.

  Theorem removed_space_registry_value_survives_satb_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            OverwriteVictim RemoveVictim ClearVictim SnapshotLive : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, RemoveVictim a -> ShadedDeletion a) ->
      (forall a, ClearVictim a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          SpaceRegistryRemovedValue OverwriteVictim RemoveVictim ClearVictim a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SnapshotLive a ->
        ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           OverwriteVictim RemoveVictim ClearVictim SnapshotLive Edge Marked Freed
           Hoverwrite Hremove Hclear Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hoverwrite_a | [Hremove_a | Hclear_a]].
    - apply Hoverwrite. exact Hoverwrite_a.
    - apply Hremove. exact Hremove_a.
    - apply Hclear. exact Hclear_a.
  Qed.
End SpaceRegistryBarrierModel.

End MeTTaTron_GC_SpaceRegistryBarriers.
