------------------------- MODULE YoungAllocationOdometer -----------------------
EXTENDS Integers
(***************************************************************************)
(* C1.c young-allocation odometer model.                                  *)
(*                                                                        *)
(* Reusing a young free slot and fresh bump allocation both increment      *)
(* young_alloc_bytes by one positive node-size unit. Promotion resets it.  *)
(*                                                                        *)
(* CountReuse=FALSE    => reused young slots do not advance the odometer.  *)
(* CountBump=FALSE     => fresh bump allocation does not advance it.       *)
(* ResetOnPromote=FALSE => stale odometer survives promotion.              *)
(***************************************************************************)

CONSTANTS CountReuse, CountBump, ResetOnPromote

VARIABLES
    phase,
    event,
    before,
    after,
    minorDue

vars == <<phase, event, before, after, minorDue>>

Budget == 2

Events == {"reuse", "bump", "promote"}

TypeOK ==
    /\ CountReuse \in BOOLEAN
    /\ CountBump \in BOOLEAN
    /\ ResetOnPromote \in BOOLEAN
    /\ phase \in {"start", "chosen", "done"}
    /\ event \in Events
    /\ before \in 0..4
    /\ after \in 0..5
    /\ minorDue \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ event = "reuse"
    /\ before = 0
    /\ after = 0
    /\ minorDue = FALSE

PickEvent ==
    /\ phase = "start"
    /\ event' \in Events
    /\ before' \in 0..4
    /\ phase' = "chosen"
    /\ UNCHANGED <<after, minorDue>>

ApplyEvent ==
    /\ phase = "chosen"
    /\ after' =
        IF event = "reuse" THEN
            IF CountReuse THEN before + 1 ELSE before
        ELSE IF event = "bump" THEN
            IF CountBump THEN before + 1 ELSE before
        ELSE
            IF ResetOnPromote THEN 0 ELSE before
    /\ minorDue' = (after' > Budget)
    /\ phase' = "done"
    /\ UNCHANGED <<event, before>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ PickEvent
    \/ ApplyEvent
    \/ Done

Spec == Init /\ [][Next]_vars

ReuseCountsYoungAllocation ==
    ~(phase = "done" /\ event = "reuse" /\ after <= before)

BumpCountsYoungAllocation ==
    ~(phase = "done" /\ event = "bump" /\ after <= before)

PromotionResetsOdometer ==
    ~(phase = "done" /\ event = "promote" /\ after # 0)

BudgetCrossingRequestsMinor ==
    ~(phase = "done" /\ after > Budget /\ ~minorDue)

=============================================================================
