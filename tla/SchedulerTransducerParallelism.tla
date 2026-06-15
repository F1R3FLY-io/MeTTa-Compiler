-------------------- MODULE SchedulerTransducerParallelism --------------------
(***************************************************************************)
(* WFST transducer parallelism model.                                       *)
(*                                                                         *)
(* ClampZeroCap = TRUE models the fixed branch-aware transducer: cap 0      *)
(* degrades to sequential degree 1. ClampZeroCap = FALSE models the old     *)
(* shape that returned degree 0 for branch_count > 1 and max_parallel = 0.  *)
(* UnderutilizeBeforeCap = TRUE models a safe but nonmaximal transducer that*)
(* leaves an available branch unused even though the branch count is below  *)
(* the cap.                                                                 *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS CostClass, BranchCount, MaxParallel,
          ClampZeroCap, UnderutilizeBeforeCap,
          OverrideDegree, ForcedDegree

VARIABLE phase

vars == <<phase>>

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

SafeCap ==
  IF ClampZeroCap THEN
    IF MaxParallel = 0 THEN 1 ELSE MaxParallel
  ELSE
    MaxParallel

Min(a, b) ==
  IF a <= b THEN a ELSE b

IdealDegree ==
  IF BranchParallel(CostClass) /\ BranchCount > 1 THEN
    Min(BranchCount, SafeCap)
  ELSE
    DefaultDegree(CostClass)

ComputedDegree ==
  IF UnderutilizeBeforeCap
     /\ BranchParallel(CostClass)
     /\ BranchCount > 1
     /\ BranchCount <= SafeCap
     /\ BranchCount > 2 THEN
    BranchCount - 1
  ELSE
    IdealDegree

Degree ==
  IF OverrideDegree THEN ForcedDegree ELSE ComputedDegree

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ CostClass \in Classes
  /\ BranchCount \in Nat
  /\ MaxParallel \in Nat
  /\ ClampZeroCap \in BOOLEAN
  /\ UnderutilizeBeforeCap \in BOOLEAN
  /\ OverrideDegree \in BOOLEAN
  /\ ForcedDegree \in Nat
  /\ phase = "checked"

NonZeroDegree ==
  Degree >= 1

CapRespected ==
  BranchParallel(CostClass) /\ BranchCount > 1 => Degree <= SafeCap

SequentialClassesStaySequential ==
  ~BranchParallel(CostClass) => Degree = 1

MaximalBeforeCap ==
  BranchParallel(CostClass)
    /\ BranchCount > 1
    /\ BranchCount <= SafeCap
    => Degree = BranchCount

DefaultGateSound ==
  Degree > 1 => BranchParallel(CostClass)

=============================================================================
