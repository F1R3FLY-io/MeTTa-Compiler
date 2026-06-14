----------------------- MODULE ThreadingEndToEndInterleaving -----------------------
(***************************************************************************)
(* End-to-end interleaving envelope for scheduler reordering, WorkPool      *)
(* execution, recurring cron dispatch, and CESK-GC sweep admission.         *)
(***************************************************************************)

EXTENDS Naturals, FiniteSets

CONSTANTS
    HasDependency,
    DependencyEdgeEncoded,
    HasEffectConflict,
    ConflictEdgeEncoded,
    DirectFanout,
    IncludeWorkerRoot,
    CloseAdmission,
    ClaimCronBeforeDispatch

TASKS == {"producer", "consumer"}

VARIABLES
    phase,
    wave,
    running,
    completed,
    consumerBeforeProducer,
    conflictSameWave,
    rootsBuilt,
    workerRooted,
    lateWorkerLive,
    valueFreed,
    inFlight,
    cronWorkerRunning,
    cronOverlap,
    dispatchCount

vars ==
    <<phase, wave, running, completed, consumerBeforeProducer,
      conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
      valueFreed, inFlight, cronWorkerRunning, cronOverlap, dispatchCount>>

BooleanConstantsOK ==
    /\ HasDependency \in BOOLEAN
    /\ DependencyEdgeEncoded \in BOOLEAN
    /\ HasEffectConflict \in BOOLEAN
    /\ ConflictEdgeEncoded \in BOOLEAN
    /\ DirectFanout \in BOOLEAN
    /\ IncludeWorkerRoot \in BOOLEAN
    /\ CloseAdmission \in BOOLEAN
    /\ ClaimCronBeforeDispatch \in BOOLEAN

TypeOK ==
    /\ BooleanConstantsOK
    /\ phase \in {"init", "scheduled", "running", "done"}
    /\ wave \in [TASKS -> Nat]
    /\ running \subseteq TASKS
    /\ completed \subseteq TASKS
    /\ consumerBeforeProducer \in BOOLEAN
    /\ conflictSameWave \in BOOLEAN
    /\ rootsBuilt \in BOOLEAN
    /\ workerRooted \in BOOLEAN
    /\ lateWorkerLive \in BOOLEAN
    /\ valueFreed \in BOOLEAN
    /\ inFlight \in BOOLEAN
    /\ cronWorkerRunning \in BOOLEAN
    /\ cronOverlap \in BOOLEAN
    /\ dispatchCount \in Nat

EdgeComplete ==
    /\ (HasDependency => DependencyEdgeEncoded)
    /\ (HasEffectConflict => ConflictEdgeEncoded)

ConsumerWave ==
    IF EdgeComplete /\ (HasDependency \/ HasEffectConflict) THEN 1 ELSE 0

Init ==
    /\ phase = "init"
    /\ wave = [t \in TASKS |-> 0]
    /\ running = {}
    /\ completed = {}
    /\ consumerBeforeProducer = FALSE
    /\ conflictSameWave = FALSE
    /\ rootsBuilt = FALSE
    /\ workerRooted = FALSE
    /\ lateWorkerLive = FALSE
    /\ valueFreed = FALSE
    /\ inFlight = FALSE
    /\ cronWorkerRunning = FALSE
    /\ cronOverlap = FALSE
    /\ dispatchCount = 0

Schedule ==
    /\ phase = "init"
    /\ phase' = "scheduled"
    /\ wave' = [t \in TASKS |-> IF t = "producer" THEN 0 ELSE ConsumerWave]
    /\ conflictSameWave' = (HasEffectConflict /\ ConsumerWave = 0)
    /\ UNCHANGED <<running, completed, consumerBeforeProducer, rootsBuilt,
                  workerRooted, lateWorkerLive, valueFreed, inFlight,
                  cronWorkerRunning, cronOverlap, dispatchCount>>

AdmissionOpen ==
    ~(rootsBuilt /\ CloseAdmission)

StartProducer ==
    /\ phase \in {"scheduled", "running"}
    /\ AdmissionOpen
    /\ "producer" \notin running
    /\ "producer" \notin completed
    /\ phase' = "running"
    /\ running' = running \cup {"producer"}
    /\ UNCHANGED <<wave, completed, consumerBeforeProducer, conflictSameWave,
                  rootsBuilt, workerRooted, lateWorkerLive, valueFreed,
                  inFlight, cronWorkerRunning, cronOverlap, dispatchCount>>

ConsumerReady ==
    IF wave["consumer"] = 0 THEN TRUE ELSE "producer" \in completed

StartConsumer ==
    /\ phase \in {"scheduled", "running"}
    /\ AdmissionOpen
    /\ ConsumerReady
    /\ "consumer" \notin running
    /\ "consumer" \notin completed
    /\ phase' = "running"
    /\ running' = running \cup {"consumer"}
    /\ consumerBeforeProducer' =
        (consumerBeforeProducer \/ (HasDependency /\ ~("producer" \in completed)))
    /\ UNCHANGED <<wave, completed, conflictSameWave, rootsBuilt, workerRooted,
                  lateWorkerLive, valueFreed, inFlight, cronWorkerRunning,
                  cronOverlap, dispatchCount>>

Complete(task) ==
    /\ task \in running
    /\ running' = running \ {task}
    /\ completed' = completed \cup {task}
    /\ phase' = IF completed' = TASKS THEN "done" ELSE "running"
    /\ UNCHANGED <<wave, consumerBeforeProducer, conflictSameWave, rootsBuilt,
                  workerRooted, lateWorkerLive, valueFreed, inFlight,
                  cronWorkerRunning, cronOverlap, dispatchCount>>

BuildRoots ==
    /\ ~rootsBuilt
    /\ rootsBuilt' = TRUE
    /\ workerRooted' = (IncludeWorkerRoot /\ running /= {})
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, lateWorkerLive, valueFreed, inFlight,
                  cronWorkerRunning, cronOverlap, dispatchCount>>

AdmitLateWorker ==
    /\ rootsBuilt
    /\ ~CloseAdmission
    /\ ~lateWorkerLive
    /\ lateWorkerLive' = TRUE
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, valueFreed,
                  inFlight, cronWorkerRunning, cronOverlap, dispatchCount>>

Sweep ==
    /\ rootsBuilt
    /\ valueFreed' =
        (valueFreed \/ ((running /= {} /\ ~workerRooted) \/ lateWorkerLive))
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  inFlight, cronWorkerRunning, cronOverlap, dispatchCount>>

CronFirstDue ==
    /\ dispatchCount = 0
    /\ inFlight' = ClaimCronBeforeDispatch
    /\ cronWorkerRunning' = TRUE
    /\ dispatchCount' = 1
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, cronOverlap>>

CronSecondDue ==
    /\ dispatchCount = 1
    /\ cronWorkerRunning
    /\ IF inFlight
       THEN /\ dispatchCount' = dispatchCount
            /\ cronOverlap' = FALSE
       ELSE /\ dispatchCount' = dispatchCount + 1
            /\ cronOverlap' = TRUE
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, inFlight, cronWorkerRunning>>

CronWorkerComplete ==
    /\ cronWorkerRunning
    /\ inFlight' = FALSE
    /\ cronWorkerRunning' = FALSE
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, cronOverlap, dispatchCount>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Idle ==
    UNCHANGED vars

Next ==
    \/ Schedule
    \/ StartProducer
    \/ StartConsumer
    \/ Complete("producer")
    \/ Complete("consumer")
    \/ BuildRoots
    \/ AdmitLateWorker
    \/ Sweep
    \/ CronFirstDue
    \/ CronSecondDue
    \/ CronWorkerComplete
    \/ Done
    \/ Idle

Spec == Init /\ [][Next]_vars

NoConsumerBeforeProducer ==
    ~consumerBeforeProducer

NoSameWaveEffectConflict ==
    ~conflictSameWave

NoDirectFanoutForDependentWork ==
    DirectFanout => ~(HasDependency \/ HasEffectConflict)

MaximalIndependentParallelism ==
    phase /= "init" /\ ~HasDependency /\ ~HasEffectConflict =>
      wave["producer"] = wave["consumer"]

NoLiveValueSwept ==
    ~valueFreed

NoOverlappingCronDispatch ==
    ~cronOverlap

EndToEndSafe ==
    /\ NoConsumerBeforeProducer
    /\ NoSameWaveEffectConflict
    /\ NoDirectFanoutForDependentWork
    /\ MaximalIndependentParallelism
    /\ NoLiveValueSwept
    /\ NoOverlappingCronDispatch

=============================================================================
