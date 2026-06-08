(** Rocq model for bytecode-VM native locals held across nested CESK eval.

    The CESK collector may collect while a bytecode VM has suspended itself and
    entered a nested trampoline.  Values stored in the VM struct are covered by
    the VM tier leaf.  Values held only in native Rust locals must be published
    through the typed K-spine `ValueVec` leaf before the nested evaluator can
    trigger a collection.
*)

Module MeTTaTron_GC_VmNestedLocals.

Section VmNestedLocalsModel.
  Variable Addr : Type.

  Inductive VmLocalClass : Type :=
  | PreEvalExpr : VmLocalClass
  | PreEvalItem : VmLocalClass
  | PreEvalResult : VmLocalClass
  | DispatchRhs : VmLocalClass
  | RuleMatchRhs : VmLocalClass
  | RuleMatchTemplate : VmLocalClass
  | RuleMatchBinding : VmLocalClass
  | SavedBinding : VmLocalClass
  | ComboExpr : VmLocalClass
  | ComboBinding : VmLocalClass
  | AccumulatedOutcome : VmLocalClass.

  Definition VmNestedLocal
      (ClassRoot : VmLocalClass -> Addr -> Prop)
      (a : Addr) : Prop :=
    exists c, ClassRoot c a.

  Theorem vm_nested_local_root_complete :
    forall (ClassRoot : VmLocalClass -> Addr -> Prop)
           (KSpineRoot : Addr -> Prop),
      (forall c a, ClassRoot c a -> KSpineRoot a) ->
      forall a,
        VmNestedLocal ClassRoot a ->
        KSpineRoot a.
  Proof.
    intros ClassRoot KSpineRoot Hclass a Hlocal.
    destruct Hlocal as [c Hroot].
    apply (Hclass c a).
    exact Hroot.
  Qed.

  Theorem vm_nested_local_survives_collection :
    forall (ClassRoot : VmLocalClass -> Addr -> Prop)
           (KSpineRoot Marked Freed : Addr -> Prop),
      (forall c a, ClassRoot c a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        VmNestedLocal ClassRoot a ->
        ~ Freed a.
  Proof.
    intros ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a Hlocal Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply (vm_nested_local_root_complete ClassRoot KSpineRoot Hclass).
    exact Hlocal.
  Qed.

  Theorem vm_pre_eval_item_survives_collection :
    forall (ClassRoot : VmLocalClass -> Addr -> Prop)
           (KSpineRoot Marked Freed : Addr -> Prop),
      (forall c a, ClassRoot c a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ClassRoot PreEvalItem a ->
        ~ Freed a.
  Proof.
    intros ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a Hitem.
    apply (vm_nested_local_survives_collection
             ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a).
    exists PreEvalItem. exact Hitem.
  Qed.

  Theorem vm_rule_match_rhs_survives_collection :
    forall (ClassRoot : VmLocalClass -> Addr -> Prop)
           (KSpineRoot Marked Freed : Addr -> Prop),
      (forall c a, ClassRoot c a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ClassRoot RuleMatchRhs a ->
        ~ Freed a.
  Proof.
    intros ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a Hrhs.
    apply (vm_nested_local_survives_collection
             ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a).
    exists RuleMatchRhs. exact Hrhs.
  Qed.

  Theorem vm_accumulated_outcome_survives_collection :
    forall (ClassRoot : VmLocalClass -> Addr -> Prop)
           (KSpineRoot Marked Freed : Addr -> Prop),
      (forall c a, ClassRoot c a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ClassRoot AccumulatedOutcome a ->
        ~ Freed a.
  Proof.
    intros ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a Houtcome.
    apply (vm_nested_local_survives_collection
             ClassRoot KSpineRoot Marked Freed Hclass Hmark Hsweep a).
    exists AccumulatedOutcome. exact Houtcome.
  Qed.
End VmNestedLocalsModel.

End MeTTaTron_GC_VmNestedLocals.
