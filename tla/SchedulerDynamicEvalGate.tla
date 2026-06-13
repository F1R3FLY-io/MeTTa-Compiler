------------------------ MODULE SchedulerDynamicEvalGate ------------------------
(***************************************************************************)
(* Dynamic eval discriminator for the scheduler parallel-dispatch gate.     *)
(*                                                                         *)
(* IncludeDynamicEvalGate = FALSE models the old gate: state mutation       *)
(* blocks, strict I/O optionally blocks, but dynamic eval does not.         *)
(* IncludeDynamicEvalGate = TRUE models the fixed gate.                     *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS IncludeDynamicEvalGate, DynamicEvalPresent, StateMutationPresent,
          StrictPrintOrder, IoPresent

VARIABLE phase

vars == <<phase>>

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ IncludeDynamicEvalGate \in BOOLEAN
  /\ DynamicEvalPresent \in BOOLEAN
  /\ StateMutationPresent \in BOOLEAN
  /\ StrictPrintOrder \in BOOLEAN
  /\ IoPresent \in BOOLEAN
  /\ phase = "checked"

BlocksParallelDispatch ==
  \/ StateMutationPresent
  \/ StrictPrintOrder /\ IoPresent
  \/ IncludeDynamicEvalGate /\ DynamicEvalPresent

NoDynamicEvalParallelBypass ==
  DynamicEvalPresent => BlocksParallelDispatch

StateMutationStillBlocks ==
  StateMutationPresent => BlocksParallelDispatch

StrictIoStillBlocks ==
  StrictPrintOrder /\ IoPresent => BlocksParallelDispatch

=============================================================================
