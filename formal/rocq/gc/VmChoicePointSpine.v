(** E3 VM choice-point continuation-spine lowering.

    The bytecode VM no longer owns its re-enterable choice-point payloads as a
    raw Vec inside the VM. The stack owns ContinuationAddr handles; each handle
    resolves through a typed continuation-spine store to a choice-point node.

    This file proves the implementation-level GC obligation for that lowering:
    if the live VM choice-point stack resolves an address to a stored node, and
    the node walker is complete for the VM choice-point fields, then every
    value a future VM fail/backtrack transition can touch is rooted before
    sweep. No registry or discovery side-channel is assumed.
*)

Module MeTTaTron_GC_VmChoicePointSpine.

Section VmChoicePointSpineModel.
  Variable Addr Value Node : Type.

  Variable LiveVmChoiceHandle : Addr -> Prop.
  Variable Resolves : Addr -> Node -> Prop.
  Variable NodeRoot : Node -> Value -> Prop.

  Variable ContinuationChunkConstant : Addr -> Value -> Prop.
  Variable AlternativeValue : Addr -> Value -> Prop.
  Variable AlternativeChunkConstant : Addr -> Value -> Prop.
  Variable RuleMatchBindingValue : Addr -> Value -> Prop.
  Variable BoundValue : Addr -> Value -> Prop.
  Variable BoundBindingValue : Addr -> Value -> Prop.
  Variable SavedCurrentBindingValue : Addr -> Value -> Prop.

  Inductive FutureVmChoiceTouch (a : Addr) (v : Value) : Prop :=
  | future_vm_continuation_chunk :
      ContinuationChunkConstant a v -> FutureVmChoiceTouch a v
  | future_vm_alternative_value :
      AlternativeValue a v -> FutureVmChoiceTouch a v
  | future_vm_alternative_chunk :
      AlternativeChunkConstant a v -> FutureVmChoiceTouch a v
  | future_vm_rule_match_binding :
      RuleMatchBindingValue a v -> FutureVmChoiceTouch a v
  | future_vm_bound_value :
      BoundValue a v -> FutureVmChoiceTouch a v
  | future_vm_bound_binding :
      BoundBindingValue a v -> FutureVmChoiceTouch a v
  | future_vm_saved_current_binding :
      SavedCurrentBindingValue a v -> FutureVmChoiceTouch a v.

  Definition VmChoiceNodeComplete : Prop :=
    forall a n v,
      LiveVmChoiceHandle a ->
      Resolves a n ->
      FutureVmChoiceTouch a v ->
      NodeRoot n v.

  Theorem vm_choice_point_spine_roots_future_touch :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        FutureVmChoiceTouch a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hfuture.
    destruct (Hresolve a Hlive) as [n Hresolves].
    apply (Hroot n v).
    apply (Hcomplete a n v Hlive Hresolves Hfuture).
  Qed.

  Theorem vm_choice_point_future_touch_not_freed :
    forall (RootedValue FreedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      (forall v, FreedValue v -> ~ RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        FutureVmChoiceTouch a v ->
        ~ FreedValue v.
  Proof.
    intros RootedValue FreedValue Hresolve Hcomplete Hroot Hfreed
           a v Hlive Hfuture Hfreed_v.
    apply (Hfreed v Hfreed_v).
    apply (vm_choice_point_spine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive Hfuture).
  Qed.

  Theorem vm_choice_point_alternative_value_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        AlternativeValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Halt.
    apply (vm_choice_point_spine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    apply future_vm_alternative_value. exact Halt.
  Qed.

  Theorem vm_choice_point_rule_match_binding_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        RuleMatchBindingValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hbinding.
    apply (vm_choice_point_spine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    apply future_vm_rule_match_binding. exact Hbinding.
  Qed.

  Theorem vm_choice_point_bound_binding_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        BoundBindingValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hbinding.
    apply (vm_choice_point_spine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    apply future_vm_bound_binding. exact Hbinding.
  Qed.

  Theorem vm_choice_point_saved_current_binding_rooted :
    forall (RootedValue : Value -> Prop),
      (forall a, LiveVmChoiceHandle a -> exists n, Resolves a n) ->
      VmChoiceNodeComplete ->
      (forall n v, NodeRoot n v -> RootedValue v) ->
      forall a v,
        LiveVmChoiceHandle a ->
        SavedCurrentBindingValue a v ->
        RootedValue v.
  Proof.
    intros RootedValue Hresolve Hcomplete Hroot a v Hlive Hsaved.
    apply (vm_choice_point_spine_roots_future_touch
             RootedValue Hresolve Hcomplete Hroot a v Hlive).
    apply future_vm_saved_current_binding. exact Hsaved.
  Qed.
End VmChoicePointSpineModel.

End MeTTaTron_GC_VmChoicePointSpine.
