--------------------------- MODULE SlabGC_Reactive ---------------------------
(*
 * TLA+ Model of Snapshot-Based Async Reactive Mark-Sweep GC
 *
 * This specification fixes ALL THREE bugs found in the original SlabGC model:
 *
 * BUG 1 (TOCTOU Use-After-Free) — FIXED by epoch-based filtering:
 *   Each free-list re-allocation increments a monotonic epoch counter and
 *   tags the slot. GC snapshots record the epoch at snapshot time. When
 *   processing the dead set, slots whose epoch is newer than the snapshot
 *   epoch are filtered out (they were re-allocated after the snapshot).
 *
 * BUG 2 (Data Race / UB) — FIXED by snapshot-based GC:
 *   The GC thread NEVER accesses live allocator state (slotState, bumpPtr,
 *   freeSet, etc.). Instead, the eval thread builds an immutable GcSnapshot
 *   (snapSlotState, snapBumpPtr, snapFreeSet, snapRoots, snapEpoch) and
 *   sends it to the GC thread. The GC thread operates exclusively on
 *   snapshot variables and its own gcMarked set. No shared mutable state.
 *
 * BUG 3 (Watermark Feedback Loop) — FIXED by full sweep:
 *   The GC sweeps ALL committed slots in the snapshot (1..snapBumpPtr-1),
 *   not just pre-watermark slots. live_count includes all marked slots in
 *   the snapshot. Thresholds are calibrated from accurate live_count.
 *   No watermark variable exists in this specification.
 *
 * ARCHITECTURE: Snapshot-based async mark-sweep with epoch filtering.
 *   - Eval thread: exclusively owns allocator state (slotState, bumpPtr,
 *     freeSet, roots, epoch, slotEpoch). Builds snapshots, processes responses.
 *   - GC thread: receives owned snapshots, performs mark-sweep on snapshot
 *     data only. Sends back dead set + snapshot_epoch for filtering.
 *   - Eval can allocate/drop during GC — no data race because GC only
 *     reads snap* variables, never live allocator state.
 *
 * DESIGN DECISIONS:
 *   - Free list modeled as SET (ordering irrelevant to correctness)
 *   - GC channels use separate flag + data variables (TLC limitation)
 *   - Epoch is a natural number, incremented on free-list re-allocations
 *   - slotEpoch maps each slot to the epoch of its last free-list alloc
 *   - Bump allocations don't need epoch tagging (always post-snapshot)
 *)

EXTENDS Integers, FiniteSets

CONSTANTS
    MaxSlots,       \* Total allocatable value slots (e.g. 6)
    MaxRoots,       \* Maximum root set size (e.g. 3)
    MaxExprs        \* Maximum expressions to evaluate before stopping

Slots == 1..MaxSlots

VARIABLES
    (*---------------------------------------------------------------*)
    (* Allocator State — EVAL THREAD ONLY (exclusive ownership)      *)
    (*---------------------------------------------------------------*)
    slotState,      \* [Slots -> {"free", "alloc", "freed"}]
    bumpPtr,        \* Next slot to bump-allocate (1..MaxSlots+1)
    freeSet,        \* SUBSET(Slots) — freed slots available for reuse
    roots,          \* SUBSET(Slots) — current GC roots
    epoch,          \* Nat — monotonic epoch counter for free-list reuse
    slotEpoch,      \* [Slots -> Nat] — epoch of last free-list alloc

    (*---------------------------------------------------------------*)
    (* GC Snapshot Channel — eval sends, GC reads (owned transfer)   *)
    (*---------------------------------------------------------------*)
    hasGcRequest,   \* BOOLEAN — is there a pending snapshot?
    snapSlotState,  \* [Slots -> {"free", "alloc", "freed"}] — frozen
    snapBumpPtr,    \* Nat — frozen bump pointer
    snapFreeSet,    \* SUBSET(Slots) — frozen free set
    snapRoots,      \* SUBSET(Slots) — frozen roots
    snapEpoch,      \* Nat — frozen epoch at snapshot time

    (*---------------------------------------------------------------*)
    (* GC Response Channel — GC sends, eval reads                    *)
    (*---------------------------------------------------------------*)
    hasGcResponse,  \* BOOLEAN — is there a pending response?
    respDeadSet,    \* SUBSET(Slots) — raw dead set from sweep
    respLiveCount,  \* Nat — live count from FULL sweep
    respEpoch,      \* Nat — snapshot epoch for TOCTOU filtering

    (*---------------------------------------------------------------*)
    (* Eval Thread State                                              *)
    (*---------------------------------------------------------------*)
    evalPhase,      \* "eval" | "between" | "waiting"
    exprCount,      \* Expressions evaluated so far

    (*---------------------------------------------------------------*)
    (* GC Thread State — GC THREAD ONLY (exclusive ownership)        *)
    (*---------------------------------------------------------------*)
    gcPhase,        \* "idle" | "marking" | "sweeping"
    gcMarked,       \* SUBSET(Slots) — values marked during tracing

    (*---------------------------------------------------------------*)
    (* Threshold State (eval thread only)                             *)
    (*---------------------------------------------------------------*)
    softThreshold,  \* Nat — soft GC trigger level
    hardThreshold,  \* Nat — hard GC trigger level (blocks eval)
    gcCalibrated,   \* BOOLEAN — thresholds calibrated by first GC?
    gcInFlight      \* BOOLEAN — a GC cycle is in progress?

vars == <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
          hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
          snapRoots, snapEpoch,
          hasGcResponse, respDeadSet, respLiveCount, respEpoch,
          evalPhase, exprCount,
          gcPhase, gcMarked,
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
    /\ epoch = 0
    /\ slotEpoch = [s \in Slots |-> 0]
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
    /\ softThreshold = 3     \* Low initial thresholds for small model
    /\ hardThreshold = 5
    /\ gcCalibrated = FALSE
    /\ gcInFlight = FALSE

(*=======================================================================*)
(* Eval Thread Actions                                                    *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* AllocateSlot: Eval thread allocates a new value slot.                  *)
(* Free-list allocs increment epoch and tag the slot.                     *)
(* Bump allocs don't need epoch tagging (always post-snapshot).           *)
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
               \* EPOCH: increment and tag the re-allocated slot
               /\ epoch' = epoch + 1
               /\ slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
               \* Bump allocs: no epoch change needed
               /\ epoch' = epoch
               /\ slotEpoch' = slotEpoch
    /\ UNCHANGED <<hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
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
        /\ UNCHANGED <<slotState, bumpPtr, freeSet, epoch, slotEpoch,
                       hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                       snapRoots, snapEpoch,
                       hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                       evalPhase, exprCount,
                       gcPhase, gcMarked,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* BeginBetweenExpressions: Expression evaluation complete.               *)
(*-----------------------------------------------------------------------*)
BeginBetweenExpressions ==
    /\ evalPhase = "eval"
    /\ exprCount < MaxExprs
    /\ evalPhase' = "between"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ProcessGcResponse: Process pending GC dead set with EPOCH FILTERING.   *)
(*                                                                        *)
(* KEY FIX FOR BUG 1 (TOCTOU):                                           *)
(* Only free slots whose slotEpoch <= respEpoch (snapshot epoch).         *)
(* Slots re-allocated after the snapshot have slotEpoch > respEpoch       *)
(* and are SKIPPED — they are live values, not dead.                      *)
(*                                                                        *)
(* KEY FIX FOR BUG 3 (Watermark):                                        *)
(* respLiveCount comes from a FULL sweep (all committed slots in          *)
(* snapshot), so thresholds are calibrated from accurate data.            *)
(*-----------------------------------------------------------------------*)
ProcessGcResponse ==
    /\ evalPhase = "between"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           \* EPOCH FILTER: only free slots not re-allocated since snapshot
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
           liveCount == respLiveCount
       IN /\ slotState' = [s \in Slots |->
                IF s \in safeDead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union safeDead
          \* ACCURATE thresholds from full sweep
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
(* RequestGcAsync: Build snapshot and send to GC thread (non-blocking).   *)
(*                                                                        *)
(* KEY FIX FOR BUG 2 (Data Race):                                        *)
(* The snapshot copies all relevant allocator state into snap* variables. *)
(* The GC thread reads ONLY snap* variables, never live allocator state.  *)
(* No shared mutable state between threads.                               *)
(*-----------------------------------------------------------------------*)
RequestGcAsync ==
    /\ evalPhase = "between"
    /\ CommittedSlots > softThreshold
    /\ ~gcInFlight
    /\ ~hasGcRequest
    \* BUILD SNAPSHOT: copy allocator state into snap* variables
    /\ hasGcRequest' = TRUE
    /\ snapSlotState' = slotState
    /\ snapBumpPtr' = bumpPtr
    /\ snapFreeSet' = freeSet
    /\ snapRoots' = roots
    /\ snapEpoch' = epoch
    /\ gcInFlight' = TRUE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated>>

(*-----------------------------------------------------------------------*)
(* RequestGcAndWait: Hard threshold exceeded, calibrated. Block.          *)
(*-----------------------------------------------------------------------*)
RequestGcAndWait ==
    /\ evalPhase = "between"
    /\ CommittedSlots > hardThreshold
    /\ gcCalibrated
    /\ ~gcInFlight
    /\ ~hasGcRequest
    \* BUILD SNAPSHOT
    /\ hasGcRequest' = TRUE
    /\ snapSlotState' = slotState
    /\ snapBumpPtr' = bumpPtr
    /\ snapFreeSet' = freeSet
    /\ snapRoots' = roots
    /\ snapEpoch' = epoch
    /\ gcInFlight' = TRUE
    /\ evalPhase' = "waiting"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
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
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ResumeAfterWait: Blocking GC response received.                        *)
(* Uses the same epoch filtering as ProcessGcResponse.                    *)
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
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* GC Thread Actions                                                      *)
(*                                                                        *)
(* KEY STRUCTURAL PROPERTY (Bug 2 fix):                                   *)
(* GC thread actions ONLY read snap* variables and gcMarked.              *)
(* They NEVER read or write slotState, bumpPtr, freeSet, roots, epoch,    *)
(* slotEpoch. This eliminates any possibility of data race.               *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* GcReceiveRequest: GC thread picks up a snapshot from the channel.      *)
(*-----------------------------------------------------------------------*)
GcReceiveRequest ==
    /\ gcPhase = "idle"
    /\ hasGcRequest
    /\ gcMarked' = {}
    /\ gcPhase' = "marking"
    /\ hasGcRequest' = FALSE
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   snapSlotState, snapBumpPtr, snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcMarkStep: Mark one reachable root value.                             *)
(* READS ONLY: snapRoots, snapSlotState (snapshot data, not live state).  *)
(*-----------------------------------------------------------------------*)
GcMarkStep ==
    /\ gcPhase = "marking"
    /\ \E s \in snapRoots :
        /\ s \notin gcMarked
        /\ snapSlotState[s] = "alloc"
        /\ gcMarked' = gcMarked \union {s}
        /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                       hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                       snapRoots, snapEpoch,
                       hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                       evalPhase, exprCount,
                       gcPhase,
                       softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcMarkComplete: All snapshot roots marked. Transition to sweeping.     *)
(* READS ONLY: snapRoots, snapSlotState (snapshot data).                  *)
(*-----------------------------------------------------------------------*)
GcMarkComplete ==
    /\ gcPhase = "marking"
    /\ \A s \in snapRoots : (snapSlotState[s] = "alloc") => (s \in gcMarked)
    /\ gcPhase' = "sweeping"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* GcSweep: Sweep ALL committed slots in the snapshot.                    *)
(*                                                                        *)
(* KEY FIX FOR BUG 3 (Watermark):                                        *)
(* Sweeps ALL slots from 1 to snapBumpPtr-1 (full committed region).      *)
(* No watermark restriction. liveCount includes ALL marked slots.         *)
(*                                                                        *)
(* READS ONLY: snapSlotState, snapBumpPtr, snapFreeSet, snapEpoch,        *)
(* gcMarked (all GC-owned data). Never touches live allocator state.      *)
(*                                                                        *)
(* Response includes snapEpoch for TOCTOU filtering by eval thread.       *)
(*-----------------------------------------------------------------------*)
GcSweep ==
    /\ gcPhase = "sweeping"
    /\ LET \* ALL committed slots in the snapshot (no watermark!)
           allCommitted == {s \in Slots : s < snapBumpPtr}
           \* Dead: committed, allocated in snapshot, not marked, not on
           \* snapshot free set (already dead at snapshot time)
           deadSet == {s \in allCommitted :
                        /\ snapSlotState[s] = "alloc"
                        /\ s \notin gcMarked
                        /\ s \notin snapFreeSet}
           \* FULL live count: ALL marked slots in snapshot
           liveCount == Cardinality({s \in allCommitted :
                        /\ snapSlotState[s] = "alloc"
                        /\ s \in gcMarked})
       IN /\ hasGcResponse' = TRUE
          /\ respDeadSet' = deadSet
          /\ respLiveCount' = liveCount
          /\ respEpoch' = snapEpoch    \* For TOCTOU epoch filtering
          /\ gcMarked' = {}
          /\ gcPhase' = "idle"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   evalPhase, exprCount,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Concurrent Eval Actions During GC                                      *)
(*                                                                        *)
(* Eval freely allocates and drops during GC. This is SAFE because:       *)
(* - Eval modifies: slotState, bumpPtr, freeSet, roots, epoch, slotEpoch  *)
(* - GC reads: snapSlotState, snapBumpPtr, snapFreeSet, snapRoots,        *)
(*             snapEpoch, gcMarked                                        *)
(* These are DISJOINT variable sets — no data race possible.              *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* EvalAllocDuringGc: Eval allocates WHILE GC is running.                 *)
(* SAFE: modifies only eval-owned variables, not snap* or gcMarked.       *)
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
               /\ epoch' = epoch + 1
               /\ slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
       ELSE LET s == bumpPtr
            IN /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ roots' = roots \union {s}
               /\ epoch' = epoch
               /\ slotEpoch' = slotEpoch
    /\ UNCHANGED <<hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* EvalDropDuringGc: Eval drops a root WHILE GC is running.               *)
(* SAFE: modifies only eval-owned roots, not snap* or gcMarked.           *)
(*-----------------------------------------------------------------------*)
EvalDropDuringGc ==
    /\ evalPhase = "eval"
    /\ gcPhase \in {"marking", "sweeping"}
    /\ roots /= {}
    /\ \E s \in roots :
        /\ roots' = roots \ {s}
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   evalPhase, exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*=======================================================================*)
(* Shutdown / Termination                                                 *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* ShutdownBegin: All expressions done, transition to shutdown.           *)
(*-----------------------------------------------------------------------*)
ShutdownBegin ==
    /\ exprCount >= MaxExprs
    /\ evalPhase = "eval"
    /\ evalPhase' = "shutdown"
    /\ UNCHANGED <<slotState, bumpPtr, freeSet, roots, epoch, slotEpoch,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   exprCount,
                   gcPhase, gcMarked,
                   softThreshold, hardThreshold, gcCalibrated, gcInFlight>>

(*-----------------------------------------------------------------------*)
(* ShutdownDrainResponse: Process pending GC response during shutdown.    *)
(* Uses epoch filtering like ProcessGcResponse.                           *)
(*-----------------------------------------------------------------------*)
ShutdownDrainResponse ==
    /\ evalPhase = "shutdown"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
       IN /\ slotState' = [s \in Slots |->
                IF s \in safeDead THEN "freed" ELSE slotState[s]]
          /\ freeSet' = freeSet \union safeDead
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

\* Maximum possible epoch value. Each free-list re-allocation increments
\* epoch by 1. Bounded by the number of possible re-allocations in the model.
\* Conservative upper bound: MaxSlots * (MaxExprs + 2) * MaxRoots covers
\* all possible alloc-free-realloc cycles.
MaxEpoch == MaxSlots * (MaxExprs + 2) * MaxRoots

TypeOK ==
    /\ slotState \in [Slots -> {"free", "alloc", "freed"}]
    /\ bumpPtr \in 1..(MaxSlots + 1)
    /\ freeSet \subseteq Slots
    /\ roots \subseteq Slots
    /\ Cardinality(roots) <= MaxRoots
    /\ epoch \in 0..MaxEpoch
    /\ slotEpoch \in [Slots -> 0..MaxEpoch]
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
\* This MUST hold (was violated in original spec due to TOCTOU)
NoLiveValueFreed ==
    \A s \in Slots :
        (s \in roots) => slotState[s] /= "freed"

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

\* STRUCTURAL PROPERTY: GC thread never accesses live allocator state.
\* This is enforced structurally — GC actions only reference snap*
\* variables and gcMarked. We encode this as an invariant that
\* always holds by construction (vacuously true — serves as documentation).
NoDataRace_Structural ==
    \* GC marking/sweeping is always safe because it reads only snapshots.
    \* This invariant captures the KEY difference from the original spec:
    \* eval can be in "eval" while GC is "marking"/"sweeping" — SAFE.
    TRUE

(*=======================================================================*)
(* Liveness Properties (Temporal)                                         *)
(*=======================================================================*)

\* Eval thread eventually finishes
EvalEventuallyCompletes ==
    <>(exprCount >= MaxExprs)

=======================================================================
