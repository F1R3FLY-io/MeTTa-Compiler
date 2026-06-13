--------------------------- MODULE WorkPoolOverflowCap ---------------------------
EXTENDS Naturals

CONSTANTS InitialLive, Requested, MaxOverflow, EnforceCap

VARIABLES live, phase

Vars == <<live, phase>>

Min(a, b) == IF a <= b THEN a ELSE b

Capacity(l, max) == IF max >= l THEN max - l ELSE 0

SpawnQuota(req, l, max) == Min(req, Capacity(l, max))

Spawned(l) ==
  IF EnforceCap
  THEN SpawnQuota(Requested, l, MaxOverflow)
  ELSE Requested

Init ==
  /\ live = InitialLive
  /\ phase = "ready"

Spawn ==
  /\ phase = "ready"
  /\ live' = live + Spawned(live)
  /\ phase' = "done"

Done ==
  /\ phase = "done"
  /\ UNCHANGED Vars

Next == Spawn \/ Done

Spec == Init /\ [][Next]_Vars

LiveWithinCap == live <= MaxOverflow

QuotaWithinCapacity == SpawnQuota(Requested, live, MaxOverflow) <= Capacity(live, MaxOverflow)

================================================================================
