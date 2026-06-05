------------------------------ MODULE SATBPhaseGate ------------------------------
(***************************************************************************)
(* E2 SATB phase-gate discriminator.                                      *)
(*                                                                        *)
(* A value-bearing E0 cache deletion that observes "not marking" is safe  *)
(* only if marker start cannot occur until that deletion publishes. The   *)
(* implementation enforces this with a read/write phase gate: deletion    *)
(* holds the read side; marker start holds the write side while arming the *)
(* SATB flag.                                                            *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT UsePhaseGate

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
    /\ phase \in {"idle", "marking", "swept"}
    /\ deleteOpen \in BOOLEAN
    /\ deleteSawMarking \in BOOLEAN

Init ==
    /\ entryInCache = TRUE
    /\ entryShaded = FALSE
    /\ entryMarked = FALSE
    /\ entryFreed = FALSE
    /\ snapshotLive = FALSE
    /\ phase = "idle"
    /\ deleteOpen = FALSE
    /\ deleteSawMarking = FALSE

BeginDelete ==
    /\ ~deleteOpen
    /\ entryInCache
    /\ phase # "swept"
    /\ deleteOpen' = TRUE
    /\ deleteSawMarking' = (phase = "marking")
    /\ UNCHANGED <<entryInCache, entryShaded, entryMarked, entryFreed,
                  snapshotLive, phase>>

StartMark ==
    /\ phase = "idle"
    /\ IF UsePhaseGate THEN ~deleteOpen ELSE TRUE
    /\ phase' = "marking"
    /\ snapshotLive' = entryInCache
    /\ UNCHANGED <<entryInCache, entryShaded, entryMarked, entryFreed,
                  deleteOpen, deleteSawMarking>>

CommitDelete ==
    /\ deleteOpen
    /\ entryInCache
    /\ entryInCache' = FALSE
    /\ entryShaded' = IF deleteSawMarking THEN TRUE ELSE entryShaded
    /\ deleteOpen' = FALSE
    /\ deleteSawMarking' = FALSE
    /\ UNCHANGED <<entryMarked, entryFreed, snapshotLive, phase>>

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
    /\ phase' = "swept"
    /\ entryFreed' = ~entryMarked
    /\ UNCHANGED <<entryInCache, entryShaded, entryMarked, snapshotLive,
                  deleteOpen, deleteSawMarking>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ BeginDelete
    \/ StartMark
    \/ CommitDelete
    \/ MarkEntry
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoSnapshotLiveFreed ==
    snapshotLive => ~entryFreed

=============================================================================
