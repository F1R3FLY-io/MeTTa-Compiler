(** Rust-to-model refinement boundary for the CESK index allocator.

    The source-coupling harness pins concrete Rust ordering facts in
    [index_arena.rs] and [index_heap.rs]: segment cells are written before
    publication, node slots are written and allocate-black-marked before slot
    publication, side payloads are published before their owning node, shared
    allocation paths are fresh-bump-only, and free-list ownership is represented
    by the persistent [free_bit].  This file composes those source-pinned facts
    into the abstract allocator contract consumed by [CESKCollectorSafety.v].
*)

From Stdlib Require Import Bool List.
Import ListNotations.

Module MeTTaTron_GC_IndexAllocatorRefinement.

Section IndexAllocatorRefinementModel.
  Variable Addr : Type.

  Definition PublishedSlotReady
      (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned :
          Addr -> Prop)
      (a : Addr) : Prop :=
    SegmentWritten a /\ SegmentPublished a /\ SlotWritten a /\ SlotPublished a /\
      AddrReturned a.

  Definition PublishedSidePayloadReady
      (SideSegmentWritten SideSegmentPublished SidePageWritten SidePagePublished
       SideChunkWritten SideChunkPublished SideEntryWritten SideEntryPublished
       SideAddrReturned OwnerNodePublished SameSegment : Addr -> Prop)
      (a : Addr) : Prop :=
    SideSegmentWritten a /\ SideSegmentPublished a /\
      SidePageWritten a /\ SidePagePublished a /\
      SideChunkWritten a /\ SideChunkPublished a /\
      SideEntryWritten a /\ SideEntryPublished a /\
      SideAddrReturned a /\ OwnerNodePublished a /\ SameSegment a.

  Definition FreeListSeparated
      (Fresh OnFreeList : Addr -> Prop) : Prop :=
    forall a, Fresh a -> ~ OnFreeList a.

  Definition FreeBitTracksFreeList
      (freeList : list Addr)
      (FreeBit : Addr -> bool)
      (OnFreeList : Addr -> Prop) : Prop :=
    (forall a, FreeBit a = true <-> In a freeList) /\
      (forall a, OnFreeList a <-> In a freeList) /\
      NoDup freeList.

  Definition ReuseIsExclusiveCurrent
      (ReuseReturned ExclusivePath CurrentSegment OnFreeList : Addr -> Prop) : Prop :=
    forall a,
      ReuseReturned a ->
      ExclusivePath a /\ CurrentSegment a /\ OnFreeList a.

  Definition AllocatorAbstractContract
      (freeList : list Addr)
      (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
       ReadObserved PublishedAlloc AllocateBlack ConcurrentReturned Fresh
       ReuseReturned ExclusivePath CurrentSegment OnFreeList
       SideSegmentWritten SideSegmentPublished SidePageWritten SidePagePublished
       SideChunkWritten SideChunkPublished SideEntryWritten SideEntryPublished
       SideAddrReturned OwnerNodePublished SameSegment SideReadObserved :
          Addr -> Prop)
      (FreeBit : Addr -> bool) : Prop :=
    (forall a,
        ReadObserved a ->
        PublishedSlotReady
          SegmentWritten SegmentPublished SlotWritten SlotPublished
          AddrReturned a) /\
      (forall a,
          SideReadObserved a ->
          PublishedSidePayloadReady
            SideSegmentWritten SideSegmentPublished SidePageWritten
            SidePagePublished SideChunkWritten SideChunkPublished
            SideEntryWritten SideEntryPublished SideAddrReturned
            OwnerNodePublished SameSegment a) /\
      (forall a, PublishedAlloc a -> AllocateBlack a) /\
      FreeListSeparated Fresh OnFreeList /\
      ReuseIsExclusiveCurrent ReuseReturned ExclusivePath CurrentSegment
        OnFreeList /\
      (forall a, ConcurrentReturned a -> ~ ReuseReturned a) /\
      FreeBitTracksFreeList freeList FreeBit OnFreeList.

  Theorem source_coupled_index_allocator_refines_abstract_contract :
    forall (freeList : list Addr)
           (SegmentWritten SegmentPublished SlotWritten SlotPublished
            AddrReturned ReadObserved PublishedAlloc AllocateBlack
            ConcurrentReturned Fresh ReuseReturned ExclusivePath CurrentSegment
            OnFreeList SideSegmentWritten SideSegmentPublished SidePageWritten
            SidePagePublished SideChunkWritten SideChunkPublished
            SideEntryWritten SideEntryPublished SideAddrReturned
            OwnerNodePublished SameSegment SideReadObserved : Addr -> Prop)
           (FreeBit : Addr -> bool),
      (forall a, ReadObserved a -> AddrReturned a) ->
      (forall a, AddrReturned a -> SlotPublished a) ->
      (forall a, SlotPublished a -> SlotWritten a) ->
      (forall a, SlotPublished a -> SegmentPublished a) ->
      (forall a, SegmentPublished a -> SegmentWritten a) ->
      (forall a, SideReadObserved a -> OwnerNodePublished a) ->
      (forall a, OwnerNodePublished a -> SideAddrReturned a) ->
      (forall a, SideAddrReturned a -> SideEntryPublished a) ->
      (forall a, SideEntryPublished a -> SideEntryWritten a) ->
      (forall a, SideEntryPublished a -> SideChunkPublished a) ->
      (forall a, SideChunkPublished a -> SideChunkWritten a) ->
      (forall a, SideChunkPublished a -> SidePagePublished a) ->
      (forall a, SidePagePublished a -> SidePageWritten a) ->
      (forall a, SideAddrReturned a -> SideSegmentPublished a) ->
      (forall a, SideSegmentPublished a -> SideSegmentWritten a) ->
      (forall a, OwnerNodePublished a -> SameSegment a) ->
      (forall a, PublishedAlloc a -> AllocateBlack a) ->
      (forall a, ConcurrentReturned a -> Fresh a) ->
      (forall a, Fresh a -> ~ OnFreeList a) ->
      (forall a, ReuseReturned a -> OnFreeList a) ->
      (forall a, ReuseReturned a -> ExclusivePath a) ->
      (forall a, ReuseReturned a -> CurrentSegment a) ->
      FreeBitTracksFreeList freeList FreeBit OnFreeList ->
      AllocatorAbstractContract freeList SegmentWritten SegmentPublished
        SlotWritten SlotPublished AddrReturned ReadObserved PublishedAlloc
        AllocateBlack ConcurrentReturned Fresh ReuseReturned ExclusivePath
        CurrentSegment OnFreeList SideSegmentWritten SideSegmentPublished
        SidePageWritten SidePagePublished SideChunkWritten SideChunkPublished
        SideEntryWritten SideEntryPublished SideAddrReturned OwnerNodePublished
        SameSegment SideReadObserved FreeBit.
  Proof.
    intros freeList SegmentWritten SegmentPublished SlotWritten SlotPublished
           AddrReturned ReadObserved PublishedAlloc AllocateBlack
           ConcurrentReturned Fresh ReuseReturned ExclusivePath CurrentSegment
           OnFreeList SideSegmentWritten SideSegmentPublished SidePageWritten
           SidePagePublished SideChunkWritten SideChunkPublished SideEntryWritten
           SideEntryPublished SideAddrReturned OwnerNodePublished SameSegment
           SideReadObserved FreeBit Hread Hreturned Hslot_written Hslot_segment
           Hsegment_written Hside_read Howner_side Hside_returned
           Hside_entry_written Hside_entry_chunk Hside_chunk_written
           Hside_chunk_page Hside_page_written Hside_segment
           Hside_segment_written Hsame_segment Hpublished_black Hfresh
           Hfresh_separated Hreuse_on_free Hreuse_exclusive Hreuse_current
           Hfree_bits.
    unfold AllocatorAbstractContract.
    split.
    - intros addr Hread_observed.
      unfold PublishedSlotReady.
      repeat split.
      + apply Hsegment_written.
        apply Hslot_segment.
        apply Hreturned.
        apply Hread.
        exact Hread_observed.
      + apply Hslot_segment.
        apply Hreturned.
        apply Hread.
        exact Hread_observed.
      + apply Hslot_written.
        apply Hreturned.
        apply Hread.
        exact Hread_observed.
      + apply Hreturned.
        apply Hread.
        exact Hread_observed.
      + apply Hread.
        exact Hread_observed.
    - split.
      + intros addr Hside_observed.
        unfold PublishedSidePayloadReady.
        repeat split.
        * apply Hside_segment_written.
          apply Hside_segment.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_segment.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_page_written.
          apply Hside_chunk_page.
          apply Hside_entry_chunk.
          apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_chunk_page.
          apply Hside_entry_chunk.
          apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_chunk_written.
          apply Hside_entry_chunk.
          apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_entry_chunk.
          apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_entry_written.
          apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_returned.
          apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Howner_side.
          apply Hside_read.
          exact Hside_observed.
        * apply Hside_read.
          exact Hside_observed.
        * apply Hsame_segment.
          apply Hside_read.
          exact Hside_observed.
      + split.
        * intros addr Hpublished.
          apply Hpublished_black.
          exact Hpublished.
        * split.
          -- intros addr Hfresh_a.
             apply Hfresh_separated.
             exact Hfresh_a.
          -- split.
             ++ intros addr Hreuse.
                repeat split.
                ** apply Hreuse_exclusive.
                   exact Hreuse.
                ** apply Hreuse_current.
                   exact Hreuse.
                ** apply Hreuse_on_free.
                   exact Hreuse.
             ++ split.
                ** intros addr Hconcurrent Hreuse.
                   pose proof (Hfresh addr Hconcurrent) as Hfresh_a.
                   pose proof (Hfresh_separated addr Hfresh_a) as Hnot_on_free.
                   apply Hnot_on_free.
                   apply Hreuse_on_free.
                   exact Hreuse.
                ** exact Hfree_bits.
  Qed.
End IndexAllocatorRefinementModel.

End MeTTaTron_GC_IndexAllocatorRefinement.
