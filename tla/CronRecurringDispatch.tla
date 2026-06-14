-------------------------- MODULE CronRecurringDispatch --------------------------
(***************************************************************************)
(* Discriminator for worker-pool recurring cron stop semantics.             *)
(*                                                                         *)
(* PublishStopBeforeIdle = TRUE models the worker storing stop_requested    *)
(* before clearing in_flight. FALSE models the old behavior: false/panic is *)
(* logged but no terminal state is visible to the cron thread, so the next  *)
(* due placeholder dispatches again.                                        *)
(* ClaimBeforeDispatch = TRUE models the cron thread setting inFlight before*)
(* submitting pooled recurring work. FALSE models a missing claim, allowing *)
(* a second due placeholder to dispatch overlapping work.                   *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT PublishStopBeforeIdle, ClaimBeforeDispatch

VARIABLES phase, inFlight, stopRequested, dispatchedAgain, dispatchCount, overlapped

vars == <<phase, inFlight, stopRequested, dispatchedAgain, dispatchCount, overlapped>>

TypeOK ==
    /\ phase \in {"firstDue", "workerRunning", "workerReturns", "finalDue", "done"}
    /\ inFlight \in BOOLEAN
    /\ stopRequested \in BOOLEAN
    /\ dispatchedAgain \in BOOLEAN
    /\ ClaimBeforeDispatch \in BOOLEAN
    /\ dispatchCount \in Nat
    /\ overlapped \in BOOLEAN

Init ==
    /\ phase = "firstDue"
    /\ inFlight = FALSE
    /\ stopRequested = FALSE
    /\ dispatchedAgain = FALSE
    /\ dispatchCount = 0
    /\ overlapped = FALSE

FirstCronDue ==
    /\ phase = "firstDue"
    /\ phase' = "workerRunning"
    /\ inFlight' = ClaimBeforeDispatch
    /\ dispatchCount' = 1
    /\ UNCHANGED <<stopRequested, dispatchedAgain, overlapped>>

SecondCronDueBeforeWorkerCompletes ==
    /\ phase = "workerRunning"
    /\ phase' = "workerReturns"
    /\ IF inFlight
       THEN /\ dispatchCount' = dispatchCount
            /\ overlapped' = FALSE
       ELSE /\ dispatchCount' = dispatchCount + 1
            /\ overlapped' = TRUE
    /\ UNCHANGED <<inFlight, stopRequested, dispatchedAgain>>

WorkerReturnsFalse ==
    /\ phase = "workerReturns"
    /\ phase' = "finalDue"
    /\ stopRequested' = IF PublishStopBeforeIdle THEN TRUE ELSE FALSE
    /\ inFlight' = FALSE
    /\ UNCHANGED <<dispatchedAgain, dispatchCount, overlapped>>

FinalCronDue ==
    /\ phase = "finalDue"
    /\ phase' = "done"
    /\ dispatchedAgain' = ~stopRequested
    /\ UNCHANGED <<inFlight, stopRequested, dispatchCount, overlapped>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ FirstCronDue
    \/ SecondCronDueBeforeWorkerCompletes
    \/ WorkerReturnsFalse
    \/ FinalCronDue
    \/ Done

Spec == Init /\ [][Next]_vars

NoOverlapDispatch ==
    ~overlapped

StopPreventsRedispatch ==
    phase = "done" => ~dispatchedAgain

=============================================================================
