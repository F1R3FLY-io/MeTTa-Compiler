(** Rocq model of Phase F3 GC store feature selection.

    Cargo features are additive: enabling a feature never disables another
    feature.  Therefore "index GC is the default" cannot be represented safely
    by only adding a legacy opt-out feature.  The policy must make the selected
    store total only when exactly one store feature is present:

    - default features select index GC;
    - explicit index GC with default features disabled still selects index GC;
    - legacy slab is valid only as a no-default-features opt-out;
    - both store features, or neither store feature, are invalid.
*)

From Stdlib Require Import Bool.Bool.

Module MeTTaTron_GC_DefaultStoreSelection.

Section FeatureSelectionModel.
  Inductive Store : Type :=
  | IndexStore : Store
  | SlabStore : Store.

  Record FeatureSet : Type := {
    default_features : bool;
    explicit_index_gc : bool;
    legacy_slab_gc : bool
  }.

  (** F3 target model: the Cargo default feature set contains [index-gc].
      With default features disabled, callers may request [index-gc] explicitly
      or request the legacy slab opt-out. *)
  Definition has_index_gc (features : FeatureSet) : bool :=
    default_features features || explicit_index_gc features.

  Definition has_legacy_slab_gc (features : FeatureSet) : bool :=
    legacy_slab_gc features.

  Definition active_store (features : FeatureSet) : option Store :=
    match has_index_gc features, has_legacy_slab_gc features with
    | true, false => Some IndexStore
    | false, true => Some SlabStore
    | _, _ => None
    end.

  Definition valid_store_selection (features : FeatureSet) : Prop :=
    exists store, active_store features = Some store.

  Definition exactly_one_store_feature (features : FeatureSet) : Prop :=
    xorb (has_index_gc features) (has_legacy_slab_gc features) = true.

  Definition default_invocation : FeatureSet :=
    {| default_features := true;
       explicit_index_gc := false;
       legacy_slab_gc := false |}.

  Definition explicit_index_no_default : FeatureSet :=
    {| default_features := false;
       explicit_index_gc := true;
       legacy_slab_gc := false |}.

  Definition legacy_slab_opt_out : FeatureSet :=
    {| default_features := false;
       explicit_index_gc := false;
       legacy_slab_gc := true |}.

  Definition no_store_feature : FeatureSet :=
    {| default_features := false;
       explicit_index_gc := false;
       legacy_slab_gc := false |}.

  Theorem default_invocation_selects_index :
    active_store default_invocation = Some IndexStore.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_no_default_selects_index :
    active_store explicit_index_no_default = Some IndexStore.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_opt_out_selects_slab :
    active_store legacy_slab_opt_out = Some SlabStore.
  Proof.
    reflexivity.
  Qed.

  Theorem no_store_feature_is_invalid :
    active_store no_store_feature = None.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_with_defaults_is_invalid :
    forall explicit_index,
      active_store
        {| default_features := true;
           explicit_index_gc := explicit_index;
           legacy_slab_gc := true |} = None.
  Proof.
    intros explicit_index.
    destruct explicit_index; reflexivity.
  Qed.

  Theorem both_explicit_store_features_invalid :
    forall default_on,
      active_store
        {| default_features := default_on;
           explicit_index_gc := true;
           legacy_slab_gc := true |} = None.
  Proof.
    intros default_on.
    destruct default_on; reflexivity.
  Qed.

  Theorem exactly_one_store_feature_iff_valid :
    forall features,
      exactly_one_store_feature features <-> valid_store_selection features.
  Proof.
    intros [default_on explicit_index legacy_slab].
    unfold exactly_one_store_feature, valid_store_selection, active_store,
      has_index_gc, has_legacy_slab_gc.
    simpl.
    destruct default_on, explicit_index, legacy_slab; split; intros H;
      try (exists IndexStore; reflexivity);
      try (exists SlabStore; reflexivity);
      try reflexivity;
      try discriminate;
      destruct H as [store Hstore]; discriminate.
  Qed.

  Theorem slab_requires_no_default_legacy_opt_out :
    forall features,
      active_store features = Some SlabStore ->
      default_features features = false /\
      explicit_index_gc features = false /\
      legacy_slab_gc features = true.
  Proof.
    intros [default_on explicit_index legacy_slab] Hstore.
    simpl in Hstore.
    destruct default_on, explicit_index, legacy_slab;
      simpl in Hstore; inversion Hstore; subst; repeat split; reflexivity.
  Qed.

  Theorem index_selection_excludes_legacy_slab :
    forall features,
      active_store features = Some IndexStore ->
      has_index_gc features = true /\ has_legacy_slab_gc features = false.
  Proof.
    intros [default_on explicit_index legacy_slab] Hstore.
    simpl in Hstore.
    destruct default_on, explicit_index, legacy_slab;
      simpl in Hstore; inversion Hstore; subst; repeat split; reflexivity.
  Qed.

  Theorem legacy_slab_is_never_silent :
    forall features,
      legacy_slab_gc features = false ->
      active_store features <> Some SlabStore.
  Proof.
    intros features Hno_legacy Hstore.
    pose proof (slab_requires_no_default_legacy_opt_out features Hstore)
      as [_ [_ Hlegacy]].
    rewrite Hno_legacy in Hlegacy.
    discriminate.
  Qed.
End FeatureSelectionModel.

End MeTTaTron_GC_DefaultStoreSelection.
