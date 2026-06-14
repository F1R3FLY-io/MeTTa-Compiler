(** Rocq model of F4 inner-pointer decode branch erasure.

    [MettaValue::inner_ptr] used a runtime [gc_mode_is_index()] branch to
    choose between an index-mode identity key and a legacy slab pointer.  With
    runtime mode erased, the choice is entirely the compile-time [index-gc]
    feature, so the method can be split into cfg-selected implementations.

    This proof models that split without relying on any pointer arithmetic:
    [null_ptr], [index_key], and [slab_ptr] are arbitrary values of the same
    pointer/key carrier type.
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_InnerPtrDecodeErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section InnerPtrDecodeErasureModel.
  Variable Ptr : Type.

  Definition compiled_index_feature (features : FeatureSet) : bool :=
    has_index_gc features.

  Definition guarded_inner_ptr
      (features : FeatureSet)
      (is_inline : bool)
      (null_ptr index_key slab_ptr : Ptr)
      : Ptr :=
    if is_inline
    then null_ptr
    else
      if compiled_index_feature features
      then index_key
      else slab_ptr.

  Definition cfg_split_inner_ptr
      (features : FeatureSet)
      (is_inline : bool)
      (null_ptr index_key slab_ptr : Ptr)
      : Ptr :=
    if compiled_index_feature features
    then
      if is_inline
      then null_ptr
      else index_key
    else
      if is_inline
      then null_ptr
      else slab_ptr.

  Theorem guarded_inner_ptr_matches_cfg_split :
    forall features is_inline null_ptr index_key slab_ptr,
      guarded_inner_ptr features is_inline null_ptr index_key slab_ptr =
      cfg_split_inner_ptr features is_inline null_ptr index_key slab_ptr.
  Proof.
    intros features is_inline null_ptr index_key slab_ptr.
    unfold guarded_inner_ptr, cfg_split_inner_ptr, compiled_index_feature.
    destruct (has_index_gc features), is_inline; reflexivity.
  Qed.

  Theorem default_index_heap_uses_index_key :
    forall null_ptr index_key slab_ptr,
      cfg_split_inner_ptr default_invocation false null_ptr index_key slab_ptr =
      index_key.
  Proof.
    intros null_ptr index_key slab_ptr.
    reflexivity.
  Qed.

  Theorem default_index_inline_uses_null :
    forall null_ptr index_key slab_ptr,
      cfg_split_inner_ptr default_invocation true null_ptr index_key slab_ptr =
      null_ptr.
  Proof.
    intros null_ptr index_key slab_ptr.
    reflexivity.
  Qed.

  Theorem explicit_index_heap_uses_index_key :
    forall null_ptr index_key slab_ptr,
      cfg_split_inner_ptr explicit_index_no_default false null_ptr index_key slab_ptr =
      index_key.
  Proof.
    intros null_ptr index_key slab_ptr.
    reflexivity.
  Qed.

  Theorem legacy_slab_heap_uses_slab_ptr :
    forall null_ptr index_key slab_ptr,
      cfg_split_inner_ptr legacy_slab_opt_out false null_ptr index_key slab_ptr =
      slab_ptr.
  Proof.
    intros null_ptr index_key slab_ptr.
    reflexivity.
  Qed.

  Theorem legacy_slab_inline_uses_null :
    forall null_ptr index_key slab_ptr,
      cfg_split_inner_ptr legacy_slab_opt_out true null_ptr index_key slab_ptr =
      null_ptr.
  Proof.
    intros null_ptr index_key slab_ptr.
    reflexivity.
  Qed.

  Theorem valid_index_key_result_requires_index_store_for_heap :
    forall features null_ptr index_key slab_ptr,
      slab_ptr <> index_key ->
      valid_store_selection features ->
      cfg_split_inner_ptr features false null_ptr index_key slab_ptr = index_key ->
      active_store features = Some IndexStore.
  Proof.
    intros [default_on explicit_index legacy_slab] null_ptr index_key slab_ptr
           Hdisjoint Hvalid Hptr.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_inner_ptr, compiled_index_feature, has_index_gc in Hptr.
    simpl in Hptr.
    destruct default_on, explicit_index, legacy_slab;
      simpl in Hptr;
      simpl in Hvalid;
      try reflexivity;
      try (exfalso; apply Hdisjoint; exact Hptr);
      destruct Hvalid as [store Hstore];
      discriminate.
  Qed.
End InnerPtrDecodeErasureModel.

End MeTTaTron_GC_InnerPtrDecodeErasure.
