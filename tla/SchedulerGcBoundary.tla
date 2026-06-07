---------------------------- MODULE SchedulerGcBoundary ----------------------------
(***************************************************************************)
(* GC-facing scheduler/thread-pool boundary for the CESK collector.        *)
(*                                                                         *)
(* This model does not specify scheduler fairness. It checks the part that *)
(* matters to GC safety: once collection admission is closed and the driver *)
(* builds roots, every scheduler-held live address must be present in a     *)
(* driver root channel before sweep. Active workers contribute through the  *)
(* worker root buffer, fan-out/collapse state through live-dispatch anchors,*)
(* and async batch handoff values through safepoint/driver-C roots. A newly *)
(* admitted worker during that window is a bug.                             *)
(***************************************************************************)

CONSTANTS
    IncludeWorker,
    IncludeDispatch,
    IncludeBatch,
    CloseAdmission

VARIABLES
    phase,
    activeWorkerLive,
    dispatchFanoutLive,
    batchHandoffLive,
    newlyAdmittedLive,
    workerRooted,
    dispatchRooted,
    batchRooted,
    swept

vars ==
    <<phase, activeWorkerLive, dispatchFanoutLive, batchHandoffLive,
      newlyAdmittedLive, workerRooted, dispatchRooted, batchRooted, swept>>

TypeOK ==
    /\ phase \in {"start", "rooted", "admitted", "swept"}
    /\ activeWorkerLive \in BOOLEAN
    /\ dispatchFanoutLive \in BOOLEAN
    /\ batchHandoffLive \in BOOLEAN
    /\ newlyAdmittedLive \in BOOLEAN
    /\ workerRooted \in BOOLEAN
    /\ dispatchRooted \in BOOLEAN
    /\ batchRooted \in BOOLEAN
    /\ swept \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ activeWorkerLive = TRUE
    /\ dispatchFanoutLive = TRUE
    /\ batchHandoffLive = TRUE
    /\ newlyAdmittedLive = FALSE
    /\ workerRooted = FALSE
    /\ dispatchRooted = FALSE
    /\ batchRooted = FALSE
    /\ swept = FALSE

BuildRoots ==
    /\ phase = "start"
    /\ phase' = "rooted"
    /\ workerRooted' = IncludeWorker /\ activeWorkerLive
    /\ dispatchRooted' = IncludeDispatch /\ dispatchFanoutLive
    /\ batchRooted' = IncludeBatch /\ batchHandoffLive
    /\ UNCHANGED <<activeWorkerLive, dispatchFanoutLive, batchHandoffLive,
                  newlyAdmittedLive, swept>>

AdmitDuringCollection ==
    /\ phase = "rooted"
    /\ ~CloseAdmission
    /\ phase' = "admitted"
    /\ newlyAdmittedLive' = TRUE
    /\ UNCHANGED <<activeWorkerLive, dispatchFanoutLive, batchHandoffLive,
                  workerRooted, dispatchRooted, batchRooted, swept>>

Sweep ==
    /\ phase \in {"rooted", "admitted"}
    /\ phase' = "swept"
    /\ swept' = TRUE
    /\ UNCHANGED <<activeWorkerLive, dispatchFanoutLive, batchHandoffLive,
                  newlyAdmittedLive, workerRooted, dispatchRooted, batchRooted>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ BuildRoots
    \/ AdmitDuringCollection
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

SchedulerBoundaryComplete ==
    swept =>
      /\ activeWorkerLive => workerRooted
      /\ dispatchFanoutLive => dispatchRooted
      /\ batchHandoffLive => batchRooted
      /\ ~newlyAdmittedLive

=============================================================================
