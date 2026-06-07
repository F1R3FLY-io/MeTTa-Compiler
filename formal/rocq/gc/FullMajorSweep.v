(** E2 SATB full-major sweep mark-lifecycle obligations.

    The first concurrent SATB path intentionally finishes with a full major
    sweep.  SATB deletion barriers can mark old addresses; a young-only sweep
    would leave those old mark bits stale for a later minor.  These theorems
    capture the abstract obligation pinned by source coupling: every address
    that SATB may mark is in the full-major swept range, and every swept address
    has its mark cleared before the next cycle.
*)

Module MeTTaTron_GC_FullMajorSweep.

Section FullMajorSweepModel.
  Variable Addr : Type.

  Theorem full_major_clears_all_satb_marks :
    forall (SATBMarked Swept Cleared : Addr -> Prop),
      (forall a, SATBMarked a -> Swept a) ->
      (forall a, Swept a -> Cleared a) ->
      forall a, SATBMarked a -> Cleared a.
  Proof.
    intros SATBMarked Swept Cleared Hsatb_swept Hswept_cleared a Hmarked.
    apply Hswept_cleared.
    apply Hsatb_swept.
    exact Hmarked.
  Qed.

  Theorem full_major_leaves_no_stale_mark :
    forall (Swept MarkedAfter : Addr -> Prop),
      (forall a, Swept a) ->
      (forall a, Swept a -> ~ MarkedAfter a) ->
      forall a, ~ MarkedAfter a.
  Proof.
    intros Swept MarkedAfter Hfull Hcleared a Hmarked_after.
    apply (Hcleared a).
    - apply Hfull.
    - exact Hmarked_after.
  Qed.

  Theorem full_major_then_promotion_preserves_no_stale_mark :
    forall (Swept MarkedAfterSweep MarkedAfterPromotion : Addr -> Prop),
      (forall a, Swept a) ->
      (forall a, Swept a -> ~ MarkedAfterSweep a) ->
      (forall a, MarkedAfterPromotion a -> MarkedAfterSweep a) ->
      forall a, ~ MarkedAfterPromotion a.
  Proof.
    intros Swept MarkedAfterSweep MarkedAfterPromotion
           Hfull Hcleared Hpromotion_no_set a Hmarked_after_promotion.
    apply (Hcleared a).
    - apply Hfull.
    - apply Hpromotion_no_set.
      exact Hmarked_after_promotion.
  Qed.

  Theorem full_major_satb_marks_not_stale_after_promotion :
    forall (SATBMarked Swept MarkedAfterPromotion : Addr -> Prop),
      (forall a, SATBMarked a -> Swept a) ->
      (forall a, Swept a -> ~ MarkedAfterPromotion a) ->
      forall a, SATBMarked a -> ~ MarkedAfterPromotion a.
  Proof.
    intros SATBMarked Swept MarkedAfterPromotion
           Hsatb_swept Hcleared a Hsatb Hmarked_after.
    apply (Hcleared a).
    - apply Hsatb_swept.
      exact Hsatb.
    - exact Hmarked_after.
  Qed.

  Theorem young_only_sweep_safe_requires_no_surviving_old_satb_mark :
    forall (SATBMarked Old MarkedAfter : Addr -> Prop),
      (forall a, SATBMarked a -> Old a -> MarkedAfter a) ->
      (forall a, ~ MarkedAfter a) ->
      forall a, SATBMarked a -> Old a -> False.
  Proof.
    intros SATBMarked Old MarkedAfter Hold_survives Hno_stale a Hsatb Hold.
    apply (Hno_stale a).
    apply Hold_survives; assumption.
  Qed.

  Theorem young_only_sweep_with_old_satb_mark_leaves_stale_mark :
    forall (SATBMarked Old Cleared MarkedAfter : Addr -> Prop),
      (exists a, SATBMarked a /\ Old a) ->
      (forall a, SATBMarked a -> Old a -> MarkedAfter a) ->
      (forall a, Cleared a -> ~ MarkedAfter a) ->
      exists a, SATBMarked a /\ Old a /\ ~ Cleared a.
  Proof.
    intros SATBMarked Old Cleared MarkedAfter Hold_exists Hold_survives Hcleared_not_after.
    destruct Hold_exists as [a [Hsatb Hold]].
    exists a. split; [exact Hsatb |].
    split; [exact Hold |].
    intro Hcleared.
    apply (Hcleared_not_after a Hcleared).
    apply Hold_survives; assumption.
  Qed.
End FullMajorSweepModel.

End MeTTaTron_GC_FullMajorSweep.
