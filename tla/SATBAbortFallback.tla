----------------------------- MODULE SATBAbortFallback -----------------------------
(***************************************************************************)
(* E2 SATB abort-to-STW discriminator.                                     *)
(*                                                                        *)
(* If the concurrent SATB rendezvous aborts after it has cleaned up its    *)
(* open rendezvous/request, the driver must run a fresh stop-the-world     *)
(* rendezvous before considering the GC request handled. Without that      *)
(* fallback, an aborted concurrent mark can silently skip the requested    *)
(* collection.                                                            *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    UseFallback,
    UseRequest

VARIABLES
    phase,
    aborted,
    stwRequested,
    stwRan

vars == <<phase, aborted, stwRequested, stwRan>>

TypeOK ==
    /\ phase \in {"satb", "aborted", "requested", "stw", "done"}
    /\ aborted \in BOOLEAN
    /\ stwRequested \in BOOLEAN
    /\ stwRan \in BOOLEAN

Init ==
    /\ phase = "satb"
    /\ aborted = FALSE
    /\ stwRequested = FALSE
    /\ stwRan = FALSE

SatbAbort ==
    /\ phase = "satb"
    /\ phase' = "aborted"
    /\ aborted' = TRUE
    /\ UNCHANGED <<stwRequested, stwRan>>

RequestFallback ==
    /\ phase = "aborted"
    /\ UseFallback
    /\ UseRequest
    /\ phase' = "requested"
    /\ stwRequested' = TRUE
    /\ UNCHANGED <<aborted, stwRan>>

FallbackSTW ==
    /\ UseFallback
    /\ IF UseRequest THEN phase = "requested" ELSE phase = "aborted"
    /\ phase' = "stw"
    /\ stwRan' = TRUE
    /\ UNCHANGED <<aborted, stwRequested>>

Done ==
    /\ phase \in {"aborted", "stw"}
    /\ IF UseFallback /\ aborted THEN stwRan ELSE TRUE
    /\ phase' = "done"
    /\ UNCHANGED <<aborted, stwRequested, stwRan>>

StutterDone ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ SatbAbort
    \/ RequestFallback
    \/ FallbackSTW
    \/ Done
    \/ StutterDone

Spec == Init /\ [][Next]_vars

AbortHasBackstop ==
    (phase = "done" /\ aborted) => stwRan

FallbackSTWRequested ==
    stwRan => stwRequested

=============================================================================
