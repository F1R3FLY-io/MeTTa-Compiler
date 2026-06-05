(** Generic SATB deletion-barrier safety obligations for the E2 concurrent mark
    plan. *)

Module MeTTaTron_GC_SATB.

Section SATBModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Theorem driver_root_is_satb_root :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop) a,
      DriverRoot a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack a Hdriver.
    right; left; exact Hdriver.
  Qed.

  Theorem deleted_preimage_is_satb_root :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop) a,
      ShadedDeletion a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack a Hshade.
    right; right; left; exact Hshade.
  Qed.

  Theorem allocate_black_is_satb_root :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop) a,
      AllocateBlack a -> SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack a Hblack.
    right; right; right; exact Hblack.
  Qed.

  Theorem no_driver_root_uaf :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, DriverRoot a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked Freed
           Hmark Hsweep a Hdriver Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply driver_root_is_satb_root.
    exact Hdriver.
  Qed.

  Theorem no_snapshot_live_uaf :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, SnapshotLive a -> Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked Freed SnapshotLive
           Hmark Hsweep Hcovered a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hcovered.
    exact Hlive.
  Qed.
End SATBModel.

End MeTTaTron_GC_SATB.
