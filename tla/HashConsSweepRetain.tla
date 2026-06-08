---- MODULE HashConsSweepRetain ----

CONSTANTS Mode, MarkedEntry, YoungEntry, RetainDeadMajor, RetainDeadYoung,
          ValidateLookup, AllocatedAfterSweep, SidePresentAfterSweep

VARIABLES retained, freed, returned, phase

Major == Mode = "major"
Minor == Mode = "minor"

RetainPredicate ==
  IF Major THEN
    MarkedEntry \/ RetainDeadMajor
  ELSE
    (~YoungEntry) \/ MarkedEntry \/ RetainDeadYoung

FreedBySweep ==
  IF Major THEN
    ~MarkedEntry
  ELSE
    YoungEntry /\ ~MarkedEntry

LookupValid ==
  ~ValidateLookup \/ (AllocatedAfterSweep /\ SidePresentAfterSweep)

Init ==
  /\ retained = FALSE
  /\ freed = FALSE
  /\ returned = FALSE
  /\ phase = "start"

Retain ==
  /\ phase = "start"
  /\ retained' = RetainPredicate
  /\ freed' = freed
  /\ returned' = returned
  /\ phase' = "retained"

Sweep ==
  /\ phase = "retained"
  /\ freed' = FreedBySweep
  /\ retained' = retained
  /\ returned' = returned
  /\ phase' = "swept"

Lookup ==
  /\ phase = "swept"
  /\ returned' = (retained /\ LookupValid)
  /\ retained' = retained
  /\ freed' = freed
  /\ phase' = "done"

Done ==
  /\ phase = "done"
  /\ UNCHANGED <<retained, freed, returned, phase>>

Next == Retain \/ Sweep \/ Lookup \/ Done

Spec == Init /\ [][Next]_<<retained, freed, returned, phase>>

NoReturnedFreed == ~(returned /\ freed)

====
