(** Rocq model of the E2 SATB phase and sweep gates.

    The deletion barrier alone is not enough unless marker start and final
    sweep are ordered against in-flight deletions.  The implementation uses a
    read/write phase gate: deletion holds the read side while it observes the
    SATB flag, shades if needed, and removes the old value; marker start and
    marker teardown/sweep take the write side.  TLC checks the finite
    discriminators in [SATBPhaseGate.tla] and [SATBSweepGate.tla].  These
    theorems prove the source-level premise those models represent: every
    snapshot-live removed pre-image is either still visible to marking or has
    become a shaded SATB root before sweep can free it.
*)

Module MeTTaTron_GC_SATBGates.

Section SATBGatesModel.
  Variable Addr : Type.

  Theorem phase_gate_removed_snapshot_preimage_shaded :
    forall (InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
            AfterStartDeletion RemovedBySweep Shaded SnapshotLive : Addr -> Prop),
      (forall a, SnapshotLive a -> InCacheAtStart a) ->
      (forall a, BeganBeforeStart a -> CommittedBeforeStart a \/ OpenAtStart a) ->
      (forall a, CommittedBeforeStart a -> ~ InCacheAtStart a) ->
      (forall a, ~ OpenAtStart a) ->
      (forall a, RemovedBySweep a -> BeganBeforeStart a \/ AfterStartDeletion a) ->
      (forall a, AfterStartDeletion a -> Shaded a) ->
      forall a,
        SnapshotLive a -> RemovedBySweep a -> Shaded a.
  Proof.
    intros InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
           AfterStartDeletion RemovedBySweep Shaded SnapshotLive
           Hsnapshot_visible Hpre_start_state Hcommitted_invisible Hphase_gate
           Hremoved_shape Hafter_start_shaded a Hsnapshot Hremoved.
    destruct (Hremoved_shape a Hremoved) as [Hbefore | Hafter].
    - destruct (Hpre_start_state a Hbefore) as [Hcommitted | Hopen].
      + exfalso.
        apply (Hcommitted_invisible a Hcommitted).
        apply Hsnapshot_visible.
        exact Hsnapshot.
      + exfalso.
        apply (Hphase_gate a).
        exact Hopen.
    - apply Hafter_start_shaded.
      exact Hafter.
  Qed.

  Theorem phase_gate_snapshot_live_survives_sweep :
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
      forall a,
        SnapshotLive a -> ~ Freed a.
  Proof.
    intros InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
           AfterStartDeletion RemovedBySweep InCacheAtSweep Shaded
           SnapshotLive Marked Freed
           Hsnapshot_visible Hpre_start_state Hcommitted_invisible Hphase_gate
           Hremoved_shape Hafter_start_shaded Hsnapshot_shape Hvisible_marked
           Hshaded_marked Hsweep a Hsnapshot Hfreed.
    apply (Hsweep a Hfreed).
    destruct (Hsnapshot_shape a Hsnapshot) as [Hremoved | Hvisible].
    - apply Hshaded_marked.
      apply (phase_gate_removed_snapshot_preimage_shaded
               InCacheAtStart BeganBeforeStart CommittedBeforeStart OpenAtStart
               AfterStartDeletion RemovedBySweep Shaded SnapshotLive).
      + exact Hsnapshot_visible.
      + exact Hpre_start_state.
      + exact Hcommitted_invisible.
      + exact Hphase_gate.
      + exact Hremoved_shape.
      + exact Hafter_start_shaded.
      + exact Hsnapshot.
      + exact Hremoved.
    - apply Hvisible_marked.
      exact Hvisible.
  Qed.

  Theorem sweep_gate_visible_or_shaded_at_sweep :
    forall (Sweep : Prop)
           (DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
            InCacheAtSweep Shaded SnapshotLive : Addr -> Prop),
      (Sweep -> forall a, ~ DeleteOpenAtSweep a) ->
      (forall a, RemovedBeforeSweep a ->
        DeleteOpenAtSweep a \/ CommittedBeforeSweep a) ->
      (forall a, CommittedBeforeSweep a -> Shaded a) ->
      (forall a, SnapshotLive a -> RemovedBeforeSweep a \/ InCacheAtSweep a) ->
      Sweep ->
      forall a,
        SnapshotLive a -> InCacheAtSweep a \/ Shaded a.
  Proof.
    intros Sweep DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
           InCacheAtSweep Shaded SnapshotLive
           Hsweep_gate Hremoved_state Hcommitted_shaded Hsnapshot_shape
           Hsweep a Hsnapshot.
    destruct (Hsnapshot_shape a Hsnapshot) as [Hremoved | Hvisible].
    - destruct (Hremoved_state a Hremoved) as [Hopen | Hcommitted].
      + exfalso.
        apply (Hsweep_gate Hsweep a).
        exact Hopen.
      + right.
        apply Hcommitted_shaded.
        exact Hcommitted.
    - left.
      exact Hvisible.
  Qed.

  Theorem sweep_gate_snapshot_live_survives_sweep :
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
      forall a,
        SnapshotLive a -> ~ Freed a.
  Proof.
    intros Sweep DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
           InCacheAtSweep Shaded SnapshotLive Marked Freed
           Hsweep_gate Hremoved_state Hcommitted_shaded Hsnapshot_shape
           Hvisible_marked Hshaded_marked Hsweep_only_unmarked Hsweep
           a Hsnapshot Hfreed.
    apply (Hsweep_only_unmarked a Hfreed).
    destruct (sweep_gate_visible_or_shaded_at_sweep
                Sweep DeleteOpenAtSweep RemovedBeforeSweep CommittedBeforeSweep
                InCacheAtSweep Shaded SnapshotLive
                Hsweep_gate Hremoved_state Hcommitted_shaded Hsnapshot_shape
                Hsweep a Hsnapshot) as [Hvisible | Hshaded].
    - apply Hvisible_marked.
      exact Hvisible.
    - apply Hshaded_marked.
      exact Hshaded.
  Qed.
End SATBGatesModel.

End MeTTaTron_GC_SATBGates.
