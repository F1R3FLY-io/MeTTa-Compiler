------------------------- MODULE YoungAllocationOdometer -----------------------
EXTENDS Integers
(***************************************************************************)
(* C1.c young-allocation odometer model.                                  *)
(*                                                                        *)
(* Reusing a young free slot and fresh bump allocation both increment      *)
(* young_alloc_bytes by one positive node-size unit. Promotion resets it.  *)
(*                                                                        *)
(* CountReuse=FALSE     => reused young slots do not advance the odometer. *)
(* CountBump=FALSE      => fresh bump allocation does not advance it.      *)
(* ResetOnPromote=FALSE => stale odometer survives promotion.              *)
(* budget is chosen once at Init and then fixed, matching the runtime       *)
(* OnceLock parser for METTATRON_INDEX_GC_YOUNG_BYTES.                     *)
(***************************************************************************)

CONSTANTS CountReuse, CountBump, ResetOnPromote

VARIABLES
    phase,
    event,
    budget,
    before,
    after,
    minorDue

vars == <<phase, event, budget, before, after, minorDue>>

Events == {"reuse", "bump", "promote"}

TypeOK ==
    /\ CountReuse \in BOOLEAN
    /\ CountBump \in BOOLEAN
    /\ ResetOnPromote \in BOOLEAN
    /\ phase \in {"start", "chosen", "done"}
    /\ event \in Events
    /\ budget \in 1..4
    /\ before \in 0..4
    /\ after \in 0..5
    /\ minorDue \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ event = "reuse"
    /\ budget \in 1..4
    /\ before = 0
    /\ after = 0
    /\ minorDue = FALSE

PickEvent ==
    /\ phase = "start"
    /\ event' \in Events
    /\ before' \in 0..4
    /\ phase' = "chosen"
    /\ UNCHANGED <<budget, after, minorDue>>

ApplyEvent ==
    /\ phase = "chosen"
    /\ after' =
        CASE event = "reuse" -> IF CountReuse THEN before + 1 ELSE before
          [] event = "bump" -> IF CountBump THEN before + 1 ELSE before
          [] OTHER -> IF ResetOnPromote THEN 0 ELSE before
    /\ minorDue' = (after' > budget)
    /\ phase' = "done"
    /\ UNCHANGED <<event, budget, before>>

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
    ~(phase = "done" /\ after > budget /\ ~minorDue)

=============================================================================
