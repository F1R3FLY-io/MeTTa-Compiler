(** Hash-cons sweep-retain obligation for the index heap.

    The ground-SExpr hash-cons table stores arena handles. A later lookup may
    return one of those handles, so sweep must drop entries whose address is
    about to be reclaimed. Major sweep retains only marked entries. Minor sweep
    retains old entries unconditionally and young entries only when marked,
    matching the young-only sweep range.
*)

Module MeTTaTron_GC_HashConsSweepRetain.

Section HashConsSweepRetainModel.
  Variable Addr : Type.

  Theorem major_hash_cons_hit_not_freed :
    forall (Retained Marked Freed Returned : Addr -> Prop),
      (forall a, Returned a -> Retained a) ->
      (forall a, Retained a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Returned a -> ~ Freed a.
  Proof.
    intros Retained Marked Freed Returned Hreturned_retained Hretained_marked
           Hsweep a Hreturned Hfreed.
    apply (Hsweep a Hfreed).
    apply Hretained_marked.
    apply Hreturned_retained.
    exact Hreturned.
  Qed.

  Theorem minor_hash_cons_hit_not_freed :
    forall (Retained Young Marked Freed Returned : Addr -> Prop),
      (forall a, Returned a -> Retained a) ->
      (forall a, Retained a -> ~ Young a \/ Marked a) ->
      (forall a, Freed a -> Young a /\ ~ Marked a) ->
      forall a, Returned a -> ~ Freed a.
  Proof.
    intros Retained Young Marked Freed Returned Hreturned_retained
           Hretained_old_or_marked Hminor_frees a Hreturned Hfreed.
    destruct (Hretained_old_or_marked a (Hreturned_retained a Hreturned))
      as [Hold | Hmarked].
    - destruct (Hminor_frees a Hfreed) as [Hyoung _].
      apply Hold.
      exact Hyoung.
    - destruct (Hminor_frees a Hfreed) as [_ Hunmarked].
      apply Hunmarked.
      exact Hmarked.
  Qed.
End HashConsSweepRetainModel.

End MeTTaTron_GC_HashConsSweepRetain.
