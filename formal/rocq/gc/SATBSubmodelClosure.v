(** E2 SATB submodel closure.

    The SATB model suite is intentionally split between temporal TLC
    discriminators and source-level Rocq safety obligations.  This file is the
    mandatory Rocq audit point for that split: every named SATB submodel whose
    premise affects source safety is represented by a checked field below.

    The temporal TLA+ configs still carry the finite interleaving
    discriminators.  This proof closes the Rocq side by composing the same
    abstract consequences used by [SATB.v], [SATBGates.v],
    [SATBFinalization.v], [FullMajorSweep.v], [E0MutationSites.v],
    [E0EvictionBarriers.v], [AllocateBlack.v], and [SATBAbortFallback.v].
*)

Module MeTTaTron_GC_SATBSubmodelClosure.

Section SATBSubmodelClosureModel.
  Variable Addr : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition SATBRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition RequestHandled (SatbSwept StwFallbackRan : Prop) : Prop :=
    SatbSwept \/ StwFallbackRan.

  Record SATBSubmodelCoverage : Prop := {
    deletion_barrier_snapshot_live_survives :
      forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop)
             (Marked Freed SnapshotLive : Addr -> Prop),
        (forall a, Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        (forall a, SnapshotLive a -> Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a) ->
        forall a, SnapshotLive a -> ~ Freed a;

    e0_mutation_removed_preimage_shaded :
      forall (SpacePreimage RulePreimage EnvPreimage Shaded : Addr -> Prop),
        (forall a, SpacePreimage a -> Shaded a) ->
        (forall a, RulePreimage a -> Shaded a) ->
        (forall a, EnvPreimage a -> Shaded a) ->
        forall a,
          SpacePreimage a \/ RulePreimage a \/ EnvPreimage a -> Shaded a;

    e0_cache_eviction_or_bulk_clear_shaded :
      forall (CapacityVictim OverwriteVictim BulkClearedEntry Shaded : Addr -> Prop),
        (forall a, CapacityVictim a -> Shaded a) ->
        (forall a, OverwriteVictim a -> Shaded a) ->
        (forall a, BulkClearedEntry a -> Shaded a) ->
        forall a,
          CapacityVictim a \/ OverwriteVictim a \/ BulkClearedEntry a -> Shaded a;

    phase_gate_snapshot_live_survives :
      forall (InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
              AfterStartDeletion RemovedBySweep InCacheAtSweep Shaded
              SnapshotLive Marked Freed : Addr -> Prop),
        (forall a, SnapshotLive a -> InCacheAtStart a) ->
        (forall a, BeganBeforeStart a -> CommittedBeforeStart a \/ OpenAtStart a) ->
        (forall a, CommittedBeforeStart a -> ~ InCacheAtStart a) ->
        (forall a, ~ OpenAtStart a) ->
        (forall a, RemovedBySweep a -> BeganBeforeStart a \/ AfterStartDeletion a) ->
        (forall a, AfterStartDeletion a -> Shaded a) ->
        (forall a, SnapshotLive a -> RemovedBySweep a \/ InCacheAtSweep a) ->
        (forall a, InCacheAtSweep a -> Marked a) ->
        (forall a, Shaded a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, SnapshotLive a -> ~ Freed a;

    sweep_gate_snapshot_live_survives :
      forall (Sweep : Prop)
             (DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
              InCacheAtSweep Shaded SnapshotLive Marked Freed : Addr -> Prop),
        (Sweep -> forall a, ~ DeleteOpenAtSweep a) ->
        (forall a, RemovedBeforeSweep a ->
          DeleteOpenAtSweep a \/ CommittedBeforeSweep a) ->
        (forall a, CommittedBeforeSweep a -> Shaded a) ->
        (forall a, SnapshotLive a -> RemovedBeforeSweep a \/ InCacheAtSweep a) ->
        (forall a, InCacheAtSweep a -> Marked a) ->
        (forall a, Shaded a -> Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        Sweep ->
        forall a, SnapshotLive a -> ~ Freed a;

    allocate_black_published_value_survives :
      forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop)
             (Marked Freed Published : Addr -> Prop),
        (forall a, Published a -> AllocateBlack a) ->
        (forall a,
            Reach (SATBRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
            Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, Published a -> ~ Freed a;

    final_remark_revisits_premarked_root_child :
      forall (FinalRoot Premarked Marked : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop) r c,
        FinalRoot r ->
        Premarked r ->
        Edge r c ->
        (forall a, Reach FinalRoot Edge a -> Marked a) ->
        Marked c;

    final_remark_root_survives :
      forall (InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack
              FinalRoot : Addr -> Prop)
             (Edge : Addr -> Addr -> Prop)
             (Marked Freed : Addr -> Prop),
        (forall a, FinalRoot a -> FinalDriverRoot a) ->
        (forall a,
            Reach (SATBRoot InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack) Edge a ->
            Marked a) ->
        (forall a, Freed a -> ~ Marked a) ->
        forall a, FinalRoot a -> ~ Freed a;

    failed_final_sweep_uses_stw_backstop :
      forall (FinalSweepReturned SatbSwept StwFallbackRan : Prop),
        (FinalSweepReturned -> ~ SatbSwept -> StwFallbackRan) ->
        FinalSweepReturned ->
        ~ SatbSwept ->
        StwFallbackRan;

    completed_satb_request_has_collection :
      forall (SatbSuccess SatbFailed SatbSwept StwFallbackRan RequestDone : Prop),
        (SatbSuccess -> SatbSwept) ->
        (SatbFailed -> StwFallbackRan) ->
        (RequestDone -> SatbSuccess \/ SatbFailed) ->
        RequestDone ->
        RequestHandled SatbSwept StwFallbackRan;

    full_major_clears_all_satb_marks :
      forall (SATBMarked Swept Cleared : Addr -> Prop),
        (forall a, SATBMarked a -> Swept a) ->
        (forall a, Swept a -> Cleared a) ->
        forall a, SATBMarked a -> Cleared a;

    young_only_sweep_safe_requires_no_old_satb_mark :
      forall (SATBMarked Old MarkedAfter : Addr -> Prop),
        (forall a, SATBMarked a -> Old a -> MarkedAfter a) ->
        (forall a, ~ MarkedAfter a) ->
        forall a, SATBMarked a -> Old a -> False
  }.

  Theorem satb_submodels_have_rocq_closure :
    SATBSubmodelCoverage.
  Proof.
    constructor.
    - intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked
             Freed SnapshotLive Hmark Hsweep Hcovered a Hlive Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmark.
      apply Hcovered.
      exact Hlive.
    - intros SpacePreimage RulePreimage EnvPreimage Shaded
             Hspace Hrule Henv a Hremoved.
      destruct Hremoved as [Hspace_removed | [Hrule_removed | Henv_removed]].
      + apply Hspace. exact Hspace_removed.
      + apply Hrule. exact Hrule_removed.
      + apply Henv. exact Henv_removed.
    - intros CapacityVictim OverwriteVictim BulkClearedEntry Shaded
             Hcapacity Hoverwrite Hbulk a Hremoved.
      destruct Hremoved as [Hcapacity_removed | [Hoverwrite_removed | Hbulk_removed]].
      + apply Hcapacity. exact Hcapacity_removed.
      + apply Hoverwrite. exact Hoverwrite_removed.
      + apply Hbulk. exact Hbulk_removed.
    - intros InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
             AfterStartDeletion RemovedBySweep InCacheAtSweep Shaded
             SnapshotLive Marked Freed
             Hsnapshot_visible Hpre_start_state Hcommitted_invisible Hphase_gate
             Hremoved_shape Hafter_start_shaded Hsnapshot_shape Hvisible_marked
             Hshaded_marked Hsweep a Hsnapshot Hfreed.
      apply (Hsweep a Hfreed).
      destruct (Hsnapshot_shape a Hsnapshot) as [Hremoved | Hvisible].
      + apply Hshaded_marked.
        destruct (Hremoved_shape a Hremoved) as [Hbefore | Hafter].
        * destruct (Hpre_start_state a Hbefore) as [Hcommitted | Hopen].
          -- exfalso.
             apply (Hcommitted_invisible a Hcommitted).
             apply Hsnapshot_visible.
             exact Hsnapshot.
          -- exfalso.
             apply (Hphase_gate a).
             exact Hopen.
        * apply Hafter_start_shaded.
          exact Hafter.
      + apply Hvisible_marked.
        exact Hvisible.
    - intros Sweep DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
             InCacheAtSweep Shaded SnapshotLive Marked Freed
             Hsweep_gate Hremoved_state Hcommitted_shaded Hsnapshot_shape
             Hvisible_marked Hshaded_marked Hsweep_only_unmarked Hsweep
             a Hsnapshot Hfreed.
      apply (Hsweep_only_unmarked a Hfreed).
      destruct (Hsnapshot_shape a Hsnapshot) as [Hremoved | Hvisible].
      + destruct (Hremoved_state a Hremoved) as [Hopen | Hcommitted].
        * exfalso.
          apply (Hsweep_gate Hsweep a).
          exact Hopen.
        * apply Hshaded_marked.
          apply Hcommitted_shaded.
          exact Hcommitted.
      + apply Hvisible_marked.
        exact Hvisible.
    - intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked
             Freed Published Hpublished_black Hmark Hsweep a Hpublished Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right; right; right.
      apply Hpublished_black.
      exact Hpublished.
    - intros FinalRoot Premarked Marked Edge r c Hfinal _ Hedge Hrevisit.
      apply Hrevisit.
      eapply reach_step.
      + apply reach_root.
        exact Hfinal.
      + exact Hedge.
    - intros InitialRoot FinalDriverRoot ShadedDeletion AllocateBlack FinalRoot
             Edge Marked Freed Hremark Hmark Hsweep a Hfinal Hfreed.
      apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right; left.
      apply Hremark.
      exact Hfinal.
    - intros FinalSweepReturned SatbSwept StwFallbackRan Hbackstop
             Hreturned Hnot_swept.
      apply Hbackstop; assumption.
    - intros SatbSuccess SatbFailed SatbSwept StwFallbackRan RequestDone
             Hsuccess_swept Hfailure_stw Hdone_case Hdone.
      destruct (Hdone_case Hdone) as [Hsuccess | Hfailure].
      + left. apply Hsuccess_swept. exact Hsuccess.
      + right. apply Hfailure_stw. exact Hfailure.
    - intros SATBMarked Swept Cleared Hsatb_swept Hswept_cleared a Hmarked.
      apply Hswept_cleared.
      apply Hsatb_swept.
      exact Hmarked.
    - intros SATBMarked Old MarkedAfter Hold_survives Hno_stale a Hsatb Hold.
      apply (Hno_stale a).
      apply Hold_survives; assumption.
  Qed.
End SATBSubmodelClosureModel.

End MeTTaTron_GC_SATBSubmodelClosure.
