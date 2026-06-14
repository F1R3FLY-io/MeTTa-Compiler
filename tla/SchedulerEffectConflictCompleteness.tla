------------------ MODULE SchedulerEffectConflictCompleteness ------------------
(***************************************************************************)
(* Wavefront effect-conflict completeness model.                            *)
(*                                                                         *)
(* The wavefront scheduler can only use dependency edges that it is given.  *)
(* MissingConflictEdge = TRUE models a caller that forgets to add an edge   *)
(* for two effect-conflicting tasks; the tasks then share wave 0, violating *)
(* conflict freedom.  NoConflicts = TRUE models the fully pure hot path.    *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS MissingConflictEdge, NoConflicts

VARIABLE phase

vars == <<phase>>

Tasks == 0..1

Conflicts(left, right) ==
  /\ ~NoConflicts
  /\ left \in Tasks
  /\ right \in Tasks
  /\ ((left = 0 /\ right = 1) \/ (left = 1 /\ right = 0))

Dep(task) ==
  IF NoConflicts \/ MissingConflictEdge THEN {}
  ELSE IF task = 1 THEN {0}
  ELSE {}

WaveOf(task) ==
  IF NoConflicts \/ MissingConflictEdge THEN 0
  ELSE IF task = 0 THEN 0
  ELSE 1

Init ==
  phase = "done"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ MissingConflictEdge \in BOOLEAN
  /\ NoConflicts \in BOOLEAN
  /\ phase = "done"
  /\ \A task \in Tasks : Dep(task) \subseteq Tasks
  /\ \A task \in Tasks : WaveOf(task) \in Nat

DependenciesBefore ==
  \A task \in Tasks :
    \A dep \in Dep(task) :
      WaveOf(dep) < WaveOf(task)

ConflictEdgesCovered ==
  \A left \in Tasks :
    \A right \in Tasks :
      Conflicts(left, right) => (left \in Dep(right) \/ right \in Dep(left))

SameWaveConflictFree ==
  \A left \in Tasks :
    \A right \in Tasks :
      Conflicts(left, right) => WaveOf(left) # WaveOf(right)

NoConflictSingleWaveMax ==
  NoConflicts =>
    /\ \A task \in Tasks : WaveOf(task) = 0
    /\ Cardinality({task \in Tasks : WaveOf(task) = 0}) = Cardinality(Tasks)

=============================================================================
