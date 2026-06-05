(** Generic SATB deletion-barrier safety obligations for the E2 concurrent mark
    plan. *)

Module MeTTaTron_GC_SATB.

Section SATBModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Theorem deleted_preimage_is_satb_root :
    forall (InitialRoot ShadedDeletion AllocateBlack : Addr -> Prop) a,
      ShadedDeletion a -> SATBRoot InitialRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot ShadedDeletion AllocateBlack a Hshade.
    right; left; exact Hshade.
  Qed.

  Theorem allocate_black_is_satb_root :
    forall (InitialRoot ShadedDeletion AllocateBlack : Addr -> Prop) a,
      AllocateBlack a -> SATBRoot InitialRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot ShadedDeletion AllocateBlack a Hblack.
    right; right; exact Hblack.
  Qed.

  Theorem no_snapshot_live_uaf :
    forall (InitialRoot ShadedDeletion AllocateBlack : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, Reach (SATBRoot InitialRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, SnapshotLive a -> Reach (SATBRoot InitialRoot ShadedDeletion AllocateBlack) Edge a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot ShadedDeletion AllocateBlack Edge Marked Freed SnapshotLive
           Hmark Hsweep Hcovered a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hcovered.
    exact Hlive.
  Qed.
End SATBModel.

End MeTTaTron_GC_SATB.
