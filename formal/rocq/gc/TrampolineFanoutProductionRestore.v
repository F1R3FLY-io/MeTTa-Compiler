(** E3 production trampoline fan-out restore through ContinuationAddr.

    The bridge proof covers the value walker for trampoline fan-out nodes.
    This companion proves the production carrier boundary introduced for E3:

      1. the trampoline loop persists each live re-enterable fan-out frame into
         a ContinuationAddr-backed spine store before the next work step;
      2. root collection over K resolves the address and walks the stored frame;
      3. process_continuation resolves/removes that same address before
         executing the frame payload;
      4. after resolve, the handle cannot name a stale live store node.

    The proof is parametric over the concrete Rust fields. Source coupling pins
    the premises to the implementation in trampoline/types.rs and eval_loop.rs.
*)

Module MeTTaTron_GC_TrampolineFanoutProductionRestore.

Section ProductionRestoreModel.
  Variable Frame Addr Value : Type.

  Variable LiveRawFanoutFrame : Frame -> Prop.
  Variable FutureTouch : Frame -> Value -> Prop.

  Variable HandleOnK : Addr -> Prop.
  Variable StoreOwns : Addr -> Frame -> Prop.
  Variable RootedByHandle : Addr -> Value -> Prop.
  Variable ResolveForExecution : Addr -> Frame -> Prop.
  Variable StoreOwnsAfterResolve : Addr -> Prop.

  Definition LoopNormalizationComplete : Prop :=
    forall f,
      LiveRawFanoutFrame f ->
      exists a, HandleOnK a /\ StoreOwns a f.

  Definition HandleRootWalkComplete : Prop :=
    forall a f v,
      HandleOnK a ->
      StoreOwns a f ->
      FutureTouch f v ->
      RootedByHandle a v.

  Definition ResolveRemovesStoredFrame : Prop :=
    forall a f,
      HandleOnK a ->
      StoreOwns a f ->
      ResolveForExecution a f /\ ~ StoreOwnsAfterResolve a.

  Theorem production_spine_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      LoopNormalizationComplete ->
      HandleRootWalkComplete ->
      (forall a v, RootedByHandle a v -> RootedValue v) ->
      forall f v,
        LiveRawFanoutFrame f ->
        FutureTouch f v ->
        RootedValue v.
  Proof.
    intros RootedValue Hnormalize Hwalk Hroot f v Hlive Hfuture.
    destruct (Hnormalize f Hlive) as [a [Hhandle Howns]].
    apply (Hroot a v).
    apply (Hwalk a f v Hhandle Howns Hfuture).
  Qed.

  Theorem production_spine_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      LoopNormalizationComplete ->
      HandleRootWalkComplete ->
      (forall a v, RootedByHandle a v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall f v,
        LiveRawFanoutFrame f ->
        FutureTouch f v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hnormalize Hwalk Hroot Hfreed
           f v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (production_spine_roots_future_touch
             RootedValue Hnormalize Hwalk Hroot f v Hlive Hfuture).
  Qed.

  Theorem production_spine_resolves_before_execution :
    LoopNormalizationComplete ->
    ResolveRemovesStoredFrame ->
    forall f,
      LiveRawFanoutFrame f ->
      exists a, HandleOnK a /\ ResolveForExecution a f.
  Proof.
    intros Hnormalize Hresolve f Hlive.
    destruct (Hnormalize f Hlive) as [a [Hhandle Howns]].
    destruct (Hresolve a f Hhandle Howns) as [Hexec _].
    exists a. split; assumption.
  Qed.

  Theorem production_spine_no_stale_node_after_resolve :
    LoopNormalizationComplete ->
    ResolveRemovesStoredFrame ->
    forall f,
      LiveRawFanoutFrame f ->
      exists a, HandleOnK a /\ ~ StoreOwnsAfterResolve a.
  Proof.
    intros Hnormalize Hresolve f Hlive.
    destruct (Hnormalize f Hlive) as [a [Hhandle Howns]].
    destruct (Hresolve a f Hhandle Howns) as [_ Hremoved].
    exists a. split; assumption.
  Qed.

  Theorem production_spine_root_then_resolve_safe :
    forall (RootedValue FreedValue : Value -> Prop),
      LoopNormalizationComplete ->
      HandleRootWalkComplete ->
      ResolveRemovesStoredFrame ->
      (forall a v, RootedByHandle a v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall f v,
        LiveRawFanoutFrame f ->
        FutureTouch f v ->
        exists a,
          HandleOnK a /\
          RootedValue v /\
          ResolveForExecution a f /\
          ~ StoreOwnsAfterResolve a /\
          ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hnormalize Hwalk Hresolve Hroot Hfreed
           f v Hlive Hfuture.
    destruct (Hnormalize f Hlive) as [a [Hhandle Howns]].
    destruct (Hresolve a f Hhandle Howns) as [Hexec Hremoved].
    exists a.
    repeat split.
    - exact Hhandle.
    - apply (Hroot a v).
      apply (Hwalk a f v Hhandle Howns Hfuture).
    - exact Hexec.
    - exact Hremoved.
    - intros Hfreed_v.
      apply (Hfreed v Hfreed_v).
      apply (Hroot a v).
      apply (Hwalk a f v Hhandle Howns Hfuture).
  Qed.

End ProductionRestoreModel.

End MeTTaTron_GC_TrampolineFanoutProductionRestore.
