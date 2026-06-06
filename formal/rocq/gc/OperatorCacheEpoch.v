(** Operator-cache sweep-epoch coherence obligation.

    The trampoline operator cache is keyed partly by an interned atom pointer.
    Under the index collector, a sweep can recycle addresses while another
    worker still owns a thread-local cache entry.  The live source therefore
    checks `gc_sweep_epoch` before every operator-cache lookup, clears the cache
    when the local epoch is stale, and records the current epoch after explicit
    clears.  This proof captures the abstract cache-coherence shape behind that
    source coupling.
*)

Module MeTTaTron_GC_OperatorCacheEpoch.

Section OperatorCacheEpochModel.
  Variables Entry Epoch : Type.
  Variable entry_epoch : Entry -> Epoch.

  Definition EnsurePost
      (heap_epoch local_epoch : Epoch)
      (CacheBefore CacheAfter : Entry -> Prop) : Prop :=
    (local_epoch = heap_epoch /\ (forall e, CacheAfter e -> CacheBefore e)) \/
    (local_epoch <> heap_epoch /\ (forall e, ~ CacheAfter e)).

  Definition ExplicitClearPost
      (heap_epoch cleared_local_epoch : Epoch)
      (CacheAfter : Entry -> Prop) : Prop :=
    (forall e, ~ CacheAfter e) /\ cleared_local_epoch = heap_epoch.

  Theorem returned_entry_has_current_epoch_after_ensure :
    forall (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      (forall e, CacheBefore e -> entry_epoch e = local_epoch) ->
      EnsurePost heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, Returned e -> entry_epoch e = heap_epoch.
  Proof.
    intros heap_epoch local_epoch CacheBefore CacheAfter Returned
           Hstamped Hensure Hlookup e Hreturned.
    destruct Hensure as [[Hcurrent Hpreserved] | [_ Hcleared]].
    - rewrite (Hstamped e (Hpreserved e (Hlookup e Hreturned))).
      exact Hcurrent.
    - exfalso.
      apply (Hcleared e).
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem stale_epoch_lookup_misses_after_ensure :
    forall (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      local_epoch <> heap_epoch ->
      EnsurePost heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, ~ Returned e.
  Proof.
    intros heap_epoch local_epoch CacheBefore CacheAfter Returned
           Hstale Hensure Hlookup e Hreturned.
    destruct Hensure as [[Hcurrent _] | [_ Hcleared]].
    - apply Hstale.
      exact Hcurrent.
    - apply (Hcleared e).
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem explicit_clear_leaves_empty_current_cache :
    forall (heap_epoch cleared_local_epoch : Epoch)
           (CacheAfter : Entry -> Prop),
      ExplicitClearPost heap_epoch cleared_local_epoch CacheAfter ->
      cleared_local_epoch = heap_epoch /\ forall e, ~ CacheAfter e.
  Proof.
    intros heap_epoch cleared_local_epoch CacheAfter Hclear.
    destruct Hclear as [Hempty Hcurrent].
    split.
    - exact Hcurrent.
    - exact Hempty.
  Qed.

  Theorem explicit_clear_lookup_misses :
    forall (heap_epoch cleared_local_epoch : Epoch)
           (CacheAfter Returned : Entry -> Prop),
      ExplicitClearPost heap_epoch cleared_local_epoch CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, ~ Returned e.
  Proof.
    intros heap_epoch cleared_local_epoch CacheAfter Returned Hclear Hlookup e Hreturned.
    destruct Hclear as [Hempty _].
    apply (Hempty e).
    apply Hlookup.
    exact Hreturned.
  Qed.
End OperatorCacheEpochModel.

End MeTTaTron_GC_OperatorCacheEpoch.
