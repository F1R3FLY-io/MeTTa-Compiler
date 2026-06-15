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
    IncludeDispatchRoot,
    IncludeBatchRoot,
    CloseAdmission,
    ClaimCronBeforeDispatch,
    PublishCronStopBeforeIdle,
    ReturnCronHandleSender,
    ReturnCronReadyReceiver,
    ReadySentInsideCronRun,
    ScheduleCronTaskAfterReady,
    CronCheckEventsPolls,
    CronDrainChannelPolls,
    WorkPoolStartupSubmissions,
    WorkPoolStartWorkers,
    WorkPoolLossyEnqueue,
    WorkPoolFirstFailure,
    WorkPoolInnerCatch,
    WorkPoolOuterCatch,
    WorkPoolRecomputeAtPop,
    WorkPoolOverflowRequested,
    WorkPoolInitialOverflowLive,
    WorkPoolMaxOverflow,
    WorkPoolEnforceOverflowCap,
    WorkPoolLifecycleMaxWorkers,
    WorkPoolLifecycleInitialActive,
    WorkPoolLifecycleInitialParked,
    WorkPoolUseTransitionResult,
    WorkPoolRespawnCountsParked,
    ActiveBranchCount,
    ActiveMinBranches,
    ActiveDegree,
    ActiveCostClass,
    ActiveMaxParallel,
    ActiveBudgetGranted,
    ActivePure,
    ActiveDynamicEvalGate,
    ActiveDynamicEval,
    ActiveStateMutation,
    ActiveStrictPrint,
    ActiveIo,
    ActiveDepthOk,
    ActivePoolOk,
    ActivePartialDispatch

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
    dispatchFanoutLive,
    batchHandoffLive,
    dispatchRooted,
    batchRooted,
    lateWorkerLive,
    valueFreed,
    inFlight,
    cronWorkerRunning,
    cronOverlap,
    cronStopRequested,
    cronDispatchedAgain,
    dispatchCount,
    startupPhase,
    cronReadyObserved,
    cronStartupTaskSubmitted,
    cronStartupTaskObserved,
    cronStartupTaskLost,
    workPoolSubmitted,
    workPoolQueue,
    workPoolCompleted,
    workPoolWorkersStarted,
    workPoolStartupPhase,
    workPoolPanicPhase,
    workPoolWorkerAlive,
    workPoolFirstHandled,
    workPoolSecondCompleted,
    workPoolRuntimeRecorded,
    workPoolCpuPublished,
    workPoolOldAge,
    workPoolHighEnqueued,
    workPoolPopped,
    workPoolOverflowLive,
    workPoolOverflowPhase,
    workPoolLifecycleActive,
    workPoolLifecycleParked,
    workPoolLifecyclePhase,
    workPoolLifecycleScenario

baseVars ==
    <<phase, wave, running, completed, consumerBeforeProducer,
      conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
      valueFreed, inFlight, cronWorkerRunning, cronOverlap,
      cronStopRequested, cronDispatchedAgain, dispatchCount>>

startupVars ==
    <<startupPhase, cronReadyObserved, cronStartupTaskSubmitted,
      cronStartupTaskObserved, cronStartupTaskLost>>

gcBoundaryVars ==
    <<dispatchFanoutLive, batchHandoffLive, dispatchRooted, batchRooted>>

workPoolStartupVars ==
    <<workPoolSubmitted, workPoolQueue, workPoolCompleted,
      workPoolWorkersStarted, workPoolStartupPhase>>

workPoolPanicVars ==
    <<workPoolPanicPhase, workPoolWorkerAlive, workPoolFirstHandled,
      workPoolSecondCompleted, workPoolRuntimeRecorded, workPoolCpuPublished>>

workPoolPriorityVars ==
    <<workPoolOldAge, workPoolHighEnqueued, workPoolPopped>>

workPoolOverflowVars ==
    <<workPoolOverflowLive, workPoolOverflowPhase>>

workPoolLifecycleVars ==
    <<workPoolLifecycleActive, workPoolLifecycleParked,
      workPoolLifecyclePhase, workPoolLifecycleScenario>>

workPoolVars ==
    <<workPoolStartupVars, workPoolPanicVars, workPoolPriorityVars, workPoolOverflowVars,
      workPoolLifecycleVars>>

vars == <<baseVars, gcBoundaryVars, startupVars, workPoolVars>>

BooleanConstantsOK ==
    /\ HasDependency \in BOOLEAN
    /\ DependencyEdgeEncoded \in BOOLEAN
    /\ HasEffectConflict \in BOOLEAN
    /\ ConflictEdgeEncoded \in BOOLEAN
    /\ DirectFanout \in BOOLEAN
    /\ IncludeWorkerRoot \in BOOLEAN
    /\ IncludeDispatchRoot \in BOOLEAN
    /\ IncludeBatchRoot \in BOOLEAN
    /\ CloseAdmission \in BOOLEAN
    /\ ClaimCronBeforeDispatch \in BOOLEAN
    /\ PublishCronStopBeforeIdle \in BOOLEAN
    /\ ReturnCronHandleSender \in BOOLEAN
    /\ ReturnCronReadyReceiver \in BOOLEAN
    /\ ReadySentInsideCronRun \in BOOLEAN
    /\ ScheduleCronTaskAfterReady \in BOOLEAN
    /\ CronCheckEventsPolls \in BOOLEAN
    /\ CronDrainChannelPolls \in BOOLEAN
    /\ WorkPoolStartWorkers \in BOOLEAN
    /\ WorkPoolLossyEnqueue \in BOOLEAN
    /\ WorkPoolInnerCatch \in BOOLEAN
    /\ WorkPoolOuterCatch \in BOOLEAN
    /\ WorkPoolRecomputeAtPop \in BOOLEAN
    /\ WorkPoolEnforceOverflowCap \in BOOLEAN
    /\ WorkPoolUseTransitionResult \in BOOLEAN
    /\ WorkPoolRespawnCountsParked \in BOOLEAN
    /\ ActivePure \in BOOLEAN
    /\ ActiveDynamicEvalGate \in BOOLEAN
    /\ ActiveDynamicEval \in BOOLEAN
    /\ ActiveStateMutation \in BOOLEAN
    /\ ActiveStrictPrint \in BOOLEAN
    /\ ActiveIo \in BOOLEAN
    /\ ActiveDepthOk \in BOOLEAN
    /\ ActivePoolOk \in BOOLEAN
    /\ ActivePartialDispatch \in BOOLEAN

Classes ==
  {"GroundCheap", "GroundArith", "SymbolicCheap", "SymbolicModerate",
   "RecursiveBounded", "RecursiveUnbounded", "ParallelPure",
   "ImpureSequential"}

BranchParallel(c) ==
  c \in {"SymbolicModerate", "ParallelPure"}

DefaultDegree(c) ==
  CASE c = "SymbolicModerate" -> 4
    [] c = "ParallelPure" -> 8
    [] OTHER -> 1

TypeOK ==
    /\ BooleanConstantsOK
    /\ WorkPoolStartupSubmissions \in Nat
    /\ WorkPoolFirstFailure \in {"task", "accounting"}
    /\ WorkPoolOverflowRequested \in Nat
    /\ WorkPoolInitialOverflowLive \in Nat
    /\ WorkPoolMaxOverflow \in Nat
    /\ WorkPoolLifecycleMaxWorkers \in Nat
    /\ WorkPoolLifecycleInitialActive \in Nat
    /\ WorkPoolLifecycleInitialParked \in Nat
    /\ ActiveBranchCount \in Nat
    /\ ActiveMinBranches \in Nat
    /\ ActiveDegree \in Nat
    /\ ActiveCostClass \in Classes
    /\ ActiveMaxParallel \in Nat
    /\ ActiveBudgetGranted \in Nat
    /\ phase \in {"init", "scheduled", "running", "done"}
    /\ wave \in [TASKS -> Nat]
    /\ running \subseteq TASKS
    /\ completed \subseteq TASKS
    /\ consumerBeforeProducer \in BOOLEAN
    /\ conflictSameWave \in BOOLEAN
    /\ rootsBuilt \in BOOLEAN
    /\ workerRooted \in BOOLEAN
    /\ dispatchFanoutLive \in BOOLEAN
    /\ batchHandoffLive \in BOOLEAN
    /\ dispatchRooted \in BOOLEAN
    /\ batchRooted \in BOOLEAN
    /\ lateWorkerLive \in BOOLEAN
    /\ valueFreed \in BOOLEAN
    /\ inFlight \in BOOLEAN
    /\ cronWorkerRunning \in BOOLEAN
    /\ cronOverlap \in BOOLEAN
    /\ cronStopRequested \in BOOLEAN
    /\ cronDispatchedAgain \in BOOLEAN
    /\ dispatchCount \in Nat
    /\ startupPhase \in {"spawned", "ready", "submitted", "observed", "lost"}
    /\ cronReadyObserved \in BOOLEAN
    /\ cronStartupTaskSubmitted \in BOOLEAN
    /\ cronStartupTaskObserved \in BOOLEAN
    /\ cronStartupTaskLost \in BOOLEAN
    /\ workPoolSubmitted \in 0..WorkPoolStartupSubmissions
    /\ workPoolQueue \in 0..WorkPoolStartupSubmissions
    /\ workPoolCompleted \in 0..WorkPoolStartupSubmissions
    /\ workPoolWorkersStarted \in BOOLEAN
    /\ workPoolStartupPhase \in {"submit", "drain", "done", "stuck"}
    /\ workPoolPanicPhase \in {"first", "second", "done", "dead"}
    /\ workPoolWorkerAlive \in BOOLEAN
    /\ workPoolFirstHandled \in BOOLEAN
    /\ workPoolSecondCompleted \in BOOLEAN
    /\ workPoolRuntimeRecorded \in BOOLEAN
    /\ workPoolCpuPublished \in BOOLEAN
    /\ workPoolOldAge \in 0..1
    /\ workPoolHighEnqueued \in BOOLEAN
    /\ workPoolPopped \in {"none", "old", "high"}
    /\ workPoolOverflowLive \in Nat
    /\ workPoolOverflowPhase \in {"ready", "done"}
    /\ workPoolLifecycleActive \in Nat
    /\ workPoolLifecycleParked \in Nat
    /\ workPoolLifecyclePhase \in {"ready", "done"}
    /\ workPoolLifecycleScenario \in {"none", "DoubleUnpark", "RespawnParked"}

EdgeComplete ==
    /\ (HasDependency => DependencyEdgeEncoded)
    /\ (HasEffectConflict => ConflictEdgeEncoded)

ConsumerWave ==
    IF EdgeComplete /\ (HasDependency \/ HasEffectConflict) THEN 1 ELSE 0

Min(a, b) == IF a <= b THEN a ELSE b

OverflowCapacity(live, max) == IF max >= live THEN max - live ELSE 0

OverflowSpawnQuota(req, live, max) ==
    Min(req, OverflowCapacity(live, max))

OverflowSpawned(live) ==
    IF WorkPoolEnforceOverflowCap
    THEN OverflowSpawnQuota(WorkPoolOverflowRequested, live, WorkPoolMaxOverflow)
    ELSE WorkPoolOverflowRequested

Init ==
    /\ phase = "init"
    /\ wave = [t \in TASKS |-> 0]
    /\ running = {}
    /\ completed = {}
    /\ consumerBeforeProducer = FALSE
    /\ conflictSameWave = FALSE
    /\ rootsBuilt = FALSE
    /\ workerRooted = FALSE
    /\ dispatchFanoutLive = TRUE
    /\ batchHandoffLive = TRUE
    /\ dispatchRooted = FALSE
    /\ batchRooted = FALSE
    /\ lateWorkerLive = FALSE
    /\ valueFreed = FALSE
    /\ inFlight = FALSE
    /\ cronWorkerRunning = FALSE
    /\ cronOverlap = FALSE
    /\ cronStopRequested = FALSE
    /\ cronDispatchedAgain = FALSE
    /\ dispatchCount = 0
    /\ startupPhase = "spawned"
    /\ cronReadyObserved = FALSE
    /\ cronStartupTaskSubmitted = FALSE
    /\ cronStartupTaskObserved = FALSE
    /\ cronStartupTaskLost = FALSE
    /\ workPoolSubmitted = 0
    /\ workPoolQueue = 0
    /\ workPoolCompleted = 0
    /\ workPoolWorkersStarted = FALSE
    /\ workPoolStartupPhase = "submit"
    /\ workPoolPanicPhase = "first"
    /\ workPoolWorkerAlive = TRUE
    /\ workPoolFirstHandled = FALSE
    /\ workPoolSecondCompleted = FALSE
    /\ workPoolRuntimeRecorded = FALSE
    /\ workPoolCpuPublished = FALSE
    /\ workPoolOldAge = 0
    /\ workPoolHighEnqueued = FALSE
    /\ workPoolPopped = "none"
    /\ workPoolOverflowLive = WorkPoolInitialOverflowLive
    /\ workPoolOverflowPhase = "ready"
    /\ workPoolLifecycleActive = WorkPoolLifecycleInitialActive
    /\ workPoolLifecycleParked = WorkPoolLifecycleInitialParked
    /\ workPoolLifecyclePhase = "ready"
    /\ workPoolLifecycleScenario = "none"

Schedule ==
    /\ phase = "init"
    /\ phase' = "scheduled"
    /\ wave' = [t \in TASKS |-> IF t = "producer" THEN 0 ELSE ConsumerWave]
    /\ conflictSameWave' = (HasEffectConflict /\ ConsumerWave = 0)
    /\ UNCHANGED <<running, completed, consumerBeforeProducer, rootsBuilt,
                  workerRooted, lateWorkerLive, valueFreed, inFlight,
                  cronWorkerRunning, cronOverlap, cronStopRequested,
                  cronDispatchedAgain, dispatchCount>>

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
                  inFlight, cronWorkerRunning, cronOverlap,
                  cronStopRequested, cronDispatchedAgain, dispatchCount>>

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
                  cronOverlap, cronStopRequested, cronDispatchedAgain,
                  dispatchCount>>

Complete(task) ==
    /\ task \in running
    /\ running' = running \ {task}
    /\ completed' = completed \cup {task}
    /\ phase' = IF completed' = TASKS THEN "done" ELSE "running"
    /\ UNCHANGED <<wave, consumerBeforeProducer, conflictSameWave, rootsBuilt,
                  workerRooted, lateWorkerLive, valueFreed, inFlight,
                  cronWorkerRunning, cronOverlap, cronStopRequested,
                  cronDispatchedAgain, dispatchCount>>

BuildRoots ==
    /\ ~rootsBuilt
    /\ rootsBuilt' = TRUE
    /\ workerRooted' = (IncludeWorkerRoot /\ running /= {})
    /\ dispatchRooted' = IncludeDispatchRoot /\ dispatchFanoutLive
    /\ batchRooted' = IncludeBatchRoot /\ batchHandoffLive
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, dispatchFanoutLive, batchHandoffLive,
                  lateWorkerLive, valueFreed, inFlight, cronWorkerRunning,
                  cronOverlap, cronStopRequested, cronDispatchedAgain,
                  dispatchCount>>

AdmitLateWorker ==
    /\ rootsBuilt
    /\ ~CloseAdmission
    /\ ~lateWorkerLive
    /\ lateWorkerLive' = TRUE
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted,
                  dispatchFanoutLive, batchHandoffLive, dispatchRooted,
                  batchRooted, valueFreed, inFlight, cronWorkerRunning,
                  cronOverlap, cronStopRequested, cronDispatchedAgain,
                  dispatchCount>>

Sweep ==
    /\ rootsBuilt
    /\ valueFreed' =
        (valueFreed \/
         ((running /= {} /\ ~workerRooted) \/
          (dispatchFanoutLive /\ ~dispatchRooted) \/
          (batchHandoffLive /\ ~batchRooted) \/
          lateWorkerLive))
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  dispatchFanoutLive, batchHandoffLive, dispatchRooted,
                  batchRooted, inFlight, cronWorkerRunning, cronOverlap,
                  cronStopRequested, cronDispatchedAgain, dispatchCount>>

CronFirstDue ==
    /\ dispatchCount = 0
    /\ inFlight' = ClaimCronBeforeDispatch
    /\ cronWorkerRunning' = TRUE
    /\ dispatchCount' = 1
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, cronOverlap, cronStopRequested,
                  cronDispatchedAgain>>

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
                  valueFreed, inFlight, cronWorkerRunning, cronStopRequested,
                  cronDispatchedAgain>>

CronWorkerComplete ==
    /\ cronWorkerRunning
    /\ inFlight' = FALSE
    /\ cronWorkerRunning' = FALSE
    /\ cronStopRequested' = PublishCronStopBeforeIdle
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, cronOverlap, cronDispatchedAgain, dispatchCount>>

CronFinalDue ==
    /\ dispatchCount >= 1
    /\ ~cronWorkerRunning
    /\ ~cronDispatchedAgain
    /\ cronDispatchedAgain' = ~cronStopRequested
    /\ UNCHANGED <<phase, wave, running, completed, consumerBeforeProducer,
                  conflictSameWave, rootsBuilt, workerRooted, lateWorkerLive,
                  valueFreed, inFlight, cronWorkerRunning, cronOverlap,
                  cronStopRequested, dispatchCount>>

CronStartupRun ==
    /\ startupPhase = "spawned"
    /\ startupPhase' = "ready"
    /\ cronReadyObserved' = (ReturnCronReadyReceiver /\ ReadySentInsideCronRun)
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronStartupTaskSubmitted, cronStartupTaskObserved,
                  cronStartupTaskLost>>

CronStartupSubmitAfterReady ==
    /\ startupPhase = "ready"
    /\ startupPhase' = "submitted"
    /\ cronStartupTaskSubmitted' =
        (ScheduleCronTaskAfterReady /\ cronReadyObserved /\ ReturnCronHandleSender)
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskObserved,
                  cronStartupTaskLost>>

CronStartupPollCheckEvents ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ CronCheckEventsPolls
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupPollDrainChannel ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ CronDrainChannelPolls
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupNoSubmittedTask ==
    /\ startupPhase = "submitted"
    /\ ~cronStartupTaskSubmitted
    /\ startupPhase' = "observed"
    /\ cronStartupTaskObserved' = FALSE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskLost>>

CronStartupLoseWithoutPollPath ==
    /\ startupPhase = "submitted"
    /\ cronStartupTaskSubmitted
    /\ ~(CronCheckEventsPolls \/ CronDrainChannelPolls)
    /\ startupPhase' = "lost"
    /\ cronStartupTaskLost' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED workPoolVars
    /\ UNCHANGED <<cronReadyObserved, cronStartupTaskSubmitted,
                  cronStartupTaskObserved>>

WorkPoolSubmit ==
    /\ workPoolStartupPhase = "submit"
    /\ workPoolSubmitted < WorkPoolStartupSubmissions
    /\ workPoolSubmitted' = workPoolSubmitted + 1
    /\ workPoolQueue' =
        IF WorkPoolLossyEnqueue /\
           workPoolSubmitted + 1 = WorkPoolStartupSubmissions
        THEN workPoolQueue
        ELSE workPoolQueue + 1
    /\ workPoolCompleted' = workPoolCompleted
    /\ workPoolWorkersStarted' = workPoolWorkersStarted
    /\ workPoolStartupPhase' = workPoolStartupPhase
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolStart ==
    /\ workPoolStartupPhase = "submit"
    /\ workPoolSubmitted = WorkPoolStartupSubmissions
    /\ workPoolSubmitted' = workPoolSubmitted
    /\ workPoolQueue' = workPoolQueue
    /\ workPoolCompleted' = workPoolCompleted
    /\ workPoolWorkersStarted' = WorkPoolStartWorkers
    /\ workPoolStartupPhase' = "drain"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolDrain ==
    /\ workPoolStartupPhase = "drain"
    /\ workPoolWorkersStarted
    /\ workPoolQueue > 0
    /\ workPoolSubmitted' = workPoolSubmitted
    /\ workPoolQueue' = workPoolQueue - 1
    /\ workPoolCompleted' = workPoolCompleted + 1
    /\ workPoolWorkersStarted' = workPoolWorkersStarted
    /\ workPoolStartupPhase' = workPoolStartupPhase
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolFinish ==
    /\ workPoolStartupPhase = "drain"
    /\ workPoolWorkersStarted
    /\ workPoolQueue = 0
    /\ workPoolSubmitted' = workPoolSubmitted
    /\ workPoolQueue' = workPoolQueue
    /\ workPoolCompleted' = workPoolCompleted
    /\ workPoolWorkersStarted' = workPoolWorkersStarted
    /\ workPoolStartupPhase' = "done"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolStuck ==
    /\ workPoolStartupPhase = "drain"
    /\ ~workPoolWorkersStarted
    /\ workPoolQueue > 0
    /\ workPoolSubmitted' = workPoolSubmitted
    /\ workPoolQueue' = workPoolQueue
    /\ workPoolCompleted' = workPoolCompleted
    /\ workPoolWorkersStarted' = workPoolWorkersStarted
    /\ workPoolStartupPhase' = "stuck"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolRunFirstTaskPanic ==
    /\ workPoolPanicPhase = "first"
    /\ WorkPoolFirstFailure = "task"
    /\ workPoolFirstHandled' = TRUE
    /\ workPoolSecondCompleted' = workPoolSecondCompleted
    /\ workPoolRuntimeRecorded' = FALSE
    /\ IF WorkPoolInnerCatch
       THEN /\ workPoolWorkerAlive' = TRUE
            /\ workPoolCpuPublished' = TRUE
            /\ workPoolPanicPhase' = "second"
       ELSE IF WorkPoolOuterCatch
            THEN /\ workPoolWorkerAlive' = TRUE
                 /\ workPoolCpuPublished' = FALSE
                 /\ workPoolPanicPhase' = "second"
            ELSE /\ workPoolWorkerAlive' = FALSE
                 /\ workPoolCpuPublished' = FALSE
                 /\ workPoolPanicPhase' = "dead"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolRunFirstAccountingPanic ==
    /\ workPoolPanicPhase = "first"
    /\ WorkPoolFirstFailure = "accounting"
    /\ workPoolFirstHandled' = TRUE
    /\ workPoolSecondCompleted' = workPoolSecondCompleted
    /\ workPoolRuntimeRecorded' = FALSE
    /\ workPoolCpuPublished' = FALSE
    /\ IF WorkPoolOuterCatch
       THEN /\ workPoolWorkerAlive' = TRUE
            /\ workPoolPanicPhase' = "second"
       ELSE /\ workPoolWorkerAlive' = FALSE
            /\ workPoolPanicPhase' = "dead"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolRunSecond ==
    /\ workPoolPanicPhase = "second"
    /\ workPoolWorkerAlive
    /\ workPoolPanicPhase' = "done"
    /\ workPoolWorkerAlive' = workPoolWorkerAlive
    /\ workPoolFirstHandled' = workPoolFirstHandled
    /\ workPoolSecondCompleted' = TRUE
    /\ workPoolRuntimeRecorded' = workPoolRuntimeRecorded
    /\ workPoolCpuPublished' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolAgeOld ==
    /\ workPoolOldAge = 0
    /\ workPoolOldAge' = 1
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED <<workPoolHighEnqueued, workPoolPopped>>
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolEnqueueHigh ==
    /\ workPoolOldAge = 1
    /\ ~workPoolHighEnqueued
    /\ workPoolHighEnqueued' = TRUE
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED <<workPoolOldAge, workPoolPopped>>
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolPopPriority ==
    /\ workPoolHighEnqueued
    /\ workPoolPopped = "none"
    /\ workPoolPopped' = IF WorkPoolRecomputeAtPop THEN "old" ELSE "high"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED <<workPoolOldAge, workPoolHighEnqueued>>
    /\ UNCHANGED workPoolOverflowVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolOverflowSpawn ==
    /\ workPoolOverflowPhase = "ready"
    /\ workPoolOverflowLive' =
        workPoolOverflowLive + OverflowSpawned(workPoolOverflowLive)
    /\ workPoolOverflowPhase' = "done"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolLifecycleVars

WorkPoolLifecycleDoubleUnpark ==
    /\ workPoolLifecyclePhase = "ready"
    /\ workPoolLifecycleParked > 0
    /\ workPoolLifecycleActive' =
        IF WorkPoolUseTransitionResult
        THEN workPoolLifecycleActive + 1
        ELSE workPoolLifecycleActive + 2
    /\ workPoolLifecycleParked' = workPoolLifecycleParked - 1
    /\ workPoolLifecyclePhase' = "done"
    /\ workPoolLifecycleScenario' = "DoubleUnpark"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars

WorkPoolLifecycleRespawnParked ==
    /\ workPoolLifecyclePhase = "ready"
    /\ workPoolLifecycleParked > 0
    /\ workPoolLifecycleActive' =
        IF WorkPoolRespawnCountsParked
        THEN workPoolLifecycleActive + 1
        ELSE workPoolLifecycleActive
    /\ workPoolLifecycleParked' = workPoolLifecycleParked - 1
    /\ workPoolLifecyclePhase' = "done"
    /\ workPoolLifecycleScenario' = "RespawnParked"
    /\ UNCHANGED baseVars
    /\ UNCHANGED gcBoundaryVars
    /\ UNCHANGED startupVars
    /\ UNCHANGED workPoolStartupVars
    /\ UNCHANGED workPoolPanicVars
    /\ UNCHANGED workPoolPriorityVars
    /\ UNCHANGED workPoolOverflowVars

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Idle ==
    UNCHANGED vars

ThreadingNoGcNext ==
    \/ Schedule
    \/ StartProducer
    \/ StartConsumer
    \/ Complete("producer")
    \/ Complete("consumer")
    \/ CronFirstDue
    \/ CronSecondDue
    \/ CronWorkerComplete
    \/ CronFinalDue

GcBoundaryNext ==
    \/ BuildRoots
    \/ AdmitLateWorker
    \/ Sweep

ThreadingNext ==
    \/ (ThreadingNoGcNext /\ UNCHANGED gcBoundaryVars)
    \/ GcBoundaryNext
    \/ Done

Next ==
    \/ (ThreadingNext /\ UNCHANGED startupVars /\ UNCHANGED workPoolVars)
    \/ CronStartupRun
    \/ CronStartupSubmitAfterReady
    \/ CronStartupPollCheckEvents
    \/ CronStartupPollDrainChannel
    \/ CronStartupNoSubmittedTask
    \/ CronStartupLoseWithoutPollPath
    \/ WorkPoolSubmit
    \/ WorkPoolStart
    \/ WorkPoolDrain
    \/ WorkPoolFinish
    \/ WorkPoolStuck
    \/ WorkPoolRunFirstTaskPanic
    \/ WorkPoolRunFirstAccountingPanic
    \/ WorkPoolRunSecond
    \/ WorkPoolAgeOld
    \/ WorkPoolEnqueueHigh
    \/ WorkPoolPopPriority
    \/ WorkPoolOverflowSpawn
    \/ WorkPoolLifecycleDoubleUnpark
    \/ WorkPoolLifecycleRespawnParked
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
    /\ WF_vars(WorkPoolSubmit)
    /\ WF_vars(WorkPoolStart)
    /\ WF_vars(WorkPoolDrain)
    /\ WF_vars(WorkPoolFinish)
    /\ WF_vars(WorkPoolStuck)
    /\ WF_vars(WorkPoolRunFirstTaskPanic)
    /\ WF_vars(WorkPoolRunFirstAccountingPanic)
    /\ WF_vars(WorkPoolRunSecond)
    /\ WF_vars(WorkPoolOverflowSpawn)
    /\ WF_vars(WorkPoolLifecycleDoubleUnpark)
    /\ WF_vars(WorkPoolLifecycleRespawnParked)

NoConsumerBeforeProducer ==
    ~consumerBeforeProducer

NoSameWaveEffectConflict ==
    ~conflictSameWave

NoDirectFanoutForDependentWork ==
    DirectFanout => ~(HasDependency \/ HasEffectConflict)

ActiveBranchCountGate ==
    ActiveBranchCount >= ActiveMinBranches

ActiveDegreeGate ==
    ActiveDegree > 1

ActiveBudgetGate ==
    ActiveBudgetGranted > 0

ActiveSafeCap ==
    IF ActiveMaxParallel = 0 THEN 1 ELSE ActiveMaxParallel

ActiveTransducerIdealDegree ==
    IF BranchParallel(ActiveCostClass) /\ ActiveBranchCount > 1 THEN
      Min(ActiveBranchCount, ActiveSafeCap)
    ELSE
      DefaultDegree(ActiveCostClass)

ActiveTransducerDegreeMatches ==
    DirectFanout => ActiveDegree = ActiveTransducerIdealDegree

ActiveTransducerMaximalBeforeCap ==
    /\ DirectFanout
    /\ BranchParallel(ActiveCostClass)
    /\ ActiveBranchCount > 1
    /\ ActiveBranchCount <= ActiveSafeCap
    => ActiveDegree = ActiveBranchCount

ActiveTransducerDefaultGateSound ==
    /\ DirectFanout
    /\ ActiveDegree > 1
    => BranchParallel(ActiveCostClass)

ActiveDispatchedCount ==
    IF DirectFanout THEN
      IF ActivePartialDispatch /\ ActiveBranchCount > 0
      THEN ActiveBranchCount - 1
      ELSE ActiveBranchCount
    ELSE 0

ActiveFanoutGateComplete ==
    DirectFanout =>
      /\ ActiveBranchCountGate
      /\ ActiveDegreeGate
      /\ ActivePure
      /\ ActiveDepthOk
      /\ ActivePoolOk
      /\ ActiveBudgetGate
      /\ ActiveDispatchedCount = ActiveBranchCount

ActiveBlocksParallelDispatch ==
    \/ ActiveStateMutation
    \/ ActiveStrictPrint /\ ActiveIo
    \/ ActiveDynamicEvalGate /\ ActiveDynamicEval

ActiveDynamicEvalGateComplete ==
    /\ DirectFanout
    /\ ActiveDynamicEval
    => ActiveBlocksParallelDispatch

ActiveStateMutationBlocks ==
    /\ DirectFanout
    /\ ActiveStateMutation
    => ActiveBlocksParallelDispatch

ActiveStrictIoBlocks ==
    /\ DirectFanout
    /\ ActiveStrictPrint
    /\ ActiveIo
    => ActiveBlocksParallelDispatch

ActiveNoBudgetParallelSafe ==
    /\ DirectFanout
    /\ ActivePure
    => ~ActiveBlocksParallelDispatch

MaximalIndependentParallelism ==
    phase /= "init" /\ ~HasDependency /\ ~HasEffectConflict =>
      wave["producer"] = wave["consumer"]

NoLiveValueSwept ==
    ~valueFreed

NoOverlappingCronDispatch ==
    ~cronOverlap

CronStopPreventsRedispatch ==
    ~cronDispatchedAgain

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

WorkPoolNoDropBeforeStart ==
    (~WorkPoolLossyEnqueue /\ workPoolStartupPhase = "submit") =>
      workPoolQueue = workPoolSubmitted

WorkPoolAllSubmittedComplete ==
    workPoolStartupPhase = "done" =>
      workPoolCompleted = WorkPoolStartupSubmissions

WorkPoolNoRuntimeRecordForTaskPanic ==
    /\ workPoolFirstHandled
    /\ WorkPoolFirstFailure = "task"
    => ~workPoolRuntimeRecorded

WorkPoolTaskPanicPublishesHeartbeat ==
    /\ workPoolFirstHandled
    /\ WorkPoolFirstFailure = "task"
    => workPoolCpuPublished

WorkPoolWorkerAliveAfterHandled ==
    workPoolFirstHandled => workPoolWorkerAlive

WorkPoolOldPopsAfterAging ==
    /\ workPoolHighEnqueued
    /\ workPoolOldAge = 1
    /\ workPoolPopped # "none"
    => workPoolPopped = "old"

WorkPoolOverflowWithinCap ==
    workPoolOverflowLive <= WorkPoolMaxOverflow

WorkPoolLifecycleCapacityConsistent ==
    workPoolLifecycleActive + workPoolLifecycleParked =
      WorkPoolLifecycleMaxWorkers

WorkPoolLifecycleActiveWithinBounds ==
    workPoolLifecycleActive <= WorkPoolLifecycleMaxWorkers

WorkPoolStartupEventuallyDrained ==
    <>(workPoolStartupPhase = "done" /\
       workPoolCompleted = WorkPoolStartupSubmissions)

WorkPoolSecondEventuallyCompleted ==
    <>workPoolSecondCompleted

EndToEndSafe ==
    /\ NoConsumerBeforeProducer
    /\ NoSameWaveEffectConflict
    /\ NoDirectFanoutForDependentWork
    /\ ActiveFanoutGateComplete
    /\ ActiveTransducerDegreeMatches
    /\ ActiveTransducerMaximalBeforeCap
    /\ ActiveTransducerDefaultGateSound
    /\ ActiveDynamicEvalGateComplete
    /\ ActiveStateMutationBlocks
    /\ ActiveStrictIoBlocks
    /\ ActiveNoBudgetParallelSafe
    /\ MaximalIndependentParallelism
    /\ NoLiveValueSwept
    /\ NoOverlappingCronDispatch
    /\ CronStopPreventsRedispatch
    /\ CronStartupReadyWaitCompletes
    /\ CronStartupScheduleAfterReadyHasHandle
    /\ CronStartupSubmittedReachable
    /\ NoCronStartupTaskLost
    /\ WorkPoolNoDropBeforeStart
    /\ WorkPoolAllSubmittedComplete
    /\ WorkPoolNoRuntimeRecordForTaskPanic
    /\ WorkPoolTaskPanicPublishesHeartbeat
    /\ WorkPoolWorkerAliveAfterHandled
    /\ WorkPoolOldPopsAfterAging
    /\ WorkPoolOverflowWithinCap
    /\ WorkPoolLifecycleCapacityConsistent
    /\ WorkPoolLifecycleActiveWithinBounds

=============================================================================
