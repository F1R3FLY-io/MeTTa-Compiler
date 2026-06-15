(** JIT payload conversion store policy.

    Generic JIT conversion helpers encode primitive values inline and encode
    heap/error values as TAG_PTR/TAG_ERROR payloads.  Those payloads are
    store-shaped:

      - index-gc heap/error payloads are arena handles and must be
        reconstructed through the mode-aware handle path;
      - legacy slab heap/error payloads are slab pointers and may be
        dereferenced;
      - inline values require no heap inspection in either store.

    This proof generalizes the special [jit_runtime_is_function] TAG_PTR
    decode proof to [metta_to_jit], [jit_to_value_generic],
    [make_jit_error], and [JitValue::to_metta].
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_JitPayloadConversionStorePolicy.

Import MeTTaTron_GC_DefaultStoreSelection.

Section JitPayloadConversionStorePolicyModel.
  Inductive JitPayloadKind : Type :=
  | InlinePayload : JitPayloadKind
  | HeapPayload : JitPayloadKind
  | ErrorPayload : JitPayloadKind.

  Inductive PayloadDecodeAction : Type :=
  | DecodeInline : PayloadDecodeAction
  | ReconstructIndexHandle : PayloadDecodeAction
  | DerefLegacySlabPointer : PayloadDecodeAction.

  Definition cfg_split_payload_decode
      (features : FeatureSet)
      (kind : JitPayloadKind)
      : option PayloadDecodeAction :=
    match kind with
    | InlinePayload => Some DecodeInline
    | HeapPayload | ErrorPayload =>
        match active_store features with
        | Some IndexStore => Some ReconstructIndexHandle
        | Some SlabStore => Some DerefLegacySlabPointer
        | None => None
        end
    end.

  (** Unsafe pre-correction shape: every valid non-inline store decodes through
      a slab dereference, which is wrong for index-gc arena handles. *)
  Definition slab_deref_payload_decode
      (features : FeatureSet)
      (kind : JitPayloadKind)
      : option PayloadDecodeAction :=
    match kind with
    | InlinePayload => Some DecodeInline
    | HeapPayload | ErrorPayload =>
        match active_store features with
        | Some _ => Some DerefLegacySlabPointer
        | None => None
        end
    end.

  (** Unsafe shape for inline payloads: primitives perform heap inspection even
      though they carry all data in the NaN-boxed payload. *)
  Definition inline_deref_payload_decode
      (features : FeatureSet)
      (kind : JitPayloadKind)
      : option PayloadDecodeAction :=
    match kind with
    | InlinePayload => Some DerefLegacySlabPointer
    | HeapPayload | ErrorPayload => cfg_split_payload_decode features kind
    end.

  Definition payload_decode_matches_store
      (features : FeatureSet)
      (kind : JitPayloadKind)
      (action : PayloadDecodeAction)
      : Prop :=
    match kind, action with
    | InlinePayload, DecodeInline => True
    | InlinePayload, _ => False
    | HeapPayload, ReconstructIndexHandle
    | ErrorPayload, ReconstructIndexHandle =>
        active_store features = Some IndexStore
    | HeapPayload, DerefLegacySlabPointer
    | ErrorPayload, DerefLegacySlabPointer =>
        active_store features = Some SlabStore
    | HeapPayload, DecodeInline
    | ErrorPayload, DecodeInline => False
    end.

  Theorem inline_payload_decodes_without_heap_inspection :
    forall features,
      cfg_split_payload_decode features InlinePayload = Some DecodeInline.
  Proof.
    intros features.
    reflexivity.
  Qed.

  Theorem default_index_heap_payload_reconstructs_handle :
    cfg_split_payload_decode default_invocation HeapPayload =
    Some ReconstructIndexHandle.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_heap_payload_reconstructs_handle :
    cfg_split_payload_decode explicit_index_no_default HeapPayload =
    Some ReconstructIndexHandle.
  Proof.
    reflexivity.
  Qed.

  Theorem default_index_error_payload_reconstructs_handle :
    cfg_split_payload_decode default_invocation ErrorPayload =
    Some ReconstructIndexHandle.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_error_payload_reconstructs_handle :
    cfg_split_payload_decode explicit_index_no_default ErrorPayload =
    Some ReconstructIndexHandle.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_heap_payload_dereferences_slab_pointer :
    cfg_split_payload_decode legacy_slab_opt_out HeapPayload =
    Some DerefLegacySlabPointer.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_error_payload_dereferences_slab_pointer :
    cfg_split_payload_decode legacy_slab_opt_out ErrorPayload =
    Some DerefLegacySlabPointer.
  Proof.
    reflexivity.
  Qed.

  Theorem cfg_split_payload_decode_matches_valid_store :
    forall features kind action,
      valid_store_selection features ->
      cfg_split_payload_decode features kind = Some action ->
      payload_decode_matches_store features kind action.
  Proof.
    intros features kind action _ Hdecode.
    destruct kind as [| |].
    - unfold cfg_split_payload_decode in Hdecode.
      simpl in Hdecode.
      inversion Hdecode.
      subst.
      simpl.
      exact I.
    - unfold cfg_split_payload_decode in Hdecode.
      simpl in Hdecode.
      destruct (active_store features) as [store |] eqn:Hstore.
      + destruct store; inversion Hdecode; subst; simpl; exact Hstore.
      + discriminate.
    - unfold cfg_split_payload_decode in Hdecode.
      simpl in Hdecode.
      destruct (active_store features) as [store |] eqn:Hstore.
      + destruct store; inversion Hdecode; subst; simpl; exact Hstore.
      + discriminate.
  Qed.

  Theorem emitted_payload_slab_deref_requires_slab_store :
    forall features kind,
      valid_store_selection features ->
      cfg_split_payload_decode features kind =
        Some DerefLegacySlabPointer ->
      active_store features = Some SlabStore.
  Proof.
    intros features kind Hvalid Hdecode.
    pose proof
      (cfg_split_payload_decode_matches_valid_store
         features kind DerefLegacySlabPointer Hvalid Hdecode) as Hmatches.
    destruct kind as [| |]; simpl in Hmatches; [contradiction | exact Hmatches | exact Hmatches].
  Qed.

  Theorem index_payload_never_dereferences_slab :
    forall features kind,
      active_store features = Some IndexStore ->
      cfg_split_payload_decode features kind <>
        Some DerefLegacySlabPointer.
  Proof.
    intros features kind Hstore Hdecode.
    unfold cfg_split_payload_decode in Hdecode.
    destruct kind as [| |];
      simpl in Hdecode;
      try discriminate;
      rewrite Hstore in Hdecode;
      discriminate.
  Qed.

  Theorem inline_payload_result_is_inline_decode :
    forall features action,
      cfg_split_payload_decode features InlinePayload = Some action ->
      action = DecodeInline.
  Proof.
    intros features action Hdecode.
    inversion Hdecode.
    reflexivity.
  Qed.

  Theorem slab_deref_payload_shape_violates_default_index_heap :
    slab_deref_payload_decode default_invocation HeapPayload =
      Some DerefLegacySlabPointer /\
    ~ payload_decode_matches_store
        default_invocation HeapPayload DerefLegacySlabPointer.
  Proof.
    split.
    - reflexivity.
    - unfold payload_decode_matches_store.
      discriminate.
  Qed.

  Theorem slab_deref_payload_shape_violates_default_index_error :
    slab_deref_payload_decode default_invocation ErrorPayload =
      Some DerefLegacySlabPointer /\
    ~ payload_decode_matches_store
        default_invocation ErrorPayload DerefLegacySlabPointer.
  Proof.
    split.
    - reflexivity.
    - unfold payload_decode_matches_store.
      discriminate.
  Qed.

  Theorem slab_deref_payload_shape_violates_explicit_index_heap :
    slab_deref_payload_decode explicit_index_no_default HeapPayload =
      Some DerefLegacySlabPointer /\
    ~ payload_decode_matches_store
        explicit_index_no_default HeapPayload DerefLegacySlabPointer.
  Proof.
    split.
    - reflexivity.
    - unfold payload_decode_matches_store.
      discriminate.
  Qed.

  Theorem inline_payload_deref_shape_violates_no_heap_inspection :
    inline_deref_payload_decode default_invocation InlinePayload =
      Some DerefLegacySlabPointer /\
    ~ payload_decode_matches_store
        default_invocation InlinePayload DerefLegacySlabPointer.
  Proof.
    split.
    - reflexivity.
    - simpl.
      intros Hfalse.
      exact Hfalse.
  Qed.
End JitPayloadConversionStorePolicyModel.

End MeTTaTron_GC_JitPayloadConversionStorePolicy.
