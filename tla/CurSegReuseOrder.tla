---------------------------- MODULE CurSegReuseOrder ----------------------------
(***************************************************************************)
(* Historical C1 skipped-old young-marker discriminator.                    *)
(*                                                                        *)
(* The live implementation is now the conservative minor marker modeled by  *)
(* ConservativeMinorMark.tla: it traverses old reachable containers and     *)
(* marks only young nodes. This model remains in the harness as an          *)
(* auxiliary discriminator for the rejected skipped-old design: if a marker *)
(* skips old nodes, it needs the allocator premise that no old node can     *)
(* point at a young node. Cur-segment-only reuse preserves that premise;    *)
(* any-young reuse breaks it after promotion.                               *)
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
