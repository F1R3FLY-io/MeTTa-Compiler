-------------- MODULE SchedulerDirectFanoutWavefrontRefinement --------------
(***************************************************************************)
(* Direct production fanout refines the wavefront model only for the        *)
(* all-independent branch case.  MissingIndependenceGate selects a faulty   *)
(* implementation that treats a dependency-bearing DAG as direct wave-0     *)
(* fanout.  PartialDispatch selects a faulty implementation that drops a    *)
(* branch slot after admission.                                             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS BranchCount, DependencyEdges,
          MissingIndependenceGate, PartialDispatch

VARIABLE phase

vars == <<phase>>

Independent ==
  DependencyEdges = 0

WavefrontSingleWave ==
  /\ BranchCount > 0
  /\ Independent

DirectAllowed ==
  /\ BranchCount > 0
  /\ IF MissingIndependenceGate THEN TRUE ELSE Independent

DirectDispatchedCount ==
  IF DirectAllowed THEN
    IF PartialDispatch /\ BranchCount > 0 THEN BranchCount - 1 ELSE BranchCount
  ELSE 0

Init ==
  phase = "checked"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ BranchCount \in Nat
  /\ DependencyEdges \in Nat
  /\ MissingIndependenceGate \in BOOLEAN
  /\ PartialDispatch \in BOOLEAN
  /\ phase = "checked"

DirectOnlyForIndependentWavefront ==
  DirectAllowed => Independent

DirectMatchesWavefrontMaxParallelism ==
  WavefrontSingleWave /\ DirectAllowed => DirectDispatchedCount = BranchCount

CompleteDirectDispatch ==
  DirectAllowed => DirectDispatchedCount = BranchCount

=============================================================================
