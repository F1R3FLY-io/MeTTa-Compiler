(** Rocq model of the F4 runtime GC-mode erasure obligation.

    F3 made [--gc] / [MTT_GC] an assertion and reporter over the store chosen
    by Cargo features.  F4 may delete the mutable runtime GC-mode bridge only if
    an accepted runtime request cannot switch stores: the effective store seen
    by evaluation is exactly the compile-time [active_store].

    This proof intentionally models the deletion precondition, not the source
    edit itself.  Source-coupling checks must later bind production code to this
    model by showing that runtime requests are accepted only when they match the
    feature-selected store, and rejected otherwise.
*)

From Stdlib Require Import Bool.Bool.
Require Import DefaultStoreSelection.

Module MeTTaTron_GC_RuntimeModeErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section RuntimeModeErasureModel.
  Inductive RuntimeRequest : Type :=
  | RequestDefault : RuntimeRequest
  | RequestIndex : RuntimeRequest
  | RequestSlab : RuntimeRequest.

  Definition store_eqb (left right : Store) : bool :=
    match left, right with
    | IndexStore, IndexStore => true
    | SlabStore, SlabStore => true
    | _, _ => false
    end.

  Definition runtime_request_matches
      (selected : Store)
      (request : RuntimeRequest) : bool :=
    match request with
    | RequestDefault => true
    | RequestIndex => store_eqb selected IndexStore
    | RequestSlab => store_eqb selected SlabStore
    end.

  Definition runtime_effective_store
      (features : FeatureSet)
      (request : RuntimeRequest) : option Store :=
    match active_store features with
    | Some selected =>
        if runtime_request_matches selected request
        then Some selected
        else None
    | None => None
    end.

  Definition erased_effective_store (features : FeatureSet) : option Store :=
    active_store features.

  Definition accepted_runtime_request
      (features : FeatureSet)
      (request : RuntimeRequest) : Prop :=
    exists selected, runtime_effective_store features request = Some selected.

  Theorem store_eqb_refl :
    forall store, store_eqb store store = true.
  Proof.
    intros [|]; reflexivity.
  Qed.

  Theorem store_eqb_eq :
    forall left right,
      store_eqb left right = true -> left = right.
  Proof.
    intros [|] [|] H; simpl in H; try reflexivity; discriminate.
  Qed.

  Theorem active_store_accepts_default_request :
    forall features selected,
      active_store features = Some selected ->
      runtime_effective_store features RequestDefault = Some selected.
  Proof.
    intros features selected Hselected.
    unfold runtime_effective_store.
    rewrite Hselected.
    reflexivity.
  Qed.

  Theorem matched_request_selects_active_store :
    forall features request selected,
      active_store features = Some selected ->
      runtime_request_matches selected request = true ->
      runtime_effective_store features request = Some selected.
  Proof.
    intros features request selected Hselected Hrequest.
    unfold runtime_effective_store.
    rewrite Hselected.
    rewrite Hrequest.
    reflexivity.
  Qed.

  Theorem invalid_store_selection_rejects_runtime_requests :
    forall features request,
      active_store features = None ->
      runtime_effective_store features request = None.
  Proof.
    intros features request Hinvalid.
    unfold runtime_effective_store.
    rewrite Hinvalid.
    reflexivity.
  Qed.

  Theorem runtime_effective_store_implies_active_store :
    forall features request selected,
      runtime_effective_store features request = Some selected ->
      active_store features = Some selected.
  Proof.
    intros features request selected Heffective.
    unfold runtime_effective_store in Heffective.
    destruct (active_store features) as [active |] eqn:Hactive.
    - destruct (runtime_request_matches active request).
      + exact Heffective.
      + discriminate.
    - discriminate.
  Qed.

  Theorem runtime_request_cannot_switch_store :
    forall features request active selected,
      active_store features = Some active ->
      runtime_effective_store features request = Some selected ->
      selected = active.
  Proof.
    intros features request active selected Hactive Heffective.
    pose proof
      (runtime_effective_store_implies_active_store
         features request selected Heffective) as Hselected.
    rewrite Hactive in Hselected.
    inversion Hselected.
    reflexivity.
  Qed.

  Theorem runtime_erasure_complete :
    forall features request selected,
      runtime_effective_store features request = Some selected ->
      erased_effective_store features = Some selected.
  Proof.
    intros features request selected Heffective.
    unfold erased_effective_store.
    apply runtime_effective_store_implies_active_store with (request := request).
    exact Heffective.
  Qed.

  Theorem runtime_erasure_sound_for_matching_requests :
    forall features request selected,
      erased_effective_store features = Some selected ->
      runtime_request_matches selected request = true ->
      runtime_effective_store features request = Some selected.
  Proof.
    intros features request selected Herased Hrequest.
    unfold erased_effective_store in Herased.
    apply matched_request_selects_active_store; assumption.
  Qed.

  Theorem default_index_rejects_slab_request :
    runtime_effective_store default_invocation RequestSlab = None.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_rejects_index_request :
    runtime_effective_store legacy_slab_opt_out RequestIndex = None.
  Proof.
    reflexivity.
  Qed.

  Theorem default_invocation_accepts_only_index_or_default :
    forall request selected,
      runtime_effective_store default_invocation request = Some selected ->
      selected = IndexStore /\
      match request with
      | RequestDefault => True
      | RequestIndex => True
      | RequestSlab => False
      end.
  Proof.
    intros request selected Heffective.
    destruct request; simpl in Heffective.
    - inversion Heffective. split; reflexivity || exact I.
    - inversion Heffective. split; reflexivity || exact I.
    - discriminate.
  Qed.

  Theorem legacy_slab_accepts_only_slab_or_default :
    forall request selected,
      runtime_effective_store legacy_slab_opt_out request = Some selected ->
      selected = SlabStore /\
      match request with
      | RequestDefault => True
      | RequestIndex => False
      | RequestSlab => True
      end.
  Proof.
    intros request selected Heffective.
    destruct request; simpl in Heffective.
    - inversion Heffective. split; reflexivity || exact I.
    - discriminate.
    - inversion Heffective. split; reflexivity || exact I.
  Qed.

  Section EvaluationPreservation.
    Variable Result : Type.
    Variable eval_with_store : Store -> Result.

    Theorem accepted_request_erasure_preserves_evaluation :
      forall features request selected,
        runtime_effective_store features request = Some selected ->
        exists active,
          erased_effective_store features = Some active /\
          selected = active /\
          eval_with_store selected = eval_with_store active.
    Proof.
      intros features request selected Heffective.
      exists selected.
      split.
      - apply runtime_erasure_complete with (request := request).
        exact Heffective.
      - split; reflexivity.
    Qed.
  End EvaluationPreservation.
End RuntimeModeErasureModel.

End MeTTaTron_GC_RuntimeModeErasure.
