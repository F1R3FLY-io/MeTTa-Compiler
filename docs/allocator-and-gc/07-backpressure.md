# Backpressure: Graduated Allocation Throttling

## Problem

When allocation rate outpaces garbage collection, committed memory grows
unboundedly. The GC cron monitor detects pressure and requests GC cycles, but
if allocating threads continue at full speed between cycles, memory usage climbs
faster than GC can reclaim it.

## Solution: 4-Level Graduated Throttling

The backpressure system provides the minimum necessary allocation throttling to
keep GC in steady state. It uses a 4-level scheme computed by the cron monitor
based on the ratio of committed bytes to the adaptive GC threshold:

| Level | Condition                        | Tier 1 Action     | Tier 2 Action              |
|-------|----------------------------------|--------------------|----------------------------|
| 0     | committed < threshold            | No-op              | No-op                      |
| 1     | committed >= threshold           | `yield_now()`      | No-op                      |
| 2     | committed >= threshold * 1.5     | `sleep(10us)`      | No-op                      |
| 3     | committed >= threshold * 2.0     | `sleep(100us)`     | Block until GC completes   |

### Two Tiers of Application

**Tier 1** (`apply_backpressure_tier1`) is called from
`SessionContext::maybe_gc()` every 256 trampoline iterations. It yields or
sleeps based on the current level, slowing allocation velocity proportionally
to memory pressure. At level 0 (below GC threshold), this is a no-op with zero
overhead on the hot path.

**Tier 2** (`apply_backpressure_tier2`) is called from `main.rs` between
top-level expression evaluations. At MAX level (3) only, it spin-yields until
`gc_cycle_in_flight()` returns false. This is the hard backstop that ensures GC
can finish before more work piles on. At levels below MAX, Tier 2 is a no-op.

### Key Design Principles

1. **Non-invasive**: The cron monitor (polling every ~100ms) computes the
   level. Eval threads read a single `AtomicU8` with `Relaxed` ordering ---
   zero contention on the hot path.

2. **Graduated**: Throttling is proportional to actual pressure. No unnecessary
   blocking when GC is keeping up.

3. **Minimum necessary**: The system throttles just enough to prevent unbounded
   memory growth, but no more than needed.

## How the Cron Monitor Computes Backpressure Level

In `gc_cron.rs`, `execute_memory_monitor()` (lines 978--991) computes the
backpressure level on each poll (~100ms interval):

```rust
let bp_level = if threshold > 0 {
    if current_committed >= threshold * 2 { 3 }
    else if current_committed >= threshold * 3 / 2 { 2 }
    else if current_committed >= threshold { 1 }
    else { 0 }
} else { 0 };
set_backpressure_level(bp_level);
```

The threshold is adaptive: after each GC cycle, it is set to
`max(live_bytes * GC_GROWTH_FACTOR, MIN_GC_THRESHOLD)` where
`GC_GROWTH_FACTOR = 2.0`.

## Faster Backpressure Feedback

Without the improvement, after GC frees memory, the backpressure level would
remain elevated until the next cron poll (up to 100ms later). This is wasteful:
threads would continue to be throttled even though memory pressure has dropped.

The improvement in `maybe_process_gc_response()` immediately re-evaluates the
backpressure level after processing a GC response:

```rust
// Immediate backpressure feedback
let committed = alloc.committed_bytes_atomic().load(Ordering::Relaxed);
let threshold = alloc.gc_threshold_atomic().load(Ordering::Relaxed);
let new_level = if threshold > 0 {
    if committed >= threshold * 2 { 3 }
    else if committed >= threshold * 3 / 2 { 2 }
    else if committed >= threshold { 1 }
    else { 0 }
} else { 0 };
set_backpressure_level(new_level);
```

This drops backpressure immediately when GC frees memory, eliminating up to
100ms of unnecessary throttling.

## TLA+ Formal Verification

The backpressure model is verified in `tla/SlabGC_Quiescent.tla`:

### Variables
- `backpressureLevel`: 0..MAX_BP (where MAX_BP == 3), mirrors
  `BACKPRESSURE_LEVEL: AtomicU8` in Rust.

### Actions
- **CronMonitorPoll**: Computes `backpressureLevel'` based on
  `CommittedSlotCount / gcThreshold` ratio (same formula as Rust).
- **ContinueEval**: Guarded by
  `~(backpressureLevel >= MAX_BP /\ GcCycleInFlight)` -- models Tier 2
  blocking at MAX level.
- **ProcessGcResponse**: Decrements `backpressureLevel` by 1 (clamped to 0) --
  models faster feedback.

### Properties Verified
- **BackpressureLevelBounded** (safety invariant): `backpressureLevel` is
  always in 0..MAX_BP.
- **BackpressureEventuallyRelaxes** (liveness property): If
  `backpressureLevel > 0`, it eventually reaches 0 or all threads terminate.
  This verifies that Tier 2 blocking cannot cause permanent starvation.
- **AllThreadsComplete** (liveness): All threads eventually finish -- confirms
  backpressure does not prevent termination.
- All 13 existing safety invariants continue to hold with backpressure.

### Correspondence: TLA+ to Rust

| TLA+ | Rust |
|------|------|
| `backpressureLevel` | `BACKPRESSURE_LEVEL: AtomicU8` |
| `MAX_BP == 3` | `MAX_BACKPRESSURE: u8 = 3` |
| `CronMonitorPoll` sets level | `execute_memory_monitor()` in gc_cron.rs |
| `ContinueEval` guard | `apply_backpressure_tier2()` in main.rs |
| `ProcessGcResponse` decrements | `maybe_process_gc_response()` re-evaluates |
| `GcCycleInFlight` | `gc_cycle_in_flight()` |
| `BackpressureEventuallyRelaxes` | Guaranteed by GC completing + cron recomputing |

## Runtime Configuration

- `--no-gc`: Disables GC entirely. Backpressure level stays at 0 since the
  cron monitor's threshold trigger is never armed. (GC_REQUESTED is consumed
  but never acted on.)
- `--gc-stats`: Reports the current backpressure level at exit alongside other
  GC statistics.
