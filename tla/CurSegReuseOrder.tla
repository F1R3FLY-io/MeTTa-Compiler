---------------------------- MODULE CurSegReuseOrder ----------------------------
(***************************************************************************)
(* C1 young-minor no-old-to-young discriminator.                           *)
(*                                                                        *)
(* The young-only marker skips old nodes, so the implementation avoids a   *)
(* remembered set only if no old node can point at a young node. The       *)
(* load-bearing allocator rule is cur-segment-only reuse: after a child is *)
(* allocated in curSeg, a parent may reuse only curSeg. If it reuses a      *)
(* lower young segment, promotion makes the parent old while the child      *)
(* remains young.                                                          *)
(*                                                                        *)
(* ReuseCurSegOnly = TRUE  => production C1 reuse policy.                  *)
(* ReuseCurSegOnly = FALSE => any-young reuse, the rejected policy.        *)
(***************************************************************************)
EXTENDS Integers

CONSTANT ReuseCurSegOnly

CurSeg == 2
InitialYoungFloor == 1
ChildSeg == CurSeg

VARIABLES
    parentAllocated,
    parentSeg,
    youngFloor,
    promoted

vars == <<parentAllocated, parentSeg, youngFloor, promoted>>

ReuseTargets ==
    IF ReuseCurSegOnly THEN {CurSeg} ELSE InitialYoungFloor..CurSeg

TypeOK ==
    /\ parentAllocated \in BOOLEAN
    /\ parentSeg \in 0..CurSeg
    /\ youngFloor \in InitialYoungFloor..CurSeg
    /\ promoted \in BOOLEAN

Init ==
    /\ parentAllocated = FALSE
    /\ parentSeg = CurSeg
    /\ youngFloor = InitialYoungFloor
    /\ promoted = FALSE

AllocParent ==
    /\ ~parentAllocated
    /\ \E s \in ReuseTargets :
        /\ parentSeg' = s
        /\ parentAllocated' = TRUE
    /\ UNCHANGED <<youngFloor, promoted>>

Promote ==
    /\ parentAllocated
    /\ ~promoted
    /\ youngFloor' = CurSeg
    /\ promoted' = TRUE
    /\ UNCHANGED <<parentAllocated, parentSeg>>

Done ==
    /\ UNCHANGED vars

Next ==
    \/ AllocParent
    \/ Promote
    \/ Done

Spec == Init /\ [][Next]_vars

NoOldToYoungAfterPromotion ==
    ~(promoted /\ parentSeg < youngFloor /\ ChildSeg >= youngFloor)

BumpOrderPreserved ==
    parentAllocated => ChildSeg <= parentSeg

=============================================================================
