---------------------- MODULE SchedulerWavefrontParallelism ----------------------
(***************************************************************************)
(* Wavefront reordering model.                                             *)
(*                                                                         *)
(* AllIndependent = TRUE models the hot path where every task can be placed *)
(* in wave 0. CycleSameWave = TRUE models the unsound fallback that places  *)
(* mutually dependent unresolved tasks in the same final wave. DeferReady   *)
(* models a safe-but-nonmaximal schedule that leaves a ready diamond branch *)
(* out of its earliest possible wave. With all constants FALSE, the model   *)
(* is a diamond DAG: 0 -> {1, 2} -> 3.                                     *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS AllIndependent, CycleSameWave, DeferReady

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
  ELSE IF DeferReady THEN
    IF t = 0 THEN 0
    ELSE IF t = 1 THEN 1
    ELSE IF t = 2 THEN 2
    ELSE IF t = 3 THEN 3
    ELSE 0
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
  /\ DeferReady \in BOOLEAN
  /\ ~(AllIndependent /\ CycleSameWave)
  /\ DeferReady => ~(AllIndependent \/ CycleSameWave)
  /\ phase = "done"
  /\ \A t \in Tasks : Dep(t) \subseteq Tasks
  /\ \A t \in Tasks : WaveOf(t) \in Nat

MaxWave ==
  IF AllIndependent THEN 0
  ELSE IF CycleSameWave THEN 0
  ELSE IF DeferReady THEN 3
  ELSE 2

DependenciesBefore ==
  \A t \in Tasks :
    \A d \in Dep(t) :
      WaveOf(d) < WaveOf(t)

SameWaveIndependent ==
  \A t \in Tasks :
    \A d \in Dep(t) :
      WaveOf(d) # WaveOf(t)

ReadyForWave(k, t) ==
  \A d \in Dep(t) :
    WaveOf(d) < k

NoReadyTaskDeferred ==
  \A k \in 0..MaxWave :
    \A t \in Tasks :
      (ReadyForWave(k, t) /\ WaveOf(t) >= k) => WaveOf(t) = k

IndependentTasksSingleWaveMax ==
  AllIndependent =>
    /\ \A t \in Tasks : WaveOf(t) = 0
    /\ Cardinality({t \in Tasks : WaveOf(t) = 0}) = Cardinality(Tasks)

=============================================================================
