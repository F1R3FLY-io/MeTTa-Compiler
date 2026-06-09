(** E1 allocator progress under concurrent reuse pressure.

    The ordinary shared allocation path remains fresh-bump-only and never
    consumes the free list.  When a collection has produced a reusable slot in
    the current bump segment, however, the factory must not keep choosing the
    shared fresh-bump path merely because [try_write] lost to readers.  It must
    enter the exclusive allocation regime, where the existing free-list-reuse
    proof applies.
*)

Module MeTTaTron_GC_ConcurrentReusePressureProgress.

Section ConcurrentReusePressureProgressModel.
  Variable Addr : Type.

  Definition ReusePressure
      (OnFreeList CurrentSegment : Addr -> Prop) : Prop :=
    exists a, OnFreeList a /\ CurrentSegment a.

  Definition ExclusiveReuseProgress
      (OnFreeList CurrentSegment ReuseReturned : Addr -> Prop) : Prop :=
    ReusePressure OnFreeList CurrentSegment ->
      exists a, ReuseReturned a /\ OnFreeList a /\ CurrentSegment a.

  Definition FactoryPolicy
      (TryWriteWon ReusePressureObserved ChoseExclusive ChoseConcurrent : Prop)
      : Prop :=
    (TryWriteWon -> ChoseExclusive) /\
    (~ TryWriteWon -> ReusePressureObserved -> ChoseExclusive) /\
    (~ TryWriteWon -> ~ ReusePressureObserved -> ChoseConcurrent) /\
    (ChoseExclusive -> ~ ChoseConcurrent).

  Theorem try_write_loss_with_reuse_pressure_chooses_exclusive :
    forall TryWriteWon ReusePressureObserved ChoseExclusive ChoseConcurrent,
      FactoryPolicy TryWriteWon ReusePressureObserved ChoseExclusive
        ChoseConcurrent ->
      ~ TryWriteWon ->
      ReusePressureObserved ->
      ChoseExclusive /\ ~ ChoseConcurrent.
  Proof.
    intros TryWriteWon ReusePressureObserved ChoseExclusive ChoseConcurrent
           Hpolicy Hlost Hpressure.
    destruct Hpolicy as [_ [Hpressure_policy [_ Hexclusive_disjoint]]].
    split.
    - apply Hpressure_policy.
      + exact Hlost.
      + exact Hpressure.
    - apply Hexclusive_disjoint.
      apply Hpressure_policy.
      + exact Hlost.
      + exact Hpressure.
  Qed.

  Theorem exclusive_pressure_progress_returns_current_free_slot :
    forall (OnFreeList CurrentSegment ReuseReturned : Addr -> Prop),
      ExclusiveReuseProgress OnFreeList CurrentSegment ReuseReturned ->
      ReusePressure OnFreeList CurrentSegment ->
      exists a, ReuseReturned a /\ OnFreeList a /\ CurrentSegment a.
  Proof.
    intros OnFreeList CurrentSegment ReuseReturned Hprogress Hpressure.
    apply Hprogress.
    exact Hpressure.
  Qed.

  Theorem pressure_policy_preserves_concurrent_fresh_only :
    forall (ConcurrentReturned Fresh OnFreeList : Addr -> Prop)
           (ChoseConcurrent : Prop),
      (ChoseConcurrent ->
        forall a, ConcurrentReturned a -> Fresh a) ->
      (forall a, Fresh a -> ~ OnFreeList a) ->
      ChoseConcurrent ->
      forall a, ConcurrentReturned a -> ~ OnFreeList a.
  Proof.
    intros ConcurrentReturned Fresh OnFreeList ChoseConcurrent
           Hfresh Hseparated Hconcurrent a Hreturned.
    apply Hseparated.
    apply Hfresh.
    - exact Hconcurrent.
    - exact Hreturned.
  Qed.

  Theorem pressure_path_reuses_without_weakening_concurrent_safety :
    forall (OnFreeList CurrentSegment ReuseReturned : Addr -> Prop)
           (TryWriteWon ReusePressureObserved ChoseExclusive ChoseConcurrent :
              Prop),
      FactoryPolicy TryWriteWon ReusePressureObserved ChoseExclusive
        ChoseConcurrent ->
      ExclusiveReuseProgress OnFreeList CurrentSegment ReuseReturned ->
      (ReusePressureObserved -> ReusePressure OnFreeList CurrentSegment) ->
      ~ TryWriteWon ->
      ReusePressureObserved ->
      (exists a, ReuseReturned a /\ OnFreeList a /\ CurrentSegment a) /\
      ~ ChoseConcurrent.
  Proof.
    intros OnFreeList CurrentSegment ReuseReturned
           TryWriteWon ReusePressureObserved ChoseExclusive ChoseConcurrent
           Hpolicy Hexclusive_progress Hobserved_pressure Hlost
           Hpressure_observed.
    pose proof (try_write_loss_with_reuse_pressure_chooses_exclusive
                  TryWriteWon ReusePressureObserved ChoseExclusive
                  ChoseConcurrent Hpolicy Hlost Hpressure_observed)
      as [_ Hnot_concurrent].
    split.
    - apply Hexclusive_progress.
      apply Hobserved_pressure.
      exact Hpressure_observed.
    - exact Hnot_concurrent.
  Qed.
End ConcurrentReusePressureProgressModel.

End MeTTaTron_GC_ConcurrentReusePressureProgress.
