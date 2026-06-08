---- MODULE NonRendezvousFanoutGate ----

CONSTANTS FanoutEnabled, UseMidloopFanoutGate

VARIABLES phase, midloopSwept, quiescenceSwept

Init ==
  /\ phase = "ready"
  /\ midloopSwept = FALSE
  /\ quiescenceSwept = FALSE

MidloopNonRendezvousSweepEnabled ==
  UseMidloopFanoutGate => ~FanoutEnabled

MidloopSweep ==
  /\ phase = "ready"
  /\ MidloopNonRendezvousSweepEnabled
  /\ midloopSwept' = TRUE
  /\ quiescenceSwept' = quiescenceSwept
  /\ phase' = "midloop_swept"

QuiescenceSweep ==
  /\ phase = "ready"
  /\ quiescenceSwept' = TRUE
  /\ midloopSwept' = midloopSwept
  /\ phase' = "quiescence_swept"

Done ==
  /\ UNCHANGED <<phase, midloopSwept, quiescenceSwept>>

Next == MidloopSweep \/ QuiescenceSweep \/ Done

Spec == Init /\ [][Next]_<<phase, midloopSwept, quiescenceSwept>>

TypeOK ==
  /\ FanoutEnabled \in BOOLEAN
  /\ UseMidloopFanoutGate \in BOOLEAN
  /\ phase \in {"ready", "midloop_swept", "quiescence_swept"}
  /\ midloopSwept \in BOOLEAN
  /\ quiescenceSwept \in BOOLEAN

NoMidloopNonRendezvousUnderFanout ==
  midloopSwept => ~FanoutEnabled

QuiescenceSweepMayCoexistWithFanout ==
  quiescenceSwept => TRUE

====
