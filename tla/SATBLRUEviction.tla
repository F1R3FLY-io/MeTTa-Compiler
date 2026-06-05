----------------------------- MODULE SATBLRUEviction -----------------------------
(***************************************************************************)
(* E2 SATB LRU capacity-eviction barrier discriminator.                   *)
(*                                                                        *)
(* For lru-style caches, a capacity insertion can evict an unrelated       *)
(* snapshot-live entry. A barrier that only shades same-key overwrite      *)
(* returns does not see that victim. The correct barrier shades the actual *)
(* capacity-evicted entry.                                                *)
(***************************************************************************)
EXTENDS Naturals

CONSTANT ShadeCapacityEviction

VARIABLES
    victimInCache,
    victimShaded,
    victimMarked,
    victimFreed,
    phase

vars == <<victimInCache, victimShaded, victimMarked, victimFreed, phase>>

TypeOK ==
    /\ victimInCache \in BOOLEAN
    /\ victimShaded \in BOOLEAN
    /\ victimMarked \in BOOLEAN
    /\ victimFreed \in BOOLEAN
    /\ phase \in {"marking", "swept"}

Init ==
    /\ victimInCache = TRUE
    /\ victimShaded = FALSE
    /\ victimMarked = FALSE
    /\ victimFreed = FALSE
    /\ phase = "marking"

CapacityInsert ==
    /\ phase = "marking"
    /\ victimInCache
    /\ victimInCache' = FALSE
    /\ victimShaded' = IF ShadeCapacityEviction THEN TRUE ELSE victimShaded
    /\ UNCHANGED <<victimMarked, victimFreed, phase>>

MarkVictim ==
    /\ phase = "marking"
    /\ ~victimMarked
    /\ (victimInCache \/ victimShaded)
    /\ victimMarked' = TRUE
    /\ UNCHANGED <<victimInCache, victimShaded, victimFreed, phase>>

MarkComplete ==
    (victimInCache \/ victimShaded) => victimMarked

Sweep ==
    /\ phase = "marking"
    /\ MarkComplete
    /\ phase' = "swept"
    /\ victimFreed' = ~victimMarked
    /\ UNCHANGED <<victimInCache, victimShaded, victimMarked>>

Done ==
    /\ phase = "swept"
    /\ UNCHANGED vars

Next ==
    \/ CapacityInsert
    \/ MarkVictim
    \/ Sweep
    \/ Done

Spec == Init /\ [][Next]_vars

NoSnapshotVictimFreed ==
    ~victimFreed

=============================================================================
