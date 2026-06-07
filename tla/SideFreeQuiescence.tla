---- MODULE SideFreeQuiescence ----

CONSTANTS IsQuiescent, FreeRequiresQuiescence, ClearShadowBeforeUse

VARIABLES sideFreed, shadowCleared, futureDeref, phase

StackLaunderLive == ~IsQuiescent
MayFreeSide == IsQuiescent \/ ~FreeRequiresQuiescence

Init ==
  /\ sideFreed = FALSE
  /\ shadowCleared = FALSE
  /\ futureDeref = FALSE
  /\ phase = "start"

FreeSide ==
  /\ phase = "start"
  /\ MayFreeSide
  /\ sideFreed' = TRUE
  /\ shadowCleared' = shadowCleared
  /\ futureDeref' = futureDeref
  /\ phase' = "freed"

DeferSideFree ==
  /\ phase = "start"
  /\ ~MayFreeSide
  /\ sideFreed' = FALSE
  /\ shadowCleared' = shadowCleared
  /\ futureDeref' = futureDeref
  /\ phase' = "done"

ClearShadow ==
  /\ phase = "freed"
  /\ sideFreed
  /\ shadowCleared' = ClearShadowBeforeUse
  /\ sideFreed' = sideFreed
  /\ futureDeref' = futureDeref
  /\ phase' = "shadow_checked"

FutureUse ==
  /\ phase = "shadow_checked"
  /\ futureDeref' = TRUE
  /\ sideFreed' = sideFreed
  /\ shadowCleared' = shadowCleared
  /\ phase' = "done"

Done ==
  /\ phase = "done"
  /\ UNCHANGED <<sideFreed, shadowCleared, futureDeref, phase>>

Next == FreeSide \/ DeferSideFree \/ ClearShadow \/ FutureUse \/ Done

Spec == Init /\ [][Next]_<<sideFreed, shadowCleared, futureDeref, phase>>

NoDanglingSideUse ==
  ~(sideFreed /\ futureDeref /\ (StackLaunderLive \/ ~shadowCleared))

====
