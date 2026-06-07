(** D-RLOCK/B2 concurrent allocation fresh-only obligations.

    Concurrent allocation runs through shared [&self] bump paths.  It must not
    consume the free list: free-list reuse is reserved for the exclusive
    quiescent [&mut] path.  This prevents a concurrent bump claim from aliasing
    a slot that was reclaimed for reuse.
*)

Module MeTTaTron_GC_ConcurrentBumpFreshOnly.

Section ConcurrentBumpFreshOnlyModel.
  Variable Addr : Type.

  Definition ConcurrentFreshOnly
      (ConcurrentReturned Fresh : Addr -> Prop) : Prop :=
    forall a, ConcurrentReturned a -> Fresh a.

  Definition FreeListSeparated
      (Fresh OnFreeList : Addr -> Prop) : Prop :=
    forall a, Fresh a -> ~ OnFreeList a.

  Definition ReuseExclusiveOnly
      (ReuseReturned Exclusive : Addr -> Prop) : Prop :=
    forall a, ReuseReturned a -> Exclusive a.

  Theorem concurrent_allocation_never_returns_free_list_slot :
    forall (ConcurrentReturned Fresh OnFreeList : Addr -> Prop),
      ConcurrentFreshOnly ConcurrentReturned Fresh ->
      FreeListSeparated Fresh OnFreeList ->
      forall a, ConcurrentReturned a -> ~ OnFreeList a.
  Proof.
    intros ConcurrentReturned Fresh OnFreeList Hfresh Hseparated a Hreturned.
    apply Hseparated.
    apply Hfresh.
    exact Hreturned.
  Qed.

  Theorem reuse_return_requires_exclusive_path :
    forall (ReuseReturned Exclusive : Addr -> Prop),
      ReuseExclusiveOnly ReuseReturned Exclusive ->
      forall a, ReuseReturned a -> Exclusive a.
  Proof.
    intros ReuseReturned Exclusive Hexclusive a Hreuse.
    apply Hexclusive.
    exact Hreuse.
  Qed.

  Theorem concurrent_return_and_reuse_are_disjoint :
    forall (ConcurrentReturned ReuseReturned Fresh OnFreeList : Addr -> Prop),
      ConcurrentFreshOnly ConcurrentReturned Fresh ->
      FreeListSeparated Fresh OnFreeList ->
      (forall a, ReuseReturned a -> OnFreeList a) ->
      forall a, ConcurrentReturned a -> ~ ReuseReturned a.
  Proof.
    intros ConcurrentReturned ReuseReturned Fresh OnFreeList
           Hfresh Hseparated Hreuse_on_free a Hconcurrent Hreuse.
    pose proof (concurrent_allocation_never_returns_free_list_slot
                  ConcurrentReturned Fresh OnFreeList Hfresh Hseparated
                  a Hconcurrent) as Hnot_free.
    apply Hnot_free.
    apply Hreuse_on_free.
    exact Hreuse.
  Qed.
End ConcurrentBumpFreshOnlyModel.

End MeTTaTron_GC_ConcurrentBumpFreshOnly.
