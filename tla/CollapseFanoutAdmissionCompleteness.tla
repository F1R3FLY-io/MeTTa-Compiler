------------------- MODULE CollapseFanoutAdmissionCompleteness -------------------
(***************************************************************************)
(* Collapse fanout admission completeness model.                            *)
(*                                                                         *)
(* PartialByThreshold = TRUE models treating the collapse threshold as a    *)
(* spawn cap. MissingThresholdGate = TRUE models admission without checking *)
(* that the result count reaches the threshold.                             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS ItemCount, Threshold, BudgetGranted,
          DepthOk, PoolOk, PartialByThreshold, MissingThresholdGate

VARIABLE phase

vars == <<phase>>

Min(a, b) ==
  IF a <= b THEN a ELSE b

ThresholdGate ==
  ItemCount >= Threshold

Admitted ==
  /\ IF MissingThresholdGate THEN TRUE ELSE ThresholdGate
  /\ DepthOk
  /\ PoolOk
  /\ BudgetGranted > 0

SpawnedCount ==
  IF PartialByThreshold THEN Min(ItemCount, Threshold) ELSE ItemCount

BudgetRequest ==
  IF ItemCount = 0 THEN 0 ELSE ItemCount - 1

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ ItemCount \in Nat
  /\ Threshold \in Nat
  /\ BudgetGranted \in Nat
  /\ DepthOk \in BOOLEAN
  /\ PoolOk \in BOOLEAN
  /\ PartialByThreshold \in BOOLEAN
  /\ MissingThresholdGate \in BOOLEAN
  /\ phase = "checked"

CompleteAdmittedCollapse ==
  Admitted => SpawnedCount = ItemCount

ThresholdGateRequired ==
  Admitted => ThresholdGate

AllExtraItemsRequested ==
  ItemCount > 0 => BudgetRequest = ItemCount - 1

NoAdmissionWithoutBudget ==
  BudgetGranted = 0 => ~Admitted

=============================================================================
