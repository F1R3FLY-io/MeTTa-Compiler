(** Side-payload free quiescence obligation.

    Index nodes may materialize laundered references into side-arena boxes.
    Reclaiming the fixed node slot is independent from dropping those side
    boxes: side boxes may be dropped only on the true-quiescence arm, and the
    per-thread materialization shadow must be cleared before any later
    dereference can observe a dropped box.
*)

Module MeTTaTron_GC_SideFreeQuiescence.

Section SideFreeQuiescenceModel.
  Variables Quiescent FullMark SideFreed StackLaunderLive ShadowCleared FutureDeref : Prop.

  Theorem side_free_at_quiescence_has_no_stack_launder_ref :
    (SideFreed -> Quiescent) ->
    (StackLaunderLive -> ~ Quiescent) ->
    SideFreed ->
    ~ StackLaunderLive.
  Proof.
    intros Hfree_quiescent Hstack_nonquiescent Hfreed Hstack.
    apply (Hstack_nonquiescent Hstack).
    apply Hfree_quiescent.
    exact Hfreed.
  Qed.

  Theorem side_free_shadow_clear_blocks_future_deref :
    (SideFreed -> Quiescent) ->
    (StackLaunderLive -> ~ Quiescent) ->
    (SideFreed -> ShadowCleared) ->
    (FutureDeref -> StackLaunderLive \/ ~ ShadowCleared) ->
    SideFreed ->
    ~ FutureDeref.
  Proof.
    intros Hfree_quiescent Hstack_nonquiescent Hclear Hderef_shape Hfreed Hderef.
    destruct (Hderef_shape Hderef) as [Hstack | Hnot_cleared].
    - apply (side_free_at_quiescence_has_no_stack_launder_ref
               Hfree_quiescent Hstack_nonquiescent Hfreed).
      exact Hstack.
    - apply Hnot_cleared.
      apply Hclear.
      exact Hfreed.
  Qed.

  Theorem nonquiescent_collection_defers_side_free :
    (SideFreed -> Quiescent) ->
    ~ Quiescent ->
    ~ SideFreed.
  Proof.
    intros Hfree_quiescent Hnot_quiescent Hfreed.
    apply Hnot_quiescent.
    apply Hfree_quiescent.
    exact Hfreed.
  Qed.

  Theorem side_free_requires_quiescent_full_mark :
    (SideFreed -> Quiescent /\ FullMark) ->
    SideFreed ->
    Quiescent /\ FullMark.
  Proof.
    intros Hfree Hfreed.
    apply Hfree.
    exact Hfreed.
  Qed.

  Theorem quiescent_minor_defers_side_free :
    (SideFreed -> FullMark) ->
    ~ FullMark ->
    ~ SideFreed.
  Proof.
    intros Hfree_full Hnot_full Hfreed.
    apply Hnot_full.
    apply Hfree_full.
    exact Hfreed.
  Qed.

  Theorem pending_side_reclaim_at_quiescence_schedules_full_mark :
    forall PendingSideReclaim MajorScheduled : Prop,
      (Quiescent -> PendingSideReclaim -> MajorScheduled) ->
      Quiescent ->
      PendingSideReclaim ->
      MajorScheduled.
  Proof.
    intros PendingSideReclaim MajorScheduled Hschedule Hquiescent Hpending.
    apply Hschedule.
    - exact Hquiescent.
    - exact Hpending.
  Qed.
End SideFreeQuiescenceModel.

Section SideReclaimSnapshotModel.
  Variable Side : Type.
  Variables Pending Live Freed : Side -> Prop.
  Variables SegmentReset SnapshotDropped : Side -> Prop.
  Variables FreeOwnerAcquired SnapshotEmitted : Side -> Prop.
  Variables MarkedOwnerStillOwns : Side -> Prop.

  Theorem pending_snapshot_drain_does_not_free_live :
    (forall s, Pending s -> ~ Live s) ->
    (forall s, Freed s -> Pending s) ->
    forall s, Freed s -> ~ Live s.
  Proof.
    intros Hpending_disjoint Hfreed_pending s Hfreed Hlive.
    apply (Hpending_disjoint s).
    - apply Hfreed_pending. exact Hfreed.
    - exact Hlive.
  Qed.

  Theorem reset_segment_pending_snapshot_must_be_dropped :
    (forall s, SegmentReset s -> SnapshotDropped s) ->
    (forall s, SnapshotDropped s -> ~ Pending s) ->
    forall s, SegmentReset s -> ~ Pending s.
  Proof.
    intros Hreset_drop Hdrop_not_pending s Hreset.
    apply Hdrop_not_pending.
    apply Hreset_drop.
    exact Hreset.
  Qed.

  Theorem side_snapshot_requires_new_free_owner :
    (forall s, SnapshotEmitted s -> FreeOwnerAcquired s) ->
    forall s, ~ FreeOwnerAcquired s -> ~ SnapshotEmitted s.
  Proof.
    intros Hemit_owner s Hnot_owner Hemitted.
    apply Hnot_owner.
    apply Hemit_owner.
    exact Hemitted.
  Qed.

  Theorem marked_live_owner_snapshot_must_not_be_freed :
    (forall s, Freed s -> Pending s /\ ~ MarkedOwnerStillOwns s) ->
    forall s, MarkedOwnerStillOwns s -> ~ Freed s.
  Proof.
    intros Hfreed_shape s Hmarked Hfreed.
    destruct (Hfreed_shape s Hfreed) as [_ Hnot_marked].
    apply Hnot_marked.
    exact Hmarked.
  Qed.
End SideReclaimSnapshotModel.

End MeTTaTron_GC_SideFreeQuiescence.
