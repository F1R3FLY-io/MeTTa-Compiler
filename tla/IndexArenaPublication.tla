------------------------- MODULE IndexArenaPublication -------------------------
(***************************************************************************)
(* Publication order for IndexArena directory cells and fresh bump slots.   *)
(***************************************************************************)

CONSTANTS
    SegmentWriteBeforePublish,
    SlotAfterSegmentPublish,
    SlotWriteBeforePublish,
    ReturnAfterSlotPublish

VARIABLES
    segmentWritten,
    segmentPublished,
    slotWritten,
    slotPublished,
    addrReturned,
    readObserved

vars ==
    <<segmentWritten, segmentPublished, slotWritten, slotPublished,
      addrReturned, readObserved>>

TypeOK ==
    /\ SegmentWriteBeforePublish \in BOOLEAN
    /\ SlotAfterSegmentPublish \in BOOLEAN
    /\ SlotWriteBeforePublish \in BOOLEAN
    /\ ReturnAfterSlotPublish \in BOOLEAN
    /\ segmentWritten \in BOOLEAN
    /\ segmentPublished \in BOOLEAN
    /\ slotWritten \in BOOLEAN
    /\ slotPublished \in BOOLEAN
    /\ addrReturned \in BOOLEAN
    /\ readObserved \in BOOLEAN

Init ==
    /\ segmentWritten = FALSE
    /\ segmentPublished = FALSE
    /\ slotWritten = FALSE
    /\ slotPublished = FALSE
    /\ addrReturned = FALSE
    /\ readObserved = FALSE

WriteSegment ==
    /\ ~segmentWritten
    /\ segmentWritten' = TRUE
    /\ UNCHANGED <<segmentPublished, slotWritten, slotPublished,
                  addrReturned, readObserved>>

PublishSegment ==
    /\ ~segmentPublished
    /\ IF SegmentWriteBeforePublish THEN segmentWritten ELSE TRUE
    /\ segmentPublished' = TRUE
    /\ UNCHANGED <<segmentWritten, slotWritten, slotPublished,
                  addrReturned, readObserved>>

WriteSlot ==
    /\ ~slotWritten
    /\ IF SlotAfterSegmentPublish THEN segmentPublished ELSE TRUE
    /\ slotWritten' = TRUE
    /\ UNCHANGED <<segmentWritten, segmentPublished, slotPublished,
                  addrReturned, readObserved>>

PublishSlot ==
    /\ ~slotPublished
    /\ IF SlotWriteBeforePublish THEN slotWritten ELSE TRUE
    /\ slotPublished' = TRUE
    /\ UNCHANGED <<segmentWritten, segmentPublished, slotWritten,
                  addrReturned, readObserved>>

ReturnAddr ==
    /\ ~addrReturned
    /\ IF ReturnAfterSlotPublish THEN slotPublished ELSE TRUE
    /\ addrReturned' = TRUE
    /\ UNCHANGED <<segmentWritten, segmentPublished, slotWritten,
                  slotPublished, readObserved>>

ReadAddr ==
    /\ addrReturned
    /\ ~readObserved
    /\ readObserved' = TRUE
    /\ UNCHANGED <<segmentWritten, segmentPublished, slotWritten,
                  slotPublished, addrReturned>>

Done ==
    /\ readObserved
    /\ UNCHANGED vars

Next ==
    \/ WriteSegment
    \/ PublishSegment
    \/ WriteSlot
    \/ PublishSlot
    \/ ReturnAddr
    \/ ReadAddr
    \/ Done

Spec == Init /\ [][Next]_vars

ReturnedAddrReady ==
    readObserved =>
      /\ segmentWritten
      /\ segmentPublished
      /\ slotWritten
      /\ slotPublished
      /\ addrReturned

=============================================================================
