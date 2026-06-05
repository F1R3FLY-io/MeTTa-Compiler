----------------------------- MODULE SATBFinalRemark -----------------------------
(***************************************************************************)
(* E2 SATB final-remark discriminator.                                    *)
(*                                                                        *)
(* A value can become a structural root during the concurrent mark window. *)
(* The second rendezvous must therefore re-mark final roots before the     *)
(* exclusive sweep. Without that final remark, a final-root value that was *)
(* not reached by the initial snapshot can be reclaimed.                  *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UseFinalRemark

VARIABLES
    finalRoot,
    marked,
    freed,
    phase

vars == <<finalRoot, marked, freed, phase>>

TypeOK ==
    /\ finalRoot \in BOOLEAN
    /\ marked \in BOOLEAN
    /\ freed \in BOOLEAN
    /\ phase \in {"final", "remarked", "swept"}

Init ==
    /\ finalRoot = TRUE
    /\ marked = FALSE
    /\ freed = FALSE
    /\ phase = "final"

FinalRemark ==
    /\ phase = "final"
    /\ UseFinalRemark
    /\ marked' = finalRoot \/ marked
    /\ phase' = "remarked"
    /\ UNCHANGED <<finalRoot, freed>>

Sweep ==
    /\ phase \in {"final", "remarked"}
    /\ IF UseFinalRemark THEN phase = "remarked" ELSE TRUE
    /\ phase' = "swept"
    /\ freed' = ~marked
    /\ UNCHANGED <<finalRoot, marked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ FinalRemark
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoFinalRootFreed ==
    finalRoot => ~freed

=============================================================================
