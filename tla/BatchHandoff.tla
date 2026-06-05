------------------------------ MODULE BatchHandoff ------------------------------
(***************************************************************************)
(* E2 batch-result handoff discriminator.                                  *)
(*                                                                         *)
(* A batch worker can finish before the async caller copies its results     *)
(* into MettaState.output. The result must therefore be protected by a      *)
(* persistent safepoint/driver-C handle while it rides worker -> gather ->  *)
(* caller, and the handle may be dropped only after the output copy.        *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    UseHandle,
    DropBeforeCopy

VARIABLES
    phase,
    resultPublished,
    handleRoot,
    outputRoot,
    freed

vars == <<phase, resultPublished, handleRoot, outputRoot, freed>>

TypeOK ==
    /\ phase \in {"worker", "registered", "caller", "caller_dropped", "copied", "done"}
    /\ resultPublished \in BOOLEAN
    /\ handleRoot \in BOOLEAN
    /\ outputRoot \in BOOLEAN
    /\ freed \in BOOLEAN

Init ==
    /\ phase = "worker"
    /\ resultPublished = FALSE
    /\ handleRoot = FALSE
    /\ outputRoot = FALSE
    /\ freed = FALSE

RegisterHandle ==
    /\ phase = "worker"
    /\ UseHandle
    /\ phase' = "registered"
    /\ handleRoot' = TRUE
    /\ UNCHANGED <<resultPublished, outputRoot, freed>>

PublishResult ==
    /\ IF UseHandle THEN phase = "registered" ELSE phase = "worker"
    /\ phase' = "caller"
    /\ resultPublished' = TRUE
    /\ UNCHANGED <<handleRoot, outputRoot, freed>>

DropHandleBeforeCopy ==
    /\ phase = "caller"
    /\ DropBeforeCopy
    /\ handleRoot
    /\ phase' = "caller_dropped"
    /\ handleRoot' = FALSE
    /\ UNCHANGED <<resultPublished, outputRoot, freed>>

CopyToOutput ==
    /\ phase \in {"caller", "caller_dropped"}
    /\ resultPublished
    /\ phase' = "copied"
    /\ outputRoot' = TRUE
    /\ UNCHANGED <<resultPublished, handleRoot, freed>>

DropHandleAfterCopy ==
    /\ phase = "copied"
    /\ ~DropBeforeCopy
    /\ handleRoot
    /\ phase' = "done"
    /\ handleRoot' = FALSE
    /\ UNCHANGED <<resultPublished, outputRoot, freed>>

FinishWithoutHandleDrop ==
    /\ phase = "copied"
    /\ ~handleRoot
    /\ phase' = "done"
    /\ UNCHANGED <<resultPublished, handleRoot, outputRoot, freed>>

Sweep ==
    /\ phase \in {"caller", "caller_dropped", "copied", "done"}
    /\ resultPublished
    /\ freed' = ~(handleRoot \/ outputRoot)
    /\ UNCHANGED <<phase, resultPublished, handleRoot, outputRoot>>

Done ==
    /\ phase = "done"
    /\ UNCHANGED vars

Next ==
    \/ RegisterHandle
    \/ PublishResult
    \/ DropHandleBeforeCopy
    \/ CopyToOutput
    \/ DropHandleAfterCopy
    \/ FinishWithoutHandleDrop
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoPublishedBatchResultFreed ==
    resultPublished => ~freed

=============================================================================
