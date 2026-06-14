(** JIT [is-function] TAG_PTR decode policy.

    [jit_runtime_is_function] classifies heap values structurally by checking
    whether a value is an S-expression whose head is [->].  TAG_PTR payloads
    are store-shaped:

      - index-gc TAG_PTR payloads are arena handles and must be reconstructed
        with the mode-aware [from_inner_ptr] path;
      - legacy slab TAG_PTR payloads are slab pointers and may be dereferenced;
      - non-pointer values require no heap inspection.

    This proof discharges the source-edit precondition for replacing an
    unconditional slab dereference in [jit_runtime_is_function] with a
    compiled-store-selected decode.
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_JitIsFunctionPointerDecode.

Import MeTTaTron_GC_DefaultStoreSelection.

Section JitIsFunctionPointerDecodeModel.
  Inductive IsFunctionInspection : Type :=
  | NonPointerNoInspect : IsFunctionInspection
  | IndexHandleDecode : IsFunctionInspection
  | SlabPointerDeref : IsFunctionInspection.

  Definition cfg_split_is_function_inspection
      (features : FeatureSet)
      (tag_ptr : bool)
      : option IsFunctionInspection :=
    if tag_ptr then
      match active_store features with
      | Some IndexStore => Some IndexHandleDecode
      | Some SlabStore => Some SlabPointerDeref
      | None => None
      end
    else Some NonPointerNoInspect.

  Definition slab_deref_is_function_inspection
      (features : FeatureSet)
      (tag_ptr : bool)
      : option IsFunctionInspection :=
    if tag_ptr then
      match active_store features with
      | Some _ => Some SlabPointerDeref
      | None => None
      end
    else Some NonPointerNoInspect.

  Definition inspection_matches_store
      (features : FeatureSet)
      (inspection : IsFunctionInspection)
      : Prop :=
    match inspection with
    | NonPointerNoInspect => True
    | IndexHandleDecode => active_store features = Some IndexStore
    | SlabPointerDeref => active_store features = Some SlabStore
    end.

  Theorem non_pointer_values_do_not_inspect_heap :
    forall features,
      cfg_split_is_function_inspection features false =
      Some NonPointerNoInspect.
  Proof.
    intros features.
    reflexivity.
  Qed.

  Theorem default_index_pointer_decodes_index_handle :
    cfg_split_is_function_inspection default_invocation true =
    Some IndexHandleDecode.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_pointer_decodes_index_handle :
    cfg_split_is_function_inspection explicit_index_no_default true =
    Some IndexHandleDecode.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_pointer_dereferences_slab_pointer :
    cfg_split_is_function_inspection legacy_slab_opt_out true =
    Some SlabPointerDeref.
  Proof.
    reflexivity.
  Qed.

  Theorem cfg_split_pointer_inspection_matches_valid_store :
    forall features inspection,
      valid_store_selection features ->
      cfg_split_is_function_inspection features true = Some inspection ->
      inspection_matches_store features inspection.
  Proof.
    intros features inspection _ Hinspection.
    unfold cfg_split_is_function_inspection in Hinspection.
    unfold inspection_matches_store.
    destruct (active_store features) as [store |] eqn:Hstore.
    - destruct store; inversion Hinspection; subst; reflexivity.
    - discriminate.
  Qed.

  Theorem emitted_slab_deref_requires_slab_store :
    forall features,
      valid_store_selection features ->
      cfg_split_is_function_inspection features true = Some SlabPointerDeref ->
      active_store features = Some SlabStore.
  Proof.
    intros features Hvalid Hinspection.
    pose proof
      (cfg_split_pointer_inspection_matches_valid_store
         features SlabPointerDeref Hvalid Hinspection) as Hmatches.
    exact Hmatches.
  Qed.

  Theorem index_pointer_never_dereferences_slab :
    forall features,
      active_store features = Some IndexStore ->
      cfg_split_is_function_inspection features true <> Some SlabPointerDeref.
  Proof.
    intros features Hstore Hinspection.
    unfold cfg_split_is_function_inspection in Hinspection.
    rewrite Hstore in Hinspection.
    discriminate.
  Qed.

  Theorem slab_deref_shape_violates_default_index :
    slab_deref_is_function_inspection default_invocation true =
      Some SlabPointerDeref /\
    ~ inspection_matches_store default_invocation SlabPointerDeref.
  Proof.
    split.
    - reflexivity.
    - unfold inspection_matches_store.
      discriminate.
  Qed.

  Theorem slab_deref_shape_violates_explicit_index :
    slab_deref_is_function_inspection explicit_index_no_default true =
      Some SlabPointerDeref /\
    ~ inspection_matches_store explicit_index_no_default SlabPointerDeref.
  Proof.
    split.
    - reflexivity.
    - unfold inspection_matches_store.
      discriminate.
  Qed.
End JitIsFunctionPointerDecodeModel.

End MeTTaTron_GC_JitIsFunctionPointerDecode.
