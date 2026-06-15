(** Rocq model of JIT type-operation store selection.

    The [jit_runtime_get_type] FFI helper may allocate a type-name atom while
    answering get-type/check-type queries.  Under the store-centric GC seam, the
    factory used for that allocation is selected by the compiled store:

      - an index-gc build must allocate through the active index factory;
      - a legacy slab build may keep using the slab factory, with the JIT
        arena pointer selecting only the slab allocator instance;
      - in an index-gc build, the JIT arena pointer must not select the store.

    This proof discharges the source-coupling obligation for the cfg-selected
    factory construction in [type_ops.rs::jit_runtime_get_type].
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_JitTypeOpsStoreSelection.

Import MeTTaTron_GC_DefaultStoreSelection.

Section JitTypeOpsStoreSelectionModel.
  Inductive TypeFactoryKind : Type :=
  | TypeActiveIndexFactory : TypeFactoryKind
  | TypeLegacySlabFactory : TypeFactoryKind.

  Inductive TypeAllocatorSource : Type :=
  | TypeActiveIndexArena : TypeAllocatorSource
  | TypeJitContextSlabArena : TypeAllocatorSource
  | TypeGlobalSlabAllocator : TypeAllocatorSource.

  Record TypeFactorySelection : Type := {
    type_factory_kind : TypeFactoryKind;
    type_allocator_source : TypeAllocatorSource
  }.

  Definition slab_type_factory_selection (ctx_arena_present : bool)
      : TypeFactorySelection :=
    {| type_factory_kind := TypeLegacySlabFactory;
       type_allocator_source :=
         if ctx_arena_present
         then TypeJitContextSlabArena
         else TypeGlobalSlabAllocator |}.

  Definition index_type_factory_selection : TypeFactorySelection :=
    {| type_factory_kind := TypeActiveIndexFactory;
       type_allocator_source := TypeActiveIndexArena |}.

  (** Correct cfg split: the compiled store chooses the factory kind.  The
      context arena pointer can affect only the legacy slab allocator instance. *)
  Definition cfg_split_type_ops_selection
      (features : FeatureSet)
      (ctx_arena_present : bool)
      : option TypeFactorySelection :=
    match active_store features with
    | Some IndexStore => Some index_type_factory_selection
    | Some SlabStore => Some (slab_type_factory_selection ctx_arena_present)
    | None => None
    end.

  (** The unsafe pre-correction shape: every valid store selection uses a slab
      factory, so an index-gc build can accidentally allocate type atoms through
      the slab factory. *)
  Definition unconditional_slab_type_ops_selection
      (features : FeatureSet)
      (ctx_arena_present : bool)
      : option TypeFactorySelection :=
    match active_store features with
    | Some _ => Some (slab_type_factory_selection ctx_arena_present)
    | None => None
    end.

  Definition type_factory_matches_store
      (features : FeatureSet)
      (selection : TypeFactorySelection)
      : Prop :=
    match active_store features, type_factory_kind selection with
    | Some IndexStore, TypeActiveIndexFactory => True
    | Some SlabStore, TypeLegacySlabFactory => True
    | _, _ => False
    end.

  Theorem default_index_get_type_uses_active_index_factory :
    forall ctx_arena_present,
      cfg_split_type_ops_selection default_invocation ctx_arena_present =
      Some index_type_factory_selection.
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem explicit_index_get_type_uses_active_index_factory :
    forall ctx_arena_present,
      cfg_split_type_ops_selection explicit_index_no_default ctx_arena_present =
      Some index_type_factory_selection.
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem legacy_slab_get_type_uses_legacy_slab_factory :
    forall ctx_arena_present,
      cfg_split_type_ops_selection legacy_slab_opt_out ctx_arena_present =
      Some (slab_type_factory_selection ctx_arena_present).
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem cfg_split_type_ops_selection_matches_valid_store :
    forall features ctx_arena_present selection,
      valid_store_selection features ->
      cfg_split_type_ops_selection features ctx_arena_present =
        Some selection ->
      type_factory_matches_store features selection.
  Proof.
    intros [default_on explicit_index legacy_slab] ctx_arena_present selection
           Hvalid Hselection.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_type_ops_selection, type_factory_matches_store,
      slab_type_factory_selection, index_type_factory_selection in *.
    simpl in *.
    destruct default_on, explicit_index, legacy_slab;
      simpl in *;
      try (inversion Hselection; subst; simpl; exact I);
      try (destruct Hvalid as [store Hstore]; discriminate);
      discriminate.
  Qed.

  Theorem emitted_type_ops_slab_factory_requires_slab_store :
    forall features ctx_arena_present selection,
      valid_store_selection features ->
      cfg_split_type_ops_selection features ctx_arena_present =
        Some selection ->
      type_factory_kind selection = TypeLegacySlabFactory ->
      active_store features = Some SlabStore.
  Proof.
    intros [default_on explicit_index legacy_slab] ctx_arena_present selection
           Hvalid Hselection Hslab.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_type_ops_selection, slab_type_factory_selection,
      index_type_factory_selection in *.
    simpl in *.
    destruct default_on, explicit_index, legacy_slab;
      simpl in *;
      try (inversion Hselection; subst; simpl in Hslab; discriminate);
      try reflexivity;
      destruct Hvalid as [store Hstore]; discriminate.
  Qed.

  Theorem index_get_type_selection_never_uses_runtime_arena_pointer :
    forall features ctx_arena_present selection,
      active_store features = Some IndexStore ->
      cfg_split_type_ops_selection features ctx_arena_present =
        Some selection ->
      type_allocator_source selection = TypeActiveIndexArena.
  Proof.
    intros features ctx_arena_present selection Hstore Hselection.
    unfold cfg_split_type_ops_selection in Hselection.
    rewrite Hstore in Hselection.
    inversion Hselection.
    reflexivity.
  Qed.

  Theorem default_index_get_type_ignores_context_arena_presence :
    cfg_split_type_ops_selection default_invocation true =
    cfg_split_type_ops_selection default_invocation false.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_get_type_ignores_context_arena_presence :
    cfg_split_type_ops_selection explicit_index_no_default true =
    cfg_split_type_ops_selection explicit_index_no_default false.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_get_type_context_arena_selects_slab_allocator_instance :
    cfg_split_type_ops_selection legacy_slab_opt_out true =
    Some
      {| type_factory_kind := TypeLegacySlabFactory;
         type_allocator_source := TypeJitContextSlabArena |}.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_get_type_without_context_arena_uses_global_slab_allocator :
    cfg_split_type_ops_selection legacy_slab_opt_out false =
    Some
      {| type_factory_kind := TypeLegacySlabFactory;
         type_allocator_source := TypeGlobalSlabAllocator |}.
  Proof.
    reflexivity.
  Qed.

  Theorem unconditional_slab_type_ops_mismatches_default_index :
    forall ctx_arena_present,
      unconditional_slab_type_ops_selection
        default_invocation ctx_arena_present <>
      cfg_split_type_ops_selection default_invocation ctx_arena_present.
  Proof.
    intros ctx_arena_present Heq.
    unfold unconditional_slab_type_ops_selection,
      cfg_split_type_ops_selection, slab_type_factory_selection,
      index_type_factory_selection in Heq.
    destruct ctx_arena_present; discriminate.
  Qed.

  Theorem unconditional_slab_type_ops_mismatches_explicit_index :
    forall ctx_arena_present,
      unconditional_slab_type_ops_selection
        explicit_index_no_default ctx_arena_present <>
      cfg_split_type_ops_selection
        explicit_index_no_default ctx_arena_present.
  Proof.
    intros ctx_arena_present Heq.
    unfold unconditional_slab_type_ops_selection,
      cfg_split_type_ops_selection, slab_type_factory_selection,
      index_type_factory_selection in Heq.
    destruct ctx_arena_present; discriminate.
  Qed.
End JitTypeOpsStoreSelectionModel.

End MeTTaTron_GC_JitTypeOpsStoreSelection.
