(** E2 allocate-black publication obligations.

    During concurrent SATB marking, a freshly allocated slot must be marked
    before publication makes it visible to the marker/sweeper.  The source
    coupling pins that order in IndexArena; this proof discharges the abstract
    safety shape used by the SATB theorem.
*)

Module MeTTaTron_GC_AllocateBlack.

Section AllocateBlackModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Theorem published_alloc_is_satb_root :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PublishedAlloc : Addr -> Prop),
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      forall a,
        PublishedAlloc a ->
        SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc
           Hpublished_black a Hpublished.
    right; right; right.
    apply Hpublished_black.
    exact Hpublished.
  Qed.

  Theorem published_alloc_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PublishedAlloc : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      (forall a,
          Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, PublishedAlloc a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc
           Edge Marked Freed Hpublished_black Hmark Hsweep a Hpublished Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (published_alloc_is_satb_root
             InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc
             Hpublished_black a).
    exact Hpublished.
  Qed.

  Theorem mark_before_publish_survives_direct_sweep :
    forall (PublishedAlloc Marked Freed : Addr -> Prop),
      (forall a, PublishedAlloc a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, PublishedAlloc a -> ~ Freed a.
  Proof.
    intros PublishedAlloc Marked Freed Hmarked Hsweep a Hpublished Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmarked.
    exact Hpublished.
  Qed.
End AllocateBlackModel.

End MeTTaTron_GC_AllocateBlack.
