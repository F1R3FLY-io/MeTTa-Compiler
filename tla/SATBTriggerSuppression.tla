---------------------------- MODULE SATBTriggerSuppression ----------------------------
(***************************************************************************)
(* E2 SATB FANOUT trigger-suppression discriminator.                       *)
(*                                                                        *)
(* A worker may request a dedicated concurrent collection when the heap    *)
(* watermark is due, but not while an E2 SATB mark is already in progress. *)
(* GateOnSatbIdle=FALSE models the bug: the watermark predicate can start  *)
(* another request even though SATB deletion barriers/finalization are      *)
(* protecting the active snapshot.                                         *)
(***************************************************************************)

CONSTANT GateOnSatbIdle

VARIABLES
    phase,
    satbMarking,
    watermarkDue,
    fanoutGate,
    requestPending,
    triggerSent

vars ==
    <<phase, satbMarking, watermarkDue, fanoutGate, requestPending, triggerSent>>

TypeOK ==
    /\ GateOnSatbIdle \in BOOLEAN
    /\ phase \in {"start", "chosen", "checked"}
    /\ satbMarking \in BOOLEAN
    /\ watermarkDue \in BOOLEAN
    /\ fanoutGate \in BOOLEAN
    /\ requestPending \in BOOLEAN
    /\ triggerSent \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ satbMarking = FALSE
    /\ watermarkDue = FALSE
    /\ fanoutGate = FALSE
    /\ requestPending = FALSE
    /\ triggerSent = FALSE

ChooseInputs ==
    /\ phase = "start"
    /\ satbMarking' \in BOOLEAN
    /\ watermarkDue' \in BOOLEAN
    /\ fanoutGate' \in BOOLEAN
    /\ requestPending' \in BOOLEAN
    /\ triggerSent' = FALSE
    /\ phase' = "chosen"

CheckTrigger ==
    /\ phase = "chosen"
    /\ triggerSent' =
        /\ fanoutGate
        /\ ~requestPending
        /\ IF GateOnSatbIdle THEN ~satbMarking ELSE TRUE
        /\ watermarkDue
    /\ phase' = "checked"
    /\ UNCHANGED <<satbMarking, watermarkDue, fanoutGate, requestPending>>

Done ==
    /\ phase = "checked"
    /\ UNCHANGED vars

Next ==
    \/ ChooseInputs
    \/ CheckTrigger
    \/ Done

Spec == Init /\ [][Next]_vars

ActiveSATBSuppressesWatermarkTrigger ==
    ~(phase = "checked" /\ satbMarking /\ triggerSent)

TriggerImpliesSATBIdle ==
    ~(phase = "checked" /\ triggerSent /\ satbMarking)

TriggerRequiresWatermarkAndFanout ==
    phase = "checked" /\ triggerSent => watermarkDue /\ fanoutGate

PendingRequestSuppressesTrigger ==
    ~(phase = "checked" /\ requestPending /\ triggerSent)

=============================================================================
