(** Rocq model of the E1/E2 driver root-union channel obligation.

    The dedicated collector does not discover roots by scanning arbitrary
    thread-locals.  It builds the driver root set from explicit channels:
    worker self-publications, safepoint roots, live environment anchors, and
    live dispatch anchors.  This module proves the non-TLC safety shape of that
    union: if each channel is included in the driver root set, then ordinary
    mark/sweep cannot free any live channel root.
*)

Module MeTTaTron_GC_DriverRootUnion.

Section DriverRootUnionModel.
  Variable Addr : Type.

  Definition DriverRootUnion
      (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor : Addr -> Prop)
      (a : Addr) : Prop :=
    WorkerRoot a \/ SafepointRoot a \/ EnvAnchor a \/ DispatchAnchor a.

  Theorem driver_root_union_complete :
    forall (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor
            DriverRoot : Addr -> Prop),
      (forall a, WorkerRoot a -> DriverRoot a) ->
      (forall a, SafepointRoot a -> DriverRoot a) ->
      (forall a, EnvAnchor a -> DriverRoot a) ->
      (forall a, DispatchAnchor a -> DriverRoot a) ->
      forall a,
        DriverRootUnion WorkerRoot SafepointRoot EnvAnchor DispatchAnchor a ->
        DriverRoot a.
  Proof.
    intros WorkerRoot SafepointRoot EnvAnchor DispatchAnchor DriverRoot
           Hworker Hsafepoint Henv Hdispatch a Hroot.
    destruct Hroot as [Hworker_a | [Hsafepoint_a | [Henv_a | Hdispatch_a]]].
    - apply Hworker. exact Hworker_a.
    - apply Hsafepoint. exact Hsafepoint_a.
    - apply Henv. exact Henv_a.
    - apply Hdispatch. exact Hdispatch_a.
  Qed.

  Theorem driver_root_union_survives_collection :
    forall (WorkerRoot SafepointRoot EnvAnchor DispatchAnchor
            DriverRoot Marked Freed : Addr -> Prop),
      (forall a, WorkerRoot a -> DriverRoot a) ->
      (forall a, SafepointRoot a -> DriverRoot a) ->
      (forall a, EnvAnchor a -> DriverRoot a) ->
      (forall a, DispatchAnchor a -> DriverRoot a) ->
      (forall a, DriverRoot a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        DriverRootUnion WorkerRoot SafepointRoot EnvAnchor DispatchAnchor a ->
        ~ Freed a.
  Proof.
    intros WorkerRoot SafepointRoot EnvAnchor DispatchAnchor DriverRoot Marked Freed
           Hworker Hsafepoint Henv Hdispatch Hmark Hsweep a Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    destruct Hroot as [Hworker_a | [Hsafepoint_a | [Henv_a | Hdispatch_a]]].
    - apply Hworker. exact Hworker_a.
    - apply Hsafepoint. exact Hsafepoint_a.
    - apply Henv. exact Henv_a.
    - apply Hdispatch. exact Hdispatch_a.
  Qed.
End DriverRootUnionModel.

End MeTTaTron_GC_DriverRootUnion.
