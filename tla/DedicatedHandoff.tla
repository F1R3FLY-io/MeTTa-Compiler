------------------------------ MODULE DedicatedHandoff ------------------------------
(***************************************************************************)
(* E1 dedicated-GC-thread root handoff discriminator.                      *)
(*                                                                        *)
(* If send fails before the root Vec moves, inline fallback is safe.       *)
(* If send succeeds, the GC thread owns the root Vec. A response-channel   *)
(* failure after that point must skip the cycle, not run inline with an    *)
(* empty/missing root set.                                                *)
(***************************************************************************)

CONSTANTS FallbackAfterConsumed,
          OmitResponseAttempt

VARIABLES
    phase,
    rootsAvailable,
    handoffConsumed,
    responseSenderCarried,
    responseSendAttempted,
    recvFailed,
    inlineRan,
    skipped

vars == <<phase, rootsAvailable, handoffConsumed, responseSenderCarried,
          responseSendAttempted, recvFailed, inlineRan, skipped>>

TypeOK ==
    /\ FallbackAfterConsumed \in BOOLEAN
    /\ OmitResponseAttempt \in BOOLEAN
    /\ phase \in {"start", "sent", "done"}
    /\ rootsAvailable \in BOOLEAN
    /\ handoffConsumed \in BOOLEAN
    /\ responseSenderCarried \in BOOLEAN
    /\ responseSendAttempted \in BOOLEAN
    /\ recvFailed \in BOOLEAN
    /\ inlineRan \in BOOLEAN
    /\ skipped \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ rootsAvailable = TRUE
    /\ handoffConsumed = FALSE
    /\ responseSenderCarried = FALSE
    /\ responseSendAttempted = FALSE
    /\ recvFailed = FALSE
    /\ inlineRan = FALSE
    /\ skipped = FALSE

SendFailsBeforeConsume ==
    /\ phase = "start"
    /\ rootsAvailable
    /\ handoffConsumed' = FALSE
    /\ rootsAvailable' = TRUE
    /\ responseSenderCarried' = FALSE
    /\ responseSendAttempted' = FALSE
    /\ recvFailed' = FALSE
    /\ inlineRan' = TRUE
    /\ skipped' = FALSE
    /\ phase' = "done"

SendSucceeds ==
    /\ phase = "start"
    /\ rootsAvailable
    /\ rootsAvailable' = FALSE
    /\ handoffConsumed' = TRUE
    /\ responseSenderCarried' = TRUE
    /\ responseSendAttempted' = FALSE
    /\ phase' = "sent"
    /\ UNCHANGED <<recvFailed, inlineRan, skipped>>

ResponseArrives ==
    /\ phase = "sent"
    /\ responseSenderCarried
    /\ phase' = "done"
    /\ recvFailed' = FALSE
    /\ responseSendAttempted' = ~OmitResponseAttempt
    /\ inlineRan' = FALSE
    /\ skipped' = FALSE
    /\ UNCHANGED <<rootsAvailable, handoffConsumed, responseSenderCarried>>

ResponseFails ==
    /\ phase = "sent"
    /\ responseSenderCarried
    /\ phase' = "done"
    /\ recvFailed' = TRUE
    /\ responseSendAttempted' = ~OmitResponseAttempt
    /\ IF FallbackAfterConsumed THEN
          /\ inlineRan' = TRUE
          /\ skipped' = FALSE
       ELSE
          /\ inlineRan' = FALSE
          /\ skipped' = TRUE
    /\ UNCHANGED <<rootsAvailable, handoffConsumed, responseSenderCarried>>

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

ConsumedRequestGetsReplyAttempt ==
    phase = "done" /\ handoffConsumed => responseSenderCarried /\ responseSendAttempted

=============================================================================
