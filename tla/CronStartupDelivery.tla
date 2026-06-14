----------------------------- MODULE CronStartupDelivery -----------------------------
(***************************************************************************)
(* Cron startup readiness and submitted-task delivery.                      *)
(*                                                                         *)
(* A task submitted after the caller observes the ready signal is safe only *)
(* when the returned CronHandle owns a sender, the ready receiver was       *)
(* returned to the caller, the ready signal is sent from inside run(), and  *)
(* the event loop polls the task channel in CheckEvents or DrainChannel.    *)
(***************************************************************************)

CONSTANTS
    ReturnHandleSender,
    ReturnReadyReceiver,
    ReadySentInsideRun,
    ScheduleAfterReady,
    CheckEventsPolls,
    DrainChannelPolls

VARIABLES
    phase,
    readyObserved,
    taskSubmitted,
    taskObserved,
    taskLost

vars == <<phase, readyObserved, taskSubmitted, taskObserved, taskLost>>

Init ==
    /\ phase = "spawned"
    /\ readyObserved = FALSE
    /\ taskSubmitted = FALSE
    /\ taskObserved = FALSE
    /\ taskLost = FALSE

StartRun ==
    /\ phase = "spawned"
    /\ phase' = "ready"
    /\ readyObserved' = (ReturnReadyReceiver /\ ReadySentInsideRun)
    /\ UNCHANGED <<taskSubmitted, taskObserved, taskLost>>

SubmitAfterReady ==
    /\ phase = "ready"
    /\ phase' = "submitted"
    /\ taskSubmitted' = (ScheduleAfterReady /\ readyObserved /\ ReturnHandleSender)
    /\ UNCHANGED <<readyObserved, taskObserved, taskLost>>

PollCheckEvents ==
    /\ phase = "submitted"
    /\ taskSubmitted
    /\ CheckEventsPolls
    /\ phase' = "observed"
    /\ taskObserved' = TRUE
    /\ UNCHANGED <<readyObserved, taskSubmitted, taskLost>>

PollDrainChannel ==
    /\ phase = "submitted"
    /\ taskSubmitted
    /\ DrainChannelPolls
    /\ phase' = "observed"
    /\ taskObserved' = TRUE
    /\ UNCHANGED <<readyObserved, taskSubmitted, taskLost>>

NoSubmittedTask ==
    /\ phase = "submitted"
    /\ ~taskSubmitted
    /\ phase' = "observed"
    /\ taskObserved' = FALSE
    /\ UNCHANGED <<readyObserved, taskSubmitted, taskLost>>

LoseWithoutPollPath ==
    /\ phase = "submitted"
    /\ taskSubmitted
    /\ ~(CheckEventsPolls \/ DrainChannelPolls)
    /\ phase' = "lost"
    /\ taskLost' = TRUE
    /\ UNCHANGED <<readyObserved, taskSubmitted, taskObserved>>

Done ==
    /\ phase \in {"observed", "lost"}
    /\ UNCHANGED vars

Next ==
    \/ StartRun
    \/ SubmitAfterReady
    \/ PollCheckEvents
    \/ PollDrainChannel
    \/ NoSubmittedTask
    \/ LoseWithoutPollPath
    \/ Done

Spec ==
    /\ Init
    /\ [][Next]_vars
    /\ WF_vars(StartRun)
    /\ WF_vars(SubmitAfterReady)
    /\ WF_vars(PollCheckEvents)
    /\ WF_vars(PollDrainChannel)
    /\ WF_vars(NoSubmittedTask)
    /\ WF_vars(LoseWithoutPollPath)

TypeOK ==
    /\ ReturnHandleSender \in BOOLEAN
    /\ ReturnReadyReceiver \in BOOLEAN
    /\ ReadySentInsideRun \in BOOLEAN
    /\ ScheduleAfterReady \in BOOLEAN
    /\ CheckEventsPolls \in BOOLEAN
    /\ DrainChannelPolls \in BOOLEAN
    /\ phase \in {"spawned", "ready", "submitted", "observed", "lost"}
    /\ readyObserved \in BOOLEAN
    /\ taskSubmitted \in BOOLEAN
    /\ taskObserved \in BOOLEAN
    /\ taskLost \in BOOLEAN

ReadyWaitCompletes ==
    (phase /= "spawned" /\ ScheduleAfterReady) => readyObserved

ScheduleAfterReadyHasHandle ==
    ScheduleAfterReady => ReturnHandleSender

SubmittedTaskReachable ==
    taskSubmitted => ReturnHandleSender /\ readyObserved

NoStartupTaskLost ==
    ~taskLost

SubmittedTaskEventuallyObserved ==
    [](taskSubmitted => <>taskObserved)

=============================================================================
