--------------------------- MODULE SlabGC_Quiescent ---------------------------
(*
 * TLA+ Model of Multi-Thread Quiescent-State GC Coordination Protocol
 *
 * This specification models the COMPLETE end-to-end allocation and garbage
 * collection lifecycle, including:
 *
 *   1. Lock-free EvalGuard + GC_IN_PROGRESS coordination protocol
 *   2. Per-thread trampoline stack roots vs. registered root providers
 *   3. Root provider lifecycle (MettaState source/output, Environment)
 *   4. Snapshot-based async mark-sweep with epoch TOCTOU filtering
 *   5. GC_CYCLE_IN_FLIGHT back-pressure (at most one cycle in flight)
 *   6. Memory pressure statistics: allocation rate monitoring, threshold-based
 *      and rate-based GC triggering, adaptive GC threshold after collection
 *   7. Graduated backpressure: 4-level allocation throttling (0=none, 1=yield,
 *      2=sleep(10us), 3=sleep(100us)) computed by cron monitor, with Tier 2
 *      blocking at max level preventing re-entry while GC cycle is in flight
 *   8. Page release: empty pages are munmap'd after GC frees all their slots.
 *      Free-list entries from released pages are filtered via atomic drain +
 *      rebuild of the Treiber stack (zero hot-path overhead).
 *   9. Session-based GC: per-eval session context IDs, bulk release of session
 *      values on eval completion, root tracing for surviving set promotion.
 *  10. GC reachability heartbeat: cron backpressure gated on GC lifecycle
 *      reachability to prevent permanent throttling in library/test code.
 *
 * ROOT SET MODEL:
 *
 *   The real system has two categories of live values:
 *
 *   (a) REGISTERED ROOTS — values in root providers (MettaState source/output,
 *       GenericEnvironment rules/bindings). These are visible to GC via
 *       collect_all_roots() -> RootProvider::collect_roots().
 *
 *   (b) STACK ROOTS — values on the trampoline's Rust call stack (work_stack,
 *       continuations, locals in apply_bindings). These are NOT visible to
 *       GC root collection.
 *
 *   The quiescent-state invariant guarantees: when GC builds a snapshot,
 *   ACTIVE_EVALUATORS == 0, meaning all trampoline call stacks have returned
 *   and their stack roots are empty. Therefore, at snapshot time,
 *   registeredRoots == totalLiveRoots.
 *
 *   This model VERIFIES this invariant (SnapshotCapturesAllRoots) rather
 *   than assuming it.
 *
 * INTRA-EVALUATION GC SAFEPOINTS:
 *
 *   Long-running evaluations (e.g., PLN Robot) accumulate dead objects
 *   because the GC can only run when ACTIVE_EVALUATORS == 0. A single
 *   eval() call holds EvalGuard for its entire duration. Cooperative
 *   safepoints allow an evaluator to temporarily pause, register its
 *   trampoline state (work_stack + continuations) as temporary roots,
 *   release the EvalGuard, allow the quiescent GC to fire, then
 *   re-acquire the guard and unregister the temporary roots.
 *
 *   The safepoint protocol mirrors the EvalGuard enter/backoff pattern:
 *     1. EvalSafepoint: register roots, decrement ACTIVE_EVALUATORS
 *        -> threadPhase transitions from "eval" to "safepoint"
 *     2. EvalSafepointResume_Increment: increment ACTIVE_EVALUATORS
 *        -> threadPhase transitions from "safepoint" to "sp_entering"
 *     3. EvalSafepointResume_Proceed: GC_IN_PROGRESS clear
 *        -> restore roots, transition to "eval"
 *     3b. EvalSafepointResume_BackOff: GC_IN_PROGRESS set
 *        -> decrement, return to "safepoint"
 *
 *   Safepoint roots are visible to GC via collect_all_roots() ->
 *   collect_safepoint_roots(). The snapshot includes both registered
 *   roots and safepoint roots.
 *
 * SESSION-BASED GC MODEL:
 *
 *   Each top-level eval creates a SessionGuard with a unique context ID
 *   (1..MaxContextIds, monotonically increasing). All allocations
 *   during that eval are tagged with the session's context ID. On eval
 *   completion (SessionGuard::drop), the context ID is enqueued for async
 *   release by a background thread (session_release_thread_main).
 *
 *   The session release thread:
 *     1. Waits for quiescence (ACTIVE_EVALUATORS == 0)
 *     2. Acquires GC_IN_PROGRESS via CAS (try_enter)
 *     3. Double-checks quiescence
 *     4. Traces surviving set (root scan)
 *     5. Releases GC_IN_PROGRESS
 *     6. For each released session: promotes surviving values to ctx=0,
 *        frees non-surviving session values
 *
 *   This model captures the session lifecycle, mutual exclusion with
 *   quiescent GC on GC_IN_PROGRESS, and the surviving set promotion.
 *
 * MEMORY PRESSURE MODEL:
 *
 *   The real system uses a cron monitor (gc_cron.rs) that polls every 100ms:
 *
 *     execute_memory_monitor():
 *       delta = alloc_count - prev_alloc_count
 *       rate = delta / elapsed_seconds
 *       if rate > ALLOC_RATE_THRESHOLD:
 *           request_gc()
 *       if committed_bytes >= gc_threshold:
 *           request_gc()
 *       prev_alloc_count = alloc_count
 *
 *   After a GC cycle completes (process_gc_response):
 *       gc_threshold = max(live_bytes * GC_GROWTH_FACTOR, MIN_GC_THRESHOLD)
 *
 *   This model abstracts the rate as "allocation delta since last poll":
 *     - allocsSinceLastPoll: incremented on each AllocateSlot, reset on poll
 *     - Rate trigger: allocsSinceLastPoll >= AllocDeltaThreshold
 *     - Threshold trigger: committedSlotCount >= gcThreshold
 *     - Adaptive threshold: gcThreshold = max(respLiveCount * 2, MinGcThreshold)
 *
 *   TLA+ does not model real time, so the poll fires non-deterministically.
 *   WF(CronMonitorPoll) ensures the monitor eventually polls.
 *
 * BACKPRESSURE MODEL:
 *
 *   The cron monitor computes a 4-level backpressure based on committed/threshold:
 *     Level 0: committed < gcThreshold          — no throttling
 *     Level 1: committed >= gcThreshold          — yield
 *     Level 2: committed >= gcThreshold * 3/2    — sleep(10us)
 *     Level 3: committed >= gcThreshold * 2      — sleep(100us)
 *
 *   Two tiers of throttling in the Rust implementation:
 *     Tier 1 (during eval): apply_backpressure_tier1() in session_context.rs
 *       — yield/sleep based on level (models allocation slowdown)
 *     Tier 2 (between expressions): apply_backpressure_tier2() in main.rs
 *       — at MAX level, spin-yield until GC cycle completes (models blocking)
 *
 *   This model captures Tier 2 as a guard on ContinueEval:
 *     At backpressureLevel >= MAX_BP, ContinueEval is disabled while a GC
 *     cycle is in flight (hasGcRequest \/ hasGcResponse \/ gcPhase /= "idle").
 *     This prevents new eval entries from outpacing GC.
 *
 *   Tier 1 is not modeled explicitly since TLA+ doesn't model time — the
 *   slowdown effect is captured abstractly by the Tier 2 blocking.
 *
 *   ProcessGcResponse recomputes backpressureLevel from the new committed/
 *   threshold ratio for immediate feedback (matching Rust lines 2030-2035)
 *   instead of waiting up to 100ms for the next cron poll.
 *
 * GC REACHABILITY HEARTBEAT:
 *
 *   The cron monitor only escalates backpressure when the GC lifecycle is
 *   reachable — i.e., code paths are calling maybe_quiescent_gc() and
 *   maybe_process_gc_response(). If the reachable counter hasn't advanced
 *   since the last poll, backpressure is set to 0 to avoid permanent
 *   throttling in library/test code.
 *
 *   This model captures gcReachableAdvanced as a BOOLEAN that is set to
 *   TRUE by TryQuiescentGc_AcquireFlag, ProcessGcResponse, and
 *   SessionGc_TraceAndRelease (the three code paths that call
 *   bump_gc_reachable()), and reset to FALSE by CronMonitorPoll.
 *
 * PAGE RELEASE MODEL:
 *
 *   Slots are partitioned into pages of SlotsPerPage slots each:
 *     Page 1 = slots 1..SlotsPerPage
 *     Page 2 = slots (SlotsPerPage+1)..(2*SlotsPerPage)
 *     etc.
 *
 *   After ProcessGcResponse frees dead slots, empty pages (all bumped slots
 *   freed, excluding the current bump page) are released:
 *     1. Free-set entries from empty pages are filtered out (drain + rebuild)
 *     2. Pages are added to releasedPages (models munmap)
 *
 *   CurrentPage is a derived definition (PageOf(bumpPtr) when bumpPtr is
 *   within bounds) — not a separate variable. This is sound because the
 *   Rust current_page AtomicPtr always tracks the page containing the bump
 *   pointer, and is deterministic given bumpPtr.
 *
 *   Safety invariants verify:
 *     - No stale free-set entries point to released pages
 *     - No live value resides on a released page
 *     - The current bump page is never released
 *     - Released pages contain only freed/free slots
 *
 * PROTOCOL OVERVIEW:
 *
 *   EvalGuard::enter():
 *     loop {
 *       ACTIVE_EVALUATORS.fetch_add(1)
 *       if !GC_IN_PROGRESS { break }  // safe to proceed
 *       ACTIVE_EVALUATORS.fetch_sub(1) // back off
 *       spin while GC_IN_PROGRESS
 *     }
 *
 *   EvalGuard::drop():
 *     ACTIVE_EVALUATORS.fetch_sub(1)
 *
 *   maybe_quiescent_gc():
 *     if !GC_REQUESTED { return }
 *     if GC_CYCLE_IN_FLIGHT { return }    // at most one cycle
 *     if ACTIVE_EVALUATORS > 0 { return }
 *     if !CAS(GC_REQUESTED, true, false) { return }
 *     if !CAS(GC_IN_PROGRESS, false, true) {  // try_enter()
 *       GC_REQUESTED = true               // re-arm
 *       return
 *     }
 *     if ACTIVE_EVALUATORS > 0 {          // double-check
 *       GC_IN_PROGRESS = false
 *       GC_REQUESTED = true               // re-arm
 *       return
 *     }
 *     GC_CYCLE_IN_FLIGHT = true
 *     trigger_gc_cycle()                  // build snapshot, send to GC thread
 *     GC_IN_PROGRESS = false
 *
 *   session_release_thread_main():
 *     wait(ACTIVE_EVALUATORS == 0)
 *     if !CAS(GC_IN_PROGRESS, false, true) { retry }
 *     if ACTIVE_EVALUATORS > 0 { GC_IN_PROGRESS = false; retry }
 *     surviving = trace_surviving_set()
 *     GC_IN_PROGRESS = false
 *     for ctx in batch: release_session(ctx, surviving)
 *
 * KEY DESIGN DECISIONS:
 *
 *   TryQuiescentGc is modeled as THREE atomic steps to expose interleavings:
 *     1. AcquireFlag: consume gcRequested, set gcInProgressFlag -> "gc_acquire"
 *     2. SnapshotOK:  double-check passes -> build snapshot, clear flag
 *     3. Abort:       double-check fails  -> re-arm gcRequested, clear flag
 *
 *   Between AcquireFlag and SnapshotOK/Abort, other threads CAN:
 *     - EvalGuardEnter_Increment (incrementing activeEvaluators)
 *     - EvalGuardEnter_BackOff (decrementing after seeing gcInProgressFlag)
 *   This models the real interleaving window in the Rust implementation.
 *
 *   SessionGc is modeled as SIX atomic steps to expose interleavings:
 *     1. WaitQuiescent: session queue non-empty, ACTIVE_EVALUATORS == 0
 *     2. AcquireFlag:   CAS GC_IN_PROGRESS false→true (try_enter succeeds)
 *     3. AcquireFail:   CAS fails (quiescent GC holds the flag)
 *     4. TraceAndRelease: double-check OK, trace roots, release flag
 *     5. DoubleCheckFail: eval snuck in, release flag, retry
 *     6. FreeSession:   free non-surviving values from one session
 *
 * INVARIANT NOTES:
 *
 *   GcFlagConsistent: gcInProgressFlag => no thread in "eval"
 *     (NOT the stronger "activeEvaluators = 0" — an "entering" thread may
 *      have incremented the counter but not yet checked the flag)
 *
 *   ActiveCountCorrect: activeEvaluators = |{t : threadPhase[t] in {"entering","eval"}}|
 *     (threads in "entering" have already incremented but may back off)
 *
 *   SnapshotCapturesAllRoots: when a snapshot is in-flight (hasGcRequest \/
 *     gcPhase /= "idle"), all thread stack roots are empty. This verifies
 *     that the quiescent protocol ensures GC sees ALL live roots.
 *
 *   GcThresholdAdapts: after processing a GC response, gcThreshold is set
 *     to max(respLiveCount * 2, MinGcThreshold), correctly modeling the
 *     adaptive threshold from gc_allocator.rs.
 *
 *   BackpressureLevelBounded: backpressureLevel in 0..MAX_BP (already covered
 *     by TypeOK but listed explicitly for clarity).
 *
 *   GcSweepIsComplete: when the GC thread produces a response, respDeadSet
 *     equals EXACTLY the set of committed, allocated, non-root, non-free-set
 *     values at snapshot time. This verifies that the mark-sweep algorithm
 *     finds ALL dead values — none escape collection.
 *
 *   NoLiveValueInDeadSet: no snapshot root appears in respDeadSet. This is
 *     the snapshot-level complement to NoLiveValueFreed (which checks after
 *     epoch filtering). Together: sweep correctness + TOCTOU safety.
 *
 *   NoStaleFreeSetEntries: no free-set entry points to a released page.
 *     Verifies the drain + filter + rebuild mechanism correctly removes
 *     dangling pointers before munmap.
 *
 *   NoLiveOnReleasedPage: no live value (registered or stack root) resides
 *     on a released (munmap'd) page.
 *
 *   CurrentPageNotReleased: the page currently being bump-allocated from
 *     is never in releasedPages (release_empty_pages() excludes current_page).
 *
 *   ReleasedPagesAreEmpty: all slots on released pages are freed or unbumped.
 *
 *   ContextIdConsistency: all allocated slots have valid context IDs.
 *
 *   SessionGcExcludesQuiescentGc: mutual exclusion — no thread can be in
 *     "gc_acquire" while session GC holds GC_IN_PROGRESS.
 *
 * FAIRNESS NOTES:
 *
 *   Strong fairness (SF) is used for four action groups:
 *
 *   1. ContinueEval — repeatedly enabled at "between" but disabled when
 *      AcquireFlag transitions to "gc_acquire", OR when Tier 2 backpressure
 *      blocks re-entry. Without SF, a thread could loop indefinitely servicing
 *      GC cycles without ever resuming evaluation. SF ensures threads
 *      eventually resume — backpressure relaxes after GC completes.
 *
 *   2. ThreadDone — same pattern as ContinueEval. A thread at "between"
 *      with threadExprs >= MaxExprs should terminate, but GC servicing can
 *      briefly disable it via "gc_acquire". SF ensures threads eventually
 *      terminate. In the real system, ThreadDone is falling out of the
 *      for-loop — it happens naturally after the last expression.
 *
 *   3. SnapshotOK / Abort — enabled when activeEvaluators=0 at "gc_acquire"
 *      but briefly disabled by other threads' EvalGuardEnter_Increment.
 *      SF ensures snapshot building eventually completes despite concurrent
 *      enter/backoff cycles. In reality, GC_IN_PROGRESS is held for
 *      sub-millisecond, so forward progress is always achieved.
 *
 *   4. SessionGc_TraceAndRelease / DoubleCheckFail — same pattern as
 *      quiescent GC: enabled when activeEvaluators=0 but briefly disabled
 *      by EvalGuardEnter_Increment. SF ensures session GC eventually
 *      completes root tracing.
 *
 * RELATIONSHIP TO SlabGC_Reactive:
 *   SlabGC_Reactive models single-thread GC with quiescent points.
 *   This model generalizes to multiple threads with lock-free coordination.
 *   SlabGC_Reactive is a specialization (NumEvalThreads=1).
 *)

EXTENDS Integers, FiniteSets, Sequences

CONSTANTS
    NumEvalThreads,     \* Number of concurrent eval threads (e.g., 2 or 3)
    MaxSlots,           \* Total allocatable value slots
    MaxRoots,           \* Maximum root set size (registered + stack combined)
    MaxExprs,           \* Maximum expressions per thread before stopping
    AllocDeltaThreshold,\* Allocation delta per monitor poll to trigger rate-based GC
                        \* Models ALLOC_RATE_THRESHOLD (abstracted from rate to delta
                        \* since TLA+ doesn't model real time — the monitor polls
                        \* non-deterministically, so rate = delta / poll_interval)
    MinGcThreshold,     \* Minimum GC threshold (slot count)
                        \* Models MIN_GC_THRESHOLD from gc_allocator.rs
                        \* gc_threshold never drops below this value
    SlotsPerPage,       \* Number of slots per page (e.g., 2)
                        \* Models PAGE_SIZE / slot_size in gc_allocator.rs
    MaxContextIds       \* Session context IDs: {0=persistent, 1..MaxContextIds=sessions}
                        \* Set to 1 for minimal state space (persistent vs one session)
                        \* Models NEXT_CONTEXT_ID: AtomicU32 from gc_allocator.rs

\* Maximum backpressure level (fixed, not a CONSTANT — mirrors MAX_BACKPRESSURE: u8 = 3)
MAX_BP == 3

Threads == 1..NumEvalThreads
Slots == 1..MaxSlots
ContextIds == 0..MaxContextIds  \* 0 = persistent, 1..MaxContextIds = session

\* Number of pages (ceiling division)
NumPages == (MaxSlots + SlotsPerPage - 1) \div SlotsPerPage
PageSet == 1..NumPages

\* Which page a slot belongs to
PageOf(s) == ((s - 1) \div SlotsPerPage) + 1

\* All slots in a page
SlotsInPage(p) == {s \in Slots : PageOf(s) = p}

VARIABLES
    (*---------------------------------------------------------------*)
    (* Per-Thread State                                              *)
    (*---------------------------------------------------------------*)
    threadPhase,    \* [Threads -> {"idle","entering","eval","between","gc_acquire","done"}]
    threadExprs,    \* [Threads -> 0..MaxExprs] — expressions evaluated per thread
    stackRoots,     \* [Threads -> SUBSET(Slots)] — per-thread trampoline stack roots
                    \* These are values on the Rust call stack (work_stack,
                    \* continuations, locals) that are NOT registered as GC roots.
                    \* Empty when thread is not in "eval" phase.
    safepointRoots, \* [Threads -> SUBSET(Slots)] — per-thread safepoint roots
                    \* When a thread enters a cooperative GC safepoint during "eval",
                    \* its stackRoots are moved here and become visible to GC via
                    \* collect_all_roots() -> collect_safepoint_roots().
                    \* Non-empty only during "safepoint" and "sp_entering" phases.
                    \* Models SAFEPOINT_ROOTS registry in gc_allocator.rs.

    (*---------------------------------------------------------------*)
    (* Lock-Free Coordination Flags (shared atomics)                 *)
    (*---------------------------------------------------------------*)
    activeEvaluators,   \* Nat — mirrors ACTIVE_EVALUATORS atomic counter
    gcInProgressFlag,   \* BOOLEAN — mirrors GC_IN_PROGRESS atomic flag
    gcRequested,        \* BOOLEAN — mirrors GC_REQUESTED flag (set by cron)

    (*---------------------------------------------------------------*)
    (* Allocator State (simplified — single shared allocator)        *)
    (*---------------------------------------------------------------*)
    slotState,          \* [Slots -> {"free", "alloc", "freed"}]
    bumpPtr,            \* Next slot to bump-allocate
    freeSet,            \* SUBSET(Slots) — freed slots for reuse
    registeredRoots,    \* SUBSET(Slots) — roots in registered providers
                        \* (MettaState source/output + Environment rules/bindings)
                        \* These ARE visible to GC via collect_all_roots().
    epoch,              \* Nat — monotonic epoch counter
    slotEpoch,          \* [Slots -> Nat] — epoch of last free-list alloc

    (*---------------------------------------------------------------*)
    (* Session Context State                                         *)
    (*---------------------------------------------------------------*)
    slotContextId,      \* [Slots -> 0..MaxContextIds] — per-slot session context ID
                        \* 0 = persistent (never released by session GC)
                        \* >0 = session allocation (released on session drop)
                        \* Models context_ids Vec<AtomicU32> in ValuePage
    threadContextId,    \* [Threads -> 0..MaxContextIds] — per-thread active session
                        \* Models THREAD_CONTEXT_ID: thread_local Cell<u32>
                        \* 0 when no session is active
    nextContextId,      \* 1..(MaxContextIds+1) — monotonic counter for session IDs
                        \* Models NEXT_CONTEXT_ID: AtomicU32 (wraps, skips 0)

    (*---------------------------------------------------------------*)
    (* Session GC State                                              *)
    (*---------------------------------------------------------------*)
    sessionReleaseQueue,\* SUBSET(1..MaxContextIds) — pending session releases
    sessionBatch,       \* SUBSET(1..MaxContextIds) — frozen batch from WaitQuiescent
                        \* Captures which sessions to free in this cycle.
                        \* New sessions added during freeing go to next cycle.
                        \* Models mpsc channel to session_release_thread_main
    sessionGcPhase,     \* "idle" | "waiting" | "acquired" | "freeing"
                        \* Models the session release thread's state machine
    sessionSurviving,   \* SUBSET(Slots) — surviving set from root trace
                        \* Models trace_surviving_set() result

    (*---------------------------------------------------------------*)
    (* GC Reachability Heartbeat                                     *)
    (*---------------------------------------------------------------*)
    gcReachableAdvanced,\* BOOLEAN — did GC lifecycle advance since last cron poll?
                        \* Models GC_REACHABLE_COUNTER comparison in gc_cron.rs
                        \* Set TRUE by quiescent GC acquire, ProcessGcResponse,
                        \* and session GC trace. Reset FALSE by CronMonitorPoll.

    (*---------------------------------------------------------------*)
    (* Memory Pressure Statistics                                    *)
    (*---------------------------------------------------------------*)
    allocsSinceLastPoll,\* Nat — allocations since last cron monitor poll
                        \* Incremented by AllocateSlot, reset by CronMonitorPoll.
                        \* Models the delta between prev_alloc_count and current
                        \* alloc_count in MonitorState (gc_cron.rs:827-831).
    gcThreshold,        \* Nat — adaptive GC threshold (in slot count)
                        \* Models gc_threshold from gc_allocator.rs.
                        \* Compared against CommittedSlotCount (bumpPtr - 1).
                        \* Updated after each GC cycle:
                        \*   gcThreshold = max(liveCount * 2, MinGcThreshold)
                        \* This models GC_GROWTH_FACTOR = 2.0 and MIN_GC_THRESHOLD.

    (*---------------------------------------------------------------*)
    (* Backpressure State                                            *)
    (*---------------------------------------------------------------*)
    backpressureLevel,  \* 0..MAX_BP — set by cron monitor, read by eval threads
                        \* Models BACKPRESSURE_LEVEL: AtomicU8 from gc_allocator.rs.
                        \* Computed by CronMonitorPoll based on committed/threshold:
                        \*   0 = none (below threshold)
                        \*   1 = light (>= threshold) — yield
                        \*   2 = medium (>= 1.5x threshold) — sleep(10us)
                        \*   3 = heavy (>= 2x threshold) — sleep(100us)
                        \* Recomputed by ProcessGcResponse for faster feedback.

    (*---------------------------------------------------------------*)
    (* Page Release State                                            *)
    (*---------------------------------------------------------------*)
    releasedPages,      \* SUBSET(PageSet) — pages that have been munmap'd
                        \* Models the pages removed by release_empty_pages()
                        \* via swap_remove triggering MmapPage::Drop (munmap).

    (*---------------------------------------------------------------*)
    (* GC Snapshot Channel                                           *)
    (*---------------------------------------------------------------*)
    hasGcRequest,   \* BOOLEAN — pending snapshot for GC thread?
    snapSlotState,  \* Frozen slot states
    snapBumpPtr,    \* Frozen bump pointer
    snapFreeSet,    \* Frozen free set
    snapRoots,      \* Frozen roots (ONLY registeredRoots — NOT stackRoots)
    snapEpoch,      \* Frozen epoch

    (*---------------------------------------------------------------*)
    (* GC Response Channel                                           *)
    (*---------------------------------------------------------------*)
    hasGcResponse,  \* BOOLEAN — pending GC response?
    respDeadSet,    \* Raw dead set from sweep
    respLiveCount,  \* Live count from sweep
    respEpoch,      \* Snapshot epoch for TOCTOU filtering

    (*---------------------------------------------------------------*)
    (* GC Thread State                                               *)
    (*---------------------------------------------------------------*)
    gcPhase,        \* "idle" | "marking" | "sweeping"
    gcMarked        \* SUBSET(Slots) — marked during tracing

vars == <<threadPhase, threadExprs, stackRoots, safepointRoots, activeEvaluators,
          gcInProgressFlag, gcRequested, slotState, bumpPtr, freeSet,
          registeredRoots, epoch, slotEpoch,
          slotContextId, threadContextId, nextContextId,
          sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
          gcReachableAdvanced,
          allocsSinceLastPoll,
          gcThreshold, backpressureLevel, releasedPages,
          hasGcRequest, snapSlotState,
          snapBumpPtr, snapFreeSet, snapRoots, snapEpoch, hasGcResponse,
          respDeadSet, respLiveCount, respEpoch, gcPhase, gcMarked>>

\* Convenience: all session GC variables for UNCHANGED clauses
sessionVars == <<slotContextId, threadContextId, nextContextId,
                 sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving>>

(*=======================================================================*)
(* Helpers                                                               *)
(*=======================================================================*)

CanAllocate ==
    \/ freeSet /= {}
    \/ bumpPtr <= MaxSlots

\* Total safepoint roots across all threads
AllSafepointRoots == UNION {safepointRoots[t] : t \in Threads}

\* Total live roots (registered + all stack roots + all safepoint roots)
AllLiveRoots ==
    registeredRoots \union UNION {stackRoots[t] : t \in Threads} \union AllSafepointRoots

\* Number of committed slots (total bump-allocated, including freed)
\* Models committed_bytes / SLOT_SIZE in the real system.
CommittedSlotCount == bumpPtr - 1

\* Whether a GC cycle is currently in flight (models gc_cycle_in_flight() in Rust).
\* True when there's a pending request, the GC thread is processing, or
\* a response is waiting to be processed.
GcCycleInFlight == hasGcRequest \/ hasGcResponse \/ gcPhase /= "idle"

\* The page currently being used for bump allocation.
\* Derived from bumpPtr — not a separate variable.
\* 0 means no valid current page (all slots exhausted).
\* In Rust, current_page is an AtomicPtr set by alloc_new_page(); it always
\* points to the page containing the next bump slot, which is PageOf(bumpPtr).
CurrentPage == IF bumpPtr <= MaxSlots THEN PageOf(bumpPtr) ELSE 0

\* A page is empty when all its bumped slots have been freed.
\* Models the Rust check: page.live_count <= 0 && page.bump_count > 0
PageIsEmpty(p) ==
    /\ \E s \in SlotsInPage(p) : s < bumpPtr  \* page has been used (bump_count > 0)
    /\ \A s \in SlotsInPage(p) :
        s >= bumpPtr \/ slotState[s] = "freed"  \* all bumped slots are freed

\* Upper bound on epoch for TypeOK.
\* +1 for the session GC sentinel epoch (MaxEpoch + 1 models u64::MAX).
\* Defined here (in Helpers) because SessionGc_FreeSession uses MaxEpoch + 1.
MaxEpoch == MaxSlots * (MaxExprs + 2) * MaxRoots * NumEvalThreads + 1

\* Compute backpressure level from committed/threshold ratio.
\* Shared helper used by both CronMonitorPoll and ProcessGcResponse.
ComputeBackpressure(committed, threshold) ==
    IF threshold > 0
    THEN IF committed >= threshold * 2 THEN 3
         ELSE IF committed >= (threshold * 3) \div 2 THEN 2
         ELSE IF committed >= threshold THEN 1
         ELSE 0
    ELSE 0

(*=======================================================================*)
(* Initial State                                                         *)
(*=======================================================================*)

Init ==
    /\ threadPhase = [t \in Threads |-> "idle"]
    /\ threadExprs = [t \in Threads |-> 0]
    /\ stackRoots = [t \in Threads |-> {}]
    /\ safepointRoots = [t \in Threads |-> {}]
    /\ activeEvaluators = 0
    /\ gcInProgressFlag = FALSE
    /\ gcRequested = FALSE
    /\ slotState = [s \in Slots |-> "free"]
    /\ bumpPtr = 1
    /\ freeSet = {}
    /\ registeredRoots = {}
    /\ epoch = 0
    /\ slotEpoch = [s \in Slots |-> 0]
    /\ slotContextId = [s \in Slots |-> 0]
    /\ threadContextId = [t \in Threads |-> 0]
    /\ nextContextId = 1
    /\ sessionReleaseQueue = {}
    /\ sessionBatch = {}
    /\ sessionGcPhase = "idle"
    /\ sessionSurviving = {}
    /\ gcReachableAdvanced = FALSE
    /\ allocsSinceLastPoll = 0
    /\ gcThreshold = MinGcThreshold
    /\ backpressureLevel = 0
    /\ releasedPages = {}
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
    /\ gcPhase = "idle"
    /\ gcMarked = {}

(*=======================================================================*)
(* EvalGuard Protocol                                                    *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* EvalGuardEnter_Increment: Thread increments ACTIVE_EVALUATORS.        *)
(* First step of EvalGuard::enter(). Thread transitions to "entering".   *)
(*                                                                        *)
(* Context ID is NOT assigned here — it's assigned in Proceed to avoid   *)
(* leaking IDs on BackOff. This matches Rust where the SessionGuard is   *)
(* created only when eval actually starts.                                *)
(*-----------------------------------------------------------------------*)
EvalGuardEnter_Increment(t) ==
    /\ threadPhase[t] = "idle"
    /\ threadExprs[t] < MaxExprs
    /\ activeEvaluators' = activeEvaluators + 1
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "entering"]
    /\ UNCHANGED <<threadExprs, stackRoots, gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalGuardEnter_Proceed: Thread checks GC_IN_PROGRESS is clear.       *)
(* Safe to proceed -> transitions to "eval".                              *)
(*                                                                        *)
(* SESSION CONTEXT: Assigns a new session context ID to the thread.      *)
(* Models SessionGuard::new() which atomically fetches NEXT_CONTEXT_ID.  *)
(* Assigned here (not in _Increment) so BackOff doesn't leak IDs.       *)
(* IDs are monotonically increasing (matching Rust's AtomicU64), never   *)
(* wrap — preventing aliasing with pending session releases.             *)
(*-----------------------------------------------------------------------*)
EvalGuardEnter_Proceed(t) ==
    /\ threadPhase[t] = "entering"
    /\ ~gcInProgressFlag
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "eval"]
    \* Assign session context ID (models SessionGuard::new())
    /\ threadContextId' = [threadContextId EXCEPT ![t] = nextContextId]
    /\ nextContextId' = nextContextId + 1   \* Monotonic, no wrapping
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested, slotState, bumpPtr,
                   freeSet, registeredRoots, epoch, slotEpoch,
                   slotContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalGuardEnter_BackOff: Thread sees GC_IN_PROGRESS is set.            *)
(* Back off: decrement counter, return to "idle" to retry.               *)
(* No context ID was assigned (it's assigned in Proceed, not Increment). *)
(*-----------------------------------------------------------------------*)
EvalGuardEnter_BackOff(t) ==
    /\ threadPhase[t] = "entering"
    /\ gcInProgressFlag
    \* Back off: decrement counter, go back to idle to retry
    /\ activeEvaluators' = activeEvaluators - 1
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<threadExprs, stackRoots, gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalGuardDrop: Thread finishes eval, decrements ACTIVE_EVALUATORS.    *)
(* Transitions to "between" (quiescent point).                           *)
(*                                                                        *)
(* CRITICAL: All stack roots are transferred to registered roots.         *)
(* This models eval_trampoline() returning results that the caller        *)
(* stores in MettaState.output (a registered root provider).              *)
(* Some values may also be dropped (non-deterministic subset transfer).   *)
(*                                                                        *)
(* SESSION RELEASE: Clears the thread's context ID and enqueues the      *)
(* session for async release (models SessionGuard::drop).                *)
(*-----------------------------------------------------------------------*)
EvalGuardDrop(t) ==
    /\ threadPhase[t] = "eval"
    /\ activeEvaluators' = activeEvaluators - 1
    /\ threadExprs' = [threadExprs EXCEPT ![t] = threadExprs[t] + 1]
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "between"]
    \* Transfer surviving stack roots to registered roots.
    \* Non-deterministically choose which stack values survive (some may be
    \* intermediate computation results that are no longer needed).
    /\ \E surviving \in SUBSET stackRoots[t] :
        /\ registeredRoots' = registeredRoots \union surviving
        /\ stackRoots' = [stackRoots EXCEPT ![t] = {}]
    \* Session release: clear context ID, enqueue for async release
    /\ LET ctxId == threadContextId[t]
       IN /\ threadContextId' = [threadContextId EXCEPT ![t] = 0]
          /\ sessionReleaseQueue' =
              IF ctxId > 0
              THEN sessionReleaseQueue \union {ctxId}
              ELSE sessionReleaseQueue
    /\ UNCHANGED <<gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, epoch, slotEpoch,
                   slotContextId, nextContextId,
                   sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* Eval Thread Actions During "eval" Phase                               *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* AllocateSlot: Thread allocates a value during evaluation.             *)
(* The value goes to the thread's STACK ROOTS (trampoline call stack),   *)
(* NOT directly to registered roots.                                     *)
(*                                                                        *)
(* Also increments allocsSinceLastPoll — this tracks the allocation rate *)
(* delta that the cron monitor uses to detect allocation pressure.        *)
(*                                                                        *)
(* SESSION CONTEXT: Tags the allocated slot with the thread's current     *)
(* session context ID (models page.set_context_id(idx, ctx_id)).         *)
(*                                                                        *)
(* PAGE RELEASE SAFETY: The bump allocation path includes a guard that   *)
(* the target page has not been released. This is a safety assertion     *)
(* (should always hold) since bump allocation only advances forward and  *)
(* release_empty_pages() excludes the current page.                      *)
(*-----------------------------------------------------------------------*)
AllocateSlot(t) ==
    /\ threadPhase[t] = "eval"
    /\ Cardinality(AllLiveRoots) < MaxRoots
    /\ CanAllocate
    /\ IF freeSet /= {}
       THEN \E s \in freeSet :
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ freeSet' = freeSet \ {s}
               /\ bumpPtr' = bumpPtr
               /\ stackRoots' = [stackRoots EXCEPT ![t] = stackRoots[t] \union {s}]
               /\ epoch' = epoch + 1
               /\ slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
               /\ slotContextId' = [slotContextId EXCEPT ![s] = threadContextId[t]]
       ELSE LET s == bumpPtr
            IN /\ PageOf(s) \notin releasedPages  \* Safety: never bump into released page
               /\ slotState' = [slotState EXCEPT ![s] = "alloc"]
               /\ bumpPtr' = bumpPtr + 1
               /\ freeSet' = freeSet
               /\ stackRoots' = [stackRoots EXCEPT ![t] = stackRoots[t] \union {s}]
               /\ epoch' = epoch
               /\ slotEpoch' = slotEpoch
               /\ slotContextId' = [slotContextId EXCEPT ![s] = threadContextId[t]]
    /\ allocsSinceLastPoll' = allocsSinceLastPoll + 1
    /\ UNCHANGED <<threadPhase, threadExprs, activeEvaluators,
                   gcInProgressFlag, gcRequested, registeredRoots,
                   threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   gcThreshold, backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* DropStackRoot: A stack root becomes unreachable during evaluation.     *)
(* Models intermediate computation results that are consumed/discarded.   *)
(*-----------------------------------------------------------------------*)
DropStackRoot(t) ==
    /\ threadPhase[t] = "eval"
    /\ stackRoots[t] /= {}
    /\ \E s \in stackRoots[t] :
        /\ stackRoots' = [stackRoots EXCEPT ![t] = stackRoots[t] \ {s}]
    /\ UNCHANGED <<threadPhase, threadExprs, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* PublishRoot: A stack value is stored in a registered root provider     *)
(* during evaluation (e.g., add_rule stores in environment, or a value   *)
(* is pushed to MettaState.output). The value moves from stack to        *)
(* registered roots.                                                     *)
(*-----------------------------------------------------------------------*)
PublishRoot(t) ==
    /\ threadPhase[t] = "eval"
    /\ stackRoots[t] /= {}
    /\ \E s \in stackRoots[t] :
        /\ stackRoots' = [stackRoots EXCEPT ![t] = stackRoots[t] \ {s}]
        /\ registeredRoots' = registeredRoots \union {s}
    /\ UNCHANGED <<threadPhase, threadExprs, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, epoch, slotEpoch,
                   slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* DropRegisteredRoot: A registered root becomes unreachable.             *)
(* Models clearing MettaState.output, removing environment rules, etc.   *)
(*-----------------------------------------------------------------------*)
DropRegisteredRoot(t) ==
    /\ threadPhase[t] \in {"eval", "between"}
    /\ registeredRoots /= {}
    /\ \E s \in registeredRoots :
        /\ registeredRoots' = registeredRoots \ {s}
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, epoch, slotEpoch,
                   slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* Intra-Evaluation GC Safepoints                                        *)
(*=======================================================================*)
(*
 * Cooperative GC safepoints allow an evaluator to temporarily pause
 * mid-evaluation, register its trampoline roots, and release the
 * EvalGuard. This enables the quiescent GC to fire during long-running
 * evaluations (e.g., PLN Robot) that would otherwise hold the guard
 * for minutes, preventing GC and accumulating dead objects.
 *
 * The protocol mirrors the EvalGuard enter/backoff pattern:
 *   EvalSafepoint:                "eval"        -> "safepoint"
 *   EvalSafepointResume_Increment: "safepoint"  -> "sp_entering"
 *   EvalSafepointResume_Proceed:   "sp_entering" -> "eval"
 *   EvalSafepointResume_BackOff:   "sp_entering" -> "safepoint"
 *
 * Models: register_temporary_roots(), drop_eval_guard_for_safepoint(),
 *         reacquire_eval_guard_after_safepoint() in gc_allocator.rs
 *)

(*-----------------------------------------------------------------------*)
(* EvalSafepoint: Thread enters cooperative GC safepoint.                *)
(* Moves stack roots to safepoint roots (visible to GC via               *)
(* collect_safepoint_roots()) and decrements ACTIVE_EVALUATORS.          *)
(*                                                                        *)
(* CRITICAL ORDERING: Roots are registered BEFORE dropping the guard.    *)
(* This prevents a window where ACTIVE_EVALUATORS == 0 but roots are    *)
(* not visible to GC (would cause use-after-free).                       *)
(*                                                                        *)
(* Models: register_temporary_roots(roots) then                           *)
(*         drop_eval_guard_for_safepoint() in session_context.rs          *)
(*-----------------------------------------------------------------------*)
EvalSafepoint(t) ==
    /\ threadPhase[t] = "eval"
    \* Move stack roots to safepoint roots (visible to GC)
    /\ safepointRoots' = [safepointRoots EXCEPT ![t] = stackRoots[t]]
    /\ stackRoots' = [stackRoots EXCEPT ![t] = {}]
    \* Decrement ACTIVE_EVALUATORS (may trigger quiescent state)
    /\ activeEvaluators' = activeEvaluators - 1
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "safepoint"]
    /\ UNCHANGED <<threadExprs, gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalSafepointResume_Increment: Thread starts re-acquiring EvalGuard.  *)
(* Increments ACTIVE_EVALUATORS (same as EvalGuardEnter_Increment).      *)
(*                                                                        *)
(* Models: reacquire_eval_guard_after_safepoint() first step —            *)
(*         ACTIVE_EVALUATORS.fetch_add(1) in gc_allocator.rs              *)
(*-----------------------------------------------------------------------*)
EvalSafepointResume_Increment(t) ==
    /\ threadPhase[t] = "safepoint"
    /\ activeEvaluators' = activeEvaluators + 1
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "sp_entering"]
    /\ UNCHANGED <<threadExprs, stackRoots, safepointRoots,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalSafepointResume_Proceed: GC_IN_PROGRESS is clear, safe to resume. *)
(* Moves safepoint roots back to stack roots and returns to "eval".      *)
(*                                                                        *)
(* Models: reacquire_eval_guard_after_safepoint() second step —           *)
(*         check GC_IN_PROGRESS, proceed if clear. Then                   *)
(*         SafepointRootHandle::drop() unregisters temporary roots.       *)
(*-----------------------------------------------------------------------*)
EvalSafepointResume_Proceed(t) ==
    /\ threadPhase[t] = "sp_entering"
    /\ ~gcInProgressFlag
    \* Restore roots from safepoint to stack
    /\ stackRoots' = [stackRoots EXCEPT ![t] = safepointRoots[t]]
    /\ safepointRoots' = [safepointRoots EXCEPT ![t] = {}]
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "eval"]
    /\ UNCHANGED <<threadExprs, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* EvalSafepointResume_BackOff: GC_IN_PROGRESS is set.                   *)
(* Back off: decrement counter, return to "safepoint" to retry.          *)
(* Same pattern as EvalGuardEnter_BackOff.                               *)
(*                                                                        *)
(* Models: reacquire_eval_guard_after_safepoint() seeing GC_IN_PROGRESS, *)
(*         decrementing and retrying.                                     *)
(*-----------------------------------------------------------------------*)
EvalSafepointResume_BackOff(t) ==
    /\ threadPhase[t] = "sp_entering"
    /\ gcInProgressFlag
    /\ activeEvaluators' = activeEvaluators - 1
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "safepoint"]
    /\ UNCHANGED <<threadExprs, stackRoots, safepointRoots,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* Cron Monitor — Memory Pressure Detection + Backpressure Computation   *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* CronMonitorPoll: Cron monitor polls allocation statistics.            *)
(*                                                                        *)
(* Models execute_memory_monitor() in gc_cron.rs (lines 948-991).        *)
(* The monitor reads allocation counters and checks two triggers:         *)
(*                                                                        *)
(*   1. RATE-BASED trigger: allocsSinceLastPoll >= AllocDeltaThreshold    *)
(*      Models: rate = delta_allocs / elapsed_time > ALLOC_RATE_THRESHOLD *)
(*      (TLA+ abstracts time — delta alone suffices since the poll fires  *)
(*       non-deterministically and WF ensures it fires eventually)        *)
(*                                                                        *)
(*   2. THRESHOLD-BASED trigger: CommittedSlotCount >= gcThreshold        *)
(*      Models: committed_bytes >= gc_threshold (gc_allocator.rs)         *)
(*      CommittedSlotCount = bumpPtr - 1 (total bump-allocated slots)     *)
(*                                                                        *)
(* After checking, the monitor resets its baseline (prevAllocCount :=     *)
(* currentAllocCount in Rust, modeled as allocsSinceLastPoll := 0).       *)
(*                                                                        *)
(* BACKPRESSURE COMPUTATION (gc_cron.rs:978-991):                         *)
(* The monitor also computes backpressure level based on committed bytes  *)
(* relative to the GC threshold:                                          *)
(*   Level 0: committed < gcThreshold          — no throttling            *)
(*   Level 1: committed >= gcThreshold         — yield                    *)
(*   Level 2: committed >= gcThreshold * 3/2   — sleep(10us)             *)
(*   Level 3: committed >= gcThreshold * 2     — sleep(100us)            *)
(*                                                                        *)
(* GC REACHABILITY GATING: Backpressure is only escalated when the GC    *)
(* lifecycle is reachable (gcReachableAdvanced = TRUE). If no code path   *)
(* is calling maybe_quiescent_gc() / maybe_process_gc_response(), we set *)
(* backpressure to 0 to avoid permanent throttling.                       *)
(*                                                                        *)
(* The poll fires non-deterministically (TLA+ doesn't model real time).   *)
(* WF(CronMonitorPoll) ensures it fires when continuously enabled.        *)
(* It is enabled as long as at least one thread has not terminated.        *)
(*-----------------------------------------------------------------------*)
CronMonitorPoll ==
    \* Monitor runs while the system is active (at least one non-done thread)
    /\ \E t \in Threads : threadPhase[t] /= "done"
    /\ LET rateTrigger == allocsSinceLastPoll >= AllocDeltaThreshold
           thresholdTrigger == CommittedSlotCount >= gcThreshold
       IN IF rateTrigger \/ thresholdTrigger
          THEN gcRequested' = TRUE
          ELSE gcRequested' = gcRequested
    /\ allocsSinceLastPoll' = 0
    \* Compute backpressure level, gated on GC reachability
    /\ backpressureLevel' =
        IF ~gcReachableAdvanced THEN 0  \* GC unreachable — don't escalate
        ELSE ComputeBackpressure(CommittedSlotCount, gcThreshold)
    \* Reset heartbeat
    /\ gcReachableAdvanced' = FALSE
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcThreshold, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* Quiescent-Point Actions (thread in "between" phase)                   *)
(*=======================================================================*)

(*-----------------------------------------------------------------------*)
(* TryQuiescentGc_AcquireFlag: Thread at quiescent point begins GC       *)
(* attempt. Consumes gcRequested and sets gcInProgressFlag.               *)
(*                                                                        *)
(* Preconditions model the Rust GC_CYCLE_IN_FLIGHT check:                 *)
(*   ~hasGcRequest  — no pending snapshot in the mpsc channel             *)
(*   ~hasGcResponse — no pending response (GC_CYCLE_IN_FLIGHT == false)   *)
(*   gcPhase="idle" — GC thread is not processing a snapshot              *)
(* Together these ensure at most one GC cycle is in flight at a time.     *)
(*                                                                        *)
(* MUTUAL EXCLUSION: Requires sessionGcPhase = "idle" to prevent         *)
(* quiescent GC from acquiring while session GC holds the flag.           *)
(* Models try_enter() CAS — if session GC holds the flag, CAS fails.     *)
(*                                                                        *)
(* This is the FIRST of three steps (AcquireFlag -> SnapshotOK/Abort).    *)
(* Between this step and the next, other threads CAN interleave:          *)
(*   - EvalGuardEnter_Increment (incrementing activeEvaluators)           *)
(*   - EvalGuardEnter_BackOff   (seeing flag, decrementing)              *)
(* This models the real interleaving window in the Rust implementation.   *)
(*-----------------------------------------------------------------------*)
TryQuiescentGc_AcquireFlag(t) ==
    /\ threadPhase[t] = "between"
    /\ gcRequested
    /\ activeEvaluators = 0
    /\ ~hasGcRequest          \* No pending request already
    /\ ~hasGcResponse         \* No pending response (models GC_CYCLE_IN_FLIGHT)
    /\ ~gcInProgressFlag      \* Not already in progress (try_enter CAS)
    /\ gcPhase = "idle"       \* GC thread is idle (not reading snap* vars)
    /\ sessionGcPhase \notin {"acquired"}  \* Session GC not holding flag
    \* Consume GC request and set flag atomically (models CAS + store)
    /\ gcRequested' = FALSE
    /\ gcInProgressFlag' = TRUE
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "gc_acquire"]
    \* Signal GC reachability (models bump_gc_reachable() in maybe_quiescent_gc)
    /\ gcReachableAdvanced' = TRUE
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* TryQuiescentGc_SnapshotOK: Double-check passes (no thread snuck in).  *)
(* Build snapshot, send to GC thread, clear flag.                         *)
(*                                                                        *)
(* CRITICAL: snapRoots captures registeredRoots AND safepointRoots        *)
(* (what GC sees via collect_all_roots() -> collect_safepoint_roots()).   *)
(* Stack roots are NOT included because they are not registered as root  *)
(* providers. The quiescent invariant ensures stack roots are empty at    *)
(* this point (activeEvaluators == 0). Safepoint roots are visible       *)
(* because threads at "safepoint" have registered them before dropping   *)
(* their EvalGuard.                                                      *)
(*-----------------------------------------------------------------------*)
TryQuiescentGc_SnapshotOK(t) ==
    /\ threadPhase[t] = "gc_acquire"
    /\ activeEvaluators = 0       \* Double-check: still quiescent
    \* BUILD SNAPSHOT: copy allocator state
    /\ hasGcRequest' = TRUE
    /\ snapSlotState' = slotState
    /\ snapBumpPtr' = bumpPtr
    /\ snapFreeSet' = freeSet
    /\ snapRoots' = registeredRoots \union AllSafepointRoots   \* registered + safepoint roots
    /\ snapEpoch' = epoch
    \* Clear flag and return to "between"
    /\ gcInProgressFlag' = FALSE
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "between"]
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcResponse, respDeadSet, respLiveCount,
                   respEpoch, safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* TryQuiescentGc_Abort: Double-check fails (a thread snuck in).         *)
(* Re-arm gcRequested, clear flag, return to "between".                   *)
(*-----------------------------------------------------------------------*)
TryQuiescentGc_Abort(t) ==
    /\ threadPhase[t] = "gc_acquire"
    /\ activeEvaluators > 0       \* Someone snuck in -> abort
    \* Re-arm GC request so it will be retried
    /\ gcRequested' = TRUE
    /\ gcInProgressFlag' = FALSE
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "between"]
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* ProcessGcResponse: Process pending GC response at quiescent point.    *)
(* Uses epoch-based TOCTOU filtering from SlabGC_Reactive.               *)
(*                                                                        *)
(* ADAPTIVE THRESHOLD: After processing the GC response, the gc_threshold*)
(* is updated based on the number of live values found during collection: *)
(*   gc_threshold = max(live_count * GC_GROWTH_FACTOR, MIN_GC_THRESHOLD) *)
(* This models gc_allocator.rs:1320-1322:                                 *)
(*   let new_threshold = (response.live_bytes * GC_GROWTH_FACTOR) as usize*)
(*   alloc.set_gc_threshold(new_threshold.max(MIN_GC_THRESHOLD))          *)
(* GC_GROWTH_FACTOR = 2.0, so we use live_count * 2 in the model.        *)
(*                                                                        *)
(* FASTER BACKPRESSURE FEEDBACK: After processing the response and        *)
(* updating the threshold, backpressure is immediately recomputed from    *)
(* the new committed/threshold ratio (matching Rust lines 2030-2035).     *)
(* This provides faster feedback than waiting for the next cron poll.     *)
(*                                                                        *)
(* PAGE RELEASE: After freeing dead slots, identifies empty pages         *)
(* (excluding CurrentPage), filters free-set entries from those pages,    *)
(* and adds them to releasedPages. Models the atomic drain + filter +     *)
(* rebuild mechanism in release_empty_pages().                            *)
(*-----------------------------------------------------------------------*)
ProcessGcResponse(t) ==
    /\ threadPhase[t] = "between"
    /\ hasGcResponse
    /\ LET rawDead == respDeadSet
           safeDead == {s \in rawDead : slotEpoch[s] <= respEpoch}
           \* Compute new slot states after freeing safeDead
           newSlotState == [s \in Slots |->
               IF s \in safeDead THEN "freed" ELSE slotState[s]]
           \* Identify empty pages (excluding CurrentPage)
           emptyPages == {p \in PageSet :
               /\ p /= CurrentPage
               /\ p \notin releasedPages
               /\ \E s \in SlotsInPage(p) : s < bumpPtr    \* page has been used
               /\ \A s \in SlotsInPage(p) :
                   s >= bumpPtr \/ newSlotState[s] = "freed" \* all bumped slots freed
           }
           \* Drain + filter: remove free-set entries from pages being released
           filteredFreeSet == (freeSet \union safeDead) \
                              {s \in Slots : PageOf(s) \in emptyPages}
       IN /\ slotState' = newSlotState
          /\ freeSet' = filteredFreeSet
          /\ releasedPages' = releasedPages \union emptyPages
    \* Adaptive threshold: max(liveCount * GC_GROWTH_FACTOR, MinGcThreshold)
    \* GC_GROWTH_FACTOR = 2.0 -> liveCount * 2
    /\ LET newThreshold == LET candidate == respLiveCount * 2
                            IN IF candidate >= MinGcThreshold
                               THEN candidate
                               ELSE MinGcThreshold
       IN /\ gcThreshold' = newThreshold
          \* Recompute backpressure from new committed/threshold ratio
          \* (matches Rust maybe_process_gc_response() lines 2030-2035)
          /\ backpressureLevel' = ComputeBackpressure(CommittedSlotCount, newThreshold)
    \* Signal GC reachability (models bump_gc_reachable() in maybe_process_gc_response)
    /\ gcReachableAdvanced' = TRUE
    /\ hasGcResponse' = FALSE
    /\ respDeadSet' = {}
    /\ respLiveCount' = 0
    /\ respEpoch' = 0
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   bumpPtr, registeredRoots, epoch, slotEpoch,
                   slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   allocsSinceLastPoll,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* ContinueEval: Thread resumes evaluation (re-enters via EvalGuard).    *)
(*                                                                        *)
(* TIER 2 BACKPRESSURE: At maximum backpressure level, a thread at       *)
(* "between" cannot re-enter eval while a GC cycle is in flight. This    *)
(* models apply_backpressure_tier2() in main.rs which spin-yields until  *)
(* gc_cycle_in_flight() returns false.                                    *)
(*                                                                        *)
(* CRITICAL: ProcessGcResponse, TryQuiescentGc_AcquireFlag, and          *)
(* ThreadDone remain enabled at "between" regardless of backpressure     *)
(* level. Only ContinueEval (re-entry into eval) is throttled.            *)
(*-----------------------------------------------------------------------*)
ContinueEval(t) ==
    /\ threadPhase[t] = "between"
    /\ threadExprs[t] < MaxExprs   \* Still has expressions to evaluate
    \* Tier 2 backpressure: at max level, block until GC cycle completes
    /\ ~(backpressureLevel >= MAX_BP /\ GcCycleInFlight)
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "idle"]
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* ThreadDone: Thread has evaluated all its expressions.                  *)
(*-----------------------------------------------------------------------*)
ThreadDone(t) ==
    /\ threadPhase[t] = "between"
    /\ threadExprs[t] >= MaxExprs
    /\ threadPhase' = [threadPhase EXCEPT ![t] = "done"]
    /\ UNCHANGED <<threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested, slotState, bumpPtr,
                   freeSet, registeredRoots, epoch, slotEpoch,
                   slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold, backpressureLevel,
                   releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* GC Thread Actions (reads only snap* variables, runs concurrently)     *)
(*=======================================================================*)

GcReceiveRequest ==
    /\ gcPhase = "idle"
    /\ hasGcRequest
    /\ gcMarked' = {}
    /\ gcPhase' = "marking"
    /\ hasGcRequest' = FALSE
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   snapSlotState, snapBumpPtr, snapFreeSet,
                   snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots>>

GcMarkStep ==
    /\ gcPhase = "marking"
    /\ \E s \in snapRoots :
        /\ s \notin gcMarked
        /\ snapSlotState[s] = "alloc"
        /\ gcMarked' = gcMarked \union {s}
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase>>

GcMarkComplete ==
    /\ gcPhase = "marking"
    /\ \A s \in snapRoots : (snapSlotState[s] = "alloc") => (s \in gcMarked)
    /\ gcPhase' = "sweeping"
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcMarked>>

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
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionGcPhase, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   safepointRoots,
                   snapFreeSet, snapRoots, snapEpoch>>

(*=======================================================================*)
(* Session-Based GC Thread Actions                                       *)
(*=======================================================================*)
(*
 * The session release thread (session_release_thread_main) runs as a
 * dedicated background thread. It processes batched session releases:
 *
 *   1. Wait for quiescence (ACTIVE_EVALUATORS == 0)
 *   2. Acquire GC_IN_PROGRESS via CAS (try_enter)
 *   3. Double-check quiescence
 *   4. Trace surviving set (root scan at quiescent point)
 *   5. Release GC_IN_PROGRESS (evals can resume)
 *   6. For each released session:
 *      - Promote surviving values (ctx -> 0)
 *      - Free non-surviving session values
 *      - Release empty pages
 *
 * Mutual exclusion with quiescent GC is via CAS on GC_IN_PROGRESS:
 *   - Quiescent GC: try_enter() in maybe_quiescent_gc()
 *   - Session GC: try_enter() in session_release_thread_main()
 *   Only one can succeed at a time.
 *)

(*-----------------------------------------------------------------------*)
(* SessionGc_WaitQuiescent: Background thread detects pending session     *)
(* releases and waits for quiescence. Transitions from "idle" to         *)
(* "waiting" when the release queue is non-empty and no evaluators are   *)
(* active.                                                                *)
(*                                                                        *)
(* Models the condvar wait loop in session_release_thread_main():         *)
(*   while ACTIVE_EVALUATORS.load() > 0 { QUIESCENT_CONDVAR.wait() }     *)
(*-----------------------------------------------------------------------*)
SessionGc_WaitQuiescent ==
    /\ sessionGcPhase = "idle"
    /\ sessionReleaseQueue /= {}
    /\ activeEvaluators = 0
    /\ sessionGcPhase' = "waiting"
    \* Freeze current release queue as the batch for this cycle.
    \* Models Rust's channel drain at the top of the loop (recv + try_recv).
    \* New sessions added during freeing will be handled in the next cycle.
    /\ sessionBatch' = sessionReleaseQueue
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* SessionGc_AcquireFlag: CAS GC_IN_PROGRESS false→true (try_enter).    *)
(* Models the CAS loop in session_release_thread_main().                 *)
(*-----------------------------------------------------------------------*)
SessionGc_AcquireFlag ==
    /\ sessionGcPhase = "waiting"
    /\ ~gcInProgressFlag                    \* CAS(false→true) succeeds
    /\ gcInProgressFlag' = TRUE
    /\ sessionGcPhase' = "acquired"
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* SessionGc_AcquireFail: CAS fails (quiescent GC holds the flag).      *)
(* Retry from idle state.                                                *)
(* Models the spin + condvar wait loop on GC_IN_PROGRESS.                *)
(*-----------------------------------------------------------------------*)
SessionGc_AcquireFail ==
    /\ sessionGcPhase = "waiting"
    /\ gcInProgressFlag                     \* CAS fails
    /\ sessionGcPhase' = "idle"             \* Retry from idle
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* SessionGc_TraceAndRelease: Double-check passes, trace roots, release  *)
(* GC_IN_PROGRESS. The surviving set is captured at this quiescent point *)
(* and used for all session releases in the batch.                       *)
(*                                                                        *)
(* CRITICAL: GC_IN_PROGRESS is released BEFORE freeing. This is safe     *)
(* because:                                                               *)
(*   - Surviving values are promoted to ctx=0 (persistent)               *)
(*   - Only non-surviving values with the target session ID are freed    *)
(*   - release_empty_pages() only frees live_count=0 pages              *)
(*   - alloc() validates free-list pointers                              *)
(*-----------------------------------------------------------------------*)
SessionGc_TraceAndRelease ==
    /\ sessionGcPhase = "acquired"
    /\ activeEvaluators = 0                  \* Double-check: still quiescent
    \* Trace surviving set = registered roots + safepoint roots at quiescent point
    \* (stackRoots are empty at quiescence — verified by SnapshotCapturesAllRoots)
    /\ sessionSurviving' = registeredRoots \union AllSafepointRoots
    \* Release GC_IN_PROGRESS — evals can resume
    /\ gcInProgressFlag' = FALSE
    /\ sessionGcPhase' = "freeing"
    \* Signal GC reachability (models bump_gc_reachable())
    /\ gcReachableAdvanced' = TRUE
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* SessionGc_DoubleCheckFail: An eval snuck in between condvar wake      *)
(* and double-check. Abort: release flag and retry from idle.            *)
(*-----------------------------------------------------------------------*)
SessionGc_DoubleCheckFail ==
    /\ sessionGcPhase = "acquired"
    /\ activeEvaluators > 0                  \* Eval snuck in
    /\ gcInProgressFlag' = FALSE
    /\ sessionGcPhase' = "idle"              \* Retry
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcRequested,
                   slotState, bumpPtr, freeSet, registeredRoots, epoch,
                   slotEpoch, slotContextId, threadContextId, nextContextId,
                   sessionReleaseQueue, sessionBatch, sessionSurviving,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel, releasedPages,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*-----------------------------------------------------------------------*)
(* SessionGc_FreeSession: Free non-surviving values from one session.    *)
(*                                                                        *)
(* For each session context ID in the release queue:                      *)
(*   - Identify session slots (alloc'd with the target context ID)       *)
(*   - Surviving = intersection with sessionSurviving                    *)
(*   - Dead = session slots NOT in sessionSurviving                      *)
(*   - Promote surviving to persistent (ctx -> 0)                        *)
(*   - Free dead slots (transition to "freed")                           *)
(*   - Release empty pages (same logic as ProcessGcResponse)             *)
(*   - Set freed slot epochs to MaxEpoch+1 sentinel (models u64::MAX)    *)
(*                                                                        *)
(* Models release_session_with_surviving() in gc_allocator.rs.            *)
(*-----------------------------------------------------------------------*)
SessionGc_FreeSession ==
    /\ sessionGcPhase = "freeing"
    \* Only process sessions from the frozen batch (captured at WaitQuiescent).
    \* New sessions added to sessionReleaseQueue during freeing are safe —
    \* they'll be handled in the next cycle.
    /\ \E ctx \in sessionBatch :
        LET sessionSlots == {s \in Slots :
                slotContextId[s] = ctx /\ slotState[s] = "alloc"}
            surviving == sessionSlots \intersect sessionSurviving
            dead == sessionSlots \ sessionSurviving
            \* Compute new slot states after freeing dead
            newSlotState == [s \in Slots |->
                IF s \in dead THEN "freed" ELSE slotState[s]]
            \* Identify empty pages (excluding CurrentPage) — same as ProcessGcResponse
            emptyPages == {p \in PageSet :
                /\ p /= CurrentPage
                /\ p \notin releasedPages
                /\ \E s \in SlotsInPage(p) : s < bumpPtr
                /\ \A s \in SlotsInPage(p) :
                    s >= bumpPtr \/ newSlotState[s] = "freed"
            }
            \* Filter free-set entries from pages being released
            filteredFreeSet == (freeSet \union dead) \
                               {s \in Slots : PageOf(s) \in emptyPages}
            remainingBatch == sessionBatch \ {ctx}
        IN
        \* Promote surviving slots to persistent (ctx -> 0)
        /\ slotContextId' = [s \in Slots |->
            IF s \in surviving THEN 0
            ELSE slotContextId[s]]
        \* Free dead slots
        /\ slotState' = newSlotState
        /\ freeSet' = filteredFreeSet
        \* Set sentinel epoch on freed slots (models u64::MAX to prevent
        \* false positive epoch filtering by future quiescent GC responses)
        /\ slotEpoch' = [s \in Slots |->
            IF s \in dead THEN MaxEpoch + 1
            ELSE slotEpoch[s]]
        /\ releasedPages' = releasedPages \union emptyPages
        \* Remove from both the batch and the release queue
        /\ sessionBatch' = remainingBatch
        /\ sessionReleaseQueue' = sessionReleaseQueue \ {ctx}
        /\ sessionGcPhase' = IF remainingBatch = {} THEN "idle" ELSE "freeing"
        /\ sessionSurviving' = IF remainingBatch = {} THEN {} ELSE sessionSurviving
    /\ UNCHANGED <<threadPhase, threadExprs, stackRoots, activeEvaluators,
                   gcInProgressFlag, gcRequested,
                   bumpPtr, registeredRoots, epoch,
                   threadContextId, nextContextId,
                   gcReachableAdvanced,
                   allocsSinceLastPoll, gcThreshold,
                   backpressureLevel,
                   hasGcRequest, snapSlotState, snapBumpPtr,
                   snapFreeSet, snapRoots, snapEpoch,
                   hasGcResponse, respDeadSet, respLiveCount, respEpoch,
                   safepointRoots, gcPhase, gcMarked>>

(*=======================================================================*)
(* Termination                                                           *)
(*=======================================================================*)

AllDone ==
    /\ \A t \in Threads : threadPhase[t] = "done"
    /\ gcPhase = "idle"
    \* Pending GC requests/responses are harmless after all threads terminate.
    \* In the real system, the process exits and the GC thread is abandoned.
    /\ UNCHANGED vars

(*=======================================================================*)
(* Next State Relation                                                   *)
(*=======================================================================*)

Next ==
    \/ \E t \in Threads :
        \/ EvalGuardEnter_Increment(t)
        \/ EvalGuardEnter_Proceed(t)
        \/ EvalGuardEnter_BackOff(t)
        \/ EvalGuardDrop(t)
        \/ AllocateSlot(t)
        \/ DropStackRoot(t)
        \/ PublishRoot(t)
        \/ DropRegisteredRoot(t)
        \/ EvalSafepoint(t)
        \/ EvalSafepointResume_Increment(t)
        \/ EvalSafepointResume_Proceed(t)
        \/ EvalSafepointResume_BackOff(t)
        \/ TryQuiescentGc_AcquireFlag(t)
        \/ TryQuiescentGc_SnapshotOK(t)
        \/ TryQuiescentGc_Abort(t)
        \/ ProcessGcResponse(t)
        \/ ContinueEval(t)
        \/ ThreadDone(t)
    \/ CronMonitorPoll
    \/ GcReceiveRequest
    \/ GcMarkStep
    \/ GcMarkComplete
    \/ GcSweep
    \/ SessionGc_WaitQuiescent
    \/ SessionGc_AcquireFlag
    \/ SessionGc_AcquireFail
    \/ SessionGc_TraceAndRelease
    \/ SessionGc_DoubleCheckFail
    \/ SessionGc_FreeSession
    \/ AllDone

Spec == Init /\ [][Next]_vars

(*=======================================================================*)
(* Fairness                                                              *)
(*=======================================================================*)

FairSpec == Spec
    /\ \A t \in Threads :
        /\ WF_vars(EvalGuardEnter_Increment(t))
        /\ WF_vars(EvalGuardEnter_Proceed(t))
        /\ WF_vars(EvalGuardEnter_BackOff(t))
        /\ WF_vars(EvalGuardDrop(t))
        /\ WF_vars(AllocateSlot(t))
        \* Safepoint resume actions: WF ensures threads eventually resume
        \* after entering a safepoint. Same CAS pattern as EvalGuardEnter.
        \* EvalSafepoint itself has NO fairness — safepoints are opportunistic
        \* (the model non-deterministically chooses to enter one or not).
        /\ WF_vars(EvalSafepointResume_Increment(t))
        /\ WF_vars(EvalSafepointResume_Proceed(t))
        /\ WF_vars(EvalSafepointResume_BackOff(t))
        \* Strong fairness for ContinueEval: may be repeatedly enabled at
        \* "between" then disabled when AcquireFlag transitions to "gc_acquire",
        \* or when Tier 2 backpressure blocks re-entry. SF ensures threads
        \* eventually resume evaluation — backpressure relaxes after GC
        \* completes and ProcessGcResponse recomputes the level.
        /\ SF_vars(ContinueEval(t))
        /\ WF_vars(TryQuiescentGc_AcquireFlag(t))
        \* Strong fairness for SnapshotOK and Abort: these are infinitely often
        \* enabled (whenever activeEvaluators returns to 0) but may be briefly
        \* disabled by another thread's EvalGuardEnter_Increment. SF ensures
        \* they eventually fire despite the enter/backoff cycle toggling
        \* activeEvaluators. In the real implementation, the gc_acquire thread
        \* holds GC_IN_PROGRESS for sub-millisecond, ensuring forward progress.
        /\ SF_vars(TryQuiescentGc_SnapshotOK(t))
        /\ SF_vars(TryQuiescentGc_Abort(t))
        /\ WF_vars(ProcessGcResponse(t))
        \* Strong fairness for ThreadDone: may be briefly disabled when thread
        \* enters "gc_acquire" during GC servicing, but infinitely often enabled
        \* at "between". In the real system, ThreadDone is falling out of the
        \* for-loop — it happens naturally after the last expression.
        /\ SF_vars(ThreadDone(t))
    \* Cron monitor: WF ensures the monitor eventually fires.
    \* In the real system, the monitor polls every 100ms via CronStateMachine.
    \* WF guarantees: if CronMonitorPoll is continuously enabled (at least one
    \* non-done thread exists), it will eventually fire.
    /\ WF_vars(CronMonitorPoll)
    /\ WF_vars(GcReceiveRequest)
    /\ WF_vars(GcMarkStep)
    /\ WF_vars(GcMarkComplete)
    /\ WF_vars(GcSweep)
    \* Session GC thread fairness
    /\ WF_vars(SessionGc_WaitQuiescent)
    /\ WF_vars(SessionGc_AcquireFlag)
    /\ WF_vars(SessionGc_AcquireFail)
    \* Strong fairness for TraceAndRelease: may be briefly disabled by
    \* EvalGuardEnter_Increment (same pattern as quiescent GC SnapshotOK).
    /\ SF_vars(SessionGc_TraceAndRelease)
    /\ SF_vars(SessionGc_DoubleCheckFail)
    /\ WF_vars(SessionGc_FreeSession)

(*=======================================================================*)
(* Safety Invariants                                                     *)
(*=======================================================================*)

\* Upper bound on allocsSinceLastPoll for TypeOK.
\* Between two CronMonitorPoll events, the max number of allocations is
\* bounded by how many AllocateSlot actions can fire. Each requires a free
\* slot and AllLiveRoots < MaxRoots. With free-list reuse, this can
\* theoretically be unbounded, but in practice is limited by the model's
\* finite state space. We use a generous upper bound.
MaxAllocsSinceLastPoll == MaxSlots * (MaxExprs + 1) * MaxRoots * NumEvalThreads

\* Upper bound on gcThreshold for TypeOK.
MaxGcThreshold == MaxSlots * 2

TypeOK ==
    /\ threadPhase \in [Threads -> {"idle", "entering", "eval", "safepoint", "sp_entering", "between", "gc_acquire", "done"}]
    /\ threadExprs \in [Threads -> 0..MaxExprs]
    /\ stackRoots \in [Threads -> SUBSET Slots]
    /\ safepointRoots \in [Threads -> SUBSET Slots]
    /\ activeEvaluators \in 0..NumEvalThreads
    /\ gcInProgressFlag \in BOOLEAN
    /\ gcRequested \in BOOLEAN
    /\ slotState \in [Slots -> {"free", "alloc", "freed"}]
    /\ bumpPtr \in 1..(MaxSlots + 1)
    /\ freeSet \subseteq Slots
    /\ registeredRoots \subseteq Slots
    /\ epoch \in 0..MaxEpoch
    /\ slotEpoch \in [Slots -> 0..(MaxEpoch + 1)]  \* +1 for session GC sentinel
    /\ slotContextId \in [Slots -> ContextIds]
    /\ threadContextId \in [Threads -> ContextIds]
    /\ nextContextId \in 1..(MaxContextIds + 1)
    /\ sessionReleaseQueue \subseteq 1..MaxContextIds
    /\ sessionBatch \subseteq 1..MaxContextIds
    /\ sessionGcPhase \in {"idle", "waiting", "acquired", "freeing"}
    /\ sessionSurviving \subseteq Slots
    /\ gcReachableAdvanced \in BOOLEAN
    /\ allocsSinceLastPoll \in 0..MaxAllocsSinceLastPoll
    /\ gcThreshold \in 1..MaxGcThreshold
    /\ backpressureLevel \in 0..MAX_BP
    /\ releasedPages \subseteq PageSet
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
    /\ gcPhase \in {"idle", "marking", "sweeping"}
    /\ gcMarked \subseteq Slots

\* CRITICAL SAFETY: No live value (registered OR stack root) may be freed.
\* This covers ALL live values — not just registered roots.
NoLiveValueFreed ==
    \A s \in Slots :
        (s \in AllLiveRoots) => slotState[s] /= "freed"

\* CRITICAL SAFETY: When GC_IN_PROGRESS is set by quiescent GC (not session
\* GC), ALL thread stack roots are empty. This verifies that the quiescent-
\* state protocol ensures GC sees the complete root set — because at
\* quiescent points, all trampoline call stacks have returned and their
\* values are in registered root providers.
\*
\* Session GC can hold GC_IN_PROGRESS while evals have stack roots (an eval
\* sneaked in between the condvar check and the CAS). The double-check in
\* SessionGc_DoubleCheckFail will abort, and SessionGc_TraceAndRelease
\* requires activeEvaluators = 0 (ensuring stack roots are empty when the
\* surviving set is actually captured).
\*
\* NOTE: We check this at the snapshot BUILD time (when snapRoots is
\* captured), not during the entire GC cycle. After the snapshot is built,
\* new evals CAN start and create new stack roots — that's fine because
\* epoch-based TOCTOU filtering protects any newly allocated values.
SnapshotCapturesAllRoots ==
    (gcInProgressFlag /\ sessionGcPhase /= "acquired") =>
        \A t \in Threads : stackRoots[t] = {}

\* When GC_IN_PROGRESS is set by quiescent GC (not session GC), no thread
\* is in "eval" phase. Session GC can acquire the flag while evals are
\* running (the condvar check was earlier; an eval can sneak in between
\* the condvar wake and the CAS). The double-check in
\* SessionGc_DoubleCheckFail handles this case.
GcFlagConsistent ==
    (gcInProgressFlag /\ sessionGcPhase /= "acquired") =>
        \A t \in Threads : threadPhase[t] /= "eval"

\* ACTIVE_EVALUATORS is consistent with actual thread count.
\* Includes "sp_entering" — threads re-acquiring EvalGuard after safepoint
\* have already incremented the counter (same as "entering").
ActiveCountCorrect ==
    activeEvaluators = Cardinality({t \in Threads :
        threadPhase[t] \in {"entering", "eval", "sp_entering"}})

\* All registered roots are allocated
RegisteredRootsAreAllocated ==
    \A s \in registeredRoots : slotState[s] = "alloc"

\* All stack roots are allocated
StackRootsAreAllocated ==
    \A t \in Threads : \A s \in stackRoots[t] : slotState[s] = "alloc"

\* Free set entries are freed
FreeSetValid ==
    \A s \in freeSet : slotState[s] = "freed"

\* Bump pointer valid
BumpPtrValid ==
    \A s \in Slots : s >= bumpPtr => slotState[s] = "free"

\* Memory bounded
MemoryBounded ==
    bumpPtr - 1 <= MaxSlots

\* At most one thread can be in "gc_acquire" at a time
AtMostOneGcAcquire ==
    Cardinality({t \in Threads : threadPhase[t] = "gc_acquire"}) <= 1

\* Stack roots are only non-empty for threads in "eval" phase
\* (threads at "idle", "entering", "safepoint", "sp_entering", "between",
\* "gc_acquire", "done" have no trampoline call stack active — safepoint
\* threads have moved their roots to safepointRoots)
StackRootsOnlyDuringEval ==
    \A t \in Threads :
        threadPhase[t] /= "eval" => stackRoots[t] = {}

\* Safepoint roots are only non-empty for threads in "safepoint" or
\* "sp_entering" phase (roots registered during safepoint, moved back
\* to stackRoots on resume)
SafepointRootsOnlyDuringSafepoint ==
    \A t \in Threads :
        threadPhase[t] \notin {"safepoint", "sp_entering"} => safepointRoots[t] = {}

\* All safepoint roots are allocated
SafepointRootsAreAllocated ==
    \A t \in Threads : \A s \in safepointRoots[t] : slotState[s] = "alloc"

\* When ACTIVE_EVALUATORS == 0, all live values are reachable via
\* registeredRoots ∪ safepointRoots (stack roots are empty at quiescence).
\* This is the key invariant for safepoint correctness: GC sees all live
\* values through registered and safepoint roots when it can fire.
QuiescentRootsComplete ==
    activeEvaluators = 0 =>
        AllLiveRoots = registeredRoots \union AllSafepointRoots

\* GC threshold is always at least MinGcThreshold
GcThresholdPositive ==
    gcThreshold >= MinGcThreshold

\* Backpressure level is always bounded by MAX_BP
\* (redundant with TypeOK but explicit for clarity)
BackpressureLevelBounded ==
    backpressureLevel \in 0..MAX_BP

\* GC sweep is COMPLETE: the dead set in a GC response contains EXACTLY all
\* values that were allocated but not rooted at snapshot time. This verifies
\* that the mark-sweep algorithm identifies ALL dead values — no value escapes
\* collection regardless of allocation path (bump vs. free-list) or root
\* provider type (MettaState source/output vs. Environment rules/bindings).
\*
\* This directly addresses: "all memory that was allocated gets collected
\* when it is flagged as no longer in use." When a value is dropped from
\* MettaState (DropRegisteredRoot) or discarded by a worker thread
\* (DropStackRoot / EvalGuardDrop), it is no longer in snapRoots at the
\* next snapshot. GcSweepIsComplete verifies that such values appear in
\* respDeadSet — meaning GC WILL free them when the response is processed.
\*
\* SOUNDNESS: snap* variables are stable while hasGcResponse is true because
\* TryQuiescentGc_AcquireFlag requires ~hasGcResponse (at most one GC cycle
\* in flight). So the snap* state referenced here is the exact state that
\* produced the response.
GcSweepIsComplete ==
    hasGcResponse =>
        respDeadSet = {s \in Slots :
            /\ s < snapBumpPtr
            /\ snapSlotState[s] = "alloc"
            /\ s \notin snapRoots
            /\ s \notin snapFreeSet}

\* No live value (as seen at snapshot time) is ever incorrectly identified
\* as dead by the GC sweep. This is the snapshot-level complement to
\* NoLiveValueFreed (which checks after epoch filtering):
\*
\*   NoLiveValueInDeadSet: GC sweep never reports a rooted value as dead
\*   NoLiveValueFreed:     epoch filtering never lets a false positive through
\*
\* Together they form a two-layer safety net: sweep correctness + TOCTOU safety.
NoLiveValueInDeadSet ==
    hasGcResponse =>
        \A s \in respDeadSet : s \notin snapRoots

(*-----------------------------------------------------------------------*)
(* Page Release Safety Invariants                                        *)
(*-----------------------------------------------------------------------*)

\* No free-set entry points to a released page (drain correctness).
\* After release_empty_pages() drains the Treiber stack and filters out
\* entries from released pages, no surviving free-set entry should reference
\* a munmap'd page. Violation would mean pop() dereferences unmapped memory.
NoStaleFreeSetEntries ==
    \A s \in freeSet : PageOf(s) \notin releasedPages

\* No live value (registered or stack root) resides on a released page.
\* release_empty_pages() only releases pages with live_count <= 0, so no
\* live value should ever be on a released page.
NoLiveOnReleasedPage ==
    \A s \in AllLiveRoots : PageOf(s) \notin releasedPages

\* The current bump allocation page is never released.
\* release_empty_pages() excludes current_page from the release set.
\* CurrentPage = 0 means no valid page (all slots exhausted), which is fine.
CurrentPageNotReleased ==
    CurrentPage /= 0 => CurrentPage \notin releasedPages

\* Released pages contain only freed or unbumped slots.
\* A released page must have all its bumped slots in "freed" state.
ReleasedPagesAreEmpty ==
    \A p \in releasedPages :
        \A s \in SlotsInPage(p) :
            s >= bumpPtr \/ slotState[s] = "freed"

(*-----------------------------------------------------------------------*)
(* Session GC Safety Invariants                                          *)
(*-----------------------------------------------------------------------*)

\* All allocated slots have valid context IDs.
\* Verifies that slotContextId is always in the valid range.
ContextIdConsistency ==
    \A s \in Slots :
        slotState[s] = "alloc" => slotContextId[s] \in ContextIds

\* Mutual exclusion: quiescent GC and session GC cannot both hold
\* GC_IN_PROGRESS simultaneously. This is enforced by CAS (try_enter)
\* in both code paths.
SessionGcExcludesQuiescentGc ==
    \A t \in Threads :
        ~(threadPhase[t] = "gc_acquire" /\ sessionGcPhase = "acquired")

(*=======================================================================*)
(* Liveness Properties                                                   *)
(*=======================================================================*)

\* All eval threads eventually complete
AllThreadsComplete ==
    <>(\A t \in Threads : threadPhase[t] = "done")

\* GC eventually triggers when requested, OR the system terminates.
GcEventuallyTriggered ==
    gcRequested ~> (\/ ~gcRequested
                    \/ \A t \in Threads : threadPhase[t] = "done")

\* Backpressure eventually relaxes after GC runs, OR all threads terminate.
\* This verifies that Tier 2 blocking (which gates ContinueEval at MAX level)
\* cannot cause permanent starvation — GC will complete, ProcessGcResponse
\* will recompute the level, and threads will eventually resume.
BackpressureEventuallyRelaxes ==
    (backpressureLevel > 0) ~>
        (backpressureLevel = 0 \/ \A t \in Threads : threadPhase[t] = "done")

\* Every dead value (allocated but unreachable from any root) is eventually
\* either freed by GC or the system terminates (all threads done).
\*
\* This verifies RECLAMATION COMPLETENESS for all three collection paths:
\*
\*   (a) MettaState drop (DropRegisteredRoot): When MettaState goes out of
\*       scope, its source/output values are removed from registeredRoots.
\*       These values become dead and will be collected by the next GC cycle.
\*
\*   (b) Worker thread discard (DropStackRoot / EvalGuardDrop non-surviving):
\*       When a worker thread discards an intermediate computation result,
\*       the value leaves stackRoots. If no other root references it, the
\*       value is dead and will be collected by the next GC cycle.
\*
\*   (c) Session release (SessionGc_FreeSession): When a session ends, its
\*       non-surviving values are freed by the session GC thread.
\*
\* The "all threads done" disjunct handles the edge case where values die
\* during the final expression and GC doesn't run before system shutdown.
\* In the real Rust implementation, the OS reclaims all slab pages at process
\* exit via munmap. In the model, AllThreadsComplete (verified separately)
\* guarantees termination, so this disjunct is always eventually reachable.
\*
\* COMBINED with GcSweepIsComplete, this gives end-to-end reclamation:
\*   1. GcSweepIsComplete: when GC runs, it finds ALL dead values
\*   2. AllDeadValuesEventuallyFreed: GC eventually runs (or system exits)
\*   Together: no permanent memory leak during operation.
AllDeadValuesEventuallyFreed ==
    \A s \in Slots :
        (slotState[s] = "alloc" /\ s \notin AllLiveRoots) ~>
            (slotState[s] = "freed" \/ \A t \in Threads : threadPhase[t] = "done")

\* Empty pages are eventually released (or the system terminates).
\* This verifies that the page release mechanism in ProcessGcResponse
\* and SessionGc_FreeSession eventually reclaims empty pages, returning
\* physical memory to the OS.
EmptyPagesEventuallyReleased ==
    \A p \in PageSet :
        PageIsEmpty(p) ~>
            (p \in releasedPages \/ \A t \in Threads : threadPhase[t] = "done")

\* Session values not in roots are eventually freed or system terminates.
\* This verifies that session-based GC eventually processes all released
\* sessions and frees their non-surviving values.
SessionValuesEventuallyFreed ==
    \A s \in Slots :
        (slotState[s] = "alloc" /\ slotContextId[s] > 0 /\ s \notin AllLiveRoots) ~>
            (slotState[s] = "freed" \/ \A t \in Threads : threadPhase[t] = "done")

\* Pending session releases are eventually processed or system terminates.
\* This verifies that the session release thread eventually drains its queue.
SessionQueueEventuallyDrained ==
    sessionReleaseQueue /= {} ~>
        (sessionReleaseQueue = {} \/ \A t \in Threads : threadPhase[t] = "done")

=======================================================================
