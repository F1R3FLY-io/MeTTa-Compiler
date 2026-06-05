(** E2 SATB E0 mutation-site obligations.

    The SATB proof for concurrent marking assumes every snapshot-live value
    removed from a value-bearing E0 substore is available to the marker as a
    shaded deletion pre-image.  The source-coupling harness pins the concrete
    categories that realize this obligation: space-local roots, rule-index
    roots, and environment/token/state roots.
*)

Module MeTTaTron_GC_E0MutationSites.

Section E0MutationSitesModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ FinalDriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition E0RemovedPreimage
      (SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
      (a : Addr) : Prop :=
    SpacePreimage a \/ RulePreimage a \/ EnvPreimage a.

  Theorem e0_removed_preimage_shaded :
    forall (SpacePreimage RulePreimage EnvPreimage ShadedDeletion : Addr -> Prop),
      (forall a, SpacePreimage a -> ShadedDeletion a) ->
      (forall a, RulePreimage a -> ShadedDeletion a) ->
      (forall a, EnvPreimage a -> ShadedDeletion a) ->
      forall a,
        E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a ->
        ShadedDeletion a.
  Proof.
    intros SpacePreimage RulePreimage EnvPreimage ShadedDeletion
           Hspace Hrule Henv a Hremoved.
    destruct Hremoved as [Hspace_a | [Hrule_a | Henv_a]].
    - apply Hspace; exact Hspace_a.
    - apply Hrule; exact Hrule_a.
    - apply Henv; exact Henv_a.
  Qed.

  Theorem e0_removed_preimage_is_satb_root :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            SpacePreimage RulePreimage EnvPreimage : Addr -> Prop),
      (forall a, SpacePreimage a -> ShadedDeletion a) ->
      (forall a, RulePreimage a -> ShadedDeletion a) ->
      (forall a, EnvPreimage a -> ShadedDeletion a) ->
      forall a,
        E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a ->
        SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           SpacePreimage RulePreimage EnvPreimage
           Hspace Hrule Henv a Hremoved.
    right; right; left.
    apply (e0_removed_preimage_shaded
             SpacePreimage RulePreimage EnvPreimage ShadedDeletion
             Hspace Hrule Henv a).
    exact Hremoved.
  Qed.

  Theorem e0_removed_snapshot_live_survives_collection :
    forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
            SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, SpacePreimage a -> ShadedDeletion a) ->
      (forall a, RulePreimage a -> ShadedDeletion a) ->
      (forall a, EnvPreimage a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a) ->
      (forall a,
          Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
           SpacePreimage RulePreimage EnvPreimage Edge Marked Freed SnapshotLive
           Hspace Hrule Henv Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (e0_removed_preimage_is_satb_root
             InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
             SpacePreimage RulePreimage EnvPreimage
             Hspace Hrule Henv a).
    apply Hremoved.
    exact Hlive.
  Qed.
End E0MutationSitesModel.

End MeTTaTron_GC_E0MutationSites.
