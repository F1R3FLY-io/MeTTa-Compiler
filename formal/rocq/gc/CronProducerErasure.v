(** Rocq model of the F4 cron legacy-producer erasure obligation.

    The GC cron monitor has two logically distinct effects:

      - it returns the pressure decision used by tests/telemetry; and
      - in the legacy slab opt-out it may emit the old [request_gc] producer.

    In the default index build the dedicated CESK collector is the sole valid
    producer of index collection requests.  Therefore F4 may erase the legacy
    cron producer from index code only if the monitor decision is preserved and
    a legacy cron request can be emitted only by the slab opt-out under pressure.
*)

Require Import DefaultStoreSelection.

Module MeTTaTron_GC_CronProducerErasure.

Import MeTTaTron_GC_DefaultStoreSelection.

Section CronProducerErasureModel.
  Inductive CronRequestEffect : Type :=
  | NoLegacyCronRequest : CronRequestEffect
  | EmitLegacyCronRequest : CronRequestEffect.

  Definition cron_request_for_store (store : Store) : CronRequestEffect :=
    match store with
    | IndexStore => NoLegacyCronRequest
    | SlabStore => EmitLegacyCronRequest
    end.

  Definition cron_monitor_return (should_gc : bool) : bool :=
    should_gc.

  Definition cron_request_effect
      (features : FeatureSet)
      (should_gc : bool) : CronRequestEffect :=
    match active_store features with
    | Some selected =>
        if should_gc
        then cron_request_for_store selected
        else NoLegacyCronRequest
    | None => NoLegacyCronRequest
    end.

  Theorem monitor_return_preserved_by_cron_erasure :
    forall (features : FeatureSet) should_gc,
      cron_monitor_return should_gc = should_gc.
  Proof.
    intros features should_gc.
    reflexivity.
  Qed.

  Theorem index_selection_erases_legacy_cron_request :
    forall features should_gc,
      active_store features = Some IndexStore ->
      cron_request_effect features should_gc = NoLegacyCronRequest.
  Proof.
    intros features should_gc Hstore.
    unfold cron_request_effect.
    rewrite Hstore.
    destruct should_gc; reflexivity.
  Qed.

  Theorem default_index_erases_legacy_cron_request :
    forall should_gc,
      cron_request_effect default_invocation should_gc = NoLegacyCronRequest.
  Proof.
    intros should_gc.
    apply index_selection_erases_legacy_cron_request.
    apply default_invocation_selects_index.
  Qed.

  Theorem explicit_index_erases_legacy_cron_request :
    forall should_gc,
      cron_request_effect explicit_index_no_default should_gc = NoLegacyCronRequest.
  Proof.
    intros should_gc.
    apply index_selection_erases_legacy_cron_request.
    apply explicit_index_no_default_selects_index.
  Qed.

  Theorem legacy_slab_pressure_emits_legacy_cron_request :
    cron_request_effect legacy_slab_opt_out true = EmitLegacyCronRequest.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_slab_no_pressure_emits_no_legacy_cron_request :
    cron_request_effect legacy_slab_opt_out false = NoLegacyCronRequest.
  Proof.
    reflexivity.
  Qed.

  Theorem legacy_cron_request_requires_slab_and_pressure :
    forall features should_gc,
      cron_request_effect features should_gc = EmitLegacyCronRequest ->
      active_store features = Some SlabStore /\ should_gc = true.
  Proof.
    intros features should_gc Heffect.
    unfold cron_request_effect in Heffect.
    destruct (active_store features) as [selected |] eqn:Hstore.
    - destruct selected; destruct should_gc; simpl in Heffect;
        try discriminate.
      inversion Heffect.
      split; reflexivity.
    - discriminate.
  Qed.
End CronProducerErasureModel.

End MeTTaTron_GC_CronProducerErasure.
