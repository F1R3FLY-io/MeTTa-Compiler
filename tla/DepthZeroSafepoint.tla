----------------------------- MODULE DepthZeroSafepoint -----------------------------
(***************************************************************************)
(* E1 cooperative-safepoint depth-zero discriminator.                       *)
(*                                                                         *)
(* A caller with EvalGuard depth zero is not in the dedicated collector's   *)
(* participant snapshot. It must therefore return without parking and       *)
(* without dropping an EvalGuard.                                           *)
(***************************************************************************)

CONSTANT UseDepthGuard

VARIABLES
    phase,
    depth,
    parked,
    droppedGuard,
    returned

vars == <<phase, depth, parked, droppedGuard, returned>>

TypeOK ==
    /\ UseDepthGuard \in BOOLEAN
    /\ phase \in {"start", "done"}
    /\ depth \in {0, 1}
    /\ parked \in BOOLEAN
    /\ droppedGuard \in BOOLEAN
    /\ returned \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ depth \in {0, 1}
    /\ parked = FALSE
    /\ droppedGuard = FALSE
    /\ returned = FALSE

CooperativeSafepoint ==
    /\ phase = "start"
    /\ IF UseDepthGuard /\ depth = 0 THEN
          /\ parked' = FALSE
          /\ droppedGuard' = FALSE
          /\ returned' = TRUE
       ELSE
          /\ parked' = TRUE
          /\ droppedGuard' = TRUE
          /\ returned' = FALSE
    /\ phase' = "done"
    /\ UNCHANGED depth

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ CooperativeSafepoint
    \/ Done

Spec == Init /\ [][Next]_vars

DepthZeroDoesNotPark ==
    depth = 0 => ~parked

DepthZeroDoesNotDropGuard ==
    depth = 0 => ~droppedGuard

DepthPositiveParks ==
    phase = "done" /\ depth = 1 => parked /\ droppedGuard

=============================================================================
