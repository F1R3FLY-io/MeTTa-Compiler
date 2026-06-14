-------------------------- MODULE WorkPoolPanicIsolation --------------------------
(***************************************************************************)
(* WorkPool panic-isolation model.                                          *)
(*                                                                         *)
(* The first queued task hits a failure edge. The second queued task should *)
(* still run when the relevant catch layer is present.                      *)
(*                                                                         *)
(* FirstFailure = "task" models a panic in the task closure. InnerCatch     *)
(* should convert it to runtime zero and still publish the worker heartbeat *)
(* before the loop processes the next task.                                 *)
(*                                                                         *)
(* FirstFailure = "accounting" models a panic after task execution, such as *)
(* runtime tracking or WFST weight update. OuterCatch must keep the worker  *)
(* alive so the next queued task can run.                                   *)
(***************************************************************************)
EXTENDS Naturals, TLC

CONSTANTS FirstFailure, InnerCatch, OuterCatch

VARIABLES phase, workerAlive, firstHandled, secondCompleted,
          runtimeRecorded, cpuPublished

vars == <<phase, workerAlive, firstHandled, secondCompleted,
          runtimeRecorded, cpuPublished>>

Init ==
  /\ phase = "first"
  /\ workerAlive = TRUE
  /\ firstHandled = FALSE
  /\ secondCompleted = FALSE
  /\ runtimeRecorded = FALSE
  /\ cpuPublished = FALSE

RunFirstTaskPanic ==
  /\ phase = "first"
  /\ FirstFailure = "task"
  /\ firstHandled' = TRUE
  /\ secondCompleted' = secondCompleted
  /\ runtimeRecorded' = FALSE
  /\ IF InnerCatch
     THEN /\ workerAlive' = TRUE
          /\ cpuPublished' = TRUE
          /\ phase' = "second"
     ELSE IF OuterCatch
          THEN /\ workerAlive' = TRUE
               /\ cpuPublished' = FALSE
               /\ phase' = "second"
          ELSE /\ workerAlive' = FALSE
               /\ cpuPublished' = FALSE
               /\ phase' = "dead"

RunFirstAccountingPanic ==
  /\ phase = "first"
  /\ FirstFailure = "accounting"
  /\ firstHandled' = TRUE
  /\ secondCompleted' = secondCompleted
  /\ runtimeRecorded' = FALSE
  /\ cpuPublished' = FALSE
  /\ IF OuterCatch
     THEN /\ workerAlive' = TRUE
          /\ phase' = "second"
     ELSE /\ workerAlive' = FALSE
          /\ phase' = "dead"

RunSecond ==
  /\ phase = "second"
  /\ workerAlive
  /\ phase' = "done"
  /\ workerAlive' = workerAlive
  /\ firstHandled' = firstHandled
  /\ secondCompleted' = TRUE
  /\ runtimeRecorded' = runtimeRecorded
  /\ cpuPublished' = TRUE

Done ==
  /\ phase \in {"done", "dead"}
  /\ UNCHANGED vars

Next == RunFirstTaskPanic \/ RunFirstAccountingPanic \/ RunSecond \/ Done

Spec ==
  /\ Init
  /\ [][Next]_vars
  /\ WF_vars(RunFirstTaskPanic)
  /\ WF_vars(RunFirstAccountingPanic)
  /\ WF_vars(RunSecond)

TypeOK ==
  /\ FirstFailure \in {"task", "accounting"}
  /\ InnerCatch \in BOOLEAN
  /\ OuterCatch \in BOOLEAN
  /\ phase \in {"first", "second", "done", "dead"}
  /\ workerAlive \in BOOLEAN
  /\ firstHandled \in BOOLEAN
  /\ secondCompleted \in BOOLEAN
  /\ runtimeRecorded \in BOOLEAN
  /\ cpuPublished \in BOOLEAN

NoRuntimeRecordForTaskPanic ==
  /\ firstHandled
  /\ FirstFailure = "task"
  => ~runtimeRecorded

TaskPanicPublishesHeartbeat ==
  /\ firstHandled
  /\ FirstFailure = "task"
  => cpuPublished

EventuallySecondCompleted ==
  <>secondCompleted

=============================================================================
