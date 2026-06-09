(** E3 stored branch-coroutine spine lowering.

    The production lazy rule-match continuation no longer owns its
    BranchCoroutine payload directly. It owns a ContinuationAddr; that address
    resolves through the selective continuation-spine store to a branch
    coroutine node, and the node walker contributes every remaining branch RHS,
    remaining branch binding value, and yielded result to K roots.

    This file proves the source-level obligation for that lowering: collecting
    roots through the address is as strong as collecting roots from the embedded
    coroutine payload for every value a future lazy resume can touch.
*)

Module MeTTaTron_GC_StoredBranchCoroutineSpine.

Section StoredBranchCoroutineSpineModel.
  Variable Addr Value Node : Type.

  Variable LiveHandle : Addr -> Prop.
  Variable Resolves : Addr -> Node -> Prop.
  Variable NodeRoot : Node -> Value -> Prop.

  Variable RemainingRhs : Addr -> Value -> Prop.
  Variable RemainingBindingValue : Addr -> Value -> Prop.
  Variable YieldedValue : Addr -> Value -> Prop.

  Definition FutureCoroutineTouch (a : Addr) (v : Value) : Prop :=
    RemainingRhs a v \/ RemainingBindingValue a v \/ YieldedValue a v.

  Definition StoredNodeComplete : Prop :=
    forall a n v,
      LiveHandle a ->
      Resolves a n ->
      FutureCoroutineTouch a v ->
      NodeRoot n v.

  Theorem stored_branch_coroutine_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveHandle a -> exists n, Resolves a n) ->
      StoredNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveHandle a ->
        FutureCoroutineTouch a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hfuture.
    destruct (Hresolve a Hlive) as [n Hresolves].
    apply (Hroot n v).
    apply (Hcomplete a n v Hlive Hresolves Hfuture).
  Qed.

  Theorem stored_branch_coroutine_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      (forall a, LiveHandle a -> exists n, Resolves a n) ->
      StoredNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall a v,
        LiveHandle a ->
        FutureCoroutineTouch a v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hresolve Hcomplete Hroot Hfreed
           a v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (stored_branch_coroutine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive Hfuture).
  Qed.

  Theorem stored_branch_coroutine_remaining_rhs_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveHandle a -> exists n, Resolves a n) ->
      StoredNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveHandle a ->
        RemainingRhs a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hrhs.
    apply (stored_branch_coroutine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    left. exact Hrhs.
  Qed.

  Theorem stored_branch_coroutine_remaining_binding_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveHandle a -> exists n, Resolves a n) ->
      StoredNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveHandle a ->
        RemainingBindingValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hbinding.
    apply (stored_branch_coroutine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    right. left. exact Hbinding.
  Qed.

  Theorem stored_branch_coroutine_yielded_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveHandle a -> exists n, Resolves a n) ->
      StoredNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveHandle a ->
        YieldedValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hyielded.
    apply (stored_branch_coroutine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    right. right. exact Hyielded.
  Qed.
End StoredBranchCoroutineSpineModel.

End MeTTaTron_GC_StoredBranchCoroutineSpine.
