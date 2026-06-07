(** C2 abstract-GC live-field narrowing.

    `Continuation.collect_live_values` narrows the K register by omitting three
    post-cut iterator fields. The narrowing is sound only when every value a
    future transition can touch is a full K-frame root and is not one of the
    dead fields.
*)

Module MeTTaTron_GC_AbstractGCLiveNarrowing.

Inductive NarrowedFrame : Type :=
| ProcessRuleMatches
| ProcessAmb
| ProcessMatchTemplates.

Section AbstractGCLiveNarrowingModel.
  Variable Addr : Type.

  Definition AbstractRoot
      (FullRoot DeadForNext : Addr -> Prop)
      (a : Addr) : Prop :=
    FullRoot a /\ ~ DeadForNext a.

  Theorem narrowed_roots_subset_full :
    forall (FullRoot DeadForNext : Addr -> Prop) a,
      AbstractRoot FullRoot DeadForNext a ->
      FullRoot a.
  Proof.
    intros FullRoot DeadForNext a Hroot.
    destruct Hroot as [Hfull _].
    exact Hfull.
  Qed.

  Theorem no_dead_field_narrowing_equals_full :
    forall (FullRoot DeadForNext : Addr -> Prop),
      (forall a, ~ DeadForNext a) ->
      forall a, FullRoot a -> AbstractRoot FullRoot DeadForNext a.
  Proof.
    intros FullRoot DeadForNext Hno_dead a Hfull.
    split.
    - exact Hfull.
    - apply Hno_dead.
  Qed.

  Theorem future_touch_is_abstract_root :
    forall (FullRoot DeadForNext FutureTouch : Addr -> Prop),
      (forall a, FutureTouch a -> FullRoot a) ->
      (forall a, FutureTouch a -> ~ DeadForNext a) ->
      forall a, FutureTouch a -> AbstractRoot FullRoot DeadForNext a.
  Proof.
    intros FullRoot DeadForNext FutureTouch Hfuture_full Hfuture_not_dead a Htouch.
    split.
    - apply Hfuture_full.
      exact Htouch.
    - apply Hfuture_not_dead.
      exact Htouch.
  Qed.

  Theorem future_touch_survives_live_narrowing :
    forall (FullRoot DeadForNext FutureTouch Marked Freed : Addr -> Prop),
      (forall a, FutureTouch a -> FullRoot a) ->
      (forall a, FutureTouch a -> ~ DeadForNext a) ->
      (forall a, AbstractRoot FullRoot DeadForNext a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros FullRoot DeadForNext FutureTouch Marked Freed
           Hfuture_full Hfuture_not_dead Hmark Hsweep a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply future_touch_is_abstract_root with (FutureTouch := FutureTouch).
    - exact Hfuture_full.
    - exact Hfuture_not_dead.
    - exact Htouch.
  Qed.

  Theorem skipped_live_field_is_not_justified :
    forall (DeadForNext FutureTouch : Addr -> Prop) a,
      FutureTouch a ->
      DeadForNext a ->
      (forall b, FutureTouch b -> ~ DeadForNext b) ->
      False.
  Proof.
    intros DeadForNext FutureTouch a Htouch Hdead Hfuture_not_dead.
    apply (Hfuture_not_dead a Htouch).
    exact Hdead.
  Qed.
End AbstractGCLiveNarrowingModel.

End MeTTaTron_GC_AbstractGCLiveNarrowing.
