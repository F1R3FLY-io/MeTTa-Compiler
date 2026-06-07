(** Rocq model of the single-threaded mid-loop CESK root-union obligation.

    The mid-loop collector runs while the trampoline machine is live, so
    its root vector must contain the live S/C/K registers, persistent E0 roots,
    global anchors, the reified K-spine, deferred environment drops, and the
    caller-held driver-C safepoint channel.  This module proves the parametric
    mark/sweep safety shape for that union.
*)

Module MeTTaTron_GC_MidloopRootUnion.

Section MidloopRootUnionModel.
  Variable Addr : Type.

  Definition MidloopRootUnion
      (LiveSCK Env0 Global KSpine Deferred DriverC : Addr -> Prop)
      (a : Addr) : Prop :=
    LiveSCK a \/ Env0 a \/ Global a \/ KSpine a \/ Deferred a \/ DriverC a.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Theorem midloop_root_union_complete :
    forall (LiveSCK Env0 Global KSpine Deferred DriverC MidloopRoot : Addr -> Prop),
      (forall a, LiveSCK a -> MidloopRoot a) ->
      (forall a, Env0 a -> MidloopRoot a) ->
      (forall a, Global a -> MidloopRoot a) ->
      (forall a, KSpine a -> MidloopRoot a) ->
      (forall a, Deferred a -> MidloopRoot a) ->
      (forall a, DriverC a -> MidloopRoot a) ->
      forall a,
        MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC a ->
        MidloopRoot a.
  Proof.
    intros LiveSCK Env0 Global KSpine Deferred DriverC MidloopRoot
           HliveSCK Henv0 Hglobal HkSpine Hdeferred HdriverC a Hroot.
    destruct Hroot as [HliveSCK_a | [Henv0_a | [Hglobal_a | [HkSpine_a | [Hdeferred_a | HdriverC_a]]]]].
    - apply HliveSCK. exact HliveSCK_a.
    - apply Henv0. exact Henv0_a.
    - apply Hglobal. exact Hglobal_a.
    - apply HkSpine. exact HkSpine_a.
    - apply Hdeferred. exact Hdeferred_a.
    - apply HdriverC. exact HdriverC_a.
  Qed.

  Theorem midloop_root_union_survives_collection :
    forall (LiveSCK Env0 Global KSpine Deferred DriverC
            MidloopRoot Marked Freed : Addr -> Prop),
      (forall a, LiveSCK a -> MidloopRoot a) ->
      (forall a, Env0 a -> MidloopRoot a) ->
      (forall a, Global a -> MidloopRoot a) ->
      (forall a, KSpine a -> MidloopRoot a) ->
      (forall a, Deferred a -> MidloopRoot a) ->
      (forall a, DriverC a -> MidloopRoot a) ->
      (forall a, MidloopRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC a ->
        ~ Freed a.
  Proof.
    intros LiveSCK Env0 Global KSpine Deferred DriverC MidloopRoot Marked Freed
           HliveSCK Henv0 Hglobal HkSpine Hdeferred HdriverC Hmark Hsweep
           a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    destruct Hroot as [HliveSCK_a | [Henv0_a | [Hglobal_a | [HkSpine_a | [Hdeferred_a | HdriverC_a]]]]].
    - apply HliveSCK. exact HliveSCK_a.
    - apply Henv0. exact Henv0_a.
    - apply Hglobal. exact Hglobal_a.
    - apply HkSpine. exact HkSpine_a.
    - apply Hdeferred. exact Hdeferred_a.
    - apply HdriverC. exact HdriverC_a.
  Qed.

  Theorem midloop_future_touch_survives_collection :
    forall (LiveSCK Env0 Global KSpine Deferred DriverC
            Marked Freed FutureTouch : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a,
          Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a,
          FutureTouch a ->
          Reach (MidloopRootUnion LiveSCK Env0 Global KSpine Deferred DriverC) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros LiveSCK Env0 Global KSpine Deferred DriverC
           Marked Freed FutureTouch Edge Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.
End MidloopRootUnionModel.

End MeTTaTron_GC_MidloopRootUnion.
