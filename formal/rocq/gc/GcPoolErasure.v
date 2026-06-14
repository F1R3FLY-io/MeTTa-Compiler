(** Rocq model of the F4 legacy GC-pool erasure obligation.

    The adaptive [gc_pool.rs] worker pool is the legacy slab mark-sweep/session
    release executor.  In the index store, collection is driven by the dedicated
    CESK collector and its rendezvous driver; initializing or submitting work to
    the slab pool is therefore bridge surface, not part of the index regime.

    This file proves the source-edit precondition: default/explicit index store
    selections erase every legacy pool effect, while any emitted legacy pool
    effect requires the active store to be [SlabStore].
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_GcPoolErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section GcPoolErasureModel.
  Inductive LegacyPoolEffect : Type :=
  | InitializePool : LegacyPoolEffect
  | ScaleWorkers : LegacyPoolEffect
  | SubmitCollect : LegacyPoolEffect
  | SubmitSessionRelease : LegacyPoolEffect
  | ReceiveResponse : LegacyPoolEffect.

  Definition legacy_pool_effect_allowed (store : Store) : bool :=
    match store with
    | IndexStore => false
    | SlabStore => true
    end.

  Definition legacy_pool_effect
      (features : FeatureSet)
      (requested : option LegacyPoolEffect) : option LegacyPoolEffect :=
    match active_store features, requested with
    | Some selected, Some effect =>
        if legacy_pool_effect_allowed selected
        then Some effect
        else None
    | _, _ => None
    end.

  Theorem index_selection_erases_legacy_pool_effects :
    forall features requested,
      active_store features = Some IndexStore ->
      legacy_pool_effect features requested = None.
  Proof.
    intros features requested Hstore.
    unfold legacy_pool_effect.
    rewrite Hstore.
    destruct requested; reflexivity.
  Qed.

  Theorem default_index_erases_legacy_pool_effects :
    forall requested,
      legacy_pool_effect default_invocation requested = None.
  Proof.
    intros requested.
    apply index_selection_erases_legacy_pool_effects.
    apply default_invocation_selects_index.
  Qed.

  Theorem explicit_index_erases_legacy_pool_effects :
    forall requested,
      legacy_pool_effect explicit_index_no_default requested = None.
  Proof.
    intros requested.
    apply index_selection_erases_legacy_pool_effects.
    apply explicit_index_no_default_selects_index.
  Qed.

  Theorem legacy_slab_preserves_requested_pool_effect :
    forall effect,
      legacy_pool_effect legacy_slab_opt_out (Some effect) = Some effect.
  Proof.
    intros effect.
    reflexivity.
  Qed.

  Theorem no_requested_pool_effect_emits_no_pool_effect :
    forall features,
      legacy_pool_effect features None = None.
  Proof.
    intros features.
    unfold legacy_pool_effect.
    destruct (active_store features); reflexivity.
  Qed.

  Theorem emitted_pool_effect_requires_slab :
    forall features requested effect,
      legacy_pool_effect features requested = Some effect ->
      active_store features = Some SlabStore.
  Proof.
    intros features requested effect Heffect.
    unfold legacy_pool_effect in Heffect.
    destruct (active_store features) as [selected |] eqn:Hstore.
    - destruct selected.
      + destruct requested; discriminate.
      + destruct requested; try discriminate.
        reflexivity.
    - discriminate.
  Qed.
End GcPoolErasureModel.

End MeTTaTron_GC_GcPoolErasure.
