------------------------- MODULE EpochProtectedCaches -------------------------
(***************************************************************************)
(* Epoch-protected Addr-cache discriminator.                                *)
(*                                                                         *)
(* A reclaiming CESK index-GC sweep bumps gc_sweep_epoch after it frees     *)
(* slots. Worker-local caches that can key by, or return values containing, *)
(* a reusable Addr must check that epoch before lookup. The positive config *)
(* checks every modeled cache; each negative config omits one cache and must *)
(* stale-hit after the sweep.                                               *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    CheckedCaches

Caches ==
    {"valueHash", "morkGround", "hashCons", "evalMemo", "matchResult", "operator"}

VARIABLES
    phase,
    heapEpoch,
    localEpoch,
    cacheHasEntry,
    entryEpoch,
    lookupReturned,
    staleReturned

vars ==
    <<phase, heapEpoch, localEpoch, cacheHasEntry,
      entryEpoch, lookupReturned, staleReturned>>

TypeOK ==
    /\ CheckedCaches \subseteq Caches
    /\ phase \in {"start", "cached", "swept", "ensured", "looked"}
    /\ heapEpoch \in 0..1
    /\ localEpoch \in [Caches -> 0..1]
    /\ cacheHasEntry \in [Caches -> BOOLEAN]
    /\ entryEpoch \in [Caches -> 0..1]
    /\ lookupReturned \in BOOLEAN
    /\ staleReturned \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ heapEpoch = 0
    /\ localEpoch = [c \in Caches |-> 0]
    /\ cacheHasEntry = [c \in Caches |-> FALSE]
    /\ entryEpoch = [c \in Caches |-> 0]
    /\ lookupReturned = FALSE
    /\ staleReturned = FALSE

SeedCaches ==
    /\ phase = "start"
    /\ phase' = "cached"
    /\ cacheHasEntry' = [c \in Caches |-> TRUE]
    /\ entryEpoch' = [c \in Caches |-> heapEpoch]
    /\ UNCHANGED <<heapEpoch, localEpoch, lookupReturned, staleReturned>>

ReclaimingSweep ==
    /\ phase = "cached"
    /\ heapEpoch = 0
    /\ phase' = "swept"
    /\ heapEpoch' = 1
    /\ UNCHANGED <<localEpoch, cacheHasEntry, entryEpoch,
                  lookupReturned, staleReturned>>

EnsureCaches ==
    /\ phase = "swept"
    /\ phase' = "ensured"
    /\ localEpoch' =
        [c \in Caches |->
            IF c \in CheckedCaches /\ localEpoch[c] # heapEpoch
            THEN heapEpoch
            ELSE localEpoch[c]]
    /\ cacheHasEntry' =
        [c \in Caches |->
            IF c \in CheckedCaches /\ localEpoch[c] # heapEpoch
            THEN FALSE
            ELSE cacheHasEntry[c]]
    /\ UNCHANGED <<heapEpoch, entryEpoch, lookupReturned, staleReturned>>

Lookup ==
    /\ phase = "ensured"
    /\ phase' = "looked"
    /\ lookupReturned' = \E c \in Caches : cacheHasEntry[c]
    /\ staleReturned' =
        \E c \in Caches : cacheHasEntry[c] /\ entryEpoch[c] # heapEpoch
    /\ UNCHANGED <<heapEpoch, localEpoch, cacheHasEntry, entryEpoch>>

Done ==
    /\ phase = "looked"
    /\ UNCHANGED vars

Next ==
    \/ SeedCaches
    \/ ReclaimingSweep
    \/ EnsureCaches
    \/ Lookup
    \/ Done

Spec == Init /\ [][Next]_vars

NoStaleAddrCacheHit ==
    ~staleReturned

=============================================================================
