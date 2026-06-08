(** Source-coupled side-payload ownership and reclaim refinement.

    Side payloads are reclaimed through a two-stage protocol: fixed node slots
    may be returned to the index arena free list, but side boxes are only freed
    after a reclaim-time owner snapshot has survived the full-mark owner filter
    and the drain is on the true-quiescence path.  The source-coupling harness
    pins the Rust ordering around [side_reclaim_for_addr],
    [drop_or_free_pending_side_reclaims_after_full_mark],
    released-segment pending drops, and [clear_inner_shadow].

    This file composes those source facts into the abstract side-reclaim safety
    contract used by the CESK/index collector proof boundary.
*)

Module MeTTaTron_GC_SideReclaimRefinement.

Section SideReclaimRefinementModel.
  Variable Side : Type.

  Definition PublishedSidePayloadReady
      (PageReady ChunkReady EntryWritten EntryPublished OwnerNodePublished
       SameSegment : Side -> Prop)
      (s : Side) : Prop :=
    PageReady s /\ ChunkReady s /\ EntryWritten s /\ EntryPublished s /\
      OwnerNodePublished s /\ SameSegment s.

  Definition FreedSideIsSafe
      (Pending MarkedOwnerStillOwns Quiescent FullMark ShadowCleared Freed :
          Side -> Prop)
      (s : Side) : Prop :=
    Pending s /\ ~ MarkedOwnerStillOwns s /\ Quiescent s /\ FullMark s /\
      ShadowCleared s /\ Freed s.

  Definition SideReclaimRefinementContract
      (PageReady ChunkReady EntryWritten EntryPublished OwnerNodePublished
       SameSegment ReadObserved FutureSideRead Live Pending SegmentReset
       SnapshotDropped FreeOwnerAcquired SnapshotEmitted MarkedOwnerStillOwns
       Quiescent FullMark ShadowCleared Freed : Side -> Prop)
      : Prop :=
    (forall s,
        ReadObserved s ->
        PublishedSidePayloadReady PageReady ChunkReady EntryWritten
          EntryPublished OwnerNodePublished SameSegment s) /\
      (forall s, FutureSideRead s -> ~ Freed s) /\
      (forall s, Freed s -> FreedSideIsSafe Pending MarkedOwnerStillOwns
        Quiescent FullMark ShadowCleared Freed s) /\
      (forall s, MarkedOwnerStillOwns s -> ~ Freed s) /\
      (forall s, SegmentReset s -> ~ Pending s) /\
      (forall s, ~ FreeOwnerAcquired s -> ~ SnapshotEmitted s).

  Theorem source_coupled_side_reclaim_refines_safe_lifetime :
    forall (PagePublished ChunkPublished PageReady ChunkReady EntryWritten
            EntryPublished OwnerNodePublished SameSegment ReadObserved
            FutureSideRead Live Pending SegmentReset SnapshotDropped
            FreeOwnerAcquired SnapshotEmitted MarkedOwnerStillOwns Quiescent
            FullMark ShadowCleared Freed : Side -> Prop),
      (forall s, ReadObserved s -> EntryPublished s) ->
      (forall s, EntryPublished s -> EntryWritten s) ->
      (forall s, EntryPublished s -> ChunkPublished s) ->
      (forall s, ChunkPublished s -> PagePublished s) ->
      (forall s, PagePublished s -> PageReady s) ->
      (forall s, ChunkPublished s -> ChunkReady s) ->
      (forall s, ReadObserved s -> OwnerNodePublished s) ->
      (forall s, OwnerNodePublished s -> SameSegment s) ->
      (forall s, FutureSideRead s -> Live s) ->
      (forall s, Pending s -> ~ Live s) ->
      (forall s, Freed s -> Pending s /\ ~ MarkedOwnerStillOwns s) ->
      (forall s, Freed s -> Quiescent s /\ FullMark s /\ ShadowCleared s) ->
      (forall s, SegmentReset s -> SnapshotDropped s) ->
      (forall s, SnapshotDropped s -> ~ Pending s) ->
      (forall s, SnapshotEmitted s -> FreeOwnerAcquired s) ->
      SideReclaimRefinementContract PageReady ChunkReady EntryWritten
        EntryPublished OwnerNodePublished SameSegment ReadObserved
        FutureSideRead Live Pending SegmentReset SnapshotDropped
        FreeOwnerAcquired SnapshotEmitted MarkedOwnerStillOwns Quiescent
        FullMark ShadowCleared Freed.
  Proof.
    intros PagePublished ChunkPublished PageReady ChunkReady EntryWritten
           EntryPublished OwnerNodePublished SameSegment ReadObserved
           FutureSideRead Live Pending SegmentReset SnapshotDropped
           FreeOwnerAcquired SnapshotEmitted MarkedOwnerStillOwns Quiescent
           FullMark ShadowCleared Freed Hread_entry Hentry_written Hentry_chunk
           Hchunk_page Hpage_ready Hchunk_ready Hread_owner Hsame_segment
           Hfuture_live Hpending_dead Hfreed_shape Hfreed_drain Hreset_drop
           Hdrop_not_pending Hemit_owner.
    unfold SideReclaimRefinementContract.
    split.
    - intros s Hread.
      unfold PublishedSidePayloadReady.
      repeat split.
      + apply Hpage_ready.
        apply Hchunk_page.
        apply Hentry_chunk.
        apply Hread_entry.
        exact Hread.
      + apply Hchunk_ready.
        apply Hentry_chunk.
        apply Hread_entry.
        exact Hread.
      + apply Hentry_written.
        apply Hread_entry.
        exact Hread.
      + apply Hread_entry.
        exact Hread.
      + apply Hread_owner.
        exact Hread.
      + apply Hsame_segment.
        apply Hread_owner.
        exact Hread.
    - split.
      + intros s Hfuture Hfreed.
        destruct (Hfreed_shape s Hfreed) as [Hpending _].
        apply (Hpending_dead s Hpending).
        apply Hfuture_live.
        exact Hfuture.
      + split.
        * intros s Hfreed.
          unfold FreedSideIsSafe.
          destruct (Hfreed_shape s Hfreed) as [Hpending Hnot_marked].
          destruct (Hfreed_drain s Hfreed) as [Hquiescent [Hfull Hshadow]].
          repeat split.
          -- exact Hpending.
          -- exact Hnot_marked.
          -- exact Hquiescent.
          -- exact Hfull.
          -- exact Hshadow.
          -- exact Hfreed.
        * split.
          -- intros s Hmarked Hfreed.
             destruct (Hfreed_shape s Hfreed) as [_ Hnot_marked].
             apply Hnot_marked.
             exact Hmarked.
          -- split.
             ++ intros s Hreset.
                apply Hdrop_not_pending.
                apply Hreset_drop.
                exact Hreset.
             ++ intros s Hnot_owner Hemitted.
                apply Hnot_owner.
                apply Hemit_owner.
                exact Hemitted.
  Qed.

  Theorem nonquiescent_side_drain_is_impossible :
    forall (NonQuiescent Quiescent Freed : Side -> Prop),
      (forall s, NonQuiescent s -> ~ Quiescent s) ->
      (forall s, Freed s -> Quiescent s) ->
      forall s,
        NonQuiescent s ->
        ~ Freed s.
  Proof.
    intros NonQuiescent Quiescent Freed Hnonquiescent Hfreed_quiescent
           s Hnon Hfreed.
    apply (Hnonquiescent s Hnon).
    apply Hfreed_quiescent.
    exact Hfreed.
  Qed.
End SideReclaimRefinementModel.

End MeTTaTron_GC_SideReclaimRefinement.
