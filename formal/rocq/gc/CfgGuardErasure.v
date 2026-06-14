(** Rocq model of F4 cfg-gated runtime-guard erasure.

    Several index-only fast paths are already behind
    [#[cfg(feature = "index-gc")]] and then redundantly test
    [gc_mode_is_index()].  After F4 erased the mutable runtime mode bridge,
    [gc_mode_is_index()] is the same compile-time feature predicate that made
    the cfg-gated block exist.

    This file proves the source-edit precondition for deleting such inner
    runtime guards: a block compiled only when [index-gc] is present has the
    same effect whether it additionally tests [gc_mode_is_index()] or executes
    directly.  For valid store selections, any effect emitted by the erased
    form requires the active store to be [IndexStore].
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_CfgGuardErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section CfgGuardErasureModel.
  Inductive CfgGuardEffect : Type :=
  | FastNegativeAtom : CfgGuardEffect
  | FastNegativeSExpr : CfgGuardEffect
  | FastNegativeError : CfgGuardEffect
  | FastNegativeConjunction : CfgGuardEffect
  | FastNegativeQuoted : CfgGuardEffect
  | FastNegativeTraitError : CfgGuardEffect
  | FastNegativeHeadSymbol : CfgGuardEffect.

  Definition cfg_index_compiles (features : FeatureSet) : bool :=
    has_index_gc features.

  Definition erased_gc_mode_guard (features : FeatureSet) : bool :=
    has_index_gc features.

  Definition cfg_guarded_effect
      (features : FeatureSet)
      (requested : option CfgGuardEffect)
      : option CfgGuardEffect :=
    if cfg_index_compiles features
    then
      if erased_gc_mode_guard features
      then requested
      else None
    else None.

  Definition cfg_erased_effect
      (features : FeatureSet)
      (requested : option CfgGuardEffect)
      : option CfgGuardEffect :=
    if cfg_index_compiles features
    then requested
    else None.

  Theorem cfg_runtime_guard_matches_index_cfg :
    forall features,
      cfg_index_compiles features = erased_gc_mode_guard features.
  Proof.
    intros features.
    reflexivity.
  Qed.

  Theorem cfg_guard_erasure_preserves_effect :
    forall features requested,
      cfg_guarded_effect features requested =
      cfg_erased_effect features requested.
  Proof.
    intros features requested.
    unfold cfg_guarded_effect, cfg_erased_effect, cfg_index_compiles,
      erased_gc_mode_guard.
    destruct (has_index_gc features); reflexivity.
  Qed.

  Theorem default_index_cfg_guard_emits_requested_effect :
    forall requested,
      cfg_guarded_effect default_invocation requested = requested.
  Proof.
    intros requested.
    reflexivity.
  Qed.

  Theorem default_index_erased_cfg_emits_requested_effect :
    forall requested,
      cfg_erased_effect default_invocation requested = requested.
  Proof.
    intros requested.
    reflexivity.
  Qed.

  Theorem explicit_index_cfg_guard_emits_requested_effect :
    forall requested,
      cfg_guarded_effect explicit_index_no_default requested = requested.
  Proof.
    intros requested.
    reflexivity.
  Qed.

  Theorem legacy_slab_cfg_guard_emits_no_effect :
    forall requested,
      cfg_guarded_effect legacy_slab_opt_out requested = None.
  Proof.
    intros requested.
    reflexivity.
  Qed.

  Theorem legacy_slab_erased_cfg_emits_no_effect :
    forall requested,
      cfg_erased_effect legacy_slab_opt_out requested = None.
  Proof.
    intros requested.
    reflexivity.
  Qed.

  Theorem emitted_erased_cfg_effect_requires_index_feature :
    forall features requested effect,
      cfg_erased_effect features requested = Some effect ->
      has_index_gc features = true.
  Proof.
    intros features requested effect Heffect.
    unfold cfg_erased_effect, cfg_index_compiles in Heffect.
    destruct (has_index_gc features) eqn:Hindex.
    - reflexivity.
    - discriminate.
  Qed.

  Theorem valid_emitted_erased_cfg_effect_requires_index_store :
    forall features requested effect,
      valid_store_selection features ->
      cfg_erased_effect features requested = Some effect ->
      active_store features = Some IndexStore.
  Proof.
    intros [default_on explicit_index legacy_slab] requested effect
           Hvalid Heffect.
    unfold valid_store_selection in Hvalid.
    unfold cfg_erased_effect, cfg_index_compiles, has_index_gc in Heffect.
    simpl in Heffect.
    destruct default_on, explicit_index, legacy_slab;
      simpl in Heffect;
      try discriminate;
      simpl in Hvalid;
      try reflexivity;
      destruct Hvalid as [store Hstore];
      discriminate.
  Qed.

  Section BranchPreservation.
    Variable Result : Type.
    Variable body fallthrough : Result.

    Definition cfg_guarded_branch (features : FeatureSet) : Result :=
      if cfg_index_compiles features
      then
        if erased_gc_mode_guard features
        then body
        else fallthrough
      else fallthrough.

    Definition cfg_erased_branch (features : FeatureSet) : Result :=
      if cfg_index_compiles features
      then body
      else fallthrough.

    Theorem cfg_guard_erasure_preserves_branch_result :
      forall features,
        cfg_guarded_branch features = cfg_erased_branch features.
    Proof.
      intros features.
      unfold cfg_guarded_branch, cfg_erased_branch, cfg_index_compiles,
        erased_gc_mode_guard.
      destruct (has_index_gc features); reflexivity.
    Qed.

    Theorem legacy_slab_erased_branch_is_fallthrough :
      cfg_erased_branch legacy_slab_opt_out = fallthrough.
    Proof.
      reflexivity.
    Qed.

    Theorem default_index_erased_branch_is_body :
      cfg_erased_branch default_invocation = body.
    Proof.
      reflexivity.
    Qed.
  End BranchPreservation.
End CfgGuardErasureModel.

End MeTTaTron_GC_CfgGuardErasure.
