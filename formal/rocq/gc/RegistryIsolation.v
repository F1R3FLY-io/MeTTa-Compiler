(** Rocq companion for [tla/RegistryIsolation.tla].

    Index-mode GC root construction is intentionally isolated from the legacy
    slab RootProvider registry.  The index collector's root set may contain
    structural CESK roots and explicit driver transport roots; a value that is
    reachable only through the registry is not an index collector root.  This is
    the proof-assistant counterpart to the TLC discriminator that fails when the
    registry is enabled as an index-mode root source.
*)

Module MeTTaTron_GC_RegistryIsolation.

Section RegistryIsolationModel.
  Variable Addr : Type.

  Inductive RootSource : Type :=
  | StructuralSource : RootSource
  | DriverSource : RootSource
  | RegistrySource : RootSource.

  Definition IndexRootSource (source : RootSource) : Prop :=
    match source with
    | StructuralSource => True
    | DriverSource => True
    | RegistrySource => False
    end.

  Definition IndexRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Definition IndexRootSetComplete
      (IncludeStructural IncludeDriver RegistryRooted : Prop) : Prop :=
    IncludeStructural /\ IncludeDriver /\ ~ RegistryRooted.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Theorem registry_is_not_index_root_source :
    ~ IndexRootSource RegistrySource.
  Proof.
    intro Hsource.
    exact Hsource.
  Qed.

  Theorem index_root_source_not_registry :
    forall source,
      IndexRootSource source ->
      source <> RegistrySource.
  Proof.
    intros source Hsource Hregistry.
    destruct source.
    - discriminate Hregistry.
    - discriminate Hregistry.
    - exact Hsource.
  Qed.

  Theorem index_root_set_complete_without_registry :
    forall IncludeStructural IncludeDriver RegistryRooted,
      IncludeStructural ->
      IncludeDriver ->
      ~ RegistryRooted ->
      IndexRootSetComplete IncludeStructural IncludeDriver RegistryRooted.
  Proof.
    intros IncludeStructural IncludeDriver RegistryRooted
           Hstructural Hdriver Hno_registry.
    split.
    - exact Hstructural.
    - split.
      + exact Hdriver.
      + exact Hno_registry.
  Qed.

  Theorem registry_enabled_violates_index_isolation :
    forall IncludeStructural IncludeDriver RegistryRooted,
      RegistryRooted ->
      ~ IndexRootSetComplete IncludeStructural IncludeDriver RegistryRooted.
  Proof.
    intros IncludeStructural IncludeDriver RegistryRooted
           Hregistry Hcomplete.
    destruct Hcomplete as [_ [_ Hno_registry]].
    apply Hno_registry.
    exact Hregistry.
  Qed.

  Theorem registry_only_value_not_index_root :
    forall (StructuralRoot DriverRoot RegistryRoot : Addr -> Prop) a,
      RegistryRoot a ->
      ~ StructuralRoot a ->
      ~ DriverRoot a ->
      ~ IndexRoot StructuralRoot DriverRoot a.
  Proof.
    intros StructuralRoot DriverRoot RegistryRoot a _ Hnot_structural
           Hnot_driver Hroot.
    destruct Hroot as [Hstructural | Hdriver].
    - apply Hnot_structural.
      exact Hstructural.
    - apply Hnot_driver.
      exact Hdriver.
  Qed.

  Theorem registry_independent_future_touches_survive :
    forall (StructuralRoot DriverRoot RegistryRoot FutureTouch Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall a, FutureTouch a -> Reach (IndexRoot StructuralRoot DriverRoot) Edge a) ->
      (forall a, Reach (IndexRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        FutureTouch a ->
        ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot RegistryRoot FutureTouch Marked Freed Edge
           Hfuture_reach Hmark Hsweep a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture_reach.
    exact Htouch.
  Qed.
End RegistryIsolationModel.

End MeTTaTron_GC_RegistryIsolation.
