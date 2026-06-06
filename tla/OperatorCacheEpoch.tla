--------------------------- MODULE OperatorCacheEpoch ---------------------------
(***************************************************************************)
(* Operator-cache sweep-epoch discriminator.                                *)
(*                                                                         *)
(* OPERATOR_CACHE is keyed partly by head.as_ptr().  After an index-GC      *)
(* sweep, another worker's thread-local cache can still contain entries     *)
(* keyed by recycled pointers.  The positive config checks gc_sweep_epoch   *)
(* before lookup and clears on mismatch.  The negative config performs the  *)
(* lookup directly after sweep and must stale-hit.                          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    EnsureBeforeLookup

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
    /\ phase \in {"start", "cached", "swept", "ensured", "looked"}
    /\ heapEpoch \in 0..1
    /\ localEpoch \in 0..1
    /\ cacheHasEntry \in BOOLEAN
    /\ entryEpoch \in 0..1
    /\ lookupReturned \in BOOLEAN
    /\ staleReturned \in BOOLEAN

Init ==
    /\ phase = "start"
    /\ heapEpoch = 0
    /\ localEpoch = 0
    /\ cacheHasEntry = FALSE
    /\ entryEpoch = 0
    /\ lookupReturned = FALSE
    /\ staleReturned = FALSE

SeedCache ==
    /\ phase = "start"
    /\ phase' = "cached"
    /\ cacheHasEntry' = TRUE
    /\ entryEpoch' = heapEpoch
    /\ UNCHANGED <<heapEpoch, localEpoch, lookupReturned, staleReturned>>

SweepOnOtherWorker ==
    /\ phase = "cached"
    /\ heapEpoch = 0
    /\ phase' = "swept"
    /\ heapEpoch' = 1
    /\ UNCHANGED <<localEpoch, cacheHasEntry, entryEpoch,
                  lookupReturned, staleReturned>>

EnsureCurrent ==
    /\ phase = "swept"
    /\ EnsureBeforeLookup
    /\ phase' = "ensured"
    /\ IF localEpoch # heapEpoch
       THEN /\ cacheHasEntry' = FALSE
            /\ localEpoch' = heapEpoch
       ELSE /\ cacheHasEntry' = cacheHasEntry
            /\ localEpoch' = localEpoch
    /\ UNCHANGED <<heapEpoch, entryEpoch, lookupReturned, staleReturned>>

Lookup ==
    /\ (phase = "ensured" \/ (~EnsureBeforeLookup /\ phase = "swept"))
    /\ phase' = "looked"
    /\ lookupReturned' = cacheHasEntry
    /\ staleReturned' = (cacheHasEntry /\ entryEpoch # heapEpoch)
    /\ UNCHANGED <<heapEpoch, localEpoch, cacheHasEntry, entryEpoch>>

Done ==
    /\ phase = "looked"
    /\ UNCHANGED vars

Next ==
    \/ SeedCache
    \/ SweepOnOtherWorker
    \/ EnsureCurrent
    \/ Lookup
    \/ Done

Spec == Init /\ [][Next]_vars

NoStaleOperatorCacheHit ==
    ~staleReturned

=============================================================================
