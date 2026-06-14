(** Rocq model of the F4 legacy root-discovery erasure obligation.

    The dynamic [RootProvider] registry, [collect_all_roots], [frame_chain],
    and the current-iterator RootProvider bridge are legacy slab discovery
    mechanisms.  The index collector must obtain roots from structural CESK
    registers and explicit driver transport channels instead.

    This proof connects that architectural isolation to Cargo feature store
    selection: default/explicit index selections erase every legacy discovery
    effect, and any emitted legacy discovery effect requires the active store
    to be [SlabStore].  It also reuses [RegistryIsolation.v] to show that a
    legacy-discovery-only value is not an index collector root.
*)

Require Import DefaultStoreSelection.
Require Import RegistryIsolation.

Module MeTTaTron_GC_RootDiscoveryErasure.

Import MeTTaTron_GC_DefaultStoreSelection.
Import MeTTaTron_GC_RegistryIsolation.

Section RootDiscoveryErasureModel.
  Inductive LegacyRootDiscoveryEffect : Type :=
  | UseRootProviderTrait : LegacyRootDiscoveryEffect
  | InitializeRootRegistry : LegacyRootDiscoveryEffect
  | RegisterRootProvider : LegacyRootDiscoveryEffect
  | CollectAllRoots : LegacyRootDiscoveryEffect
  | CollectAllRootsReadonly : LegacyRootDiscoveryEffect
  | PushFrameChain : LegacyRootDiscoveryEffect
  | CollectFrameChainRoots : LegacyRootDiscoveryEffect
  | RegisterCurrentIterRootProvider : LegacyRootDiscoveryEffect.

  Definition legacy_discovery_allowed (store : Store) : bool :=
    match store with
    | IndexStore => false
    | SlabStore => true
    end.

  Definition legacy_discovery_effect
      (features : FeatureSet)
      (requested : option LegacyRootDiscoveryEffect)
      : option LegacyRootDiscoveryEffect :=
    match active_store features, requested with
    | Some selected, Some effect =>
        if legacy_discovery_allowed selected
        then Some effect
        else None
    | _, _ => None
    end.

  Definition legacy_discovery_source
      (_effect : LegacyRootDiscoveryEffect)
      : RootSource :=
    RegistrySource.

  Theorem every_legacy_discovery_effect_is_not_index_source :
    forall effect,
      ~ IndexRootSource (legacy_discovery_source effect).
  Proof.
    intros effect.
    unfold legacy_discovery_source.
    apply registry_is_not_index_root_source.
  Qed.

  Theorem index_selection_erases_legacy_discovery_effects :
    forall features requested,
      active_store features = Some IndexStore ->
      legacy_discovery_effect features requested = None.
  Proof.
    intros features requested Hstore.
    unfold legacy_discovery_effect.
    rewrite Hstore.
    destruct requested; reflexivity.
  Qed.

  Theorem default_index_erases_legacy_discovery_effects :
    forall requested,
      legacy_discovery_effect default_invocation requested = None.
  Proof.
    intros requested.
    apply index_selection_erases_legacy_discovery_effects.
    apply default_invocation_selects_index.
  Qed.

  Theorem explicit_index_erases_legacy_discovery_effects :
    forall requested,
      legacy_discovery_effect explicit_index_no_default requested = None.
  Proof.
    intros requested.
    apply index_selection_erases_legacy_discovery_effects.
    apply explicit_index_no_default_selects_index.
  Qed.

  Theorem legacy_slab_preserves_requested_discovery_effect :
    forall effect,
      legacy_discovery_effect legacy_slab_opt_out (Some effect) = Some effect.
  Proof.
    intros effect.
    reflexivity.
  Qed.

  Theorem no_requested_discovery_effect_emits_no_effect :
    forall features,
      legacy_discovery_effect features None = None.
  Proof.
    intros features.
    unfold legacy_discovery_effect.
    destruct (active_store features); reflexivity.
  Qed.

  Theorem emitted_discovery_effect_requires_slab :
    forall features requested effect,
      legacy_discovery_effect features requested = Some effect ->
      active_store features = Some SlabStore.
  Proof.
    intros features requested effect Heffect.
    unfold legacy_discovery_effect in Heffect.
    destruct (active_store features) as [selected |] eqn:Hstore.
    - destruct selected.
      + destruct requested; discriminate.
      + destruct requested; try discriminate.
        reflexivity.
    - discriminate.
  Qed.

  Theorem emitted_discovery_effect_is_slab_and_not_index_source :
    forall features requested effect,
      legacy_discovery_effect features requested = Some effect ->
      active_store features = Some SlabStore /\
      ~ IndexRootSource (legacy_discovery_source effect).
  Proof.
    intros features requested effect Heffect.
    split.
    - exact (emitted_discovery_effect_requires_slab
               features requested effect Heffect).
    - apply every_legacy_discovery_effect_is_not_index_source.
  Qed.

  Theorem index_discovery_effect_contradiction :
    forall features requested effect,
      active_store features = Some IndexStore ->
      legacy_discovery_effect features requested = Some effect ->
      False.
  Proof.
    intros features requested effect Hindex Heffect.
    rewrite (index_selection_erases_legacy_discovery_effects
               features requested Hindex) in Heffect.
    discriminate.
  Qed.

  Theorem default_index_discovery_effect_contradiction :
    forall requested effect,
      legacy_discovery_effect default_invocation requested = Some effect ->
      False.
  Proof.
    intros requested effect Heffect.
    apply (index_discovery_effect_contradiction
             default_invocation requested effect
             default_invocation_selects_index Heffect).
  Qed.

  Theorem legacy_only_value_not_index_collector_root :
    forall (Addr : Type)
           (StructuralRoot DriverRoot LegacyDiscoveryRoot : Addr -> Prop)
           (a : Addr),
      LegacyDiscoveryRoot a ->
      ~ StructuralRoot a ->
      ~ DriverRoot a ->
      ~ IndexRoot Addr StructuralRoot DriverRoot a.
  Proof.
    intros Addr StructuralRoot DriverRoot LegacyDiscoveryRoot a
           Hlegacy Hnot_structural Hnot_driver.
    apply registry_only_value_not_index_root
      with (RegistryRoot := LegacyDiscoveryRoot).
    - exact Hlegacy.
    - exact Hnot_structural.
    - exact Hnot_driver.
  Qed.

  Theorem future_touch_survives_without_legacy_discovery :
    forall (Addr : Type)
           (StructuralRoot DriverRoot LegacyDiscoveryRoot FutureTouch
            Marked Freed : Addr -> Prop)
      (Edge : Addr -> Addr -> Prop),
      (forall a, FutureTouch a ->
        Reach Addr (IndexRoot Addr StructuralRoot DriverRoot) Edge a) ->
      (forall a, Reach Addr (IndexRoot Addr StructuralRoot DriverRoot) Edge a ->
        Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a,
        FutureTouch a ->
        ~ Freed a.
  Proof.
    intros Addr StructuralRoot DriverRoot LegacyDiscoveryRoot FutureTouch
           Marked Freed Edge Hfuture Hmark Hsweep a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.
End RootDiscoveryErasureModel.

End MeTTaTron_GC_RootDiscoveryErasure.
