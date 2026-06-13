-------------------------- MODULE CounterFlushExclusion --------------------------
EXTENDS TLC

CONSTANTS UseFlushLock

VARIABLES sync, gc, owner, gcFlag

Vars == <<sync, gc, owner, gcFlag>>

Init ==
  /\ sync = "idle"
  /\ gc = "idle"
  /\ owner = "none"
  /\ gcFlag = FALSE

SyncSkipOnFlag ==
  /\ sync = "idle"
  /\ gcFlag
  /\ sync' = "done"
  /\ UNCHANGED <<gc, owner, gcFlag>>

SyncCheck ==
  /\ sync = "idle"
  /\ ~gcFlag
  /\ sync' = "checked"
  /\ UNCHANGED <<gc, owner, gcFlag>>

SyncLock ==
  /\ sync = "checked"
  /\ IF UseFlushLock
     THEN /\ owner = "none"
          /\ owner' = "sync"
     ELSE /\ owner' = owner
  /\ sync' = "locked"
  /\ UNCHANGED <<gc, gcFlag>>

SyncRecheckSkip ==
  /\ sync = "locked"
  /\ gcFlag
  /\ sync' = "done"
  /\ IF UseFlushLock THEN owner' = "none" ELSE owner' = owner
  /\ UNCHANGED <<gc, gcFlag>>

SyncScan ==
  /\ sync = "locked"
  /\ ~gcFlag
  /\ sync' = "scanning"
  /\ UNCHANGED <<gc, owner, gcFlag>>

SyncFinish ==
  /\ sync = "scanning"
  /\ sync' = "done"
  /\ IF UseFlushLock THEN owner' = "none" ELSE owner' = owner
  /\ UNCHANGED <<gc, gcFlag>>

GcStart ==
  /\ gc = "idle"
  /\ gc' = "flagged"
  /\ gcFlag' = TRUE
  /\ UNCHANGED <<sync, owner>>

GcLock ==
  /\ gc = "flagged"
  /\ IF UseFlushLock
     THEN /\ owner = "none"
          /\ owner' = "gc"
     ELSE /\ owner' = owner
  /\ gc' = "locked"
  /\ UNCHANGED <<sync, gcFlag>>

GcFree ==
  /\ gc = "locked"
  /\ gc' = "freeing"
  /\ UNCHANGED <<sync, owner, gcFlag>>

GcDone ==
  /\ gc = "freeing"
  /\ gc' = "done"
  /\ gcFlag' = FALSE
  /\ IF UseFlushLock THEN owner' = "none" ELSE owner' = owner
  /\ UNCHANGED sync

StutterDone ==
  /\ sync = "done"
  /\ gc = "done"
  /\ UNCHANGED Vars

Next ==
  \/ SyncSkipOnFlag
  \/ SyncCheck
  \/ SyncLock
  \/ SyncRecheckSkip
  \/ SyncScan
  \/ SyncFinish
  \/ GcStart
  \/ GcLock
  \/ GcFree
  \/ GcDone
  \/ StutterDone

Spec == Init /\ [][Next]_Vars

NoCounterSyncFreeOverlap ==
  ~(sync = "scanning" /\ gc = "freeing")

LockOwnerExclusive ==
  owner \in {"none", "sync", "gc"}

===============================================================================
