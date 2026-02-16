# Chapter 6: Formal Verification with TLA+

## Why Formal Verification?

Concurrent garbage collection bugs are among the hardest defects to find through testing. They require specific thread interleavings to manifest — interleavings that depend on CPU scheduling, cache coherency timing, and memory ordering, none of which are reproducible. A GC might pass millions of test runs and still harbor a use-after-free that triggers once in a billion executions under production load.

[TLA+](https://lamport.azurewebeb.com/tla/tla.html) (Temporal Logic of Actions) is a formal specification language designed by Leslie Lamport for modeling concurrent and distributed systems. Its model checker, TLC, **exhaustively explores all possible thread interleavings** for a bounded state space. If an invariant violation exists within the bounds, TLC will find it and produce a minimal counterexample — a step-by-step execution trace showing exactly how the bug manifests.

MeTTaTron's slab allocator and GC were verified with three TLA+ models. These models discovered **four bugs**, all of which were fixed and verified. The models are located in `tla/`:

| Model | File | Purpose |
|-------|------|---------|
| Original | `tla/SlabGC.tla` | Models the initial (buggy) design; discovers Bugs 1-3 |
| Reactive | `tla/SlabGC_Reactive.tla` | Models the fixed design with snapshot + epoch; verifies Bugs 1-3 fixed |
| Pages | `tla/SlabGC_Pages.tla` | Extends Reactive with page-level tracking; discovers and fixes Bug 4 |
| Quiescent | `tla/SlabGC_Quiescent.tla` | Multi-thread quiescent protocol, cron pressure, backpressure, reclamation completeness |

## SlabGC.tla — The Original Model

This model captures the initial concurrent slab allocator design and demonstrates three critical bugs.

### State Variables

The model uses four groups of state variables:

**Allocator state (shared between eval and GC threads):**

| Variable | Type | Description |
|----------|------|-------------|
| `slotState` | Slots → {free, alloc, freed} | Lifecycle state of each slot |
| `bumpPtr` | Nat | Next slot for bump allocation (1-indexed) |
| `freeSet` | Subset(Slots) | Freed slots available for free-list reuse |

**Root set (eval thread):**

| Variable | Type | Description |
|----------|------|-------------|
| `roots` | Subset(Slots) | Currently reachable GC root values |

**GC request/response channels:**

| Variable | Type | Description |
|----------|------|-------------|
| `hasGcRequest` | Boolean | Pending request flag |
| `reqRoots` | Subset(Slots) | Root set snapshot in request |
| `reqWatermark` | Nat | Allocation watermark at request time |
| `hasGcResponse` | Boolean | Pending response flag |
| `respDeadSet` | Subset(Slots) | Dead slots identified by GC sweep |
| `respLiveCount` | Nat | Live slots (pre-watermark only) |

**Thread phases:**

| Variable | Type | Description |
|----------|------|-------------|
| `evalPhase` | {eval, between, waiting, shutdown} | Eval thread state |
| `gcPhase` | {idle, marking, sweeping} | GC thread state |
| `gcRoots` | Subset(Slots) | Snapshot of roots during marking |
| `gcWatermark` | Nat | Snapshot of watermark for sweep |
| `gcMarked` | Subset(Slots) | Slots marked reachable during tracing |

### Key Actions

The model defines actions for each thread:

**Eval thread:**
- `AllocateSlot`: Allocate from free set or bump pointer; add to roots
- `DropRoot`: A root becomes unreachable (removed from roots)
- `BeginBetweenExpressions`: Expression complete, transition to between-expressions phase
- `ProcessGcResponse`: Process pending response (free dead slots)
- `RequestGcAsync`: Build and send GC request when soft threshold exceeded
- `ContinueEval`: Resume evaluation after between-expressions processing

**GC thread:**
- `GcReceiveRequest`: Pick up request, snapshot roots/watermark
- `GcMarkStep`: Mark one root value as reachable
- `GcMarkComplete`: All roots marked, transition to sweeping
- `GcSweep`: Sweep pre-watermark region, build dead set, send response

**Concurrent interference (modeling data races):**
- `EvalAllocDuringGc`: Eval allocates while GC marks/sweeps (accesses shared state)
- `EvalDropDuringGc`: Eval drops a root while GC marks/sweeps

### Invariants

```
NoLiveValueFreed == ∀s ∈ roots : slotState[s] ≠ "freed"
NoDoubleFree     == respDeadSet ⊆ {s : slotState[s] = "alloc"}
RootsAreAllocated == ∀s ∈ roots : slotState[s] = "alloc"
FreeSetValid     == ∀s ∈ freeSet : slotState[s] = "freed"
BumpPtrValid     == ∀s ≥ bumpPtr : slotState[s] = "free"
MemoryBounded    == bumpPtr ≤ MaxSlots + 1
```

**Expected violations:**
- `NoDataRace`: Modeled as expected-to-fail — captures the data race where both threads access shared allocator state
- `DeadValuesEventuallyCollected`: Expected to fail due to watermark feedback loop

### Bugs Discovered

**Bug 1: TOCTOU Use-After-Free** (23-state counterexample)

The model checker found this sequence:

```
1. Eval allocates slot S, adds to roots
2. Eval drops root for S
3. Eval frees S (pushed to freeSet)
4. Eval builds GC request:
     reqRoots = {} (S not a root)
     snapshot captures freeSet containing S
5. GC receives request, begins marking
6. Eval re-allocates S from freeSet
     S now holds a NEW LIVE VALUE
7. GC sweep: S was in reqRoots? No → S is dead
8. GC response: respDeadSet = {S}
9. Eval processes response: frees S
     → USE-AFTER-FREE: S contained a live value!
```

The root cause: the GC's snapshot was taken before the re-allocation, but the response was processed after. No mechanism existed to detect that S was re-allocated between snapshot and response processing.

**Bug 2: Data Race (Undefined Behavior)**

Both threads access `slotState`, `bumpPtr`, and `freeSet` concurrently:

- GC reads `pages` Vec while eval pushes new pages (Vec reallocation → stale iterator)
- GC iterates `free_list` while eval modifies it (Treiber stack corruption)
- GC reads `bump_count` while eval writes it (torn read on non-atomic access)

The model captures this as `EvalAllocDuringGc` and `EvalDropDuringGc` actions that modify shared state while GC is in marking/sweeping phase.

**Bug 3: Watermark Feedback Loop**

The GC sweep only counts `live_bytes` from slots **before the watermark**:

```
Sweep region: slots 1..watermark (pre-watermark only)
Post-watermark slots: never counted, never swept
```

When `update_thresholds(live_bytes)` calibrates from this artificially low count, the threshold is set too low, triggering GC constantly. Meanwhile, garbage in post-watermark slots is never collected, causing unbounded RSS growth.

## SlabGC_Reactive.tla — The Fixed Model

This model fixes all three bugs using snapshot-based GC with epoch-based TOCTOU filtering.

### Key Architectural Changes

**1. Complete snapshot ownership** (fixes Bug 2)

Instead of sharing `slotState`/`bumpPtr`/`freeSet` between threads, the eval thread builds a complete snapshot and transfers ownership:

```
New variables (GC thread exclusive):
  snapSlotState  — frozen copy of slotState
  snapBumpPtr    — frozen copy of bumpPtr
  snapFreeSet    — frozen copy of freeSet
  snapRoots      — frozen copy of roots
  snapEpoch      — frozen copy of epoch
```

The GC thread reads **only** `snap*` variables and `gcMarked`. The eval thread reads **only** `slotState`, `bumpPtr`, `freeSet`, `roots`, `epoch`, `slotEpoch`. The variable sets are completely disjoint — no data race is possible.

**2. Epoch-based TOCTOU filtering** (fixes Bug 1)

New variables:
```
epoch     : Nat          — monotonic counter, incremented on free-list reallocation
slotEpoch : Slots → Nat  — per-slot epoch tag (set on free-list reuse)
respEpoch : Nat          — snapshot epoch returned in GC response
```

When processing a GC response, the eval thread filters dead slots:

```
safeDead = {s ∈ rawDead : slotEpoch[s] ≤ respEpoch}
```

Slots with `slotEpoch[s] > respEpoch` were re-allocated after the snapshot — they are live values, not dead. They are skipped.

**3. Full sweep** (fixes Bug 3)

The GC sweeps ALL committed slots in the snapshot (`1..snapBumpPtr-1`), not just pre-watermark slots. `liveCount` includes all marked slots across the entire snapshot, giving accurate data for threshold calibration.

### Verification Results

TLC explored **54.7 million states** in 2 minutes 46 seconds. All invariants hold:

| Invariant | Result |
|-----------|--------|
| `NoLiveValueFreed` | **HOLDS** (was violated in SlabGC) |
| `NoDoubleFree` | HOLDS |
| `RootsAreAllocated` | HOLDS |
| `FreeSetValid` | HOLDS |
| `BumpPtrValid` | HOLDS |
| `MemoryBounded` | HOLDS |
| `NoDataRace_Structural` | HOLDS (vacuously — disjoint variable sets) |

## SlabGC_Pages.tla — Page-Level Model

This model extends `SlabGC_Reactive` with page-level tracking to discover and fix Bug 4.

### New State Variables

```
pageLiveCount : Pages → Nat      — live value count per page
pageReleased  : Pages → Boolean  — TRUE if page has been munmap'd
```

A static mapping function `SlotToPage(s)` assigns slots to pages:
```
SlotToPage(s) = ((s-1) ÷ SlotsPerPage) + 1
```

### Bug 4: Page-Level Use-After-Free

The model checker discovered that free-list re-allocation fails to increment the page's `live_count`, leading to premature page release:

```
1. Page P has 3 slots; all 3 bump-allocated
     pageLiveCount[P] = 3
2. GC cycle: 2 values on Page P are dead
     pageLiveCount[P] decremented to 1
3. GC cycle: the last value on Page P is dead
     pageLiveCount[P] decremented to 0
     Page P released (munmap'd)
4. Meanwhile, slot S on Page P was re-allocated from free list
     BUT: free-list alloc did NOT increment pageLiveCount
     Slot S points into released page memory
     → USE-AFTER-FREE when accessing slot S
```

The root cause: the `AllocateSlot` action incremented `pageLiveCount` for bump allocations but not for free-list re-allocations. The page's live count fell to zero and the page was released while live values still referenced it.

### The Fix

Increment `pageLiveCount[SlotToPage(s)]` on **both** allocation paths:

```
AllocateSlot:
  IF s ∈ freeSet THEN          — Free-list path
    pageLiveCount' = [pageLiveCount EXCEPT ![SlotToPage(s)] = @ + 1]  ← FIX
    epoch' = epoch + 1
    slotEpoch' = [slotEpoch EXCEPT ![s] = epoch + 1]
  ELSE                          — Bump path
    pageLiveCount' = [pageLiveCount EXCEPT ![SlotToPage(s)] = @ + 1]  ← already correct
    bumpPtr' = bumpPtr + 1
```

### New Invariant

```
PageSafetyInvariant ==
  ∀s ∈ Slots : slotState[s] = "alloc" ⇒ ¬pageReleased[SlotToPage(s)]
```

This invariant states: if a slot is allocated, its page must not have been released. With the fix applied, TLC verifies this invariant holds across all reachable states.

## Summary Table

| # | Bug | Model Found | Root Cause | Fix | States Verified |
|---|-----|-------------|------------|-----|-----------------|
| 1 | TOCTOU Use-After-Free | SlabGC.tla | GC snapshot stale; re-allocated slot incorrectly freed | Epoch tagging + filtering in `process_gc_response()` | 54.7M |
| 2 | Data Race (UB) | SlabGC.tla | Both threads access shared mutable allocator state | Snapshot ownership — GC reads only `snap*` variables | Structural proof |
| 3 | Watermark Feedback Loop | SlabGC.tla | Pre-watermark-only sweep undercounts live values | Full sweep over all committed slots | 54.7M |
| 4 | Page-Level UAF | SlabGC_Pages.tla | Free-list reuse skips `page.live_count++` | Increment `page.live_count` on both bump and free-list paths | Complete |

## Relationship to Code

Each TLA+ action maps to a specific Rust function:

| TLA+ Action | Rust Function | Source File |
|-------------|---------------|-------------|
| `AllocateSlot` (bump) | `ValuePage::bump_alloc()` | `gc_allocator.rs:170` |
| `AllocateSlot` (free-list) | `ValueAllocator::alloc()` free-list path | `gc_allocator.rs:626` |
| `DropRoot` | Root value going out of scope | (implicit in evaluation) |
| `RequestGcAsync` | `trigger_gc_cycle()` / `build_snapshot()` | `gc_allocator.rs:1197`, `1294` |
| `ProcessGcResponse` | `SlabAllocator::process_gc_response()` | `gc_allocator.rs:1334` |
| `GcReceiveRequest` | `gc_thread_main()` recv | `gc_thread.rs:163` |
| `GcMarkStep` | `mark_snapshot()` worklist loop | `gc_allocator.rs:1469` |
| `GcSweep` | `sweep_snapshot()` | `gc_allocator.rs:1555` |
| `EvalAllocDuringGc` | Any allocation while GC thread runs | (concurrent execution) |
| Page release | `ValueAllocator::release_empty_pages()` | `gc_allocator.rs:707` |

Each TLA+ invariant maps to a safety property in the implementation:

| TLA+ Invariant | Rust Safety Property |
|----------------|---------------------|
| `NoLiveValueFreed` | Epoch filter in `process_gc_response()` prevents freeing live values |
| `NoDataRace_Structural` | `GcSnapshot` owns snapshot data; GC thread never accesses `SlabAllocator` |
| `PageSafetyInvariant` | `page.live_count.fetch_add(1)` on both bump and free-list paths |
| `BumpPtrValid` | `bump_alloc()` CAS ensures sequential slot assignment |
| `FreeSetValid` | `free_list.push(ptr)` only called on dead slots after epoch filtering |

## SlabGC_Quiescent.tla — Multi-Thread Quiescent-State Protocol + Session-Based GC

This model generalizes the single-thread reactive model to multi-threaded
evaluation with lock-free coordination, quiescent-state GC triggering,
cron-based memory pressure monitoring, graduated backpressure, and
**session-based GC** for targeted bulk release of per-eval session values.

### State Space

| Variable Group | Variables | Description |
|----------------|-----------|-------------|
| Per-thread | `threadPhase`, `threadExprs`, `stackRoots` | Thread lifecycle and stack roots |
| Coordination | `activeEvaluators`, `gcInProgressFlag`, `gcRequested` | Lock-free EvalGuard protocol |
| Allocator | `slotState`, `bumpPtr`, `freeSet`, `registeredRoots`, `epoch`, `slotEpoch` | Shared allocator state |
| Session | `slotContextId`, `threadContextId`, `nextContextId` | Per-slot/thread session context IDs |
| Session GC | `sessionReleaseQueue`, `sessionBatch`, `sessionGcPhase`, `sessionSurviving` | Background session release thread |
| Pressure | `allocsSinceLastPoll`, `gcThreshold`, `backpressureLevel`, `gcReachableAdvanced` | Cron monitor, backpressure, GC heartbeat |
| Pages | `releasedPages` | Released (munmap'd) pages |
| Snapshot | `hasGcRequest`, `snap*` (5 vars) | GC snapshot channel |
| Response | `hasGcResponse`, `resp*` (3 vars) | GC response channel |
| GC thread | `gcPhase`, `gcMarked` | Mark-sweep state |

### Key Protocol Actions

| Action | Models |
|--------|--------|
| `EvalGuardEnter_{Increment,Proceed,BackOff}` | Lock-free `EvalGuard::enter()` with retry; `Proceed` assigns session context ID |
| `EvalGuardDrop` | Guard drop with stack→registered root transfer; enqueues session for async release |
| `TryQuiescentGc_{AcquireFlag,SnapshotOK,Abort}` | 3-step quiescent GC with double-check; mutual exclusion with session GC |
| `SessionGc_{WaitQuiescent,AcquireFlag,AcquireFail}` | Session GC condvar wait, CAS acquire, retry |
| `SessionGc_{TraceAndRelease,DoubleCheckFail}` | Root trace at quiescent point, abort if eval sneaks in |
| `SessionGc_FreeSession` | Free non-surviving session values, promote survivors to persistent |
| `CronMonitorPoll` | Memory pressure + backpressure computation, gated on GC reachability heartbeat |
| `ProcessGcResponse` | Epoch-filtered dead slot freeing + adaptive threshold + backpressure recomputation |
| `ContinueEval` | Tier 2 backpressure guard on re-entry |

### Session-Based GC Model

Each top-level eval creates a session with a unique, monotonically increasing
context ID (1..MaxContextIds). All allocations during that eval are tagged with
the session's context ID. On eval completion (`EvalGuardDrop`), the context ID
is enqueued for async release.

The session release thread follows this lifecycle:

```
idle → WaitQuiescent (freeze batch) → AcquireFlag (CAS) → TraceAndRelease (double-check + roots) → FreeSession (per-session free) → idle
```

**Key safety mechanisms:**
- **Batch freezing**: `sessionBatch` captures `sessionReleaseQueue` at WaitQuiescent time; new sessions added during freeing are deferred to the next cycle
- **Monotonic IDs**: Context IDs never wrap (matching Rust's `AtomicU64`), preventing aliasing with pending releases
- **Surviving set promotion**: Values in both `sessionSurviving` and a released session are promoted to persistent (ctx=0) rather than freed
- **Sentinel epoch**: Freed session slots get epoch `MaxEpoch + 1` to prevent false-positive epoch filtering by future quiescent GC
- **Mutual exclusion**: `GC_IN_PROGRESS` flag shared between quiescent GC and session GC via CAS

#### TLA+ → Rust Mapping

| TLA+ Variable/Action | Rust Implementation |
|-----------------------|--------------------|
| `slotContextId` | `SlabPage::context_ids[slot_idx]` |
| `threadContextId` | `NEXT_CONTEXT_ID.fetch_add(1)` in `SessionGuard::new()` |
| `sessionReleaseQueue` | `mpsc::Receiver<u32>` in `session_release_thread_main()` |
| `sessionBatch` | `vec![first_id] + try_recv()` drain at top of loop |
| `sessionGcPhase` | Control flow states in `session_release_thread_main()` |
| `sessionSurviving` | `trace_surviving_set()` result |
| `gcReachableAdvanced` | `GC_REACHABLE_COUNTER` atomic in `gc_cron.rs` |
| `SessionGc_AcquireFlag` | `GcInProgressGuard::try_enter()` in session thread |
| `SessionGc_FreeSession` | `release_session_with_surviving()` |
| `ComputeBackpressure()` | `compute_backpressure_level()` in `gc_cron.rs` |

### Safety Invariants (22 total)

| Invariant | What It Verifies |
|-----------|------------------|
| `TypeOK` | All variables within bounds |
| `NoLiveValueFreed` | No root (registered OR stack) is in "freed" state |
| `SnapshotCapturesAllRoots` | Stack roots empty when quiescent GC snapshot is built |
| `GcFlagConsistent` | `GC_IN_PROGRESS` (quiescent) → no thread in "eval" |
| `ActiveCountCorrect` | Atomic counter matches actual entering+eval threads |
| `RegisteredRootsAreAllocated` | All registered roots have "alloc" state |
| `StackRootsAreAllocated` | All stack roots have "alloc" state |
| `FreeSetValid` | Free set entries have "freed" state |
| `BumpPtrValid` | Slots beyond bump pointer are "free" |
| `MemoryBounded` | Bump pointer within bounds |
| `AtMostOneGcAcquire` | At most one thread in GC acquire phase |
| `StackRootsOnlyDuringEval` | Stack roots empty outside "eval" phase |
| `GcThresholdPositive` | Threshold >= minimum |
| `BackpressureLevelBounded` | Level in 0..3 |
| `GcSweepIsComplete` | **Reclamation**: sweep finds ALL dead values at snapshot time |
| `NoLiveValueInDeadSet` | **Reclamation**: no snapshot root appears in dead set |
| `NoStaleFreeSetEntries` | Free set entries are not on released pages |
| `NoLiveOnReleasedPage` | No allocated value on a released page |
| `CurrentPageNotReleased` | The current allocation page is not released |
| `ReleasedPagesAreEmpty` | All slots on released pages are freed |
| `ContextIdConsistency` | Allocated slots have valid context IDs |
| `SessionGcExcludesQuiescentGc` | Mutual exclusion between GC paths on `GC_IN_PROGRESS` |

### Liveness Properties (7 total)

| Property | What It Verifies |
|----------|------------------|
| `AllThreadsComplete` | All threads eventually finish |
| `GcEventuallyTriggered` | GC requests are eventually consumed |
| `BackpressureEventuallyRelaxes` | Backpressure cannot permanently stall threads |
| `AllDeadValuesEventuallyFreed` | **Reclamation**: dead values eventually freed |
| `EmptyPagesEventuallyReleased` | Empty pages are eventually released (munmap'd) |
| `SessionValuesEventuallyFreed` | Non-root session values eventually freed or system terminates |
| `SessionQueueEventuallyDrained` | Pending session releases eventually processed |

### Verification Results

#### Safety (22 invariants)

- **Small** (MaxSlots=3, MaxContextIds=2): 8.8M distinct states, 46 seconds, all hold
- **Standard** (MaxSlots=4, MaxContextIds=2): 73.5M distinct states, 11 min, all hold

#### Liveness (7 properties)

- **Minimal** (MaxSlots=2, MaxRoots=1, MaxContextIds=2): Pending verification

## Model Checking Configuration

The TLC model checker uses small constant bounds for exhaustive exploration:

```
MaxSlots = 6         — total slots across all pages
MaxRoots = 3         — max simultaneous root values
MaxExprs = 3         — max expressions to evaluate
SlotsPerPage = 3     — for SlabGC_Pages.tla
MaxPages = 2         — for SlabGC_Pages.tla
MaxContextIds = 2-4  — for SlabGC_Quiescent.tla (session-based GC)
```

These bounds are small enough for exhaustive state space exploration (tens of millions of states) while large enough to exercise all interesting interleavings. The key insight is that concurrency bugs depend on the **number of interleaving points**, not on the absolute number of values — if a bug exists with 6 slots, it exists with 6,000 slots too.

The model checking wrappers are:
- `tla/MC_SlabGC.tla` — configures TLC for `SlabGC.tla`
- `tla/MC_SlabGC_Reactive.tla` — configures TLC for `SlabGC_Reactive.tla`
- `tla/MC_SlabGC_Quiescent.tla` — configures TLC for `SlabGC_Quiescent.tla`
  - `tla/SlabGC_Quiescent.cfg` — safety invariant checking (22 invariants)
  - `tla/SlabGC_Quiescent_small.cfg` — fast feedback safety checking
  - `tla/SlabGC_Quiescent_deadlock.cfg` — liveness + deadlock checking (7 properties)
