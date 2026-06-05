(** E2 batch-result handoff rooting obligations.

    `run_state_async` receives result vectors from worker tasks after those
    workers have left the rendezvous participant set.  The values must therefore
    be rooted by a persistent driver-C/safepoint handle until the caller copies
    them into `MettaState.output`, where they become driver program roots.  This
    proof captures that handoff shape independently of the concrete Rust
    container used to transport the values.
*)

Module MeTTaTron_GC_BatchHandoff.

Section BatchHandoffModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition CollectorRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Definition BatchHandoffRoot
      (HandleRoot OutputRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    HandleRoot a \/ OutputRoot a.

  Theorem batch_result_handoff_rooted :
    forall (BatchResult HandleRoot OutputRoot : Addr -> Prop),
      (forall a, BatchResult a -> HandleRoot a \/ OutputRoot a) ->
      forall a,
        BatchResult a ->
        BatchHandoffRoot HandleRoot OutputRoot a.
  Proof.
    intros BatchResult HandleRoot OutputRoot Hcovered a Hresult.
    apply Hcovered.
    exact Hresult.
  Qed.

  Theorem batch_result_survives_while_handle_live :
    forall (StructuralRoot DriverRoot HandleRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, HandleRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, HandleRoot a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot HandleRoot Edge Marked Freed
           Hhandle_driver Hmark Hsweep a Hhandle Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Hhandle_driver.
    exact Hhandle.
  Qed.

  Theorem batch_result_survives_after_output_copy :
    forall (StructuralRoot DriverRoot OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, OutputRoot a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot OutputRoot Edge Marked Freed
           Houtput_driver Hmark Hsweep a Houtput Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Houtput_driver.
    exact Houtput.
  Qed.

  Theorem batch_result_handoff_survives_collection :
    forall (StructuralRoot DriverRoot BatchResult
            HandleRoot OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> HandleRoot a \/ OutputRoot a) ->
      (forall a, HandleRoot a -> DriverRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult HandleRoot OutputRoot
           Edge Marked Freed Hcovered Hhandle_driver Houtput_driver Hmark Hsweep
           a Hresult Hfreed.
    destruct (Hcovered a Hresult) as [Hhandle | Houtput].
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Hhandle_driver.
      exact Hhandle.
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Houtput_driver.
      exact Houtput.
  Qed.

  Theorem dropping_handle_after_output_copy_is_safe :
    forall (StructuralRoot DriverRoot BatchResult OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> OutputRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult OutputRoot Edge Marked Freed
           Hcopied Houtput_driver Hmark Hsweep a Hresult Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Houtput_driver.
    apply Hcopied.
    exact Hresult.
  Qed.
End BatchHandoffModel.

End MeTTaTron_GC_BatchHandoff.
