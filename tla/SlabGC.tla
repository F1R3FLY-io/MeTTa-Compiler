--------------------------- MODULE SlabGC ---------------------------
(*
 * TLA+ Model of Concurrent Slab Allocator + Mark-Sweep GC
 *
 * Models the MeTTaTron evaluator's concurrent GC protocol at the logical
 * slot level. Two threads — an evaluation thread and a background GC
 * thread — interact via message-passing channels and shared allocator
 * state.
 *
 * This specification models TWO known bugs:
 *
 * 1. DATA RACE (UB): The GC thread accesses the SlabAllocator's mutable
 *    state (via UnsafeCell<SlabAllocatorInner>) concurrently with the
 *    eval thread. Both call inner() which yields &mut SlabAllocatorInner.
 *    Specifically:
 *      - GC iterates pages Vec while eval may push (Vec reallocation)
 *      - sweep() reads free_list into HashSet while eval may push/pop
 *      - GC reads page.bump_count while eval writes it during allocation
 *
 * 2. WATERMARK FEEDBACK LOOP: sweep() only counts live_bytes from values
 *    before the watermark. Values allocated after the watermark are not
 *    counted. When update_thresholds(live_bytes) calibrates from this
 *    artificially low number, GC triggers constantly but only sweeps the
 *    pre-watermark region. Post-watermark pages are never fully emptied.
 *
 * The model uses small finite bounds (MaxSlots=6, MaxRoots=3) to enable
 * exhaustive TLC model checking.
 *
 * DESIGN DECISIONS:
 * - Free list modeled as SET (ordering irrelevant to correctness)
 * - GC channels use separate flag + data variables (TLC cannot compare
 *   records with strings via = / /=)
 *)

EXTENDS Integers, FiniteSets

CONSTANTS
    MaxSlots,       \* Total allocatable value slots (e.g. 6)
    MaxRoots,       \* Maximum root set size (e.g. 3)
    MaxExprs        \* Maximum expressions to evaluate before stopping

Slots == 1..MaxSlots

VARIABLES
    (*---------------------------------------------------------------*)
    (* Allocator State (shared between threads)                       *)
    (*---------------------------------------------------------------*)
    slotState,      \* [Slots -> {"free", "alloc", "freed"}]
    bumpPtr,        \* Next slot to bump-allocate (1..MaxSlots+1)
    freeSet,        \* SUBSET(Slots) — freed slots available for reuse

    (*---------------------------------------------------------------*)
    (* Root Set                                                       *)
    (*---------------------------------------------------------------*)
    roots,          \* SUBSET(Slots) — current GC roots

    (*---------------------------------------------------------------*)
    (* GC Request Channel                                             *)
    (*---------------------------------------------------------------*)
    hasGcRequest,   \* BOOLEAN — is there a pending request?
    reqRoots,       \* SUBSET(Slots) — roots snapshot in request
    reqWatermark,   \* Nat — watermark in request

    (*---------------------------------------------------------------*)
    (* GC Response Channel                                            *)
    (*---------------------------------------------------------------*)
    hasGcResponse,  \* BOOLEAN — is there a pending response?
    respDeadSet,    \* SUBSET(Slots) — dead set in response
    respLiveCount,  \* Nat — live count in response

    (*---------------------------------------------------------------*)
    (* Eval Thread State                                              *)
    (*---------------------------------------------------------------*)
    evalPhase,      \* "eval" | "between" | "waiting"
    exprCount,      \* Expressions evaluated so far

    (*---------------------------------------------------------------*)
    (* GC Thread State                                                *)
    (*---------------------------------------------------------------*)
    gcPhase,        \* "idle" | "marking" | "sweeping"
    gcRoots,        \* SUBSET(Slots) — snapshot from request
    gcWatermark,    \* Nat — snapshot watermark from request
    gcMarked,       \* SUBSET(Slots) — values marked so far

    (*---------------------------------------------------------------*)
    (* Threshold State                                                *)
    (*---------------------------------------------------------------*)
    softThreshold,  \* Nat — soft GC trigger level
    hardThreshold,  \* Nat — hard GC trigger level (blocks eval)
    gcCalibrated,   \* BOOLEAN — thresholds calibrated by first GC?
    gcInFlight      \* BOOLEAN — a GC cycle is in progress?

vars == <<slotState, bumpPtr, freeSet, roots,
          hasGcRequest, reqRoots, reqWatermark,
          hasGcResponse, respDeadSet, respLiveCount,
          evalPhase, exprCount,
          gcPhase, gcRoots, gcWatermark, gcMarked,
          softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Helper Operators                                                       *)
(*=======================================================================*)

\* Number of committed slots (everything that has been bumped)
CommittedSlots == bumpPtr - 1

\* Can we allocate another slot?
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
    /\ hasGcRequest = FALSE
    /\ reqRoots = {}
    /\ reqWatermark = 0
    /\ hasGcResponse = FALSE
    /\ respDeadSet = {}
    /\ respLiveCount = 0
    /\ evalPhase = "eval"
    /\ exprCount = 0
    /\ gcPhase = "idle"
    /\ gcRoots = {}
    /\ gcWatermark = 0
    /\ gcMarked = {}
    /\ softThreshold = 3     \* Low initial thresholds for small model
    /\ hardThreshold = 5
    /\ gcCalibrated = FALSE
    /\ gcInFlight = FALSE

(*=======================================================================*)
(* Eval Thread Actions                                                    *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* AllocateSlot: Eval thread allocates a new value slot.                  *)
(* Tries free set first, then bump pointer.                               *)
(* The new slot is added to roots (simulates "value becomes reachable").  *)
(*-----------------------------------------------------------------------*)
AllocateSlot ==
    /\ evalPhase = "eval"
    /\ Cardinality(roots) < MaxRoots
    /\ CanAllocate
    /\ IF freeSet /= {}
       THEN \E s \in freeSet :
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ freeSet' = freeSet \ {s}
               /\ bumpPtr' = bumpPtr
               /\ roots' = roots \union {s}
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
    /\ UNCHANGED <<hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* DropRoot: Some root becomes unreachable (simulates env cleanup).       *)
(* The value stays "alloc" in the allocator but is no longer a root.      *)
(*-----------------------------------------------------------------------*)
DropRoot ==
    /\ evalPhase = "eval"
    /\ roots /= {}
    /\ \E s \in roots :
        /\ roots' = roots \ {s}
        /\ UNCHANGED <<slotState, bumpPtr, freeSet,
                       hasGcRequest, reqRoots, reqWatermark,
                       hasGcResponse, respDeadSet, respLiveCount,
                       evalPhase, exprCount,
                       gcPhase, gcRoots, gcWatermark, gcMarked,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* BeginBetweenExpressions: Expression evaluation complete.               *)
(*-----------------------------------------------------------------------*)
BeginBetweenExpressions ==
    /\ evalPhase = "eval"
    /\ exprCount < MaxExprs
    /\ evalPhase' = "between"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ProcessGcResponse: Process pending GC dead set (non-blocking).         *)
(*-----------------------------------------------------------------------*)
ProcessGcResponse ==
    /\ evalPhase = "between"
    /\ hasGcResponse
    /\ LET dead == respDeadSet
           liveCount == respLiveCount
       IN /\ slotState' = [s \in Slots |->
                IF s \in dead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union dead
          \* BUG: uses only pre-watermark liveCount for thresholds
          /\ softThreshold' = IF liveCount * 2 > 2 THEN liveCount * 2 ELSE 2
          /\ hardThreshold' = IF liveCount * 4 > 4 THEN liveCount * 4 ELSE 4
          /\ gcCalibrated' = TRUE
          /\ gcInFlight' = FALSE
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
    /\ UNCHANGED <<bumpPtr, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked>>

(*-----------------------------------------------------------------------*)
(* RequestGcAsync: Soft threshold exceeded, no GC in flight.              *)
(*-----------------------------------------------------------------------*)
RequestGcAsync ==
    /\ evalPhase = "between"
    /\ CommittedSlots > softThreshold
    /\ ~gcInFlight
    /\ ~hasGcRequest
    /\ hasGcRequest' = TRUE
    /\ reqRoots' = roots
    /\ reqWatermark' = bumpPtr
    /\ gcInFlight' = TRUE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* RequestGcAndWait: Hard threshold exceeded, calibrated.                 *)
(*-----------------------------------------------------------------------*)
RequestGcAndWait ==
    /\ evalPhase = "between"
    /\ CommittedSlots > hardThreshold
    /\ gcCalibrated
    /\ ~gcInFlight
    /\ ~hasGcRequest
    /\ hasGcRequest' = TRUE
    /\ reqRoots' = roots
    /\ reqWatermark' = bumpPtr
    /\ gcInFlight' = TRUE
    /\ evalPhase' = "waiting"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcResponse, respDeadSet, respLiveCount,
                   exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* WaitForGcInFlight: Hard threshold exceeded, GC already in flight.      *)
(*-----------------------------------------------------------------------*)
WaitForGcInFlight ==
    /\ evalPhase = "between"
    /\ CommittedSlots > hardThreshold
    /\ gcCalibrated
    /\ gcInFlight
    /\ evalPhase' = "waiting"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ResumeAfterWait: Blocking GC response received.                        *)
(*-----------------------------------------------------------------------*)
ResumeAfterWait ==
    /\ evalPhase = "waiting"
    /\ hasGcResponse
    /\ LET dead == respDeadSet
           liveCount == respLiveCount
       IN /\ slotState' = [s \in Slots |->
                IF s \in dead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union dead
          /\ softThreshold' = IF liveCount * 2 > 2 THEN liveCount * 2 ELSE 2
          /\ hardThreshold' = IF liveCount * 4 > 4 THEN liveCount * 4 ELSE 4
          /\ gcCalibrated' = TRUE
          /\ gcInFlight' = FALSE
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
    /\ evalPhase' = "eval"
    /\ exprCount' = exprCount + 1
    /\ UNCHANGED <<bumpPtr, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   gcPhase, gcRoots, gcWatermark, gcMarked>>

(*-----------------------------------------------------------------------*)
(* ContinueEval: No GC needed, resume evaluation.                        *)
(*-----------------------------------------------------------------------*)
ContinueEval ==
    /\ evalPhase = "between"
    /\ ~hasGcResponse         \* No pending response to process first
    /\ \/ CommittedSlots <= softThreshold
       \/ gcInFlight           \* Already requested async GC
    /\ ~(CommittedSlots > hardThreshold /\ gcCalibrated)
    /\ evalPhase' = "eval"
    /\ exprCount' = exprCount + 1
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* GC Thread Actions                                                      *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* GcReceiveRequest: GC thread picks up a request from the channel.       *)
(*-----------------------------------------------------------------------*)
GcReceiveRequest ==
    /\ gcPhase = "idle"
    /\ hasGcRequest
    /\ gcRoots' = reqRoots
    /\ gcWatermark' = reqWatermark
    /\ gcMarked' = {}
    /\ gcPhase' = "marking"
    /\ hasGcRequest' = FALSE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcMarkStep: Mark one reachable root value.                             *)
(*-----------------------------------------------------------------------*)
GcMarkStep ==
    /\ gcPhase = "marking"
    /\ \E s \in gcRoots :
        /\ s \notin gcMarked
        /\ slotState[s] = "alloc"
        /\ gcMarked' = gcMarked \union {s}
        /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                       hasGcRequest, reqRoots, reqWatermark,
                       hasGcResponse, respDeadSet, respLiveCount,
                       evalPhase, exprCount,
                       gcPhase, gcRoots, gcWatermark,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcMarkComplete: All roots marked. Transition to sweeping.              *)
(*-----------------------------------------------------------------------*)
GcMarkComplete ==
    /\ gcPhase = "marking"
    /\ \A s \in gcRoots : (slotState[s] = "alloc") => (s \in gcMarked)
    /\ gcPhase' = "sweeping"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcSweep: Sweep all slots before watermark, build dead set.             *)
(* BUG modeled: only counts live slots in pre-watermark region.           *)
(*-----------------------------------------------------------------------*)
GcSweep ==
    /\ gcPhase = "sweeping"
    /\ LET preWatermark == {s \in Slots : s < gcWatermark}
           deadSet == {s \in preWatermark :
                        /\ slotState[s] = "alloc"
                        /\ s \notin gcMarked
                        /\ s \notin freeSet}
           \* THE BUG: only count pre-watermark live slots
           liveCount == Cardinality({s \in preWatermark :
                        /\ slotState[s] = "alloc"
                        /\ s \in gcMarked})
       IN /\ hasGcResponse' = TRUE
          /\ respDeadSet' = deadSet
          /\ respLiveCount' = liveCount
          /\ gcMarked' = {}
          /\ gcPhase' = "idle"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   evalPhase, exprCount,
                   gcRoots, gcWatermark,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Concurrent Interference Actions (modeling the data race)               *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* EvalAllocDuringGc: Eval allocates WHILE GC is marking or sweeping.     *)
(*-----------------------------------------------------------------------*)
EvalAllocDuringGc ==
    /\ evalPhase = "eval"
    /\ gcPhase \in {"marking", "sweeping"}
    /\ Cardinality(roots) < MaxRoots
    /\ CanAllocate
    /\ IF freeSet /= {}
       THEN \E s \in freeSet :
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ freeSet' = freeSet \ {s}
               /\ bumpPtr' = bumpPtr
               /\ roots' = roots \union {s}
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
    /\ UNCHANGED <<hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* EvalDropDuringGc: Eval drops a root WHILE GC is marking or sweeping.   *)
(*-----------------------------------------------------------------------*)
EvalDropDuringGc ==
    /\ evalPhase = "eval"
    /\ gcPhase \in {"marking", "sweeping"}
    /\ roots /= {}
    /\ \E s \in roots :
        /\ roots' = roots \ {s}
    /\ UNCHANGED <<slotState, bumpPtr, freeSet,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Shutdown / Termination                                                 *)
(*                                                                        *)
(* Models ArenaState::drop() which shuts down the GC thread before        *)
(* dropping the allocator. The eval thread must drain any pending GC       *)
(* responses before terminating.                                          *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* ShutdownBegin: All expressions done, transition to shutdown.           *)
(* The eval thread enters "shutdown" phase to drain pending GC work.      *)
(*-----------------------------------------------------------------------*)
ShutdownBegin ==
    /\ exprCount >= MaxExprs
    /\ evalPhase = "eval"
    /\ evalPhase' = "shutdown"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   hasGcResponse, respDeadSet, respLiveCount,
                   exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ShutdownDrainResponse: Process pending GC response during shutdown.    *)
(*-----------------------------------------------------------------------*)
ShutdownDrainResponse ==
    /\ evalPhase = "shutdown"
    /\ hasGcResponse
    /\ LET dead == respDeadSet
       IN /\ slotState' = [s \in Slots |->
                IF s \in dead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union dead
          /\ hasGcResponse' = FALSE
          /\ respDeadSet' = {}
          /\ respLiveCount' = 0
          /\ gcInFlight' = FALSE
    /\ UNCHANGED <<bumpPtr, roots,
                   hasGcRequest, reqRoots, reqWatermark,
                   evalPhase, exprCount,
                   gcPhase, gcRoots, gcWatermark, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* ShutdownWaitGc: Wait for in-flight GC to complete during shutdown.     *)
(* GC thread actions (GcReceiveRequest, GcMarkStep, etc.) continue to     *)
(* run concurrently, so this step is just a no-op waiting for the GC to   *)
(* produce a response that ShutdownDrainResponse can process.             *)
(*-----------------------------------------------------------------------*)
\* No explicit action needed — GC thread actions handle this.

(*-----------------------------------------------------------------------*)
(* Terminated: All done — GC is idle, no in-flight cycle, eval shutdown.  *)
(*-----------------------------------------------------------------------*)
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

TypeOK ==
    /\ slotState \in [Slots -> {"free", "alloc", "freed"}]
    /\ bumpPtr \in 1..(MaxSlots + 1)
    /\ freeSet \subseteq Slots
    /\ roots \subseteq Slots
    /\ Cardinality(roots) <= MaxRoots
    /\ hasGcRequest \in BOOLEAN
    /\ reqRoots \subseteq Slots
    /\ reqWatermark \in 0..(MaxSlots + 1)
    /\ hasGcResponse \in BOOLEAN
    /\ respDeadSet \subseteq Slots
    /\ respLiveCount \in 0..MaxSlots
    /\ evalPhase \in {"eval", "between", "waiting", "shutdown"}
    /\ exprCount \in 0..(MaxExprs + 1)
    /\ gcPhase \in {"idle", "marking", "sweeping"}
    /\ gcRoots \subseteq Slots
    /\ gcWatermark \in 0..(MaxSlots + 1)
    /\ gcMarked \subseteq Slots
    /\ gcCalibrated \in BOOLEAN
    /\ gcInFlight \in BOOLEAN

\* A slot that is a current root must not be freed
NoLiveValueFreed ==
    \A s \in Slots :
        (s \in roots) => slotState[s] /= "freed"

\* A freed slot must not be in the pending dead set
NoDoubleFree ==
    hasGcResponse =>
        \A s \in respDeadSet : slotState[s] = "alloc"

\* Dead set only contains pre-watermark slots
SweepCorrectness ==
    hasGcResponse =>
        \A s \in respDeadSet : s < gcWatermark

\* EXPECTED VIOLATION: concurrent access to allocator state
NoDataRace ==
    ~(evalPhase = "eval" /\ gcPhase \in {"marking", "sweeping"})

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

(*=======================================================================*)
(* Liveness Properties (Temporal)                                         *)
(*=======================================================================*)

\* EXPECTED VIOLATION: watermark feedback loop prevents collection
DeadValuesEventuallyCollected ==
    \A s \in Slots :
        (slotState[s] = "alloc" /\ s \notin roots) ~> (slotState[s] = "freed")

\* Eval thread eventually finishes
EvalEventuallyCompletes ==
    <>(exprCount >= MaxExprs)

=======================================================================
