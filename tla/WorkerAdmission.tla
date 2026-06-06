------------------------------ MODULE WorkerAdmission ------------------------------
(***************************************************************************)
(* E1 WorkerEnter/admission-gate discriminator.                            *)
(*                                                                         *)
(* The dedicated collector closes admission with GC_IN_PROGRESS before it   *)
(* snapshots participant threads. A new worker must not join the active     *)
(* evaluator set between that snapshot and sweep: its machine would be live *)
(* but absent from the snapshot root set.                                   *)
(*                                                                         *)
(* UseAdmissionGate = TRUE models EvalGuard::enter/reacquire backing out    *)
(* while GC_IN_PROGRESS is set. UseAdmissionGate = FALSE admits the bug: a  *)
(* worker can enter during collection after the snapshot and then be freed. *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UseAdmissionGate

VARIABLES
    phase,
    workerJoined,
    workerInSnapshot,
    workerFreed

vars == <<phase, workerJoined, workerInSnapshot, workerFreed>>

TypeOK ==
    /\ phase \in {"mutating", "collecting", "swept"}
    /\ workerJoined \in BOOLEAN
    /\ workerInSnapshot \in BOOLEAN
    /\ workerFreed \in BOOLEAN

Init ==
    /\ phase = "mutating"
    /\ workerJoined = FALSE
    /\ workerInSnapshot = FALSE
    /\ workerFreed = FALSE

WorkerEnter ==
    /\ ~workerJoined
    /\ phase # "swept"
    /\ IF UseAdmissionGate THEN phase # "collecting" ELSE TRUE
    /\ workerJoined' = TRUE
    /\ UNCHANGED <<phase, workerInSnapshot, workerFreed>>

StartGC ==
    /\ phase = "mutating"
    /\ phase' = "collecting"
    /\ workerInSnapshot' = workerJoined
    /\ UNCHANGED <<workerJoined, workerFreed>>

Sweep ==
    /\ phase = "collecting"
    /\ phase' = "swept"
    /\ workerFreed' = (workerJoined /\ ~workerInSnapshot)
    /\ UNCHANGED <<workerJoined, workerInSnapshot>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ WorkerEnter
    \/ StartGC
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoUnsnapshottedWorkerFreed ==
    workerJoined => ~workerFreed

=============================================================================
