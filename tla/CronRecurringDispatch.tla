-------------------------- MODULE CronRecurringDispatch --------------------------
(***************************************************************************)
(* Discriminator for worker-pool recurring cron stop semantics.             *)
(*                                                                         *)
(* PublishStopBeforeIdle = TRUE models the worker storing stop_requested    *)
(* before clearing in_flight. FALSE models the old behavior: false/panic is *)
(* logged but no terminal state is visible to the cron thread, so the next  *)
(* due placeholder dispatches again.                                        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT PublishStopBeforeIdle

VARIABLES phase, inFlight, stopRequested, dispatchedAgain

vars == <<phase, inFlight, stopRequested, dispatchedAgain>>

TypeOK ==
    /\ phase \in {"workerRunning", "due", "done"}
    /\ inFlight \in BOOLEAN
    /\ stopRequested \in BOOLEAN
    /\ dispatchedAgain \in BOOLEAN

Init ==
    /\ phase = "workerRunning"
    /\ inFlight = TRUE
    /\ stopRequested = FALSE
    /\ dispatchedAgain = FALSE

WorkerReturnsFalse ==
    /\ phase = "workerRunning"
    /\ phase' = "due"
    /\ stopRequested' = IF PublishStopBeforeIdle THEN TRUE ELSE FALSE
    /\ inFlight' = FALSE
    /\ UNCHANGED dispatchedAgain

CronDue ==
    /\ phase = "due"
    /\ phase' = "done"
    /\ dispatchedAgain' = ~stopRequested
    /\ UNCHANGED <<inFlight, stopRequested>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ WorkerReturnsFalse
    \/ CronDue
    \/ Done

Spec == Init /\ [][Next]_vars

StopPreventsRedispatch ==
    phase = "done" => ~dispatchedAgain

=============================================================================
