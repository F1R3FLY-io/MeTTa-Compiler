(** End-to-end CESK collector safety obligations in Rocq.

    This file composes the local proof obligations used by the MeTTaTron
    generational index collector:

    - rendezvous witness publication puts participant roots in the driver roots;
    - eval-entry driver-C publication puts caller-held source/output roots in
      the driver roots;
    - async batch-result handoff roots worker results in driver-C until the
      caller copies them into MettaState.output;
    - the collector marks the reachability closure of structural CESK roots plus
      driver roots;
    - sweep frees only unmarked addresses;
    - young-only minor marking retains every reachable young address when there
      is no old-to-young edge;
    - conservative minor marking retains every reachable young address when the
      marker traverses the whole reachable graph but only marks young nodes;
    - E2 concurrent marking retains every snapshot-live address covered by
      initial roots, rendezvous driver roots, SATB deletion shades, or
      allocate-black publication;
    - E2 freshly published allocations survive when publication implies
      allocate-black marking;
    - E2 final-rendezvous roots survive the exclusive sweep, and a completed
      SATB request is backed by either the final SATB sweep or the abort-to-STW
      backstop;
    - E2 SATB mark bits cannot leak into later cycles when the final sweep is a
      full major that clears every swept address before promotion;
    - E2 value-bearing E0 cache capacity-eviction, overwrite, and bulk-clear
      pre-images compose into SATB coverage when those removed values are
      shaded;
    - E2 value-bearing E0 deletion categories compose into that SATB coverage
      when removed space-local, rule-index, and environment/token/state
      pre-images are shaded;
    - pointer-keyed operator-cache lookup cannot return a stale post-sweep entry
      when the local sweep-epoch guard runs before lookup;
    - write-once global anchors that are scanned structurally and cannot be
      deleted survive ordinary structural-root collection.

    These are parametric theorems over the store graph and do not assume a finite
    TLC state space.
*)

Module MeTTaTron_GC_CESKCollectorSafety.

Section CESKCollectorSafetyModel.
  Variables Addr Slot : Type.

  Inductive Reach (Root : Addr -> Prop) (Edge : Addr -> Addr -> Prop) : Addr -> Prop :=
  | reach_root : forall a, Root a -> Reach Root Edge a
  | reach_step : forall a b, Reach Root Edge a -> Edge a b -> Reach Root Edge b.

  Definition CollectorRoot
      (StructuralRoot DriverRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    StructuralRoot a \/ DriverRoot a.

  Definition ConcurrentCollectorRoot
      (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
      (a : Addr) : Prop :=
    InitialRoot a \/ DriverRoot a \/ ShadedDeletion a \/ AllocateBlack a.

  Definition RequestHandled (SatbSwept StwFallbackRan : Prop) : Prop :=
    SatbSwept \/ StwFallbackRan.

  Definition BatchHandoffRoot
      (HandleRoot OutputRoot : Addr -> Prop)
      (a : Addr) : Prop :=
    HandleRoot a \/ OutputRoot a.

  Definition E0RemovedPreimage
      (SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
      (a : Addr) : Prop :=
    SpacePreimage a \/ RulePreimage a \/ EnvPreimage a.

  Definition E0CacheRemovedPreimage
      (CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
      (a : Addr) : Prop :=
    CapacityVictim a \/ OverwriteVictim a \/ BulkClearedEntry a.

  Definition OperatorCacheEnsurePost
      (Entry Epoch : Type)
      (heap_epoch local_epoch : Epoch)
      (CacheBefore CacheAfter : Entry -> Prop) : Prop :=
    (local_epoch = heap_epoch /\ (forall e, CacheAfter e -> CacheBefore e)) \/
    (local_epoch <> heap_epoch /\ (forall e, ~ CacheAfter e)).

  Definition WriteOnceAnchorLive
      (Anchor : Type)
      (Initialized Deleted : Anchor -> Prop)
      (slot : Anchor) : Prop :=
    Initialized slot /\ ~ Deleted slot.

  Theorem rendezvous_participant_root_survives_collection :
    forall (Occupied Published : Slot -> Prop)
           (SlotRoot : Slot -> Addr -> Prop)
           (BufferRoot DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall s, Occupied s -> Published s) ->
      (forall s a, Published s -> SlotRoot s a -> BufferRoot a) ->
      (forall a, BufferRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall s a, Occupied s -> SlotRoot s a -> ~ Freed a.
  Proof.
    intros Occupied Published SlotRoot BufferRoot DriverRoot StructuralRoot
           Edge Marked Freed Hwait Hbuffer Hdrain Hmark Hsweep s a Hoccupied Hroot Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Hdrain.
    apply (Hbuffer s a).
    - apply Hwait.
      exact Hoccupied.
    - exact Hroot.
  Qed.

  Theorem structural_future_touch_survives_collection :
    forall (StructuralRoot DriverRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed FutureTouch : Addr -> Prop),
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a, FutureTouch a -> Reach (CollectorRoot StructuralRoot DriverRoot) Edge a) ->
      forall a, FutureTouch a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot Edge Marked Freed FutureTouch Hmark Hsweep Hfuture a Htouch Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hfuture.
    exact Htouch.
  Qed.

  Theorem published_driver_c_survives_collection :
    forall (DriverC DriverRoot StructuralRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, DriverC a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, DriverC a -> ~ Freed a.
  Proof.
    intros DriverC DriverRoot StructuralRoot Edge Marked Freed
           Hpublished Hmark Hsweep a Hdriver Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Hpublished.
    exact Hdriver.
  Qed.

  Theorem batch_result_handoff_survives_collection :
    forall (StructuralRoot DriverRoot BatchResult
            HandleRoot OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> BatchHandoffRoot HandleRoot OutputRoot a) ->
      (forall a, HandleRoot a -> DriverRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult HandleRoot OutputRoot
           Edge Marked Freed Hcovered Hhandle_driver Houtput_driver Hmark Hsweep
           a Hresult Hfreed.
    destruct (Hcovered a Hresult) as [Hhandle | Houtput].
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Hhandle_driver.
      exact Hhandle.
    - apply (Hsweep a Hfreed).
      apply Hmark.
      apply reach_root.
      right.
      apply Houtput_driver.
      exact Houtput.
  Qed.

  Theorem batch_handle_drop_after_output_copy_survives_collection :
    forall (StructuralRoot DriverRoot BatchResult OutputRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, BatchResult a -> OutputRoot a) ->
      (forall a, OutputRoot a -> DriverRoot a) ->
      (forall a, Reach (CollectorRoot StructuralRoot DriverRoot) Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, BatchResult a -> ~ Freed a.
  Proof.
    intros StructuralRoot DriverRoot BatchResult OutputRoot Edge Marked Freed
           Hcopied Houtput_driver Hmark Hsweep a Hresult Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right.
    apply Houtput_driver.
    apply Hcopied.
    exact Hresult.
  Qed.

  Theorem young_reachable_marked :
    forall (Root Young Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      forall a, Reach Root Edge a -> Young a -> Marked a.
  Proof.
    intros Root Young Marked Edge Hroot Hno_old_to_young Hclosed a Hreach.
    induction Hreach as [a Hroot_a | parent child Hparent IH Hedge].
    - intro Hyoung.
      apply Hroot; assumption.
    - intro Hchild_young.
      pose proof (Hno_old_to_young parent child Hedge Hchild_young) as Hparent_young.
      apply Hclosed with (parent := parent); auto.
  Qed.

  Theorem young_minor_reachable_survives_sweep :
    forall (Root Young Marked Freed : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Young a -> Marked a) ->
      (forall parent child, Edge parent child -> Young child -> Young parent) ->
      (forall parent child,
          Marked parent -> Young parent -> Edge parent child -> Young child -> Marked child) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Reach Root Edge a -> Young a -> ~ Freed a.
  Proof.
    intros Root Young Marked Freed Edge Hroot Hno_old_to_young Hclosed Hsweep a Hreach Hyoung Hfreed.
    apply (Hsweep a Hfreed).
    eapply young_reachable_marked; eauto.
  Qed.

  Theorem reachable_seen :
    forall (Root Seen : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      forall a, Reach Root Edge a -> Seen a.
  Proof.
    intros Root Seen Edge Hroot_seen Hseen_closed a Hreach.
    induction Hreach as [a Hroot | parent child _ IH Hedge].
    - apply Hroot_seen.
      exact Hroot.
    - eapply Hseen_closed; eauto.
  Qed.

  Theorem conservative_young_reachable_marked :
    forall (Root Young Seen Marked : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      (forall a, Seen a -> Young a -> Marked a) ->
      forall a, Reach Root Edge a -> Young a -> Marked a.
  Proof.
    intros Root Young Seen Marked Edge Hroot_seen Hseen_closed Hseen_young_marked
      a Hreach Hyoung.
    apply Hseen_young_marked; [| exact Hyoung].
    eapply reachable_seen; eauto.
  Qed.

  Theorem conservative_young_minor_reachable_survives_sweep :
    forall (Root Young Seen Marked Freed : Addr -> Prop) (Edge : Addr -> Addr -> Prop),
      (forall a, Root a -> Seen a) ->
      (forall parent child, Seen parent -> Edge parent child -> Seen child) ->
      (forall a, Seen a -> Young a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, Reach Root Edge a -> Young a -> ~ Freed a.
  Proof.
    intros Root Young Seen Marked Freed Edge Hroot_seen Hseen_closed Hseen_young_marked
      Hsweep a Hreach Hyoung Hfreed.
    apply (Hsweep a Hfreed).
    eapply conservative_young_reachable_marked; eauto.
  Qed.

  Theorem e2_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      (forall a,
          SnapshotLive a ->
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack Edge Marked Freed SnapshotLive
           Hmark Hsweep Hcovered a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply Hcovered.
    exact Hlive.
  Qed.

  Theorem e2_published_allocate_black_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            PublishedAlloc : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, PublishedAlloc a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack PublishedAlloc
           Edge Marked Freed Hpublished_black Hmark Hsweep a Hpublished Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; right.
    apply Hpublished_black.
    exact Hpublished.
  Qed.

  Theorem e2_final_remark_root_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            FinalRoot : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed : Addr -> Prop),
      (forall a, FinalRoot a -> DriverRoot a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, FinalRoot a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack FinalRoot
           Edge Marked Freed Hremark Hmark Hsweep a Hfinal Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; left.
    apply Hremark.
    exact Hfinal.
  Qed.

  Theorem e2_closed_final_sweep_uses_stw_backstop :
    forall (FinalSweepReturned SatbSwept SatbAbort StwFallbackRan : Prop),
      (FinalSweepReturned -> ~ SatbSwept -> SatbAbort) ->
      (SatbAbort -> StwFallbackRan) ->
      FinalSweepReturned ->
      ~ SatbSwept ->
      StwFallbackRan.
  Proof.
    intros FinalSweepReturned SatbSwept SatbAbort StwFallbackRan
           Hclosed_abort Habort_stw Hreturned Hnot_swept.
    apply Habort_stw.
    apply Hclosed_abort; assumption.
  Qed.

  Theorem e2_aborted_satb_runs_requested_stw :
    forall (SatbAbort StwRequested StwRan : Prop),
      (SatbAbort -> StwRequested) ->
      (StwRequested -> StwRan) ->
      SatbAbort ->
      StwRequested /\ StwRan.
  Proof.
    intros SatbAbort StwRequested StwRan Hrequest Hrun Habort.
    split.
    - apply Hrequest; exact Habort.
    - apply Hrun.
      apply Hrequest.
      exact Habort.
  Qed.

  Theorem e2_completed_satb_request_has_collection :
    forall (SatbSuccess SatbAbort SatbSwept StwFallbackRan RequestDone : Prop),
      (SatbSuccess -> SatbSwept) ->
      (SatbAbort -> StwFallbackRan) ->
      (RequestDone -> SatbSuccess \/ SatbAbort) ->
      RequestDone ->
      RequestHandled SatbSwept StwFallbackRan.
  Proof.
    intros SatbSuccess SatbAbort SatbSwept StwFallbackRan RequestDone
           Hsuccess_swept Habort_stw Hdone_case Hdone.
    destruct (Hdone_case Hdone) as [Hsuccess | Habort].
    - left. apply Hsuccess_swept. exact Hsuccess.
    - right. apply Habort_stw. exact Habort.
  Qed.

  Theorem e2_full_major_clears_all_satb_marks :
    forall (SATBMarked Swept Cleared : Addr -> Prop),
      (forall a, SATBMarked a -> Swept a) ->
      (forall a, Swept a -> Cleared a) ->
      forall a, SATBMarked a -> Cleared a.
  Proof.
    intros SATBMarked Swept Cleared Hsatb_swept Hswept_cleared a Hmarked.
    apply Hswept_cleared.
    apply Hsatb_swept.
    exact Hmarked.
  Qed.

  Theorem e2_full_major_leaves_no_stale_mark_after_promotion :
    forall (Swept MarkedAfterSweep MarkedAfterPromotion : Addr -> Prop),
      (forall a, Swept a) ->
      (forall a, Swept a -> ~ MarkedAfterSweep a) ->
      (forall a, MarkedAfterPromotion a -> MarkedAfterSweep a) ->
      forall a, ~ MarkedAfterPromotion a.
  Proof.
    intros Swept MarkedAfterSweep MarkedAfterPromotion
           Hfull Hcleared Hpromotion_no_set a Hmarked_after_promotion.
    apply (Hcleared a).
    - apply Hfull.
    - apply Hpromotion_no_set.
      exact Hmarked_after_promotion.
  Qed.

  Theorem e2_e0_cache_removed_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            CapacityVictim OverwriteVictim BulkClearedEntry : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, CapacityVictim a -> ShadedDeletion a) ->
      (forall a, OverwriteVictim a -> ShadedDeletion a) ->
      (forall a, BulkClearedEntry a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0CacheRemovedPreimage CapacityVictim OverwriteVictim BulkClearedEntry a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           CapacityVictim OverwriteVictim BulkClearedEntry Edge Marked Freed SnapshotLive
           Hcapacity Hoverwrite Hbulk Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hcapacity_a | [Hoverwrite_a | Hbulk_a]].
    - apply Hcapacity; exact Hcapacity_a.
    - apply Hoverwrite; exact Hoverwrite_a.
    - apply Hbulk; exact Hbulk_a.
  Qed.

  Theorem e2_e0_removed_snapshot_live_survives_collection :
    forall (InitialRoot DriverRoot ShadedDeletion AllocateBlack
            SpacePreimage RulePreimage EnvPreimage : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop)
           (Marked Freed SnapshotLive : Addr -> Prop),
      (forall a, SpacePreimage a -> ShadedDeletion a) ->
      (forall a, RulePreimage a -> ShadedDeletion a) ->
      (forall a, EnvPreimage a -> ShadedDeletion a) ->
      (forall a,
          SnapshotLive a ->
          E0RemovedPreimage SpacePreimage RulePreimage EnvPreimage a) ->
      (forall a,
          Reach (ConcurrentCollectorRoot InitialRoot DriverRoot ShadedDeletion AllocateBlack) Edge a ->
          Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall a, SnapshotLive a -> ~ Freed a.
  Proof.
    intros InitialRoot DriverRoot ShadedDeletion AllocateBlack
           SpacePreimage RulePreimage EnvPreimage Edge Marked Freed SnapshotLive
           Hspace Hrule Henv Hremoved Hmark Hsweep a Hlive Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    right; right; left.
    destruct (Hremoved a Hlive) as [Hspace_a | [Hrule_a | Henv_a]].
    - apply Hspace; exact Hspace_a.
    - apply Hrule; exact Hrule_a.
    - apply Henv; exact Henv_a.
  Qed.

  Theorem operator_cache_returned_entry_current_after_ensure :
    forall (Entry Epoch : Type)
           (entry_epoch : Entry -> Epoch)
           (heap_epoch local_epoch : Epoch)
           (CacheBefore CacheAfter Returned : Entry -> Prop),
      (forall e, CacheBefore e -> entry_epoch e = local_epoch) ->
      OperatorCacheEnsurePost Entry Epoch heap_epoch local_epoch CacheBefore CacheAfter ->
      (forall e, Returned e -> CacheAfter e) ->
      forall e, Returned e -> entry_epoch e = heap_epoch.
  Proof.
    intros Entry Epoch entry_epoch heap_epoch local_epoch
           CacheBefore CacheAfter Returned Hstamped Hensure Hlookup e Hreturned.
    destruct Hensure as [[Hcurrent Hpreserved] | [_ Hcleared]].
    - rewrite (Hstamped e (Hpreserved e (Hlookup e Hreturned))).
      exact Hcurrent.
    - exfalso.
      apply (Hcleared e).
      apply Hlookup.
      exact Hreturned.
  Qed.

  Theorem write_once_anchor_survives_collection :
    forall (Anchor : Type)
           (Initialized Deleted Scanned : Anchor -> Prop)
           (AnchorValue : Anchor -> Addr -> Prop)
           (StructuralRoot Marked Freed : Addr -> Prop)
           (Edge : Addr -> Addr -> Prop),
      (forall slot, Initialized slot -> ~ Deleted slot) ->
      (forall slot, WriteOnceAnchorLive Anchor Initialized Deleted slot -> Scanned slot) ->
      (forall slot a, Scanned slot -> AnchorValue slot a -> StructuralRoot a) ->
      (forall a, Reach StructuralRoot Edge a -> Marked a) ->
      (forall a, Freed a -> ~ Marked a) ->
      forall slot a,
        Initialized slot ->
        AnchorValue slot a ->
        ~ Freed a.
  Proof.
    intros Anchor Initialized Deleted Scanned AnchorValue StructuralRoot Marked Freed Edge
           Hnot_deleted Hscanned Hroot Hmark Hsweep slot a Hinitialized Hvalue Hfreed.
    apply (Hsweep a Hfreed).
    apply Hmark.
    apply reach_root.
    apply (Hroot slot a).
    - apply Hscanned.
      split.
      + exact Hinitialized.
      + apply Hnot_deleted.
        exact Hinitialized.
    - exact Hvalue.
  Qed.
End CESKCollectorSafetyModel.

End MeTTaTron_GC_CESKCollectorSafety.
