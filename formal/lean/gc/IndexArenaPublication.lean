/-!
IndexArena segment and slot publication obligations.

Fresh allocation in the index arena is a two-level publication protocol:
initialize and publish the segment directory cell, write the claimed slot, publish
the slot into the segment's contiguous `len` prefix, then return the `Addr`.
A later reader of that returned address is safe only under that order.
-/

namespace MeTTaTron.GC.IndexArenaPublication

variable {Addr : Type u}

def PublishedSlotReady
    (SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned :
      Addr -> Prop)
    (a : Addr) : Prop :=
  SegmentWritten a ∧ SegmentPublished a ∧ SlotWritten a ∧ SlotPublished a ∧
    AddrReturned a

theorem returned_addr_read_ready
    {SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned
      ReadObserved : Addr -> Prop}
    (readOnlyAfterReturn : forall {a : Addr}, ReadObserved a -> AddrReturned a)
    (returnOnlyAfterSlotPublish :
      forall {a : Addr}, AddrReturned a -> SlotPublished a)
    (slotPublishAfterWrite : forall {a : Addr}, SlotPublished a -> SlotWritten a)
    (slotPublishAfterSegment :
      forall {a : Addr}, SlotPublished a -> SegmentPublished a)
    (segmentPublishAfterWrite :
      forall {a : Addr}, SegmentPublished a -> SegmentWritten a) :
    forall {a : Addr},
      ReadObserved a ->
      PublishedSlotReady
        SegmentWritten SegmentPublished SlotWritten SlotPublished AddrReturned a := by
  intro a hread
  have hreturned := readOnlyAfterReturn hread
  have hslotPublished := returnOnlyAfterSlotPublish hreturned
  have hslotWritten := slotPublishAfterWrite hslotPublished
  have hsegmentPublished := slotPublishAfterSegment hslotPublished
  have hsegmentWritten := segmentPublishAfterWrite hsegmentPublished
  exact And.intro hsegmentWritten
    (And.intro hsegmentPublished
      (And.intro hslotWritten
        (And.intro hslotPublished hreturned)))

end MeTTaTron.GC.IndexArenaPublication
