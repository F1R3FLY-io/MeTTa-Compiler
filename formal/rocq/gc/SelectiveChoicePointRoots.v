(** E3 selective-CESK re-enterable continuation roots.

    Selective CESK* keeps the deterministic hot K native, but every
    re-enterable continuation frame is a structural K contribution. Today that
    means the concrete VM choice-point stack, JIT choice-point stack, trampoline
    coroutine/choice state, and any captured/suspended spine must be included
    in the collector root set before a sweep can reclaim store addresses.
*)

Module MeTTaTron_GC_SelectiveChoicePointRoots.

Section SelectiveChoicePointRootsModel.
  Variable Addr : Type.

  Inductive ReenterableK
      (TrampolineChoice VmChoice JitChoice CapturedSpine : Addr -> Prop)
      : Addr -> Prop :=
  | reenterable_trampoline_choice :
      forall a, TrampolineChoice a ->
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a
  | reenterable_vm_choice :
      forall a, VmChoice a ->
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a
  | reenterable_jit_choice :
      forall a, JitChoice a ->
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a
  | reenterable_captured_spine :
      forall a, CapturedSpine a ->
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a.

  Theorem reenterable_k_root_complete :
    forall (TrampolineChoice VmChoice JitChoice CapturedSpine KRoot :
              Addr -> Prop),
      (forall a, TrampolineChoice a -> KRoot a) ->
      (forall a, VmChoice a -> KRoot a) ->
      (forall a, JitChoice a -> KRoot a) ->
      (forall a, CapturedSpine a -> KRoot a) ->
      forall a,
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a ->
        KRoot a.
  Proof.
    intros TrampolineChoice VmChoice JitChoice CapturedSpine KRoot
           Htrampoline Hvm Hjit Hcaptured a Hreenterable.
    destruct Hreenterable as [a Htramp | a Hvm_choice | a Hjit_choice | a Hcap].
    - apply Htrampoline. exact Htramp.
    - apply Hvm. exact Hvm_choice.
    - apply Hjit. exact Hjit_choice.
    - apply Hcaptured. exact Hcap.
  Qed.

  Theorem reenterable_k_survives_collection :
    forall (TrampolineChoice VmChoice JitChoice CapturedSpine
            KRoot Marked Freed : Addr -> Prop),
      (forall a, TrampolineChoice a -> KRoot a) ->
      (forall a, VmChoice a -> KRoot a) ->
      (forall a, JitChoice a -> KRoot a) ->
      (forall a, CapturedSpine a -> KRoot a) ->
      (forall a, KRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        ReenterableK TrampolineChoice VmChoice JitChoice CapturedSpine a ->
        ~ Freed a.
  Proof.
    intros TrampolineChoice VmChoice JitChoice CapturedSpine
           KRoot Marked Freed Htrampoline Hvm Hjit Hcaptured Hmark Hsweep
           a Hreenterable Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply (reenterable_k_root_complete
             TrampolineChoice VmChoice JitChoice CapturedSpine KRoot
             Htrampoline Hvm Hjit Hcaptured).
    exact Hreenterable.
  Qed.

  Theorem vm_choice_point_survives_collection :
    forall (TrampolineChoice VmChoice JitChoice CapturedSpine
            KRoot Marked Freed : Addr -> Prop),
      (forall a, TrampolineChoice a -> KRoot a) ->
      (forall a, VmChoice a -> KRoot a) ->
      (forall a, JitChoice a -> KRoot a) ->
      (forall a, CapturedSpine a -> KRoot a) ->
      (forall a, KRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, VmChoice a -> ~ Freed a.
  Proof.
    intros TrampolineChoice VmChoice JitChoice CapturedSpine
           KRoot Marked Freed Htrampoline Hvm Hjit Hcaptured Hmark Hsweep
           a Hchoice.
    apply (reenterable_k_survives_collection
             TrampolineChoice VmChoice JitChoice CapturedSpine
             KRoot Marked Freed Htrampoline Hvm Hjit Hcaptured Hmark Hsweep
             a).
    apply reenterable_vm_choice. exact Hchoice.
  Qed.

  Theorem jit_choice_point_survives_collection :
    forall (TrampolineChoice VmChoice JitChoice CapturedSpine
            KRoot Marked Freed : Addr -> Prop),
      (forall a, TrampolineChoice a -> KRoot a) ->
      (forall a, VmChoice a -> KRoot a) ->
      (forall a, JitChoice a -> KRoot a) ->
      (forall a, CapturedSpine a -> KRoot a) ->
      (forall a, KRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, JitChoice a -> ~ Freed a.
  Proof.
    intros TrampolineChoice VmChoice JitChoice CapturedSpine
           KRoot Marked Freed Htrampoline Hvm Hjit Hcaptured Hmark Hsweep
           a Hchoice.
    apply (reenterable_k_survives_collection
             TrampolineChoice VmChoice JitChoice CapturedSpine
             KRoot Marked Freed Htrampoline Hvm Hjit Hcaptured Hmark Hsweep
             a).
    apply reenterable_jit_choice. exact Hchoice.
  Qed.
End SelectiveChoicePointRootsModel.

End MeTTaTron_GC_SelectiveChoicePointRoots.
