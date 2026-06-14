(** Rocq model of JIT value-creation store selection.

    The JIT value-creation FFI helpers build heap values from NaN-boxed
    operands.  Under the store-centric GC seam, the factory used by those
    helpers is not a runtime choice:

      - an index-gc build must allocate through the active index factory;
      - a legacy slab build may keep using the slab factory, with the JIT
        arena pointer selecting only the slab allocator instance;
      - in an index-gc build, the JIT arena pointer must not select the store.

    This proof discharges the source-edit precondition for replacing an
    unconditional slab factory in value_creation.rs with cfg-selected active
    factory construction.
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_JitValueCreationStoreSelection.

Import MeTTaTron_GC_DefaultStoreSelection.

Section JitValueCreationStoreSelectionModel.
  Inductive FactoryKind : Type :=
  | ActiveIndexFactory : FactoryKind
  | LegacySlabFactory : FactoryKind.

  Inductive AllocatorSource : Type :=
  | ActiveIndexArena : AllocatorSource
  | JitContextSlabArena : AllocatorSource
  | GlobalSlabAllocator : AllocatorSource.

  Record FactorySelection : Type := {
    factory_kind : FactoryKind;
    allocator_source : AllocatorSource
  }.

  Definition slab_factory_selection (ctx_arena_present : bool)
      : FactorySelection :=
    {| factory_kind := LegacySlabFactory;
       allocator_source :=
         if ctx_arena_present then JitContextSlabArena else GlobalSlabAllocator |}.

  Definition index_factory_selection : FactorySelection :=
    {| factory_kind := ActiveIndexFactory;
       allocator_source := ActiveIndexArena |}.

  (** The corrected cfg split: choose the factory by the compiled store.  The
      context arena pointer can affect only the legacy slab allocator instance. *)
  Definition cfg_split_value_creation_selection
      (features : FeatureSet)
      (ctx_arena_present : bool)
      : option FactorySelection :=
    match active_store features with
    | Some IndexStore => Some index_factory_selection
    | Some SlabStore => Some (slab_factory_selection ctx_arena_present)
    | None => None
    end.

  (** The pre-correction shape: every valid store selection uses a slab factory,
      so an index-gc build can accidentally create slab-backed values. *)
  Definition unconditional_slab_value_creation_selection
      (features : FeatureSet)
      (ctx_arena_present : bool)
      : option FactorySelection :=
    match active_store features with
    | Some _ => Some (slab_factory_selection ctx_arena_present)
    | None => None
    end.

  Definition factory_matches_store
      (features : FeatureSet)
      (selection : FactorySelection)
      : Prop :=
    match active_store features, factory_kind selection with
    | Some IndexStore, ActiveIndexFactory => True
    | Some SlabStore, LegacySlabFactory => True
    | _, _ => False
    end.

  Theorem default_index_uses_active_index_factory :
    forall ctx_arena_present,
      cfg_split_value_creation_selection default_invocation ctx_arena_present =
      Some index_factory_selection.
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem explicit_index_uses_active_index_factory :
    forall ctx_arena_present,
      cfg_split_value_creation_selection explicit_index_no_default ctx_arena_present =
      Some index_factory_selection.
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem legacy_slab_uses_legacy_slab_factory :
    forall ctx_arena_present,
      cfg_split_value_creation_selection legacy_slab_opt_out ctx_arena_present =
      Some (slab_factory_selection ctx_arena_present).
  Proof.
    intros ctx_arena_present.
    reflexivity.
  Qed.

  Theorem cfg_split_selection_matches_valid_store :
    forall features ctx_arena_present selection,
      valid_store_selection features ->
      cfg_split_value_creation_selection features ctx_arena_present =
        Some selection ->
      factory_matches_store features selection.
  Proof.
    intros [default_on explicit_index legacy_slab] ctx_arena_present selection
           Hvalid Hselection.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_value_creation_selection, factory_matches_store,
      slab_factory_selection, index_factory_selection in *.
    simpl in *.
    destruct default_on, explicit_index, legacy_slab;
      simpl in *;
      try (inversion Hselection; subst; simpl; exact I);
      try (destruct Hvalid as [store Hstore]; discriminate);
      discriminate.
  Qed.

  Theorem emitted_slab_factory_requires_slab_store :
    forall features ctx_arena_present selection,
      valid_store_selection features ->
      cfg_split_value_creation_selection features ctx_arena_present =
        Some selection ->
      factory_kind selection = LegacySlabFactory ->
      active_store features = Some SlabStore.
  Proof.
    intros [default_on explicit_index legacy_slab] ctx_arena_present selection
           Hvalid Hselection Hslab.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_value_creation_selection, slab_factory_selection,
      index_factory_selection in *.
    simpl in *.
    destruct default_on, explicit_index, legacy_slab;
      simpl in *;
      try (inversion Hselection; subst; simpl in Hslab; discriminate);
      try reflexivity;
      destruct Hvalid as [store Hstore]; discriminate.
  Qed.

  Theorem index_selection_never_uses_runtime_arena_pointer :
    forall features ctx_arena_present selection,
      active_store features = Some IndexStore ->
      cfg_split_value_creation_selection features ctx_arena_present =
        Some selection ->
      allocator_source selection = ActiveIndexArena.
  Proof.
    intros features ctx_arena_present selection Hstore Hselection.
    unfold cfg_split_value_creation_selection in Hselection.
    rewrite Hstore in Hselection.
    inversion Hselection.
    reflexivity.
  Qed.

  Theorem default_index_ignores_context_arena_presence :
    cfg_split_value_creation_selection default_invocation true =
    cfg_split_value_creation_selection default_invocation false.
  Proof.
    reflexivity.
  Qed.

  Theorem explicit_index_ignores_context_arena_presence :
    cfg_split_value_creation_selection explicit_index_no_default true =
    cfg_split_value_creation_selection explicit_index_no_default false.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_context_arena_selects_slab_allocator_instance :
    cfg_split_value_creation_selection legacy_slab_opt_out true =
    Some
      {| factory_kind := LegacySlabFactory;
         allocator_source := JitContextSlabArena |}.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_without_context_arena_uses_global_slab_allocator :
    cfg_split_value_creation_selection legacy_slab_opt_out false =
    Some
      {| factory_kind := LegacySlabFactory;
         allocator_source := GlobalSlabAllocator |}.
  Proof.
    reflexivity.
  Qed.

  Theorem unconditional_slab_mismatches_default_index :
    forall ctx_arena_present,
      unconditional_slab_value_creation_selection
        default_invocation ctx_arena_present <>
      cfg_split_value_creation_selection default_invocation ctx_arena_present.
  Proof.
    intros ctx_arena_present Heq.
    unfold unconditional_slab_value_creation_selection,
      cfg_split_value_creation_selection, slab_factory_selection,
      index_factory_selection in Heq.
    destruct ctx_arena_present; discriminate.
  Qed.

  Theorem unconditional_slab_mismatches_explicit_index :
    forall ctx_arena_present,
      unconditional_slab_value_creation_selection
        explicit_index_no_default ctx_arena_present <>
      cfg_split_value_creation_selection
        explicit_index_no_default ctx_arena_present.
  Proof.
    intros ctx_arena_present Heq.
    unfold unconditional_slab_value_creation_selection,
      cfg_split_value_creation_selection, slab_factory_selection,
      index_factory_selection in Heq.
    destruct ctx_arena_present; discriminate.
  Qed.
End JitValueCreationStoreSelectionModel.

End MeTTaTron_GC_JitValueCreationStoreSelection.
