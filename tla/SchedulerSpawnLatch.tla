---------------------------- MODULE SchedulerSpawnLatch ----------------------------
(***************************************************************************)
(* Eval-worker spawn latch model.                                           *)
(*                                                                         *)
(* The single-threaded mid-loop index-GC gate includes the sticky           *)
(* worker_ever_spawned latch.  Every eval-worker handoff that can introduce *)
(* parallelism must set that latch before submitting work to the pool.       *)
(* SpawnBeforeLatch models the bug where the pool handoff can happen first. *)
(* MissingLatch models a handoff site that never sets the latch.             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS IndexMode, FanoutDisabled, SpawnBeforeLatch, MissingLatch

VARIABLES phase, latch, workerExists, activeEvaluators

vars == <<phase, latch, workerExists, activeEvaluators>>

Init ==
  /\ phase = "start"
  /\ latch = FALSE
  /\ workerExists = FALSE
  /\ activeEvaluators = 1

Latch ==
  /\ phase = "start"
  /\ ~MissingLatch
  /\ phase' = "latched"
  /\ latch' = TRUE
  /\ UNCHANGED <<workerExists, activeEvaluators>>

SpawnAfterLatch ==
  /\ phase = "latched"
  /\ phase' = "spawned"
  /\ workerExists' = TRUE
  /\ UNCHANGED <<latch, activeEvaluators>>

SpawnWithoutPriorLatch ==
  /\ phase = "start"
  /\ (SpawnBeforeLatch \/ MissingLatch)
  /\ phase' = "spawned"
  /\ workerExists' = TRUE
  /\ UNCHANGED <<latch, activeEvaluators>>

Done ==
  /\ phase = "spawned"
  /\ UNCHANGED vars

Next ==
  \/ Latch
  \/ SpawnAfterLatch
  \/ SpawnWithoutPriorLatch
  \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ IndexMode \in BOOLEAN
  /\ FanoutDisabled \in BOOLEAN
  /\ SpawnBeforeLatch \in BOOLEAN
  /\ MissingLatch \in BOOLEAN
  /\ phase \in {"start", "latched", "spawned"}
  /\ latch \in BOOLEAN
  /\ workerExists \in BOOLEAN
  /\ activeEvaluators \in Nat

MidloopGateOpen ==
  /\ IndexMode
  /\ FanoutDisabled
  /\ activeEvaluators = 1
  /\ ~latch

NoWorkerWithMidloopGateOpen ==
  ~(workerExists /\ MidloopGateOpen)

LatchPrecedesWorkerExistence ==
  workerExists => latch

=============================================================================
