-------------------------- MODULE NurseryBackpressure --------------------------
(***************************************************************************)
(* C1.c allocator-to-GC nursery backpressure model.                       *)
(*                                                                        *)
(* A subsequent segment open sets nursery_full_pending. The driver folds   *)
(* that flag into minor_due. Promotion then clears both the young odometer *)
(* and the pending flag so the same stale event cannot immediately re-fire *)
(* another minor.                                                         *)
(*                                                                        *)
(* SignalOnOpen = FALSE       => segment open does not request a minor.    *)
(* FoldPendingIntoMinor=FALSE => driver ignores the allocator signal.      *)
(* ClearOnPromote=FALSE       => stale pending flag immediately re-fires.  *)
(***************************************************************************)

CONSTANTS SignalOnOpen, FoldPendingIntoMinor, ClearOnPromote

VARIABLES
    phase,
    pending,
    youngOverBudget,
    minorDue,
    promoted

vars == <<phase, pending, youngOverBudget, minorDue, promoted>>

TypeOK ==
    /\ SignalOnOpen \in BOOLEAN
    /\ FoldPendingIntoMinor \in BOOLEAN
    /\ ClearOnPromote \in BOOLEAN
    /\ phase \in {"start", "opened", "checked_before", "promoted", "checked_after"}
    /\ pending \in BOOLEAN
    /\ youngOverBudget \in BOOLEAN
    /\ minorDue \in BOOLEAN
    /\ promoted \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ pending = FALSE
    /\ youngOverBudget = FALSE
    /\ minorDue = FALSE
    /\ promoted = FALSE

OpenSegment ==
    /\ phase = "start"
    /\ pending' = SignalOnOpen
    /\ phase' = "opened"
    /\ UNCHANGED <<youngOverBudget, minorDue, promoted>>

ComputeMinorDueBefore ==
    /\ phase = "opened"
    /\ minorDue' = (youngOverBudget \/ (FoldPendingIntoMinor /\ pending))
    /\ phase' = "checked_before"
    /\ UNCHANGED <<pending, youngOverBudget, promoted>>

Promote ==
    /\ phase = "checked_before"
    /\ pending' = IF ClearOnPromote THEN FALSE ELSE pending
    /\ youngOverBudget' = FALSE
    /\ promoted' = TRUE
    /\ phase' = "promoted"
    /\ UNCHANGED minorDue

ComputeMinorDueAfter ==
    /\ phase = "promoted"
    /\ minorDue' = (youngOverBudget \/ (FoldPendingIntoMinor /\ pending))
    /\ phase' = "checked_after"
    /\ UNCHANGED <<pending, youngOverBudget, promoted>>

Done ==
    /\ phase = "checked_after"
    /\ UNCHANGED vars

Next ==
    \/ OpenSegment
    \/ ComputeMinorDueBefore
    \/ Promote
    \/ ComputeMinorDueAfter
    \/ Done

Spec == Init /\ [][Next]_vars

NurseryOpenSignalsPending ==
    ~(phase = "opened" /\ ~pending)

OpenRequestsMinor ==
    ~(phase = "checked_before" /\ pending /\ ~minorDue)

PromoteRelaxesMinorTrigger ==
    ~(phase = "checked_after" /\ minorDue)

=============================================================================
