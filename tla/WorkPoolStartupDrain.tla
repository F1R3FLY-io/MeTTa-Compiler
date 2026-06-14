-------------------------- MODULE WorkPoolStartupDrain --------------------------
(***************************************************************************)
(* WorkPool startup-drain model.                                            *)
(*                                                                         *)
(* Eval tasks may be submitted before worker OS threads have reached the    *)
(* queue-pop loop. A correct WorkPool retains those tasks and drains them   *)
(* once workers start.                                                      *)
(*                                                                         *)
(* StartWorkers = FALSE models a startup path that never starts workers.    *)
(* LossyEnqueue = TRUE models an eval enqueue path that drops the final     *)
(* submitted task.                                                         *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS N, StartWorkers, LossyEnqueue

VARIABLES submitted, queue, completed, workersStarted, phase

vars == <<submitted, queue, completed, workersStarted, phase>>

Init ==
  /\ submitted = 0
  /\ queue = 0
  /\ completed = 0
  /\ workersStarted = FALSE
  /\ phase = "submit"

Submit ==
  /\ phase = "submit"
  /\ submitted < N
  /\ submitted' = submitted + 1
  /\ queue' =
       IF LossyEnqueue /\ submitted + 1 = N
       THEN queue
       ELSE queue + 1
  /\ completed' = completed
  /\ workersStarted' = workersStarted
  /\ phase' = phase

Start ==
  /\ phase = "submit"
  /\ submitted = N
  /\ submitted' = submitted
  /\ queue' = queue
  /\ completed' = completed
  /\ workersStarted' = StartWorkers
  /\ phase' = "drain"

Drain ==
  /\ phase = "drain"
  /\ workersStarted
  /\ queue > 0
  /\ submitted' = submitted
  /\ queue' = queue - 1
  /\ completed' = completed + 1
  /\ workersStarted' = workersStarted
  /\ phase' = phase

Finish ==
  /\ phase = "drain"
  /\ workersStarted
  /\ queue = 0
  /\ submitted' = submitted
  /\ queue' = queue
  /\ completed' = completed
  /\ workersStarted' = workersStarted
  /\ phase' = "done"

Stuck ==
  /\ phase = "drain"
  /\ ~workersStarted
  /\ queue > 0
  /\ submitted' = submitted
  /\ queue' = queue
  /\ completed' = completed
  /\ workersStarted' = workersStarted
  /\ phase' = "stuck"

Done ==
  /\ phase \in {"done", "stuck"}
  /\ UNCHANGED vars

Next == Submit \/ Start \/ Drain \/ Finish \/ Stuck \/ Done

Spec ==
  /\ Init
  /\ [][Next]_vars
  /\ WF_vars(Submit)
  /\ WF_vars(Start)
  /\ WF_vars(Drain)
  /\ WF_vars(Finish)
  /\ WF_vars(Stuck)

TypeOK ==
  /\ N \in Nat
  /\ StartWorkers \in BOOLEAN
  /\ LossyEnqueue \in BOOLEAN
  /\ submitted \in 0..N
  /\ queue \in 0..N
  /\ completed \in 0..N
  /\ workersStarted \in BOOLEAN
  /\ phase \in {"submit", "drain", "done", "stuck"}

NoDropBeforeStart ==
  (~LossyEnqueue /\ phase = "submit") => queue = submitted

AllSubmittedComplete ==
  phase = "done" => completed = N

EventuallyDrained ==
  <>(phase = "done" /\ completed = N)

=============================================================================
