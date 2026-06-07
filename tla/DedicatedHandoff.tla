------------------------------ MODULE DedicatedHandoff ------------------------------
(***************************************************************************)
(* E1 dedicated-GC-thread root handoff discriminator.                      *)
(*                                                                        *)
(* If send fails before the root Vec moves, inline fallback is safe.       *)
(* If send succeeds, the GC thread owns the root Vec. A response-channel   *)
(* failure after that point must skip the cycle, not run inline with an    *)
(* empty/missing root set.                                                *)
(***************************************************************************)

CONSTANT FallbackAfterConsumed

VARIABLES
    phase,
    rootsAvailable,
    handoffConsumed,
    recvFailed,
    inlineRan,
    skipped

vars == <<phase, rootsAvailable, handoffConsumed, recvFailed, inlineRan, skipped>>

TypeOK ==
    /\ FallbackAfterConsumed \in BOOLEAN
    /\ phase \in {"start", "sent", "done"}
    /\ rootsAvailable \in BOOLEAN
    /\ handoffConsumed \in BOOLEAN
    /\ recvFailed \in BOOLEAN
    /\ inlineRan \in BOOLEAN
    /\ skipped \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ rootsAvailable = TRUE
    /\ handoffConsumed = FALSE
    /\ recvFailed = FALSE
    /\ inlineRan = FALSE
    /\ skipped = FALSE

SendFailsBeforeConsume ==
    /\ phase = "start"
    /\ rootsAvailable
    /\ handoffConsumed' = FALSE
    /\ rootsAvailable' = TRUE
    /\ recvFailed' = FALSE
    /\ inlineRan' = TRUE
    /\ skipped' = FALSE
    /\ phase' = "done"

SendSucceeds ==
    /\ phase = "start"
    /\ rootsAvailable
    /\ rootsAvailable' = FALSE
    /\ handoffConsumed' = TRUE
    /\ phase' = "sent"
    /\ UNCHANGED <<recvFailed, inlineRan, skipped>>

ResponseArrives ==
    /\ phase = "sent"
    /\ phase' = "done"
    /\ recvFailed' = FALSE
    /\ inlineRan' = FALSE
    /\ skipped' = FALSE
    /\ UNCHANGED <<rootsAvailable, handoffConsumed>>

ResponseFails ==
    /\ phase = "sent"
    /\ phase' = "done"
    /\ recvFailed' = TRUE
    /\ IF FallbackAfterConsumed THEN
          /\ inlineRan' = TRUE
          /\ skipped' = FALSE
       ELSE
          /\ inlineRan' = FALSE
          /\ skipped' = TRUE
    /\ UNCHANGED <<rootsAvailable, handoffConsumed>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ SendFailsBeforeConsume
    \/ SendSucceeds
    \/ ResponseArrives
    \/ ResponseFails
    \/ Done

Spec == Init /\ [][Next]_vars

NoInlineWithoutRoots ==
    ~(inlineRan /\ ~rootsAvailable)

ConsumedResponseFailureSkips ==
    phase = "done" /\ handoffConsumed /\ recvFailed => skipped /\ ~inlineRan

=============================================================================
