------------------ MODULE SchedulerFanoutAdmissionCompleteness ------------------
(***************************************************************************)
(* Fanout admission completeness model.                                     *)
(*                                                                         *)
(* PartialByDegree = TRUE models the tempting but wrong implementation that *)
(* caps the spawned branch slots to the WFST degree. MissingDegreeGate =    *)
(* TRUE models admission without the degree > 1 gate.                       *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS BranchCount, MinBranches, Degree, BudgetGranted,
          Pure, DepthOk, PoolOk, PartialByDegree, MissingDegreeGate

VARIABLE phase

vars == <<phase>>

Min(a, b) ==
  IF a <= b THEN a ELSE b

DegreeGate ==
  Degree > 1

Admitted ==
  /\ BranchCount >= MinBranches
  /\ IF MissingDegreeGate THEN TRUE ELSE DegreeGate
  /\ Pure
  /\ DepthOk
  /\ PoolOk
  /\ BudgetGranted > 0

SpawnedCount ==
  IF PartialByDegree THEN Min(BranchCount, Degree) ELSE BranchCount

BudgetRequest ==
  IF BranchCount = 0 THEN 0 ELSE BranchCount - 1

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ BranchCount \in Nat
  /\ MinBranches \in Nat
  /\ Degree \in Nat
  /\ BudgetGranted \in Nat
  /\ Pure \in BOOLEAN
  /\ DepthOk \in BOOLEAN
  /\ PoolOk \in BOOLEAN
  /\ PartialByDegree \in BOOLEAN
  /\ MissingDegreeGate \in BOOLEAN
  /\ phase = "checked"

CompleteAdmittedFanout ==
  Admitted => SpawnedCount = BranchCount

DegreeGateRequired ==
  Admitted => DegreeGate

AllExtraBranchesRequested ==
  BranchCount > 0 => BudgetRequest = BranchCount - 1

NoAdmissionWithoutBudget ==
  BudgetGranted = 0 => ~Admitted

=============================================================================
