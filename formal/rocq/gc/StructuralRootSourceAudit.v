(** Structural-root source-coupling audit.

    This file is the Rocq closure for the source-coupling audit over the
    no-registry CESK root architecture.  The source script pins each concrete
    reader to one of the abstract families below: machine S/C/K, local frame
    environments, E0/global anchors, typed K-spine leaves, VM/JIT tier leaves,
    re-enterable choice points, deferred environments, driver-C transport,
    worker self-publications, safepoint roots, live env/dispatch anchors, and
    batch handoff roots.

    The legacy RootProvider/root-registry/frame-chain mechanisms may remain as
    slab bridge code during the migration, but they are not an index collector
    root source.
*)

Module MeTTaTron_GC_StructuralRootSourceAudit.

Section StructuralRootSourceAuditModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition StructuralContribution
      (ControlRoot EnvLocalRoot KontRoot FrameEnvRoot E0Root GlobalAnchor
       KSpineRoot VmLeafRoot JitLeafRoot ChoicePointRoot DeferredEnvRoot :
          Addr -> Prop)
      (a : Addr) : Prop :=
    ControlRoot a \/
    EnvLocalRoot a \/
    KontRoot a \/
    FrameEnvRoot a \/
    E0Root a \/
    GlobalAnchor a \/
    KSpineRoot a \/
    VmLeafRoot a \/
    JitLeafRoot a \/
    ChoicePointRoot a \/
    DeferredEnvRoot a.

  Definition DriverContribution
      (DriverCRoot WorkerRoot SafepointRoot LiveEnvRoot LiveDispatchRoot
       BatchHandoffRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    DriverCRoot a \/
    WorkerRoot a \/
    SafepointRoot a \/
    LiveEnvRoot a \/
    LiveDispatchRoot a \/
    BatchHandoffRoot a.

  Definition AuditedIndexRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Theorem structural_contribution_in_machine_root :
    forall (ControlRoot EnvLocalRoot KontRoot FrameEnvRoot E0Root GlobalAnchor
            KSpineRoot VmLeafRoot JitLeafRoot ChoicePointRoot DeferredEnvRoot
            MachineRoot : Addr -> Prop),
      (forall a, ControlRoot a -> MachineRoot a) ->
      (forall a, EnvLocalRoot a -> MachineRoot a) ->
      (forall a, KontRoot a -> MachineRoot a) ->
      (forall a, FrameEnvRoot a -> MachineRoot a) ->
      (forall a, E0Root a -> MachineRoot a) ->
      (forall a, GlobalAnchor a -> MachineRoot a) ->
      (forall a, KSpineRoot a -> MachineRoot a) ->
      (forall a, VmLeafRoot a -> MachineRoot a) ->
      (forall a, JitLeafRoot a -> MachineRoot a) ->
      (forall a, ChoicePointRoot a -> MachineRoot a) ->
      (forall a, DeferredEnvRoot a -> MachineRoot a) ->
      forall a,
        StructuralContribution ControlRoot EnvLocalRoot KontRoot FrameEnvRoot
          E0Root GlobalAnchor KSpineRoot VmLeafRoot JitLeafRoot ChoicePointRoot
          DeferredEnvRoot a ->
        MachineRoot a.
  Proof.
    intros ControlRoot EnvLocalRoot KontRoot FrameEnvRoot E0Root GlobalAnchor
           KSpineRoot VmLeafRoot JitLeafRoot ChoicePointRoot DeferredEnvRoot
           MachineRoot Hcontrol Henv Hkont Hframe Henv0 Hglobal Hkspine Hvm
           Hjit Hchoice Hdeferred a Hroot.
    destruct Hroot as
      [Hcontrol_a |
       [Henv_a |
        [Hkont_a |
         [Hframe_a |
          [Henv0_a |
           [Hglobal_a |
            [Hkspine_a |
             [Hvm_a |
              [Hjit_a |
               [Hchoice_a | Hdeferred_a]]]]]]]]]].
    - apply Hcontrol. exact Hcontrol_a.
    - apply Henv. exact Henv_a.
    - apply Hkont. exact Hkont_a.
    - apply Hframe. exact Hframe_a.
    - apply Henv0. exact Henv0_a.
    - apply Hglobal. exact Hglobal_a.
    - apply Hkspine. exact Hkspine_a.
    - apply Hvm. exact Hvm_a.
    - apply Hjit. exact Hjit_a.
    - apply Hchoice. exact Hchoice_a.
    - apply Hdeferred. exact Hdeferred_a.
  Qed.

  Theorem driver_contribution_in_driver_root :
    forall (DriverCRoot WorkerRoot SafepointRoot LiveEnvRoot LiveDispatchRoot
            BatchHandoffRoot DriverRoot : Addr -> Prop),
      (forall a, DriverCRoot a -> DriverRoot a) ->
      (forall a, WorkerRoot a -> DriverRoot a) ->
      (forall a, SafepointRoot a -> DriverRoot a) ->
      (forall a, LiveEnvRoot a -> DriverRoot a) ->
      (forall a, LiveDispatchRoot a -> DriverRoot a) ->
      (forall a, BatchHandoffRoot a -> DriverRoot a) ->
      forall a,
        DriverContribution DriverCRoot WorkerRoot SafepointRoot LiveEnvRoot
          LiveDispatchRoot BatchHandoffRoot a ->
        DriverRoot a.
  Proof.
    intros DriverCRoot WorkerRoot SafepointRoot LiveEnvRoot LiveDispatchRoot
           BatchHandoffRoot DriverRoot Hdriver_c Hworker Hsafepoint Hlive_env
           Hlive_dispatch Hbatch a Hroot.
    destruct Hroot as
      [Hdriver_c_a |
       [Hworker_a |
        [Hsafepoint_a |
         [Hlive_env_a |
          [Hlive_dispatch_a | Hbatch_a]]]]].
    - apply Hdriver_c. exact Hdriver_c_a.
    - apply Hworker. exact Hworker_a.
    - apply Hsafepoint. exact Hsafepoint_a.
    - apply Hlive_env. exact Hlive_env_a.
    - apply Hlive_dispatch. exact Hlive_dispatch_a.
    - apply Hbatch. exact Hbatch_a.
  Qed.

  Theorem audited_root_survives_collection :
    forall (StructuralRoot DriverRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, Reach (AuditedIndexRoot StructuralRoot DriverRoot) Edge a ->
                 Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        AuditedIndexRoot StructuralRoot DriverRoot a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot Marked Freed Edge Hmark Hsweep a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    exact Hroot.
  Qed.

  Theorem audited_future_touch_survives_without_registry :
    forall (StructuralRoot DriverRoot RegistryRoot FutureTouch Marked Freed :
              Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, FutureTouch a ->
        Reach (AuditedIndexRoot StructuralRoot DriverRoot) Edge a) ->
      (forall a, Reach (AuditedIndexRoot StructuralRoot DriverRoot) Edge a ->
        Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        FutureTouch a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot RegistryRoot FutureTouch Marked Freed Edge
           Hfuture Hmark Hsweep a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem registry_side_channel_not_audited_root :
    forall (StructuralRoot DriverRoot RegistryRoot : Addr -> Prop) a,
      RegistryRoot a ->
      ~ StructuralRoot a ->
      ~ DriverRoot a ->
      ~ AuditedIndexRoot StructuralRoot DriverRoot a.
  Proof.
    intros StructuralRoot DriverRoot RegistryRoot a _ Hnot_structural
           Hnot_driver Hroot.
    destruct Hroot as [Hstructural | Hdriver].
    - apply Hnot_structural. exact Hstructural.
    - apply Hnot_driver. exact Hdriver.
  Qed.
End StructuralRootSourceAuditModel.

End MeTTaTron_GC_StructuralRootSourceAudit.
