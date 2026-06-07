-------------------------- MODULE ConcurrentTriggerBackstop --------------------------
(***************************************************************************)
(* E1 FANOUT rendezvous-trigger backstop discriminator.                    *)
(*                                                                         *)
(* request_concurrent_collection sets GC_REQUESTED before handing the      *)
(* rendezvous request to the dedicated GC thread. If spawning or sending    *)
(* fails, resume_workers must clear the request and wake workers.           *)
(***************************************************************************)

CONSTANTS
    DriverAvailable,
    SendSucceeds,
    BackstopSpawnFailure,
    BackstopSendFailure

VARIABLES
    phase,
    gcRequested,
    driverPosted,
    workersResumed

vars == <<phase, gcRequested, driverPosted, workersResumed>>

TypeOK ==
    /\ DriverAvailable \in BOOLEAN
    /\ SendSucceeds \in BOOLEAN
    /\ BackstopSpawnFailure \in BOOLEAN
    /\ BackstopSendFailure \in BOOLEAN
    /\ phase \in {"start", "requested", "done"}
    /\ gcRequested \in BOOLEAN
    /\ driverPosted \in BOOLEAN
    /\ workersResumed \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ gcRequested = FALSE
    /\ driverPosted = FALSE
    /\ workersResumed = FALSE

Request ==
    /\ phase = "start"
    /\ phase' = "requested"
    /\ gcRequested' = TRUE
    /\ UNCHANGED <<driverPosted, workersResumed>>

Handoff ==
    /\ phase = "requested"
    /\ IF DriverAvailable /\ SendSucceeds THEN
          /\ driverPosted' = TRUE
          /\ gcRequested' = TRUE
          /\ workersResumed' = FALSE
       ELSE IF ~DriverAvailable THEN
          /\ driverPosted' = FALSE
          /\ gcRequested' = ~BackstopSpawnFailure
          /\ workersResumed' = BackstopSpawnFailure
       ELSE
          /\ driverPosted' = FALSE
          /\ gcRequested' = ~BackstopSendFailure
          /\ workersResumed' = BackstopSendFailure
    /\ phase' = "done"

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ Request
    \/ Handoff
    \/ Done

Spec == Init /\ [][Next]_vars

NoDriverlessPendingRequest ==
    phase = "done" /\ gcRequested => driverPosted

FailedTriggerClearsRequest ==
    phase = "done" /\ ~driverPosted => ~gcRequested

FailedTriggerResumesWorkers ==
    phase = "done" /\ ~driverPosted => workersResumed

=============================================================================
