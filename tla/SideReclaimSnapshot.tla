---- MODULE SideReclaimSnapshot ----

EXTENDS FiniteSets

CONSTANTS SnapshotBeforeReuse, DropPendingOnReset, SnapshotOnlyOnFreeOwner,
          DrainRequiresFullMark, DropMarkedLiveOwner

VARIABLES pending, nodeSide, liveSides, freed, fullMarkDone, phase

SIDES == {"old", "new"}

Init ==
  /\ pending = {}
  /\ nodeSide = "old"
  /\ liveSides = {"old"}
  /\ freed = {}
  /\ fullMarkDone = FALSE
  /\ phase = "start"

Reclaim ==
  /\ phase = "start"
  /\ liveSides' = {}
  /\ pending' = IF SnapshotBeforeReuse THEN {nodeSide} ELSE pending
  /\ fullMarkDone' = TRUE
  /\ UNCHANGED <<nodeSide, freed>>
  /\ phase' = "reclaimed"

MinorMissReclaim ==
  /\ phase = "start"
  \* Models a young-only minor that reports a side owner without the authority of
  \* a full mark. If Drain is allowed here, it can free a still-live side.
  /\ liveSides' = {"old"}
  /\ pending' = {nodeSide}
  /\ fullMarkDone' = FALSE
  /\ UNCHANGED <<nodeSide, freed>>
  /\ phase' = "minor_missed"

ReuseNodeSlot ==
  /\ phase = "reclaimed"
  /\ nodeSide' = "new"
  /\ liveSides' = {"new"}
  /\ pending' = IF SnapshotBeforeReuse THEN pending ELSE {nodeSide'}
  /\ UNCHANGED <<freed, fullMarkDone>>
  /\ phase' = "reused"

DuplicateReportAfterReuse ==
  /\ phase = "reused"
  \* Models a second sweep/report for a slot whose free-bit ownership was not
  \* newly acquired. If snapshots are not gated on that ownership edge, the
  \* collector snapshots the reused live side and Drain frees it.
  /\ pending' = IF SnapshotOnlyOnFreeOwner THEN pending ELSE pending \cup {nodeSide}
  /\ UNCHANGED <<nodeSide, liveSides, freed, fullMarkDone>>
  /\ phase' = "duplicate"

ResetSegment ==
  /\ phase = "reclaimed"
  /\ nodeSide' = "old"
  /\ liveSides' = {"old"}
  /\ pending' = IF DropPendingOnReset THEN {} ELSE pending
  /\ UNCHANGED <<freed, fullMarkDone>>
  /\ phase' = "reset"

Drain ==
  /\ phase \in {"reused", "reset", "duplicate", "minor_missed", "full_after_minor"}
  /\ DrainRequiresFullMark => fullMarkDone
  /\ freed' = pending
  /\ UNCHANGED <<pending, nodeSide, liveSides, fullMarkDone>>
  /\ phase' = "done"

DeferMinorDrain ==
  /\ phase = "minor_missed"
  /\ DrainRequiresFullMark
	  /\ UNCHANGED <<pending, nodeSide, liveSides, freed, fullMarkDone, phase>>

FullMarkAfterMinorMiss ==
  /\ phase = "minor_missed"
  \* A later full mark proves whether the pending snapshot's owner is still live
  \* and still owns the same side slot. If that marked-live owner snapshot is not
  \* dropped before draining, Drain frees a live side.
  /\ fullMarkDone' = TRUE
  /\ pending' = IF DropMarkedLiveOwner THEN pending \ liveSides ELSE pending
  /\ UNCHANGED <<nodeSide, liveSides, freed>>
  /\ phase' = "full_after_minor"

Done ==
  /\ phase = "done"
  /\ UNCHANGED <<pending, nodeSide, liveSides, freed, fullMarkDone, phase>>

Next == Reclaim \/ MinorMissReclaim \/ ReuseNodeSlot \/ DuplicateReportAfterReuse \/
        ResetSegment \/ Drain \/ DeferMinorDrain \/ FullMarkAfterMinorMiss \/ Done

Spec == Init /\ [][Next]_<<pending, nodeSide, liveSides, freed, fullMarkDone, phase>>

TypeOK ==
  /\ pending \subseteq SIDES
  /\ nodeSide \in SIDES
  /\ liveSides \subseteq SIDES
  /\ freed \subseteq SIDES
  /\ fullMarkDone \in BOOLEAN
  /\ phase \in {"start", "reclaimed", "reused", "duplicate", "minor_missed",
                "full_after_minor", "reset", "done"}

NoLiveSideFreed == freed \cap liveSides = {}

====
