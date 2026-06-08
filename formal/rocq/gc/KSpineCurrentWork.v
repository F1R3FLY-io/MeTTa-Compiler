(** Rocq model for the typed K-spine's suspended-trampoline control roots.

    A suspended trampoline activation contributes the live control register C
    from both the in-flight current work item and the pending work stack, plus
    the continuation register K.  Omitting the current work is unsound whenever
    a nested evaluator reaches a mid-loop collection while the outer activation
    has already popped a work item but has not yet pushed its successor.
*)

Module MeTTaTron_GC_KSpineCurrentWork.

Section KSpineCurrentWorkModel.
  Variable Addr : Type.

  Definition SuspendedControl
      (CurrentWork WorkStack Kont : Addr -> Prop)
      (a : Addr) : Prop :=
    CurrentWork a \/ WorkStack a \/ Kont a.

  Theorem suspended_control_root_complete :
    forall (CurrentWork WorkStack Kont KSpineRoot : Addr -> Prop),
      (forall a, CurrentWork a -> KSpineRoot a) ->
      (forall a, WorkStack a -> KSpineRoot a) ->
      (forall a, Kont a -> KSpineRoot a) ->
      forall a,
        SuspendedControl CurrentWork WorkStack Kont a ->
        KSpineRoot a.
  Proof.
    intros CurrentWork WorkStack Kont KSpineRoot
           Hcurrent Hwork Hkont a Hcontrol.
    destruct Hcontrol as [Hcurrent_a | [Hwork_a | Hkont_a]].
    - apply Hcurrent. exact Hcurrent_a.
    - apply Hwork. exact Hwork_a.
    - apply Hkont. exact Hkont_a.
  Qed.

  Theorem suspended_control_survives_collection :
    forall (CurrentWork WorkStack Kont KSpineRoot Marked Freed : Addr -> Prop),
      (forall a, CurrentWork a -> KSpineRoot a) ->
      (forall a, WorkStack a -> KSpineRoot a) ->
      (forall a, Kont a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        SuspendedControl CurrentWork WorkStack Kont a ->
        ~ Freed a.
  Proof.
    intros CurrentWork WorkStack Kont KSpineRoot Marked Freed
           Hcurrent Hwork Hkont Hmark Hsweep a Hcontrol Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply (suspended_control_root_complete
             CurrentWork WorkStack Kont KSpineRoot
             Hcurrent Hwork Hkont).
    exact Hcontrol.
  Qed.

  Theorem current_work_survives_collection :
    forall (CurrentWork WorkStack Kont KSpineRoot Marked Freed : Addr -> Prop),
      (forall a, CurrentWork a -> KSpineRoot a) ->
      (forall a, WorkStack a -> KSpineRoot a) ->
      (forall a, Kont a -> KSpineRoot a) ->
      (forall a, KSpineRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, CurrentWork a -> ~ Freed a.
  Proof.
    intros CurrentWork WorkStack Kont KSpineRoot Marked Freed
           Hcurrent Hwork Hkont Hmark Hsweep a Hcur.
    apply (suspended_control_survives_collection
             CurrentWork WorkStack Kont KSpineRoot Marked Freed
             Hcurrent Hwork Hkont Hmark Hsweep a).
    left. exact Hcur.
  Qed.
End KSpineCurrentWorkModel.

End MeTTaTron_GC_KSpineCurrentWork.
