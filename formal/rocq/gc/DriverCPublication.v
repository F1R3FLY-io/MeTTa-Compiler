(** Driver-C publication obligation for the CESK index collector.

    MettaState.source/output are caller-held control roots. During an eval
    transition they must be published into the narrow driver/safepoint channel
    used by midloop and rendezvous collection. Once published, ordinary
    root-complete marking and sweep safety retain them.
*)

Module MeTTaTron_GC_DriverCPublication.

Section DriverCPublicationModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition CollectorRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Theorem published_driver_c_is_collector_root :
    forall (DriverC DriverRoot StructuralRoot : Addr -> Prop),
      (forall a, DriverC a -> DriverRoot a) ->
      forall a, DriverC a -> CollectorRoot StructuralRoot DriverRoot a.
  Proof.
    intros DriverC DriverRoot StructuralRoot Hpublished a Hdriver.
    right.
    apply Hpublished.
    exact Hdriver.
  Qed.

  Theorem published_driver_c_survives_sweep :
    forall (DriverC DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, DriverC a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, DriverC a -> ~ Freed a.
  Proof.
    intros DriverC DriverRoot StructuralRoot Edge Marked Freed
           Hpublished Hmark Hsweep a Hdriver Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    eapply published_driver_c_is_collector_root; eauto.
  Qed.
End DriverCPublicationModel.

End MeTTaTron_GC_DriverCPublication.
