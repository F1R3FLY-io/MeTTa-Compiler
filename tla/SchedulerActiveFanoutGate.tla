------------------------ MODULE SchedulerActiveFanoutGate ------------------------
(***************************************************************************)
(* Active production fanout gate model.                                    *)
(*                                                                         *)
(* The production evaluator uses direct fanout.  Dispatch is permitted only*)
(* after the branch-count, WFST degree, purity/dynamic-eval, depth, pool,   *)
(* and budget gates pass.  MissingGate selects a faulty implementation that *)
(* ignores one of those gates.                                              *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS BranchCount, MinBranches, Degree, BudgetGranted,
          Pure, DepthOk, PoolOk, MissingGate, PartialDispatch

VARIABLE phase

vars == <<phase>>

GateNames ==
  {"none", "branch_count", "degree", "purity", "depth", "pool", "budget"}

BranchCountGate ==
  BranchCount >= MinBranches

DegreeGate ==
  Degree > 1

BudgetGate ==
  BudgetGranted > 0

GateHolds(name) ==
  CASE name = "branch_count" -> BranchCountGate
    [] name = "degree" -> DegreeGate
    [] name = "purity" -> Pure
    [] name = "depth" -> DepthOk
    [] name = "pool" -> PoolOk
    [] name = "budget" -> BudgetGate
    [] OTHER -> TRUE

GateEffective(name) ==
  IF MissingGate = name THEN TRUE ELSE GateHolds(name)

Dispatched ==
  /\ GateEffective("branch_count")
  /\ GateEffective("degree")
  /\ GateEffective("purity")
  /\ GateEffective("depth")
  /\ GateEffective("pool")
  /\ GateEffective("budget")

DispatchedCount ==
  IF Dispatched THEN
    IF PartialDispatch /\ BranchCount > 0 THEN BranchCount - 1 ELSE BranchCount
  ELSE 0

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
  /\ MissingGate \in GateNames
  /\ PartialDispatch \in BOOLEAN
  /\ phase = "checked"

CompleteDispatch ==
  Dispatched => DispatchedCount = BranchCount

NoDispatchWithoutBranchCountGate ==
  ~BranchCountGate => ~Dispatched

NoDispatchWithoutDegreeGate ==
  ~DegreeGate => ~Dispatched

NoDispatchWithoutPurityGate ==
  ~Pure => ~Dispatched

NoDispatchWithoutDepthGate ==
  ~DepthOk => ~Dispatched

NoDispatchWithoutPoolGate ==
  ~PoolOk => ~Dispatched

NoDispatchWithoutBudgetGate ==
  ~BudgetGate => ~Dispatched

=============================================================================
