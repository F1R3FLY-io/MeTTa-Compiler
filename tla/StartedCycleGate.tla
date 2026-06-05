--------------------------- MODULE StartedCycleGate ---------------------------
(***************************************************************************)
(* E5 started-cycle straddle gate model.                                  *)
(*                                                                        *)
(* GC_CYCLE_GEN is bumped at cycle END, before GC_IN_PROGRESS is cleared. *)
(* In that teardown window gen already names K+1, but no K+1 driver has   *)
(* started. GC_CYCLE_STARTED stays K until a live driver prologue commits *)
(* to K+1. A straddling worker must re-park only for a started cycle, not *)
(* merely because gen advanced.                                           *)
(*                                                                        *)
(* UseStartedGate = TRUE  => gate on started > my (production E5).        *)
(* UseStartedGate = FALSE => gate on gen > my (buggy phantom re-park).    *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UseStartedGate

VARIABLES
    gen,
    started,
    gip,
    my,
    reparked,
    phantom

vars == <<gen, started, gip, my, reparked, phantom>>

TypeOK ==
    /\ gen \in 1..2
    /\ started \in 1..2
    /\ started <= gen
    /\ gip \in BOOLEAN
    /\ my \in 1..2
    /\ reparked \in BOOLEAN
    /\ phantom \in BOOLEAN

Init ==
    /\ gen = 1
    /\ started = 1
    /\ gip = TRUE
    /\ my = 1
    /\ reparked = FALSE
    /\ phantom = FALSE

(* Cycle K ends: gen is pre-bumped while gip remains true until the guard
   drops. This is the teardown window. *)
EndCycle ==
    /\ gen = 1
    /\ started = 1
    /\ gip = TRUE
    /\ gen' = 2
    /\ UNCHANGED <<started, gip, my, reparked, phantom>>

DropGip ==
    /\ gip
    /\ gip' = FALSE
    /\ UNCHANGED <<gen, started, my, reparked, phantom>>

StartNextCycle ==
    /\ gen = 2
    /\ started = 1
    /\ gip = TRUE
    /\ started' = 2
    /\ UNCHANGED <<gen, gip, my, reparked, phantom>>

ShouldRepark ==
    IF UseStartedGate
      THEN started > my
      ELSE gen > my

Repark ==
    /\ gip
    /\ ShouldRepark
    /\ ~reparked
    /\ reparked' = TRUE
    /\ phantom' = (started <= my)
    /\ my' = IF started > my THEN started ELSE my
    /\ UNCHANGED <<gen, started, gip>>

Done ==
    /\ UNCHANGED vars

Next ==
    \/ EndCycle
    \/ StartNextCycle
    \/ DropGip
    \/ Repark
    \/ Done

Spec == Init /\ [][Next]_vars

NoPhantomRepark ==
    ~phantom

StartedGateReparksWhenRealCycleStarts ==
    started = 2 /\ gip => reparked

=============================================================================
