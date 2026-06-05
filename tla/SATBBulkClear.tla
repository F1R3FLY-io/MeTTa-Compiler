------------------------------ MODULE SATBBulkClear ------------------------------
(***************************************************************************)
(* E2 SATB bulk-clear barrier discriminator.                              *)
(*                                                                        *)
(* Clearing a value-bearing E0 anchor cache removes every cached value     *)
(* from reach(E0). During an active snapshot-at-the-beginning mark, each   *)
(* removed pre-image must be shaded before the clear becomes invisible.    *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT ShadeBulkClear

VARIABLES
    entryInCache,
    entryShaded,
    entryMarked,
    entryFreed,
    phase

vars == <<entryInCache, entryShaded, entryMarked, entryFreed, phase>>

TypeOK ==
    /\ entryInCache \in BOOLEAN
    /\ entryShaded \in BOOLEAN
    /\ entryMarked \in BOOLEAN
    /\ entryFreed \in BOOLEAN
    /\ phase \in {"marking", "swept"}

Init ==
    /\ entryInCache = TRUE
    /\ entryShaded = FALSE
    /\ entryMarked = FALSE
    /\ entryFreed = FALSE
    /\ phase = "marking"

BulkClear ==
    /\ phase = "marking"
    /\ entryInCache
    /\ entryInCache' = FALSE
    /\ entryShaded' = IF ShadeBulkClear THEN TRUE ELSE entryShaded
    /\ UNCHANGED <<entryMarked, entryFreed, phase>>

MarkEntry ==
    /\ phase = "marking"
    /\ ~entryMarked
    /\ (entryInCache \/ entryShaded)
    /\ entryMarked' = TRUE
    /\ UNCHANGED <<entryInCache, entryShaded, entryFreed, phase>>

MarkComplete ==
    (entryInCache \/ entryShaded) => entryMarked

Sweep ==
    /\ phase = "marking"
    /\ MarkComplete
    /\ phase' = "swept"
    /\ entryFreed' = ~entryMarked
    /\ UNCHANGED <<entryInCache, entryShaded, entryMarked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ BulkClear
    \/ MarkEntry
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoSnapshotClearedEntryFreed ==
    ~entryFreed

=============================================================================
