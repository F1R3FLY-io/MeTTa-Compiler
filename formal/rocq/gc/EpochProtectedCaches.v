(** Epoch-protected cache coherence for reused index addresses.

    The CESK index collector can recycle an Addr after sweep. A worker-local
    cache whose key or payload can encode that address is safe only if every
    lookup first observes the global sweep epoch and either keeps a current
    cache or removes old entries.  This file proves the shared shape used by
    VALUE_HASH_CACHE, MORK ground fragments, hash-cons, eval/match memo tables,
    and OPERATOR_CACHE.
*)

Module MeTTaTron_GC_EpochProtectedCaches.

Inductive CacheKind : Type :=
| ValueHash
| MorkGround
| HashCons
| EvalMemo
| MatchResult
| Operator.

Section EpochProtectedCacheModel.
  Variables Entry Epoch : Type.
  Variable Stale : Epoch -> Entry -> Prop.

  Definition ValidatePost
      (heap_epoch local_epoch : Epoch)
      (CacheBefore CacheAfter : Entry -> Prop) : Prop :=
    (local_epoch = heap_epoch /\ (forall e, CacheAfter e -> CacheBefore e)) \/
    (local_epoch <> heap_epoch /\
      (forall e, CacheAfter e -> CacheBefore e) /\
      (forall e, CacheAfter e -> ~ Stale heap_epoch e)).

  Definition ExplicitClearPost
      (heap_epoch cleared_local_epoch : Epoch)
      (CacheAfter : Entry -> Prop) : Prop :=
    (forall e, ~ CacheAfter e) /\ cleared_local_epoch = heap_epoch.

  Theorem lookup_after_validate_not_stale :
    forall (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      (local_epoch = heap_epoch ->
        forall e, CacheBefore e -> ~ Stale heap_epoch e) ->
      ValidatePost heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, Returned e -> ~ Stale heap_epoch e.
  Proof.
    intros heap_epoch local_epoch CacheBefore CacheAfter Returned
           Hcurrent_safe Hvalidated Hlookup e Hreturned.
    destruct Hvalidated as [[Hcurrent Hsubset] | [_ [_ Hnot_stale]]].
    - apply Hcurrent_safe.
      + exact Hcurrent.
      + apply Hsubset.
        apply Hlookup.
        exact Hreturned.
    - apply Hnot_stale.
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem stale_local_old_entries_miss_after_validate :
    forall (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      local_epoch <> heap_epoch ->
      (forall e, CacheBefore e -> Stale heap_epoch e) ->
      ValidatePost heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, ~ Returned e.
  Proof.
    intros heap_epoch local_epoch CacheBefore CacheAfter Returned
           Hstale_local Hold_entries_stale Hvalidated Hlookup e Hreturned.
    destruct Hvalidated as [[Hcurrent _] | [_ [Hsubset Hnot_stale]]].
    - apply Hstale_local.
      exact Hcurrent.
    - pose proof (Hlookup e Hreturned) as Hafter.
      pose proof (Hsubset e Hafter) as Hbefore.
      apply (Hnot_stale e Hafter).
      apply Hold_entries_stale.
      exact Hbefore.
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
End EpochProtectedCacheModel.

End MeTTaTron_GC_EpochProtectedCaches.
