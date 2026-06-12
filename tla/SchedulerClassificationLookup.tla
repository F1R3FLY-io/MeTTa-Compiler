---------------------- MODULE SchedulerClassificationLookup ----------------------
(***************************************************************************)
(* Discriminator for SchedulerAutomaton L2 range compactness.              *)
(*                                                                         *)
(* ShiftLaterStarts = TRUE models inserting a new entry at the end of the   *)
(* target head's contiguous range and shifting every later L1 start. FALSE  *)
(* models appending without shifting, so the target range overlaps the next *)
(* head's range after its count is incremented.                             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT ShiftLaterStarts

VARIABLES phase, startA, countA, startB

vars == <<phase, startA, countA, startB>>

TypeOK ==
    /\ phase \in {"before", "after"}
    /\ startA \in 0..2
    /\ countA \in 1..2
    /\ startB \in 1..2

Init ==
    /\ phase = "before"
    /\ startA = 0
    /\ countA = 1
    /\ startB = 1

InsertA2 ==
    /\ phase = "before"
    /\ phase' = "after"
    /\ countA' = 2
    /\ startB' = IF ShiftLaterStarts THEN 2 ELSE 1
    /\ UNCHANGED startA

Done ==
    /\ phase = "after"
    /\ UNCHANGED vars

Next ==
    \/ InsertA2
    \/ Done

Spec == Init /\ [][Next]_vars

RangesDisjoint ==
    phase = "after" =>
      ~(startB < startA + countA /\ startA < startB + 1)

=============================================================================
