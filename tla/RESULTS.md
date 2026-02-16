# TLA+ Model Checking Results: Concurrent Slab Allocator + Mark-Sweep GC

## Part 1: Original Protocol — Bug Discovery (SlabGC.tla)

### Model Summary

| Parameter | Value |
|-----------|-------|
| Specification | `SlabGC.tla` |
| MaxSlots | 6 |
| MaxRoots | 3 |
| MaxExprs | 3 |
| States generated | ~21.5M |
| Distinct states | ~6.9M |
| Depth | 56 |
| Workers | 36 |

### Bug Findings

#### BUG 1: TOCTOU Use-After-Free (CRITICAL)

**Invariant violated**: `NoLiveValueFreed`

**Root cause**: The GC protocol has a time-of-check to time-of-use (TOCTOU) race
between the root snapshot and the dead set processing. When a GC request captures
roots, the eval thread can subsequently re-allocate a slot from the free list that
was freed by a previous GC cycle. The new slot is added as a root. When the GC
response arrives with that slot in its dead set (because the snapshot had empty
roots), the eval thread blindly frees it — destroying a live value.

**Counterexample trace** (23 states):

```
State 14: GC sweeps and produces deadSet = {1,2,3,4} (all unreachable)
State 15: Eval processes dead set → thresholds drop (soft=2, hard=4)
State 16: Eval requests ANOTHER async GC with roots={} (all dropped)
State 17: Eval continues evaluation
State 18: Eval re-allocates slot 1 from free set → roots={1}, slot[1]="alloc"
State 19-20: GC receives the request (with STALE empty roots from state 16)
State 21-22: GC marks nothing, sweeps → deadSet={1} (slot 1 looks dead!)
State 23: Eval processes dead set → FREES slot 1 even though roots={1}
         *** VIOLATION: roots={1} but slotState[1]="freed" ***
```

**Impact**: Use-after-free. The freed slot can be re-allocated for a different
value, causing the root reference to silently point to wrong data. In the Rust
implementation, this is undefined behavior via the `UnsafeCell`.

#### BUG 2: Data Race / Undefined Behavior

**Invariant violated**: `NoDataRace`

**Root cause**: The eval thread continues allocating (`evalPhase="eval"`) while
the GC thread performs marking and sweeping (`gcPhase \in {"marking", "sweeping"}`).
Both threads call `alloc.inner()` which returns `&mut SlabAllocatorInner` via
`UnsafeCell`. This violates Rust's aliasing rules — two `&mut` references to the
same data exist simultaneously.

**Concrete race conditions**:
1. GC iterates `pages` Vec while eval `push`es a new page → Vec reallocation
   invalidates GC's iterator
2. GC reads `free_list` into a HashSet while eval pushes/pops → inconsistent view
3. GC reads `page.bump_count` while eval increments it → torn read

**Counterexample**: TLC finds the violation immediately — any state where eval
is in "eval" phase and GC has received a request produces the race.

#### BUG 3: Watermark Feedback Loop (Memory Growth)

**Invariant**: Not directly checked as an invariant (it's a liveness property).

**Root cause**: The `sweep()` function only counts `live_bytes` from values before
the watermark. Post-watermark values (allocated during the GC cycle) are not
counted. When `update_thresholds(live_bytes)` calibrates from this undercount,
the thresholds are set too low relative to actual committed memory. This causes:
1. GC triggers constantly (committed > lowered soft threshold)
2. Each GC cycle only sweeps the pre-watermark region
3. Post-watermark pages accumulate dead values that are never swept
4. RSS grows unboundedly

### Original Invariants That Hold

| Invariant | Result | States Checked |
|-----------|--------|----------------|
| `TypeOK` | HOLDS | 6.9M |
| `NoDoubleFree` | HOLDS | 5.0M |
| `BumpPtrValid` | HOLDS | 6.9M |
| `MemoryBounded` | HOLDS | 6.9M |
| `SweepCorrectness` | HOLDS | 5.0M |
| Deadlock freedom | HOLDS | 6.9M |

### Original Invariants That Fail

| Invariant | Result | Bug |
|-----------|--------|-----|
| `NoLiveValueFreed` | **VIOLATED** | TOCTOU use-after-free |
| `NoDataRace` | **VIOLATED** | Concurrent `&mut` access |
| `RootsAreAllocated` | **VIOLATED** | Consequence of Bug 1 |
| `FreeSetValid` | **VIOLATED** | Consequence of Bug 1 |

---

## Part 2: Fixed Protocol — Snapshot-Based Async GC (SlabGC_Reactive.tla)

### Architecture: Snapshot-Based Async Mark-Sweep with Epoch Filtering

The fixed protocol addresses all three bugs through **ownership separation**:

| Bug | Root Cause | Fix Mechanism |
|-----|-----------|--------------|
| Bug 1 (TOCTOU) | Stale root snapshot; re-allocated slot in dead set | **Epoch-based filtering**: tag free-list re-allocations with monotonic epoch; filter dead set against snapshot epoch before freeing |
| Bug 2 (Data Race) | Both threads call `inner()` getting `&mut` | **Snapshot-based**: GC receives owned `GcSnapshot` with frozen page pointers + GC-owned mark bitmaps; never touches allocator |
| Bug 3 (Watermark) | sweep() only counts pre-watermark live bytes | **Full sweep**: iterate ALL committed slots in snapshot (no watermark restriction), accurate live_bytes |

**Key design principles**:
- Eval thread exclusively owns mutable allocator state (`slotState`, `bumpPtr`, `freeSet`, `roots`, `epoch`, `slotEpoch`)
- GC thread operates exclusively on owned `GcSnapshot` (`snapSlotState`, `snapBumpPtr`, `snapFreeSet`, `snapRoots`, `snapEpoch`) and its own `gcMarked` set
- Eval can allocate/drop freely during GC — disjoint variable sets prevent data races
- Dead set processing filters by epoch: `safeDead = {s ∈ rawDead : slotEpoch[s] ≤ respEpoch}`

### Verification Results

#### Small Model (Complete — 2 seconds)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 4 |
| MaxRoots | 2 |
| MaxExprs | 2 |
| States generated | 709,918 |
| Distinct states | 271,968 |
| Depth | 50 |
| Queue remaining | 0 (COMPLETE) |
| Time | 2 seconds |

**Result**: **Model checking completed. No error has been found.** All 7 safety invariants hold across the entire state space.

#### Medium Model (Complete — 2 min 46 sec)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 5 |
| MaxRoots | 2 |
| MaxExprs | 3 |
| States generated | 140,155,331 |
| Distinct states | 54,744,064 |
| Depth | 74 |
| Queue remaining | 0 (COMPLETE) |
| Time | 2 min 46 sec |

**Result**: **Model checking completed. No error has been found.** All 7 safety invariants hold across the entire state space.

#### Large Model (Partial — 24 minutes, terminated)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 6 |
| MaxRoots | 3 |
| MaxExprs | 3 |
| States generated | 1,482,993,164 |
| Distinct states | 555,973,778 |
| Depth | 49+ |
| Queue remaining | 164,247,042 (still growing) |
| Time | 24 min (terminated) |

**Result**: No violations found after 1.48 billion states. The epoch variables create a significantly larger state space than the original spec (MaxEpoch = MaxSlots × (MaxExprs + 2) × MaxRoots = 90). Complete exploration at this scale would require hours. The partial result provides very strong confidence when combined with the complete small and medium verifications.

#### Deadlock Freedom (Complete)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 4 |
| MaxRoots | 2 |
| MaxExprs | 2 |
| States generated | 709,918 |
| Distinct states | 271,968 |
| Queue remaining | 0 (COMPLETE) |

**Result**: **No deadlock found.** The reactive state machine terminates correctly.

### All Safety Invariants — Fixed Protocol

| Invariant | Small (4,2,2) | Medium (5,2,3) | Large (6,3,3) |
|-----------|--------------|----------------|---------------|
| `TypeOK` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `NoLiveValueFreed` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `RootsAreAllocated` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `FreeSetValid` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `BumpPtrValid` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `MemoryBounded` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| `NoDoubleFree` | HOLDS (complete) | HOLDS (complete) | HOLDS (1.48B states) |
| Deadlock freedom | HOLDS (complete) | — | — |

### Comparison: Original vs Fixed

| Property | Original (SlabGC.tla) | Fixed (SlabGC_Reactive.tla) |
|----------|----------------------|----------------------------|
| `NoLiveValueFreed` | **VIOLATED** (state 23) | HOLDS (54.7M states) |
| `NoDataRace` | **VIOLATED** (immediate) | Eliminated by design |
| Watermark feedback | Unbounded RSS growth | Full sweep, accurate thresholds |
| `RootsAreAllocated` | **VIOLATED** | HOLDS |
| `FreeSetValid` | **VIOLATED** | HOLDS |
| `NoDoubleFree` | HOLDS | HOLDS |
| Deadlock freedom | HOLDS | HOLDS |

---

## Rust Implementation

### Files Modified

| File | Change |
|------|--------|
| `src/backend/models/gc_allocator.rs` | Added epoch tracking (`ValueAllocator.epoch`, `slot_epochs`), `GcSnapshot`/`GcResponse` structs, `build_snapshot()`, `process_gc_response()` with epoch filtering, `mark_snapshot()`/`sweep_snapshot()` (snapshot-only mark-sweep) |
| `src/backend/models/gc_thread.rs` | Complete rewrite: `GcThread::spawn()` takes no args, receives owned `GcSnapshot` via channel, performs mark-sweep on snapshot only, sends `GcResponse` with epoch |
| `src/backend/models/arena_state.rs` | Updated to use snapshot-based protocol: `build_snapshot(roots)` instead of watermark, `process_gc_response()` instead of `process_dead_set()`, accurate `live_bytes` from full sweep |
| `src/backend/models/mod.rs` | Added `GcSnapshot`, `GcResponse`, `mark_snapshot`, `sweep_snapshot` to exports |
| `src/rholang_integration.rs` | Added `between_expressions()` calls in `run_state` and `run_state_async` |
| `src/backend/bytecode/tiered_cache.rs` | Fixed flaky `test_arena_tiered_cache_get_best_tier` (race with async rayon compilation) |

### Test Results

- **Library tests**: 2,896 passed, 0 failed
- **Build**: Clean (debug and release)

---

## Part 3: Session-Based GC — Model Extension and Re-Verification (SlabGC_Quiescent.tla)

### Context

The quiescent-state model (`SlabGC_Quiescent.tla`) was extended to capture
**session-based GC** — a second GC mechanism that shares the quiescent protocol.
The Rust implementation uses session context IDs to track which eval session
allocated each value, enabling targeted bulk release when a `MettaState` is dropped.

### Changes to SlabGC_Quiescent.tla

#### New Variables (8 total)

| Variable | Type | Description |
|----------|------|-------------|
| `slotContextId` | Slots → 0..MaxContextIds | Per-slot session context ID (0=persistent) |
| `threadContextId` | Threads → 0..MaxContextIds | Per-thread active session ID |
| `nextContextId` | 1..(MaxContextIds+1) | Monotonic counter for session IDs |
| `sessionReleaseQueue` | SUBSET(1..MaxContextIds) | Pending session releases |
| `sessionBatch` | SUBSET(1..MaxContextIds) | Frozen batch from WaitQuiescent |
| `sessionGcPhase` | {"idle","waiting","acquired","freeing"} | Session GC thread state |
| `sessionSurviving` | SUBSET(Slots) | Surviving set from root trace |
| `gcReachableAdvanced` | BOOLEAN | GC reachability heartbeat for cron gating |

#### New Actions (6 for session GC lifecycle)

| Action | Models |
|--------|--------|
| `SessionGc_WaitQuiescent` | Condvar wait + batch capture (channel drain) |
| `SessionGc_AcquireFlag` | CAS `GC_IN_PROGRESS` false→true |
| `SessionGc_AcquireFail` | CAS fails (quiescent GC holds flag) → retry |
| `SessionGc_TraceAndRelease` | Double-check quiescence, trace roots, release flag |
| `SessionGc_DoubleCheckFail` | Eval snuck in → abort, release flag |
| `SessionGc_FreeSession` | Free non-surviving values from one session in batch |

#### Modified Actions

| Action | Change |
|--------|--------|
| `EvalGuardEnter_Proceed` | Assigns session context ID (monotonic) |
| `EvalGuardDrop` | Enqueues session ID for async release |
| `AllocateSlot` | Tags allocated slot with thread's context ID |
| `TryQuiescentGc_AcquireFlag` | Mutual exclusion with session GC (`sessionGcPhase \notin {"acquired"}`) |
| `ProcessGcResponse` | Backpressure recomputed (not decremented); GC reachability heartbeat |
| `CronMonitorPoll` | Backpressure gated on `gcReachableAdvanced` heartbeat |

#### New Safety Invariants (2)

| Invariant | What It Verifies |
|-----------|------------------|
| `ContextIdConsistency` | Allocated slots have valid context IDs (0..MaxContextIds) |
| `SessionGcExcludesQuiescentGc` | Mutual exclusion: no thread in gc_acquire while session GC acquired |

#### New Liveness Properties (2)

| Property | What It Verifies |
|----------|------------------|
| `SessionValuesEventuallyFreed` | Non-root session values are eventually freed or system terminates |
| `SessionQueueEventuallyDrained` | Pending session releases are eventually processed |

### Bug Fixes Found During Verification

**1. Context ID Aliasing (Model Bug)**

The initial model wrapped `nextContextId` at `MaxContextIds`. With small
constants, this allowed two threads to get the same context ID, causing
session GC to free values belonging to a different session. Fixed by making
IDs monotonic (matching Rust's `AtomicU64::fetch_add`).

**2. Stale Surviving Set (Model Bug)**

`SessionGc_FreeSession` could pick sessions added to `sessionReleaseQueue`
AFTER the surviving set was captured. A new eval could allocate values with
a new context ID, drop its eval, and session GC would free those values using
the stale (empty) surviving set. Fixed by adding `sessionBatch` to freeze
the release queue at `WaitQuiescent` time.

**3. GcFlagConsistent / SnapshotCapturesAllRoots Relaxation**

Session GC can acquire `GC_IN_PROGRESS` while evals are running (the condvar
check was earlier; an eval can sneak in between condvar wake and the CAS).
The invariants were relaxed to exclude the session GC "acquired" phase,
since `SessionGc_DoubleCheckFail` handles this case.

**4. Rust Race: `enter()` → `try_enter()` (Code Fix)**

`maybe_quiescent_gc()` used unconditional `GcInProgressGuard::enter()` which
could overwrite `GC_IN_PROGRESS` set by session GC. Fixed to CAS-based
`try_enter()`, matching the TLA+ model's `~gcInProgressFlag` precondition.

### Verification Results

#### Small Model (Complete — 46 seconds)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 3 |
| MaxRoots | 2 |
| MaxExprs | 1 |
| MaxContextIds | 2 |
| SlotsPerPage | 2 |
| States generated | 41,822,735 |
| Distinct states | 8,842,098 |
| Depth | 68 |
| Time | 46 seconds |

**Result**: **Model checking completed. No error has been found.** All 22 safety invariants hold.

#### Standard Model (Complete — 11 min 13 sec)

| Parameter | Value |
|-----------|-------|
| MaxSlots | 4 |
| MaxRoots | 2 |
| MaxExprs | 1 |
| MaxContextIds | 2 |
| SlotsPerPage | 2 |
| States generated | 349,532,171 |
| Distinct states | 73,459,202 |
| Depth | 77 |
| Time | 11 min 13 sec |

**Result**: **Model checking completed. No error has been found.** All 22 safety invariants hold.

#### Liveness (Pending)

Constants reduced to `MaxSlots=2, MaxRoots=1, MaxExprs=1, MaxContextIds=2`
for tractability. Liveness checking is extremely expensive due to cycle
detection in the state graph. Results pending.

### All Safety Invariants — Session-Based GC Extension

| Invariant | Small (3,2,1) | Standard (4,2,1) |
|-----------|--------------|------------------|
| `TypeOK` | HOLDS (complete) | HOLDS (complete) |
| `NoLiveValueFreed` | HOLDS (complete) | HOLDS (complete) |
| `SnapshotCapturesAllRoots` | HOLDS (complete) | HOLDS (complete) |
| `GcFlagConsistent` | HOLDS (complete) | HOLDS (complete) |
| `ActiveCountCorrect` | HOLDS (complete) | HOLDS (complete) |
| `RegisteredRootsAreAllocated` | HOLDS (complete) | HOLDS (complete) |
| `StackRootsAreAllocated` | HOLDS (complete) | HOLDS (complete) |
| `FreeSetValid` | HOLDS (complete) | HOLDS (complete) |
| `BumpPtrValid` | HOLDS (complete) | HOLDS (complete) |
| `MemoryBounded` | HOLDS (complete) | HOLDS (complete) |
| `AtMostOneGcAcquire` | HOLDS (complete) | HOLDS (complete) |
| `StackRootsOnlyDuringEval` | HOLDS (complete) | HOLDS (complete) |
| `GcThresholdPositive` | HOLDS (complete) | HOLDS (complete) |
| `BackpressureLevelBounded` | HOLDS (complete) | HOLDS (complete) |
| `GcSweepIsComplete` | HOLDS (complete) | HOLDS (complete) |
| `NoLiveValueInDeadSet` | HOLDS (complete) | HOLDS (complete) |
| `NoStaleFreeSetEntries` | HOLDS (complete) | HOLDS (complete) |
| `NoLiveOnReleasedPage` | HOLDS (complete) | HOLDS (complete) |
| `CurrentPageNotReleased` | HOLDS (complete) | HOLDS (complete) |
| `ReleasedPagesAreEmpty` | HOLDS (complete) | HOLDS (complete) |
| `ContextIdConsistency` | HOLDS (complete) | HOLDS (complete) |
| `SessionGcExcludesQuiescentGc` | HOLDS (complete) | HOLDS (complete) |

---

## Files

| File | Purpose |
|------|---------|
| `tla/SlabGC.tla` | Original TLA+ specification (bug discovery) |
| `tla/MC_SlabGC.tla` | TLC wrapper for original spec |
| `tla/SlabGC.cfg` | Original TLC configuration |
| `tla/SlabGC_datarace.cfg` | Original NoDataRace check |
| `tla/SlabGC_doublefree.cfg` | Original NoDoubleFree check |
| `tla/SlabGC_deadlock.cfg` | Original deadlock freedom check |
| `tla/SlabGC_Reactive.tla` | **Fixed TLA+ specification (snapshot + epoch)** |
| `tla/MC_SlabGC_Reactive.tla` | TLC wrapper for fixed spec |
| `tla/SlabGC_Reactive.cfg` | Fixed spec — all safety invariants (MaxSlots=6) |
| `tla/SlabGC_Reactive_small.cfg` | Fixed spec — small model (MaxSlots=4, completable) |
| `tla/SlabGC_Reactive_medium.cfg` | Fixed spec — medium model (MaxSlots=5, completable) |
| `tla/SlabGC_Reactive_deadlock.cfg` | Fixed spec — deadlock freedom check |
| `tla/SlabGC_Reactive_deadlock_small.cfg` | Fixed spec — deadlock freedom (small model) |
| `tla/SlabGC_Quiescent.tla` | **Multi-thread quiescent + session-based GC** |
| `tla/MC_SlabGC_Quiescent.tla` | TLC wrapper for quiescent spec |
| `tla/SlabGC_Quiescent.cfg` | Standard safety invariant checking (22 invariants) |
| `tla/SlabGC_Quiescent_small.cfg` | Small model for fast iteration |
| `tla/SlabGC_Quiescent_deadlock.cfg` | Liveness + deadlock checking (7 properties) |
| `tla/RESULTS.md` | This file |
