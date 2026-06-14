(** JIT out-of-line Long store selection.

    [JitValue::from_long] encodes small signed integers inline.  Values outside
    the inline 48-bit range must be boxed.  Under the store-centric F4 seam,
    that boxing store is selected by the compiled store, not by a runtime
    gc-mode branch that leaves a slab fallback compiled into index builds.
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_JitLongBoxStoreSelection.

Import MeTTaTron_GC_DefaultStoreSelection.

Section JitLongBoxStoreSelectionModel.
  Inductive LongEncoding : Type :=
  | InlineLong : LongEncoding
  | IndexHeapLong : LongEncoding
  | LegacySlabLong : LongEncoding.

  Definition cfg_split_long_selection
      (features : FeatureSet)
      (inline_fits : bool)
      : option LongEncoding :=
    if inline_fits then Some InlineLong
    else
      match active_store features with
      | Some IndexStore => Some IndexHeapLong
      | Some SlabStore => Some LegacySlabLong
      | None => None
      end.

  Definition cfg_guarded_long_selection
      (features : FeatureSet)
      (inline_fits : bool)
      : option LongEncoding :=
    if inline_fits then Some InlineLong
    else if has_index_gc features then Some IndexHeapLong
    else
      match active_store features with
      | Some SlabStore => Some LegacySlabLong
      | _ => None
      end.

  Definition unconditional_slab_long_selection
      (features : FeatureSet)
      (inline_fits : bool)
      : option LongEncoding :=
    if inline_fits then Some InlineLong
    else
      match active_store features with
      | Some _ => Some LegacySlabLong
      | None => None
      end.

  Definition long_selection_matches_store
      (features : FeatureSet)
      (encoding : LongEncoding)
      : Prop :=
    match encoding with
    | InlineLong => True
    | IndexHeapLong => active_store features = Some IndexStore
    | LegacySlabLong => active_store features = Some SlabStore
    end.

  Theorem inline_long_does_not_select_heap_store :
    forall features,
      cfg_split_long_selection features true = Some InlineLong.
  Proof.
    intros features.
    reflexivity.
  Qed.

  Theorem default_index_overflow_uses_index_heap :
    cfg_split_long_selection default_invocation false = Some IndexHeapLong.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_overflow_uses_index_heap :
    cfg_split_long_selection explicit_index_no_default false = Some IndexHeapLong.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_overflow_uses_legacy_slab :
    cfg_split_long_selection legacy_slab_opt_out false = Some LegacySlabLong.
  Proof.
    reflexivity.
  Qed.

  Theorem guarded_and_cfg_split_agree_for_valid_features :
    forall features inline_fits selection,
      valid_store_selection features ->
      cfg_guarded_long_selection features inline_fits = Some selection ->
      cfg_split_long_selection features inline_fits = Some selection.
  Proof.
    intros [default_on explicit_index legacy_slab] inline_fits selection
           Hvalid Hguarded.
    unfold valid_store_selection in Hvalid.
    unfold cfg_guarded_long_selection, cfg_split_long_selection,
      active_store, has_index_gc, has_legacy_slab_gc in *.
    simpl in *.
    destruct inline_fits.
    - exact Hguarded.
    - destruct default_on, explicit_index, legacy_slab;
        simpl in *;
        try exact Hguarded;
        try discriminate;
        destruct Hvalid as [store Hstore];
        discriminate.
  Qed.

  Theorem cfg_split_overflow_selection_matches_store :
    forall features selection,
      valid_store_selection features ->
      cfg_split_long_selection features false = Some selection ->
      long_selection_matches_store features selection.
  Proof.
    intros features selection _ Hselection.
    unfold cfg_split_long_selection in Hselection.
    unfold long_selection_matches_store.
    destruct (active_store features) as [store |] eqn:Hstore.
    - destruct store; inversion Hselection; subst; reflexivity.
    - discriminate.
  Qed.

  Theorem emitted_slab_overflow_requires_slab_store :
    forall features,
      valid_store_selection features ->
      cfg_split_long_selection features false = Some LegacySlabLong ->
      active_store features = Some SlabStore.
  Proof.
    intros features Hvalid Hselection.
    pose proof
      (cfg_split_overflow_selection_matches_store
         features LegacySlabLong Hvalid Hselection) as Hmatches.
    exact Hmatches.
  Qed.

  Theorem index_overflow_never_uses_legacy_slab :
    forall features,
      active_store features = Some IndexStore ->
      cfg_split_long_selection features false <> Some LegacySlabLong.
  Proof.
    intros features Hstore Hselection.
    unfold cfg_split_long_selection in Hselection.
    rewrite Hstore in Hselection.
    discriminate.
  Qed.

  Theorem unconditional_slab_misselects_default_index_overflow :
    ~ long_selection_matches_store
        default_invocation
        LegacySlabLong.
  Proof.
    unfold long_selection_matches_store.
    discriminate.
  Qed.

  Theorem unconditional_slab_overflow_violates_default_index :
    unconditional_slab_long_selection default_invocation false =
      Some LegacySlabLong /\
    ~ long_selection_matches_store default_invocation LegacySlabLong.
  Proof.
    split.
    - reflexivity.
    - apply unconditional_slab_misselects_default_index_overflow.
  Qed.
End JitLongBoxStoreSelectionModel.

End MeTTaTron_GC_JitLongBoxStoreSelection.
