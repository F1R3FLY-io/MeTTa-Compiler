(** IndexArena segment and slot publication obligations.

    Fresh allocation in the index arena is a two-level publication protocol:
    initialize and publish the segment directory cell, write the claimed slot,
    publish the slot into the segment's contiguous [len] prefix, then return the
    [Addr]. A later reader of that returned address is safe only under that
    order.
*)

Module MeTTaTron_GC_IndexArenaPublication.

Section IndexArenaPublicationModel.
  Variable Addr : Type.

  Definition PublishedSlotReady
      (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned :
          Addr -> Prop)
      (a : Addr) : Prop :=
    SegmentWritten a /\ SegmentPublished a /\ SlotWritten a /\ SlotPublished a /\
      AddrReturned a.

  Theorem returned_addr_read_ready :
    forall (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
            ReadObserved : Addr -> Prop),
      (forall a, ReadObserved a -> AddrReturned a) ->
      (forall a, AddrReturned a -> SlotPublished a) ->
      (forall a, SlotPublished a -> SlotWritten a) ->
      (forall a, SlotPublished a -> SegmentPublished a) ->
      (forall a, SegmentPublished a -> SegmentWritten a) ->
      forall a,
        ReadObserved a ->
        PublishedSlotReady
          SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned a.
  Proof.
    intros SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
           ReadObserved Hread Hreturn Hslot_write Hslot_segment Hsegment_write
           a Hread_observed.
    split.
    - apply Hsegment_write. apply Hslot_segment. apply Hreturn. apply Hread.
      exact Hread_observed.
    - split.
      + apply Hslot_segment. apply Hreturn. apply Hread. exact Hread_observed.
      + split.
        * apply Hslot_write. apply Hreturn. apply Hread. exact Hread_observed.
        * split.
          -- apply Hreturn. apply Hread. exact Hread_observed.
          -- apply Hread. exact Hread_observed.
  Qed.
End IndexArenaPublicationModel.

End MeTTaTron_GC_IndexArenaPublication.
