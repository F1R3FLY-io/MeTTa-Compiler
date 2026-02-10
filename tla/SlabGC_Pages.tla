--------------------------- MODULE SlabGC_Pages ---------------------------
(*
 * TLA+ Model of Slab GC with Page-Level State Tracking
 *
 * Extends SlabGC_Reactive with page-level live_count tracking and
 * page release logic. This model captures BUG 4 (page-level UAF):
 *
 * BUG 4 (Page-Level Use-After-Free) — FIXED by incrementing page live_count
 *   on free-list re-allocation:
 *
 *   The original code incremented page.live_count only during bump allocation
 *   (page.bump_alloc()). When a slot was recycled from the free list, the
 *   allocator-level live_count was incremented but the PAGE's live_count
 *   was not. After GC freed enough slots, the page's live_count reached 0
 *   and release_empty_pages() dropped the page — even though free-list-
 *   recycled slots in that page still held live values.
 *
 * This model adds:
 *   - slotPage[s]:       which page contains each slot
 *   - pageLiveCount[p]:  page-level live count
 *   - pageReleased[p]:   TRUE if page has been released
 *   - GcReleasePages:    action that releases pages with pageLiveCount == 0
 *   - PageSafetyInvariant: allocated slots must not be in released pages
 *
 * CONFIGURATION:
 *   MaxSlots = 6, MaxPages = 2, SlotsPerPage = 3, MaxRoots = 3, MaxExprs = 4
 *
 * With the bug (no page live_count increment on free-list reuse),
 * PageSafetyInvariant is VIOLATED. With the fix, it HOLDS.
 *)

EXTENDS Integers, FiniteSets

CONSTANTS
    MaxSlots,       \* Total allocatable value slots (e.g. 6)
    MaxPages,       \* Number of pages (e.g. 2)
    SlotsPerPage,   \* Slots per page (e.g. 3)
    MaxRoots,       \* Maximum root set size (e.g. 3)
    MaxExprs        \* Maximum expressions to evaluate before stopping

Slots == 1..MaxSlots
Pages == 1..MaxPages

\* Map each slot to its page: slot s is on page ((s-1) \div SlotsPerPage) + 1
SlotToPage(s) == ((s - 1) \div SlotsPerPage) + 1

VARIABLES
    (*---------------------------------------------------------------*)
    (* Allocator State — EVAL THREAD ONLY                            *)
    (*---------------------------------------------------------------*)
    slotState,      \* [Slots -> {"free", "alloc", "freed"}]
    bumpPtr,        \* Next slot to bump-allocate (1..MaxSlots+1)
    freeSet,        \* SUBSET(Slots) — freed slots available for reuse
    roots,          \* SUBSET(Slots) — current GC roots
    epoch,          \* Nat — monotonic epoch counter for free-list reuse
    slotEpoch,      \* [Slots -> Nat] — epoch of last free-list alloc

    (*---------------------------------------------------------------*)
    (* Page State — EVAL THREAD ONLY                                 *)
    (*---------------------------------------------------------------*)
    pageLiveCount,  \* [Pages -> Nat] — page-level live count
    pageReleased,   \* [Pages -> BOOLEAN] — TRUE if page has been released

    (*---------------------------------------------------------------*)
    (* GC Snapshot Channel                                           *)
    (*---------------------------------------------------------------*)
    hasGcRequest,
    snapSlotState,
    snapBumpPtr,
    snapFreeSet,
    snapRoots,
    snapEpoch,

    (*---------------------------------------------------------------*)
    (* GC Response Channel                                           *)
    (*---------------------------------------------------------------*)
    hasGcResponse,
    respDeadSet,
    respLiveCount,
    respEpoch,

    (*---------------------------------------------------------------*)
    (* Eval Thread State                                              *)
    (*---------------------------------------------------------------*)
    evalPhase,      \* "eval" | "between" | "waiting"
    exprCount,

    (*---------------------------------------------------------------*)
    (* GC Thread State                                                *)
    (*---------------------------------------------------------------*)
    gcPhase,        \* "idle" | "marking" | "sweeping"
    gcMarked,

    (*---------------------------------------------------------------*)
    (* Threshold State                                                *)
    (*---------------------------------------------------------------*)
    softThreshold,
    hardThreshold,
    gcCalibrated,
    gcInFlight

vars == <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
          pageLiveCount, pageReleased,
          hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
          snapRoots, snapEpoch,
          hasGcResponse, respDeadSet, respLiveCount, respEpoch,
          evalPhase, exprCount,
          gcPhase, gcMarked,
          softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

pageVars == <<pageLiveCount, pageReleased>>

(*=======================================================================*)
(* Helper Operators                                                       *)
(*=======================================================================*)

CommittedSlots == bumpPtr - 1

CanAllocate ==
    \/ freeSet /= {}
    \/ bumpPtr <= MaxSlots

(*=======================================================================*)
(* Initial State                                                          *)
(*=======================================================================*)

Init ==
    /\ slotState = [s \in Slots |-> "free"]
    /\ bumpPtr = 1
    /\ freeSet = {}
    /\ roots = {}
    /\ epoch = 0
    /\ slotEpoch = [s \in Slots |-> 0]
    /\ pageLiveCount = [p \in Pages |-> 0]
    /\ pageReleased = [p \in Pages |-> FALSE]
    /\ hasGcRequest = FALSE
    /\ snapSlotState = [s \in Slots |-> "free"]
    /\ snapBumpPtr = 1
    /\ snapFreeSet = {}
    /\ snapRoots = {}
    /\ snapEpoch = 0
    /\ hasGcResponse = FALSE
    /\ respDeadSet = {}
    /\ respLiveCount = 0
    /\ respEpoch = 0
    /\ evalPhase = "eval"
    /\ exprCount = 0
    /\ gcPhase = "idle"
    /\ gcMarked = {}
    /\ softThreshold = 3
    /\ hardThreshold = 5
    /\ gcCalibrated = FALSE
    /\ gcInFlight = FALSE

(*=======================================================================*)
(* Eval Thread Actions                                                    *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* AllocateSlot: allocate from free list or bump.                         *)
(* KEY FIX: free-list reuse increments pageLiveCount.                     *)
(*-----------------------------------------------------------------------*)
AllocateSlot ==
    /\ evalPhase = "eval"
    /\ Cardinality(roots) < MaxRoots
    /\ CanAllocate
    /\ IF freeSet /= {}
       THEN \E s \in freeSet :
               /\ ~pageReleased[SlotToPage(s)]  \* Can't alloc from released page
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ freeSet' = freeSet \ {s}
               /\ bumpPtr' = bumpPtr
               /\ roots' = roots \union {s}
               /\ epoch' = epoch + 1
               /\ slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
               \* FIX: increment page live count on free-list reuse
               /\ pageLiveCount' = [pageLiveCount EXCEPT
                    ![SlotToPage(s)] = pageLiveCount[SlotToPage(s)] + 1]
               /\ pageReleased' = pageReleased
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
               /\ epoch' = epoch
               /\ slotEpoch' = slotEpoch
               \* Bump alloc: page live count incremented
               /\ pageLiveCount' = [pageLiveCount EXCEPT
                    ![SlotToPage(s)] = pageLiveCount[SlotToPage(s)] + 1]
               /\ pageReleased' = pageReleased
    /\ UNCHANGED <<hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* DropRoot: root becomes unreachable.                                    *)
(*-----------------------------------------------------------------------*)
DropRoot ==
    /\ evalPhase = "eval"
    /\ roots /= {}
    /\ \E s \in roots :
        /\ roots' = roots \ {s}
        /\ UNCHANGED <<slotState, bumpPtr, freeSet, epoch, slotEpoch,
                       pageVars,
                       hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                       snapRoots, snapEpoch,
                       hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                       evalPhase, exprCount,
                       gcPhase, gcMarked,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* BeginBetweenExpressions                                                *)
(*-----------------------------------------------------------------------*)
BeginBetweenExpressions ==
    /\ evalPhase = "eval"
    /\ exprCount < MaxExprs
    /\ evalPhase' = "between"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ProcessGcResponse: free dead slots, decrement page live counts,        *)
(* then release empty pages.                                              *)
(*-----------------------------------------------------------------------*)
ProcessGcResponse ==
    /\ evalPhase = "between"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
           liveCount == respLiveCount
       IN /\ slotState' = [s \in Slots |->
                IF s \in safeDead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union safeDead
          \* Decrement page live counts for freed slots
          /\ pageLiveCount' = [p \in Pages |->
                pageLiveCount[p] -
                    Cardinality({s \in safeDead : SlotToPage(s) = p})]
          \* Release pages whose live count drops to 0 (and had slots bumped)
          /\ pageReleased' = [p \in Pages |->
                IF pageReleased[p] THEN TRUE
                ELSE IF pageLiveCount[p] -
                         Cardinality({s \in safeDead : SlotToPage(s) = p}) = 0
                      /\ \E s \in Slots : SlotToPage(s) = p /\ s < bumpPtr
                    THEN TRUE
                    ELSE FALSE]
          /\ softThreshold' = IF liveCount * 2 > 2 THEN liveCount * 2 ELSE 2
          /\ hardThreshold' = IF liveCount * 4 > 4 THEN liveCount * 4 ELSE 4
          /\ gcCalibrated' = TRUE
          /\ gcInFlight' = FALSE
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
          /\ respEpoch' = 0
    /\ UNCHANGED <<bumpPtr, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* RequestGcAsync                                                         *)
(*-----------------------------------------------------------------------*)
RequestGcAsync ==
    /\ evalPhase = "between"
    /\ CommittedSlots > softThreshold
    /\ ~gcInFlight
    /\ ~hasGcRequest
    /\ hasGcRequest' = TRUE
    /\ snapSlotState' = slotState
    /\ snapBumpPtr' = bumpPtr
    /\ snapFreeSet' = freeSet
    /\ snapRoots' = roots
    /\ snapEpoch' = epoch
    /\ gcInFlight' = TRUE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* RequestGcAndWait                                                       *)
(*-----------------------------------------------------------------------*)
RequestGcAndWait ==
    /\ evalPhase = "between"
    /\ CommittedSlots > hardThreshold
    /\ gcCalibrated
    /\ ~gcInFlight
    /\ ~hasGcRequest
    /\ hasGcRequest' = TRUE
    /\ snapSlotState' = slotState
    /\ snapBumpPtr' = bumpPtr
    /\ snapFreeSet' = freeSet
    /\ snapRoots' = roots
    /\ snapEpoch' = epoch
    /\ gcInFlight' = TRUE
    /\ evalPhase' = "waiting"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* WaitForGcInFlight                                                      *)
(*-----------------------------------------------------------------------*)
WaitForGcInFlight ==
    /\ evalPhase = "between"
    /\ CommittedSlots > hardThreshold
    /\ gcCalibrated
    /\ gcInFlight
    /\ evalPhase' = "waiting"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ResumeAfterWait                                                        *)
(*-----------------------------------------------------------------------*)
ResumeAfterWait ==
    /\ evalPhase = "waiting"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
           liveCount == respLiveCount
       IN /\ slotState' = [s \in Slots |->
                IF s \in safeDead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union safeDead
          /\ pageLiveCount' = [p \in Pages |->
                pageLiveCount[p] -
                    Cardinality({s \in safeDead : SlotToPage(s) = p})]
          /\ pageReleased' = [p \in Pages |->
                IF pageReleased[p] THEN TRUE
                ELSE IF pageLiveCount[p] -
                         Cardinality({s \in safeDead : SlotToPage(s) = p}) = 0
                      /\ \E s \in Slots : SlotToPage(s) = p /\ s < bumpPtr
                    THEN TRUE
                    ELSE FALSE]
          /\ softThreshold' = IF liveCount * 2 > 2 THEN liveCount * 2 ELSE 2
          /\ hardThreshold' = IF liveCount * 4 > 4 THEN liveCount * 4 ELSE 4
          /\ gcCalibrated' = TRUE
          /\ gcInFlight' = FALSE
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
          /\ respEpoch' = 0
    /\ evalPhase' = "eval"
    /\ exprCount' = exprCount + 1
    /\ UNCHANGED <<bumpPtr, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* ContinueEval                                                           *)
(*-----------------------------------------------------------------------*)
ContinueEval ==
    /\ evalPhase = "between"
    /\ ~hasGcResponse
    /\ \/ CommittedSlots <= softThreshold
       \/ gcInFlight
    /\ ~(CommittedSlots > hardThreshold /\ gcCalibrated)
    /\ evalPhase' = "eval"
    /\ exprCount' = exprCount + 1
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* GC Thread Actions                                                      *)
(*=======================================================================*)

GcReceiveRequest ==
    /\ gcPhase = "idle"
    /\ hasGcRequest
    /\ gcMarked' = {}
    /\ gcPhase' = "marking"
    /\ hasGcRequest' = FALSE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   snapSlotState, snapBumpPtr, snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

GcMarkStep ==
    /\ gcPhase = "marking"
    /\ \E s \in snapRoots :
        /\ s \notin gcMarked
        /\ snapSlotState[s] = "alloc"
        /\ gcMarked' = gcMarked \union {s}
        /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                       pageVars,
                       hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                       snapRoots, snapEpoch,
                       hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                       evalPhase, exprCount,
                       gcPhase,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

GcMarkComplete ==
    /\ gcPhase = "marking"
    /\ \A s \in snapRoots : (snapSlotState[s] = "alloc") => (s \in gcMarked)
    /\ gcPhase' = "sweeping"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

GcSweep ==
    /\ gcPhase = "sweeping"
    /\ LET allCommitted == {s \in Slots : s < snapBumpPtr}
           deadSet == {s \in allCommitted :
                        /\ snapSlotState[s] = "alloc"
                        /\ s \notin gcMarked
                        /\ s \notin snapFreeSet}
           liveCount == Cardinality({s \in allCommitted :
                        /\ snapSlotState[s] = "alloc"
                        /\ s \in gcMarked})
       IN /\ hasGcResponse' = TRUE
          /\ respDeadSet' = deadSet
          /\ respLiveCount' = liveCount
          /\ respEpoch' = snapEpoch
          /\ gcMarked' = {}
          /\ gcPhase' = "idle"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   evalPhase, exprCount,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Concurrent Eval Actions During GC                                      *)
(*=======================================================================*)

EvalAllocDuringGc ==
    /\ evalPhase = "eval"
    /\ gcPhase \in {"marking", "sweeping"}
    /\ Cardinality(roots) < MaxRoots
    /\ CanAllocate
    /\ IF freeSet /= {}
       THEN \E s \in freeSet :
               /\ ~pageReleased[SlotToPage(s)]
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ freeSet' = freeSet \ {s}
               /\ bumpPtr' = bumpPtr
               /\ roots' = roots \union {s}
               /\ epoch' = epoch + 1
               /\ slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
               \* FIX: increment page live count on free-list reuse
               /\ pageLiveCount' = [pageLiveCount EXCEPT
                    ![SlotToPage(s)] = pageLiveCount[SlotToPage(s)] + 1]
               /\ pageReleased' = pageReleased
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
               /\ epoch' = epoch
               /\ slotEpoch' = slotEpoch
               /\ pageLiveCount' = [pageLiveCount EXCEPT
                    ![SlotToPage(s)] = pageLiveCount[SlotToPage(s)] + 1]
               /\ pageReleased' = pageReleased
    /\ UNCHANGED <<hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

EvalDropDuringGc ==
    /\ evalPhase = "eval"
    /\ gcPhase \in {"marking", "sweeping"}
    /\ roots /= {}
    /\ \E s \in roots :
        /\ roots' = roots \ {s}
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Shutdown                                                               *)
(*=======================================================================*)

ShutdownBegin ==
    /\ exprCount >= MaxExprs
    /\ evalPhase = "eval"
    /\ evalPhase' = "shutdown"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   pageVars,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

ShutdownDrainResponse ==
    /\ evalPhase = "shutdown"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
       IN /\ slotState' = [s \in Slots |->
                IF s \in safeDead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union safeDead
          /\ pageLiveCount' = [p \in Pages |->
                pageLiveCount[p] -
                    Cardinality({s \in safeDead : SlotToPage(s) = p})]
          /\ pageReleased' = [p \in Pages |->
                IF pageReleased[p] THEN TRUE
                ELSE IF pageLiveCount[p] -
                         Cardinality({s \in safeDead : SlotToPage(s) = p}) = 0
                      /\ \E s \in Slots : SlotToPage(s) = p /\ s < bumpPtr
                    THEN TRUE
                    ELSE FALSE]
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
          /\ respEpoch' = 0
          /\ gcInFlight' = FALSE
    /\ UNCHANGED <<bumpPtr, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

Terminated ==
    /\ evalPhase = "shutdown"
    /\ gcPhase = "idle"
    /\ ~gcInFlight
    /\ ~hasGcRequest
    /\ ~hasGcResponse
    /\ UNCHANGED vars

(*=======================================================================*)
(* Next State Relation                                                    *)
(*=======================================================================*)

Next ==
    \/ AllocateSlot
    \/ DropRoot
    \/ BeginBetweenExpressions
    \/ ProcessGcResponse
    \/ RequestGcAsync
    \/ RequestGcAndWait
    \/ WaitForGcInFlight
    \/ ResumeAfterWait
    \/ ContinueEval
    \/ GcReceiveRequest
    \/ GcMarkStep
    \/ GcMarkComplete
    \/ GcSweep
    \/ EvalAllocDuringGc
    \/ EvalDropDuringGc
    \/ ShutdownBegin
    \/ ShutdownDrainResponse
    \/ Terminated

Spec == Init /\ [][Next]_vars

(*=======================================================================*)
(* Fairness                                                               *)
(*=======================================================================*)

FairSpec == Spec
    /\ WF_vars(GcReceiveRequest)
    /\ WF_vars(GcMarkStep)
    /\ WF_vars(GcMarkComplete)
    /\ WF_vars(GcSweep)
    /\ WF_vars(ProcessGcResponse)
    /\ WF_vars(ResumeAfterWait)
    /\ WF_vars(ContinueEval)
    /\ WF_vars(BeginBetweenExpressions)
    /\ WF_vars(ShutdownBegin)
    /\ WF_vars(ShutdownDrainResponse)

(*=======================================================================*)
(* Safety Properties (Invariants)                                         *)
(*=======================================================================*)

MaxEpoch == MaxSlots * (MaxExprs + 2) * MaxRoots

TypeOK ==
    /\ slotState \in [Slots -> {"free", "alloc", "freed"}]
    /\ bumpPtr \in 1..(MaxSlots + 1)
    /\ freeSet \subseteq Slots
    /\ roots \subseteq Slots
    /\ Cardinality(roots) <= MaxRoots
    /\ epoch \in 0..MaxEpoch
    /\ slotEpoch \in [Slots -> 0..MaxEpoch]
    /\ pageLiveCount \in [Pages -> -MaxSlots..MaxSlots]
    /\ pageReleased \in [Pages -> BOOLEAN]
    /\ hasGcRequest \in BOOLEAN
    /\ snapSlotState \in [Slots -> {"free", "alloc", "freed"}]
    /\ snapBumpPtr \in 1..(MaxSlots + 1)
    /\ snapFreeSet \subseteq Slots
    /\ snapRoots \subseteq Slots
    /\ snapEpoch \in 0..MaxEpoch
    /\ hasGcResponse \in BOOLEAN
    /\ respDeadSet \subseteq Slots
    /\ respLiveCount \in 0..MaxSlots
    /\ respEpoch \in 0..MaxEpoch
    /\ evalPhase \in {"eval", "between", "waiting", "shutdown"}
    /\ exprCount \in 0..(MaxExprs + 1)
    /\ gcPhase \in {"idle", "marking", "sweeping"}
    /\ gcMarked \subseteq Slots
    /\ gcCalibrated \in BOOLEAN
    /\ gcInFlight \in BOOLEAN

\* CRITICAL: A slot that is a current root must not be freed
NoLiveValueFreed ==
    \A s \in Slots :
        (s \in roots) => slotState[s] /= "freed"

\* CRITICAL: An allocated slot must not be in a released page.
\* This is the PageSafetyInvariant — the invariant that catches Bug 4.
PageSafetyInvariant ==
    \A s \in Slots :
        slotState[s] = "alloc" => ~pageReleased[SlotToPage(s)]

\* No double-freeing of slots
NoDoubleFree ==
    hasGcResponse =>
        \A s \in respDeadSet : snapSlotState[s] = "alloc"

\* All roots must be allocated
RootsAreAllocated ==
    \A s \in roots : slotState[s] = "alloc"

\* All free-set entries are freed slots
FreeSetValid ==
    \A s \in freeSet : slotState[s] = "freed"

\* Slots at or beyond bumpPtr have never been allocated
BumpPtrValid ==
    \A s \in Slots : s >= bumpPtr => slotState[s] = "free"

\* Committed slots never exceed MaxSlots
MemoryBounded ==
    CommittedSlots <= MaxSlots

\* Page live counts are non-negative for non-released pages
PageLiveCountNonNeg ==
    \A p \in Pages :
        ~pageReleased[p] => pageLiveCount[p] >= 0

(*=======================================================================*)
(* Liveness Properties                                                    *)
(*=======================================================================*)

EvalEventuallyCompletes ==
    <>(exprCount >= MaxExprs)

=======================================================================
