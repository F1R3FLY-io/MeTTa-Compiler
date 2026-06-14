(** Rocq model of F4 arena-address decode branch erasure.

    [MettaValue::as_arena_addr] used a runtime [gc_mode_is_index()] branch to
    distinguish index handles from legacy slab pointers.  Once
    [gc_mode_is_index()] is a compile-time feature predicate, the same behavior
    can be expressed as two cfg-selected implementations:

      - index build: inline values return [None], heap handles decode [Some addr];
      - slab build: every value returns [None].

    This proof discharges the source-edit precondition for that split.
*)

From Stdlib Require Import Bool.Bool.
Require Import DefaultStoreSelection.

Module MeTTaTron_GC_ArenaAddrDecodeErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section ArenaAddrDecodeErasureModel.
  Variable Addr : Type.

  Definition compiled_index_feature (features : FeatureSet) : bool :=
    has_index_gc features.

  Definition guarded_arena_addr
      (features : FeatureSet)
      (is_inline : bool)
      (addr : Addr)
      : option Addr :=
    if is_inline || negb (compiled_index_feature features)
    then None
    else Some addr.

  Definition cfg_split_arena_addr
      (features : FeatureSet)
      (is_inline : bool)
      (addr : Addr)
      : option Addr :=
    if compiled_index_feature features
    then
      if is_inline
      then None
      else Some addr
    else None.

  Theorem guarded_arena_addr_matches_cfg_split :
    forall features is_inline addr,
      guarded_arena_addr features is_inline addr =
      cfg_split_arena_addr features is_inline addr.
  Proof.
    intros features is_inline addr.
    unfold guarded_arena_addr, cfg_split_arena_addr, compiled_index_feature.
    destruct (has_index_gc features), is_inline; reflexivity.
  Qed.

  Theorem default_index_heap_decodes_addr :
    forall addr,
      cfg_split_arena_addr default_invocation false addr = Some addr.
  Proof.
    intros addr.
    reflexivity.
  Qed.

  Theorem default_index_inline_has_no_addr :
    forall addr,
      cfg_split_arena_addr default_invocation true addr = None.
  Proof.
    intros addr.
    reflexivity.
  Qed.

  Theorem explicit_index_heap_decodes_addr :
    forall addr,
      cfg_split_arena_addr explicit_index_no_default false addr = Some addr.
  Proof.
    intros addr.
    reflexivity.
  Qed.

  Theorem legacy_slab_has_no_arena_addr :
    forall is_inline addr,
      cfg_split_arena_addr legacy_slab_opt_out is_inline addr = None.
  Proof.
    intros is_inline addr.
    destruct is_inline; reflexivity.
  Qed.

  Theorem emitted_addr_requires_index_feature :
    forall features is_inline addr decoded,
      cfg_split_arena_addr features is_inline addr = Some decoded ->
      has_index_gc features = true /\ is_inline = false /\ decoded = addr.
  Proof.
    intros features is_inline addr decoded Hdecoded.
    unfold cfg_split_arena_addr, compiled_index_feature in Hdecoded.
    destruct (has_index_gc features) eqn:Hindex.
    - destruct is_inline eqn:Hinline.
      + discriminate.
      + inversion Hdecoded.
        split; [reflexivity |].
        split; reflexivity.
    - discriminate.
  Qed.

  Theorem valid_emitted_addr_requires_index_store :
    forall features is_inline addr decoded,
      valid_store_selection features ->
      cfg_split_arena_addr features is_inline addr = Some decoded ->
      active_store features = Some IndexStore.
  Proof.
    intros [default_on explicit_index legacy_slab] is_inline addr decoded
           Hvalid Hdecoded.
    unfold valid_store_selection in Hvalid.
    unfold cfg_split_arena_addr, compiled_index_feature, has_index_gc in Hdecoded.
    simpl in Hdecoded.
    destruct default_on, explicit_index, legacy_slab, is_inline;
      simpl in Hdecoded;
      try discriminate;
      simpl in Hvalid;
      try reflexivity;
      destruct Hvalid as [store Hstore];
      discriminate.
  Qed.
End ArenaAddrDecodeErasureModel.

End MeTTaTron_GC_ArenaAddrDecodeErasure.
