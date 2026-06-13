----------------------------- MODULE WorkPoolLifecycle -----------------------------
(***************************************************************************)
(* WorkPool/AdaptiveGcPool park-state and active-count lifecycle model.     *)
(*                                                                         *)
(* UseTransitionResult = FALSE models the old shape where a caller checks  *)
(* is_parked separately and can count the same transition twice.            *)
(* RespawnCountsParked = FALSE models respawning a parked/dead worker as    *)
(* unparked without incrementing the aggregate active count.                *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS MaxWorkers, InitialActive, InitialParked,
          UseTransitionResult, RespawnCountsParked, Scenarios

VARIABLES active, parked, phase, scenario

vars == <<active, parked, phase, scenario>>

Init ==
  /\ active = InitialActive
  /\ parked = InitialParked
  /\ phase = "ready"
  /\ scenario = "none"

DoubleUnpark ==
  /\ phase = "ready"
  /\ "DoubleUnpark" \in Scenarios
  /\ parked > 0
  /\ active' = IF UseTransitionResult THEN active + 1 ELSE active + 2
  /\ parked' = parked - 1
  /\ phase' = "done"
  /\ scenario' = "DoubleUnpark"

RespawnParked ==
  /\ phase = "ready"
  /\ "RespawnParked" \in Scenarios
  /\ parked > 0
  /\ active' = IF RespawnCountsParked THEN active + 1 ELSE active
  /\ parked' = parked - 1
  /\ phase' = "done"
  /\ scenario' = "RespawnParked"

Done ==
  /\ phase = "done"
  /\ UNCHANGED vars

Next == DoubleUnpark \/ RespawnParked \/ Done

Spec == Init /\ [][Next]_vars

TypeOK ==
  /\ MaxWorkers \in Nat
  /\ InitialActive \in Nat
  /\ InitialParked \in Nat
  /\ UseTransitionResult \in BOOLEAN
  /\ RespawnCountsParked \in BOOLEAN
  /\ Scenarios \subseteq {"DoubleUnpark", "RespawnParked"}
  /\ active \in Nat
  /\ parked \in Nat
  /\ phase \in {"ready", "done"}
  /\ scenario \in {"none", "DoubleUnpark", "RespawnParked"}

CapacityConsistent ==
  active + parked = MaxWorkers

ActiveWithinBounds ==
  active <= MaxWorkers

=============================================================================
