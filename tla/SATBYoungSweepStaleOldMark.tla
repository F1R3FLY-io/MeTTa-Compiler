----------------------- MODULE SATBYoungSweepStaleOldMark -----------------------
(***************************************************************************)
(* E2 SATB stale-old-mark discriminator.                                  *)
(*                                                                         *)
(* SATB deletion barriers may mark old-generation nodes. A young-only      *)
(* final sweep clears young marks only; if an old SATB mark exists, it can *)
(* remain stale into the next cycle. A full-major final sweep clears both  *)
(* young and old marks and preserves NoStaleOldMark.                       *)
(***************************************************************************)

CONSTANTS
    ClearOldMarks

VARIABLES
    phase,
    youngMarked,
    oldMarked

vars == <<phase, youngMarked, oldMarked>>

TypeOK ==
    /\ phase \in {"satb_marked", "swept", "promoted"}
    /\ youngMarked \in BOOLEAN
    /\ oldMarked \in BOOLEAN

Init ==
    /\ phase = "satb_marked"
    /\ youngMarked = TRUE
    /\ oldMarked = TRUE

FinalSweep ==
    /\ phase = "satb_marked"
    /\ phase' = "swept"
    /\ youngMarked' = FALSE
    /\ oldMarked' = IF ClearOldMarks THEN FALSE ELSE oldMarked

Promote ==
    /\ phase = "swept"
    /\ phase' = "promoted"
    /\ UNCHANGED <<youngMarked, oldMarked>>

Done ==
    /\ phase = "promoted"
    /\ UNCHANGED vars

Next ==
    \/ FinalSweep
    \/ Promote
    \/ Done

Spec == Init /\ [][Next]_vars

NoStaleOldMark ==
    phase = "promoted" => ~oldMarked

=============================================================================
