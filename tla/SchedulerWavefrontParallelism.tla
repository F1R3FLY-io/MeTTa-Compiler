---------------------- MODULE SchedulerWavefrontParallelism ----------------------
(***************************************************************************)
(* Wavefront reordering model.                                             *)
(*                                                                         *)
(* AllIndependent = TRUE models the hot path where every task can be placed *)
(* in wave 0. CycleSameWave = TRUE models the unsound fallback that places  *)
(* mutually dependent unresolved tasks in the same final wave. With both    *)
(* constants FALSE, the model is a diamond DAG: 0 -> {1, 2} -> 3.          *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS AllIndependent, CycleSameWave

VARIABLE phase

vars == <<phase>>

Tasks ==
  IF CycleSameWave THEN 0..1 ELSE 0..3

Dep(t) ==
  IF AllIndependent THEN {}
  ELSE IF CycleSameWave THEN
    IF t = 0 THEN {1} ELSE {0}
  ELSE IF t = 0 THEN {}
  ELSE IF t = 1 THEN {0}
  ELSE IF t = 2 THEN {0}
  ELSE IF t = 3 THEN {1, 2}
  ELSE {}

WaveOf(t) ==
  IF AllIndependent THEN 0
  ELSE IF CycleSameWave THEN 0
  ELSE IF t = 0 THEN 0
  ELSE IF t = 1 THEN 1
  ELSE IF t = 2 THEN 1
  ELSE IF t = 3 THEN 2
  ELSE 0

Init ==
  phase = "done"

Next ==
  UNCHANGED vars

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ AllIndependent \in BOOLEAN
  /\ CycleSameWave \in BOOLEAN
  /\ ~(AllIndependent /\ CycleSameWave)
  /\ phase = "done"
  /\ \A t \in Tasks : Dep(t) \subseteq Tasks
  /\ \A t \in Tasks : WaveOf(t) \in Nat

DependenciesBefore ==
  \A t \in Tasks :
    \A d \in Dep(t) :
      WaveOf(d) < WaveOf(t)

SameWaveIndependent ==
  \A t \in Tasks :
    \A d \in Dep(t) :
      WaveOf(d) # WaveOf(t)

IndependentTasksSingleWaveMax ==
  AllIndependent =>
    /\ \A t \in Tasks : WaveOf(t) = 0
    /\ Cardinality({t \in Tasks : WaveOf(t) = 0}) = Cardinality(Tasks)

=============================================================================
