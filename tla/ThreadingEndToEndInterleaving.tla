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
    ClaimCronBeforeDispatch,
    ReturnCronHandleSender,
    ReturnCronReadyReceiver,
    ReadySentInsideCronRun,
    ScheduleCronTaskAfterReady,
    CronCheckEventsPolls,
    CronDrainChannelPolls

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
    dispatchCount,
    startupPhase,
    cronReadyObserved,
    cronStartupTaskSubmitted,
    cronStartupTaskObserved,
    cronStartupTaskLost

baseVars ==
    <<phase, wave, running, completed, consumerBeforeProducer,
      conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
      valueFreed, inFlight, cronWorkerRunning, cronOverlap, dispatchCount>>

startupVars ==
    <<startupPhase, cronReadyObserved, cronStartupTaskSubmitted,
      cronStartupTaskObserved, cronStartupTaskLost>>

vars == <<baseVars, startupVars>>

BooleanConstantsOK ==
    /\ HasDependency \in BOOLEAN
    /\ DependencyEdgeEncoded \in BOOLEAN
    /\ HasEffectConflict \in BOOLEAN
    /\ ConflictEdgeEncoded \in BOOLEAN
    /\ DirectFanout \in BOOLEAN
    /\ IncludeWorkerRoot \in BOOLEAN
    /\ CloseAdmission \in BOOLEAN
    /\ ClaimCronBeforeDispatch \in BOOLEAN
    /\ ReturnCronHandleSender \in BOOLEAN
    /\ ReturnCronReadyReceiver \in BOOLEAN
    /\ ReadySentInsideCronRun \in BOOLEAN
    /\ ScheduleCronTaskAfterReady \in BOOLEAN
    /\ CronCheckEventsPolls \in BOOLEAN
    /\ CronDrainChannelPolls \in BOOLEAN

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
    /\ startupPhase \in {"spawned", "ready", "submitted", "observed", "lost"}
    /\ cronReadyObserved \in BOOLEAN
    /\ cronStartupTaskSubmitted \in BOOLEAN
    /\ cronStartupTaskObserved \in BOOLEAN
    /\ cronStartupTaskLost \in BOOLEAN

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
    /\ startupPhase = "spawned"
    /\ cronReadyObserved = FALSE
    /\ cronStartupTaskSubmitted = FALSE
    /\ cronStartupTaskObserved = FALSE
    /\ cronStartupTaskLost = FALSE

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

CronStartupRun ==
    /\ startupPhase = "spawned"
    /\ startupPhase' = "ready"
    /\ cronReadyObserved' = (ReturnCronReadyReceiver /\ ReadySentInsideCronRun)
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronStartupTaskSubmitted, cronStartupTaskObserved,
                  cronStartupTaskLost>>

CronStartupSubmitAfterReady ==
    /\ startupPhase = "ready"
    /\ startupPhase' = "submitted"
    /\ cronStartupTaskSubmitted' =
        (ScheduleCronTaskAfterReady /\ cronReadyObserved /\ ReturnCronHandleSender)
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskObserved,
                  cronStartupTaskLost>>

CronStartupPollCheckEvents ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ CronCheckEventsPolls
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupPollDrainChannel ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ CronDrainChannelPolls
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupNoSubmittedTask ==
    /\ startupPhase = "submitted"
    /\ ~cronStartupTaskSubmitted
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = FALSE
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupLoseWithoutPollPath ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ ~(CronCheckEventsPolls \/ CronDrainChannelPolls)
    /\ startupPhase' = "lost"
    /\ cronStartupTaskLost' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskObserved>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Idle ==
    UNCHANGED vars

ThreadingNext ==
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

Next ==
    \/ (ThreadingNext /\ UNCHANGED startupVars)
    \/ CronStartupRun
    \/ CronStartupSubmitAfterReady
    \/ CronStartupPollCheckEvents
    \/ CronStartupPollDrainChannel
    \/ CronStartupNoSubmittedTask
    \/ CronStartupLoseWithoutPollPath
    \/ Idle

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(CronStartupRun)
    /\ WF_vars(CronStartupSubmitAfterReady)
    /\ WF_vars(CronStartupPollCheckEvents)
    /\ WF_vars(CronStartupPollDrainChannel)
    /\ WF_vars(CronStartupNoSubmittedTask)
    /\ WF_vars(CronStartupLoseWithoutPollPath)

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

CronStartupReadyWaitCompletes ==
    (startupPhase /= "spawned" /\ ScheduleCronTaskAfterReady) =>
      cronReadyObserved

CronStartupScheduleAfterReadyHasHandle ==
    ScheduleCronTaskAfterReady => ReturnCronHandleSender

CronStartupSubmittedReachable ==
    cronStartupTaskSubmitted => ReturnCronHandleSender /\ cronReadyObserved

NoCronStartupTaskLost ==
    ~cronStartupTaskLost

CronStartupSubmittedEventuallyObserved ==
    [](cronStartupTaskSubmitted => <>cronStartupTaskObserved)

EndToEndSafe ==
    /\ NoConsumerBeforeProducer
    /\ NoSameWaveEffectConflict
    /\ NoDirectFanoutForDependentWork
    /\ MaximalIndependentParallelism
    /\ NoLiveValueSwept
    /\ NoOverlappingCronDispatch
    /\ CronStartupReadyWaitCompletes
    /\ CronStartupScheduleAfterReadyHasHandle
    /\ CronStartupSubmittedReachable
    /\ NoCronStartupTaskLost

=============================================================================
