(** E3 JIT choice-point continuation-spine bridge.

    The JIT execution ABI still stores native [JitChoicePoint] records in a
    repr(C) buffer. For collection, the live prefix is materialized as typed
    ContinuationAddr-backed spine nodes and walked through that store. This
    file proves the GC obligation for that bridge: every value a future JIT
    fail/backtrack transition can touch from a live choice point is rooted
    through the bridge node walker.
*)

Module MeTTaTron_GC_JitChoicePointSpineBridge.

Section JitChoicePointSpineBridgeModel.
  Variable NativeChoice Addr Value Node : Type.

  Variable LiveNativeChoice : NativeChoice -> Prop.
  Variable BridgeHandle : NativeChoice -> Addr -> Prop.
  Variable Resolves : Addr -> Node -> Prop.
  Variable NodeRoot : Node -> Value -> Prop.

  Variable SavedChunkConstant : NativeChoice -> Value -> Prop.
  Variable SavedStackPoolValue : NativeChoice -> Value -> Prop.
  Variable AlternativeValue : NativeChoice -> Value -> Prop.
  Variable SpaceMatchTemplateValue : NativeChoice -> Value -> Prop.
  Variable AlternativeChunkConstant : NativeChoice -> Value -> Prop.
  Variable RuleMatchChunkConstant : NativeChoice -> Value -> Prop.

  Inductive FutureJitChoiceTouch (cp : NativeChoice) (v : Value) : Prop :=
  | future_jit_saved_chunk :
      SavedChunkConstant cp v -> FutureJitChoiceTouch cp v
  | future_jit_saved_stack_pool :
      SavedStackPoolValue cp v -> FutureJitChoiceTouch cp v
  | future_jit_alternative_value :
      AlternativeValue cp v -> FutureJitChoiceTouch cp v
  | future_jit_space_match_template :
      SpaceMatchTemplateValue cp v -> FutureJitChoiceTouch cp v
  | future_jit_alternative_chunk :
      AlternativeChunkConstant cp v -> FutureJitChoiceTouch cp v
  | future_jit_rule_match_chunk :
      RuleMatchChunkConstant cp v -> FutureJitChoiceTouch cp v.

  Definition BridgeComplete : Prop :=
    forall cp,
      LiveNativeChoice cp ->
      exists a n, BridgeHandle cp a /\ Resolves a n.

  Definition JitChoiceNodeComplete : Prop :=
    forall cp a n v,
      LiveNativeChoice cp ->
      BridgeHandle cp a ->
      Resolves a n ->
      FutureJitChoiceTouch cp v ->
      NodeRoot n v.

  Theorem jit_choice_bridge_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      JitChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall cp v,
        LiveNativeChoice cp ->
        FutureJitChoiceTouch cp v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot cp v Hlive Hfuture.
    destruct (Hbridge cp Hlive) as [a [n [Hhandle Hresolves]]].
    apply (Hroot n v).
    apply (Hcomplete cp a n v Hlive Hhandle Hresolves Hfuture).
  Qed.

  Theorem jit_choice_bridge_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      BridgeComplete ->
      JitChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall cp v,
        LiveNativeChoice cp ->
        FutureJitChoiceTouch cp v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hbridge Hcomplete Hroot Hfreed
           cp v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (jit_choice_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot cp v Hlive Hfuture).
  Qed.

  Theorem jit_choice_saved_stack_pool_rooted :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      JitChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall cp v,
        LiveNativeChoice cp ->
        SavedStackPoolValue cp v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot cp v Hlive Hsaved.
    apply (jit_choice_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot cp v Hlive).
    apply future_jit_saved_stack_pool. exact Hsaved.
  Qed.

  Theorem jit_choice_alternative_value_rooted :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      JitChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall cp v,
        LiveNativeChoice cp ->
        AlternativeValue cp v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot cp v Hlive Halt.
    apply (jit_choice_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot cp v Hlive).
    apply future_jit_alternative_value. exact Halt.
  Qed.

  Theorem jit_choice_saved_chunk_rooted :
    forall (RootedValue : Value -> Prop),
      BridgeComplete ->
      JitChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall cp v,
        LiveNativeChoice cp ->
        SavedChunkConstant cp v ->
        RootedValue v.
  Proof.
    intros RootedValue Hbridge Hcomplete Hroot cp v Hlive Hchunk.
    apply (jit_choice_bridge_roots_future_touch
             RootedValue Hbridge Hcomplete Hroot cp v Hlive).
    apply future_jit_saved_chunk. exact Hchunk.
  Qed.
End JitChoicePointSpineBridgeModel.

End MeTTaTron_GC_JitChoicePointSpineBridge.
