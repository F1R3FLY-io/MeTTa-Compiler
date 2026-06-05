------------------------------ MODULE SATBSweepGate ------------------------------
(***************************************************************************)
(* E2 SATB sweep-gate discriminator.                                      *)
(*                                                                        *)
(* A deletion barrier is not enough if sweep can start while a deletion   *)
(* has already removed a snapshot-live E0 entry but has not yet shaded    *)
(* the removed pre-image. The E2 marker must either stop mutators for the *)
(* sweep or otherwise wait for in-flight barriers to finish before        *)
(* reclaim.                                                              *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UseSweepGate

VARIABLES
    entryInCache,
    entryShaded,
    entryMarked,
    entryFreed,
    snapshotLive,
    phase,
    deleteOpen,
    deleteSawMarking

vars ==
    <<entryInCache, entryShaded, entryMarked, entryFreed, snapshotLive,
      phase, deleteOpen, deleteSawMarking>>

TypeOK ==
    /\ entryInCache \in BOOLEAN
    /\ entryShaded \in BOOLEAN
    /\ entryMarked \in BOOLEAN
    /\ entryFreed \in BOOLEAN
    /\ snapshotLive \in BOOLEAN
    /\ phase \in {"marking", "swept"}
    /\ deleteOpen \in BOOLEAN
    /\ deleteSawMarking \in BOOLEAN

Init ==
    /\ entryInCache = TRUE
    /\ entryShaded = FALSE
    /\ entryMarked = FALSE
    /\ entryFreed = FALSE
    /\ snapshotLive = TRUE
    /\ phase = "marking"
    /\ deleteOpen = FALSE
    /\ deleteSawMarking = FALSE

BeginDelete ==
    /\ phase = "marking"
    /\ ~deleteOpen
    /\ entryInCache
    /\ entryInCache' = FALSE
    /\ deleteOpen' = TRUE
    /\ deleteSawMarking' = TRUE
    /\ UNCHANGED <<entryShaded, entryMarked, entryFreed, snapshotLive, phase>>

CommitDelete ==
    /\ deleteOpen
    /\ deleteOpen' = FALSE
    /\ deleteSawMarking' = FALSE
    /\ entryShaded' = IF deleteSawMarking THEN TRUE ELSE entryShaded
    /\ entryMarked' = IF deleteSawMarking THEN TRUE ELSE entryMarked
    /\ UNCHANGED <<entryInCache, entryFreed, snapshotLive, phase>>

MarkEntry ==
    /\ phase = "marking"
    /\ ~entryMarked
    /\ (entryInCache \/ entryShaded)
    /\ entryMarked' = TRUE
    /\ UNCHANGED <<entryInCache, entryShaded, entryFreed, snapshotLive,
                  phase, deleteOpen, deleteSawMarking>>

MarkComplete ==
    (entryInCache \/ entryShaded) => entryMarked

Sweep ==
    /\ phase = "marking"
    /\ MarkComplete
    /\ IF UseSweepGate THEN ~deleteOpen ELSE TRUE
    /\ phase' = "swept"
    /\ entryFreed' = ~entryMarked
    /\ UNCHANGED <<entryInCache, entryShaded, entryMarked, snapshotLive,
                  deleteOpen, deleteSawMarking>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ BeginDelete
    \/ CommitDelete
    \/ MarkEntry
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoSnapshotLiveFreed ==
    snapshotLive => ~entryFreed

=============================================================================
