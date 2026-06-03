//! GC-Specific Cron Tasks
//!
//! Wraps the generic `task_scheduler::CronStateMachine` with GC-specific
//! monitoring and task scheduling.
//!
//! ## GC Integration
//!
//! The `GcCronSingleton` wraps the generic `TaskSchedulerSingleton` with GC-specific
//! tasks:
//!
//! **Memory monitor** (100ms interval): Reads atomics, computes allocation
//! rate (allocs/s), calls `request_gc()` if rate exceeds threshold OR
//! committed bytes exceed `gc_threshold`.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, OnceLock};

use std::time::{Duration, Instant};

use super::adaptive_pool::{Ema, HillClimber, ScaleAction};
use super::gc_allocator::{gc_values_freed_total, maybe_async_gc, request_gc};
use super::gc_pool::global_gc_pool;
use super::task_scheduler::TaskSchedulerSingleton;
use super::work_pool::WorkPool;

// Re-export generic types that external code depends on
pub use super::task_scheduler::{
    now_ms, spawn_cron_with_interval, CronEvent, CronHandle, CronState, CronStateMachine,
    ScheduledTask, TaskMetadata, UnixTimestampMs,
};

// ============================================================================
// GC-specific constants
// ============================================================================

/// Memory monitor poll interval (100ms).
const MONITOR_INTERVAL_MS: u64 = 100;

/// Interval for periodic counter synchronization (200ms).
///
/// Flushes per-slot `exec_counts` from ValuePages to the global TieredCache
/// DashMap, enabling tier transitions (bytecode/JIT compilation) for live
/// expressions regardless of GC cadence.
const COUNTER_SYNC_INTERVAL_MS: u64 = 200;

/// Allocation rate threshold (allocs/s) to set gc_requested.
/// When exceeded, the cron sets the flag so the trampoline's `maybe_gc()`
/// triggers a GC cycle at the next check.
const ALLOC_RATE_THRESHOLD: u64 = 100_000;

// --- GC Pool Hill Climbing Constants ---

// --- Cron Worker Pool Constants ---

/// Minimum number of cron worker threads.
const CRON_MIN_WORKERS: usize = 1;

/// Maximum number of cron worker threads.
const CRON_MAX_WORKERS: usize = 8;

/// Number of cron workers that start active (unparked).
const CRON_INITIAL_ACTIVE: usize = 2;

/// Global cron worker pool singleton.
static CRON_WORK_POOL: OnceLock<Arc<WorkPool>> = OnceLock::new();

/// Get or initialize the global cron worker pool.
///
/// The pool has `CRON_MIN_WORKERS`..`CRON_MAX_WORKERS` threads with
/// `CRON_INITIAL_ACTIVE` starting unparked. Used exclusively by the GC
/// cron scheduler for off-thread task execution.
pub fn cron_work_pool() -> Arc<WorkPool> {
    Arc::clone(CRON_WORK_POOL.get_or_init(|| {
        Arc::new(WorkPool::with_threads_initial(
            CRON_MIN_WORKERS,
            CRON_MAX_WORKERS,
            CRON_INITIAL_ACTIVE,
        ))
    }))
}

// --- GC Pool Hill Climbing Constants ---

/// EMA smoothing factor for GC alloc/free rate ratio (half-life ~3.1 samples = ~310ms).
const GC_EMA_ALPHA: f64 = 0.2;

/// Cooldown period for GC hill climber (7 ticks = ~700ms settling time).
const GC_COOLDOWN_PERIOD: u32 = 7;

/// Minimum improvement required for GC hill climber to accept a perturbation.
const GC_IMPROVEMENT_THRESHOLD: f64 = 0.05;

// ============================================================================
// MonitorState — Per-Poll Tracking for Memory Monitor
// ============================================================================

/// Tracking state for the memory monitor task.
struct MonitorState {
    /// Allocation count at the previous poll.
    prev_alloc_count: u64,
    /// Timestamp of the previous poll.
    prev_poll_time: Instant,
    /// GC reachability heartbeat at the previous poll.
    /// If this hasn't advanced, GC lifecycle is unreachable and backpressure
    /// must not be escalated (would cause permanent throttling).
    prev_reachable_counter: u64,
    /// Previous GC freed count for delta computation.
    prev_freed_count: u64,
    /// EMA of the alloc/free rate ratio.
    /// Ratio > 1.0 means allocation outpaces GC → need more workers.
    /// Ratio < 1.0 means GC is keeping up → may reduce workers.
    ema_alloc_free_ratio: Ema,
    /// Hill climber for GC pool adaptive sizing.
    gc_climber: HillClimber,
}

impl MonitorState {
    /// Create a new MonitorState with explicit pool parameters.
    ///
    /// Production code reads from `global_gc_pool()` and passes values.
    /// Tests pass hardcoded values without initializing the global pool.
    fn with_params(min_workers: usize, max_workers: usize, active_workers: usize) -> Self {
        Self {
            prev_alloc_count: 0,
            prev_poll_time: Instant::now(),
            prev_reachable_counter: 0,
            prev_freed_count: 0,
            ema_alloc_free_ratio: Ema::new(GC_EMA_ALPHA),
            gc_climber: HillClimber::new(
                GC_COOLDOWN_PERIOD,
                GC_IMPROVEMENT_THRESHOLD,
                min_workers,
                max_workers,
                active_workers,
            ),
        }
    }

    /// Create a new MonitorState from the global GC pool.
    fn new() -> Self {
        let pool = global_gc_pool();
        Self::with_params(
            pool.min_workers(),
            pool.max_workers(),
            pool.active_workers(),
        )
    }
}

// ============================================================================
// GcCronSingleton — GC-specific wrapper
// ============================================================================

/// GC cron manager singleton wrapping `TaskSchedulerSingleton` with GC-specific tasks.
///
/// This is a type alias — the underlying `TaskSchedulerSingleton` handles
/// lifetime management, and `CronHandle` provides lock-free task submission.
pub type GcCronSingleton = TaskSchedulerSingleton;

// ============================================================================
// spawn_gc_cron — GC-specific entry point
// ============================================================================

/// Spawn the GC cron manager on a dedicated thread with pre-configured tasks.
///
/// This function:
/// 1. Spawns a `CronStateMachine` via `spawn_cron()`
/// 2. Waits for the ready signal to ensure the event loop is running
/// 3. Schedules the **memory monitor** task (100ms recurring)
/// 4. Returns a `GcCronSingleton` for lifetime management
///
/// # Arguments
///
/// * `committed_bytes` - Atomic committed bytes counter (read-only for cron)
/// * `alloc_count` - Atomic allocation counter (read-only for cron)
/// * `gc_threshold` - Atomic GC threshold (read-only for cron)
pub fn spawn_gc_cron(
    committed_bytes: Arc<AtomicUsize>,
    alloc_count: Arc<AtomicU64>,
    gc_threshold: Arc<AtomicUsize>,
) -> GcCronSingleton {
    use super::task_scheduler::spawn_cron_with_pool;
    use std::sync::atomic::AtomicBool;

    let cron_pool = cron_work_pool();

    let terminating = Arc::new(AtomicBool::new(false));
    let (handle, thread_handle, ready_rx) = spawn_cron_with_pool(
        Arc::clone(&terminating),
        CronStateMachine::DEFAULT_POLL_INTERVAL_MS,
        "mettatron-gc-cron",
        Arc::clone(&cron_pool),
    );

    // Wait for the event loop to start before scheduling tasks.
    // This prevents a race where tasks are submitted before the receiver is live.
    // Use recv_timeout to avoid blocking forever if the cron thread panics during startup.
    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(()) => {}
        Err(_) => {
            tracing::warn!("GC cron thread did not signal ready within 5s — continuing without cron scheduling");
        }
    }

    // Schedule memory monitor (recurring, 100ms)
    let committed_clone = Arc::clone(&committed_bytes);
    let alloc_clone = Arc::clone(&alloc_count);
    let threshold_clone = Arc::clone(&gc_threshold);
    let mut monitor = MonitorState::new();
    let gc_pool = global_gc_pool();

    // Initial delay matches the interval so the first poll has a meaningful
    // baseline (prev_alloc_count / prev_poll_time are set at construction).
    handle.schedule_recurring(
        MONITOR_INTERVAL_MS,
        MONITOR_INTERVAL_MS,
        "memory-monitor",
        move || {
            execute_memory_monitor(
                &committed_clone,
                &alloc_clone,
                &threshold_clone,
                &mut monitor,
                gc_pool,
            );
            true // always reschedule
        },
    );

    // Schedule counter sync (recurring, 200ms)
    //
    // Flushes per-slot exec_counts from ValuePages to the global TieredCache.
    // Ensures long-lived hot expressions are promoted to bytecode/JIT promptly
    // regardless of GC cadence.
    handle.schedule_recurring(
        COUNTER_SYNC_INTERVAL_MS,
        COUNTER_SYNC_INTERVAL_MS,
        "counter-sync",
        move || {
            execute_counter_sync();
            true // always reschedule
        },
    );

    TaskSchedulerSingleton::with_pool(handle, thread_handle, cron_pool)
}

// ============================================================================
// Task Implementations
// ============================================================================

/// Memory monitor task: reads atomic counters and calls `request_gc()` if
/// allocation rate exceeds threshold or committed bytes exceed GC threshold.
///
/// Two complementary triggers:
/// - **Rate-based**: High allocation velocity (burst detection)
/// - **Threshold-based**: Absolute memory pressure (steady-state detection)
fn execute_memory_monitor(
    committed_bytes: &AtomicUsize,
    alloc_count: &AtomicU64,
    gc_threshold: &AtomicUsize,
    monitor: &mut MonitorState,
    gc_pool: &super::gc_pool::AdaptiveGcPool,
) -> bool {
    let now = Instant::now();
    let current_alloc_count = alloc_count.load(AtomicOrdering::Relaxed);
    let elapsed = now.duration_since(monitor.prev_poll_time);

    let mut should_gc = false;

    // Rate-based trigger: high allocation velocity
    if elapsed.as_nanos() > 0 {
        let delta = current_alloc_count.saturating_sub(monitor.prev_alloc_count);
        let rate = (delta as f64 / elapsed.as_secs_f64()) as u64;

        if rate > ALLOC_RATE_THRESHOLD {
            should_gc = true;
        }
    }

    // Threshold-based trigger: absolute memory pressure
    let current_committed = committed_bytes.load(AtomicOrdering::Relaxed);
    let threshold = gc_threshold.load(AtomicOrdering::Relaxed);
    if current_committed >= threshold {
        should_gc = true;
    }

    // Back-pressure computation: throttle allocation when GC can't keep up.
    //
    // IMPORTANT: Only escalate backpressure if GC lifecycle is reachable.
    // If the reachable counter hasn't advanced since the last poll, it means
    // no code path is calling maybe_quiescent_gc() / maybe_process_gc_response().
    // Escalating backpressure when GC is unreachable causes permanent throttling
    // (livelock) in library/test code that calls eval() without the main.rs
    // between-expression loop.
    let current_reachable = super::gc_allocator::gc_reachable_counter();
    let bp_level = if current_reachable == monitor.prev_reachable_counter {
        // GC lifecycle unreachable — don't escalate (would cause permanent throttling)
        0
    } else if threshold > 0 {
        if current_committed >= threshold * 2 {
            3 // Heavy: > 2x threshold
        } else if current_committed >= threshold * 3 / 2 {
            2 // Medium: 1.5x - 2x threshold
        } else if current_committed >= threshold {
            1 // Light: 1x - 1.5x threshold
        } else {
            0 // None: below threshold
        }
    } else {
        0
    };
    monitor.prev_reachable_counter = current_reachable;
    super::gc_allocator::set_backpressure_level(bp_level);

    // E1-FLIP coordination fix (①c): under the dedicated GC thread the cron is a
    // driver-less GC_REQUESTED producer (and `maybe_async_gc` a 2nd collector regime)
    // that would strand parked rendezvous workers — the dedicated regime triggers
    // collection via the worker-safepoint watermark (`request_concurrent_collection`),
    // the SOLE producer under dedicated. Byte-identical OFF (dedicated default OFF).
    // `should_gc` is still RETURNED below for the unit tests' TOCTOU check.
    if should_gc && !super::gc_allocator::dedicated_gc_enabled() {
        // Phase 9: set the flag AND immediately attempt async GC from the
        // cron thread. `maybe_async_gc` honors the purely-async mandate —
        // it does NOT require `ACTIVE_EVALUATORS == 0`, so it can fire
        // mid-eval. Eliminates the request/respond ping-pong where the
        // trampoline would have to enter a safepoint to consume the flag.
        request_gc();
        let _ = maybe_async_gc();
    }

    // Return whether GC was requested (used by unit tests to avoid TOCTOU
    // races on the global GC_REQUESTED flag).
    let result = should_gc;

    // --- GC Pool Hill Climbing ---
    // Compute alloc rate and free rate, then feed the alloc/free ratio
    // (clamped to [0, 10]) into the EMA → HillClimber for adaptive sizing.
    if elapsed.as_nanos() > 0 {
        let elapsed_secs = elapsed.as_secs_f64();
        let alloc_delta = current_alloc_count.saturating_sub(monitor.prev_alloc_count);
        let alloc_rate = alloc_delta as f64 / elapsed_secs;

        let current_freed = gc_values_freed_total();
        let freed_delta = current_freed.saturating_sub(monitor.prev_freed_count);
        let free_rate = freed_delta as f64 / elapsed_secs;
        monitor.prev_freed_count = current_freed;

        // Compute ratio (alloc / free). Guard against division by zero:
        // if free_rate == 0 but alloc_rate > 0, ratio = 10 (max pressure).
        // if both are 0, ratio = 1.0 (neutral — no scaling action needed).
        let ratio = if free_rate > 0.0 {
            (alloc_rate / free_rate).min(10.0)
        } else if alloc_rate > 0.0 {
            10.0 // GC not freeing but allocation happening → max pressure
        } else {
            1.0 // Idle — neutral
        };

        // Feed ratio as objective to hill climber (higher ratio = worse,
        // hill climber minimizes objective).
        let smoothed = monitor.ema_alloc_free_ratio.update(ratio);
        let decision = monitor.gc_climber.step(smoothed);

        match decision.action {
            ScaleAction::Unpark => {
                gc_pool.unpark_n(decision.count);
            }
            ScaleAction::Park => {
                gc_pool.park_n(decision.count);
            }
            ScaleAction::Hold => {}
        }
    }

    monitor.prev_alloc_count = current_alloc_count;
    monitor.prev_poll_time = now;

    // Check for and respawn dead GC workers
    let respawned = gc_pool.check_and_respawn_workers();
    if respawned > 0 {
        tracing::warn!(respawned, "GC memory monitor: respawned dead GC workers");
    }

    result
}

// ============================================================================
// Counter Sync Infrastructure
// ============================================================================

/// Serializes periodic counter-sync and GC-triggered dead-slot flush.
///
/// With the cron worker pool, both tasks can execute concurrently on different
/// pool workers. This lock prevents double-counting and use-after-free.
///
/// Contention is near-zero: the periodic flush skips when GC is in progress,
/// so the lock only contends in the narrow window (~ns) where GC starts
/// between the periodic flush's `is_gc_in_progress()` check and lock acquisition.
pub(crate) static COUNTER_FLUSH_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// Periodic counter sync: iterate all live slots, flush non-zero exec_counts
/// to the global TieredCache. Runs on a cron worker pool thread (~1-5ms per sweep).
///
/// Skips the cycle if GC is in progress (`GC_IN_PROGRESS` flag set) to avoid
/// reading slot content that GC Phase 3 may be concurrently freeing/poisoning.
/// Unflushed counts will be picked up next cycle (200ms) or by the GC-triggered
/// flush for dead values.
///
/// Acquires `COUNTER_FLUSH_LOCK` to serialize with the GC-triggered flush
/// (both can be dispatched to different worker pool threads concurrently).
fn execute_counter_sync() {
    use crate::backend::bytecode::tiered_cache::global_tiered_cache;
    use crate::backend::models::gc_allocator::{global_allocator, is_gc_in_progress};
    use crate::backend::models::{MettaValue, MettaValueInner, MettaValueTrait};

    // Skip if GC is in progress — process_gc_response Phase 3 may be
    // freeing slots concurrently. The GC-triggered flush handles
    // dead values, and live values will be flushed next periodic cycle.
    if is_gc_in_progress() {
        return;
    }

    // Serialize with GC-triggered flush (may be running on another pool worker).
    let _flush_guard = COUNTER_FLUSH_LOCK.lock();

    // Re-check after acquiring lock — GC may have started while we waited.
    if is_gc_in_progress() {
        return;
    }

    let allocator = global_allocator();
    let slot_size = allocator.value_slot_size();
    let cache = global_tiered_cache();

    // Read lock on pages — same weight as GC snapshot, no starvation concern.
    let pages = allocator.value_pages_read();
    for page in pages.iter() {
        let bump_count = page.bump_count();
        for slot_idx in 0..bump_count {
            // Skip freed slots (epoch sentinel)
            let epoch = page.slot_epoch(slot_idx);
            if epoch == u64::MAX {
                continue;
            }
            let count = page.exec_count(slot_idx);
            if count == 0 {
                continue;
            }

            let cached_hash = page.compilation_hash(slot_idx);
            if cached_hash != 0 {
                // Fast path: reuse cached hash — DashMap lookup by u64, no recursive xxh3
                if let Some(state) = cache.entries.get(&cached_hash) {
                    state
                        .execution_count
                        .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
                    #[cfg(feature = "track-stats")]
                    cache
                        .total_executions
                        .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
                    page.exec_count_fetch_sub(slot_idx, count);
                    let new_count = state
                        .execution_count
                        .load(std::sync::atomic::Ordering::Relaxed);
                    cache.maybe_trigger_jit1(&state, new_count);
                    cache.maybe_trigger_jit2(&state, new_count);
                    continue;
                }
                // Hash cached but entry removed? Fall through to slow path.
            }

            // Skip expensive hashing for expressions below compilation threshold.
            // Let count accumulate in the slot across sync cycles until threshold
            // is crossed. Most expressions are cold (1-3 execs) and will never be
            // compiled — no point hashing their entire tree or creating a DashMap entry.
            if count < cache.bytecode_threshold {
                continue;
            }

            // Slow path: expression crossed threshold — hash, create DashMap entry, cache hash
            let ptr = page.slot_ptr(slot_idx, slot_size) as *const MettaValueInner;
            // SAFETY: slot is still live — COUNTER_FLUSH_LOCK prevents
            // concurrent GC Phase 3 freeing, and epoch != u64::MAX confirms
            // the slot hasn't been freed.
            let value = unsafe { MettaValue::from_inner_ptr(ptr) };
            let state = cache.get_or_create_state(&value);
            page.set_compilation_hash(slot_idx, state.expr_hash);
            state
                .execution_count
                .fetch_add(count, std::sync::atomic::Ordering::Relaxed);
            #[cfg(feature = "track-stats")]
            cache
                .total_executions
                .fetch_add(count as u64, std::sync::atomic::Ordering::Relaxed);
            page.exec_count_fetch_sub(slot_idx, count);
            let new_count = state
                .execution_count
                .load(std::sync::atomic::Ordering::Relaxed);
            cache.maybe_trigger_bytecode(&value, &state, new_count);
            cache.maybe_trigger_jit1(&state, new_count);
            cache.maybe_trigger_jit2(&state, new_count);
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::super::gc_pool::AdaptiveGcPool;
    use super::*;
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;

    // ========================================================================
    // GC-specific integration tests
    // ========================================================================

    /// Test spawning GcCronSingleton and clean shutdown.
    #[test]
    fn test_spawn_gc_cron_and_shutdown() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_threshold = Arc::new(AtomicUsize::new(1024 * 1024 * 1024)); // 1 GB

        let singleton = spawn_gc_cron(committed, alloc_count, gc_threshold);

        // Should shut down cleanly
        singleton.shutdown();
    }

    /// Test that high allocation rate triggers GC request.
    ///
    /// Polls the monotonic `gc_requests_total()` counter rather than the
    /// transient `GC_REQUESTED` flag. Post Phase 9 (commit `82ccb77`), the
    /// cron monitor calls `request_gc()` AND `maybe_async_gc()` back-to-back
    /// on the same thread tick — `maybe_async_gc()` consumes the flag via
    /// CAS within nanoseconds of being set, so cross-thread polling of
    /// `is_gc_requested()` would race deterministically. The monotonic
    /// counter is sticky and append-only: it captures the request event
    /// regardless of which path subsequently consumed the flag.
    #[test]
    fn test_gc_requested_on_high_alloc_rate() {
        use super::super::gc_allocator::gc_requests_total;

        // Capture baseline count before spawning the cron (other parallel
        // tests may have incremented it; we only care about the delta).
        let baseline = gc_requests_total();

        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_threshold = Arc::new(AtomicUsize::new(1024 * 1024 * 1024)); // 1 GB

        let singleton = spawn_gc_cron(
            Arc::clone(&committed),
            Arc::clone(&alloc_count),
            Arc::clone(&gc_threshold),
        );

        // Simulate high allocation rate: 1M allocs
        alloc_count.store(1_000_000, Ordering::Relaxed);

        // Poll until the monotonic counter advances (with timeout).
        // The monitor fires every 100ms; allow 20 poll cycles (2s) of
        // scheduler slack under heavy parallel test load.
        let deadline = Instant::now() + Duration::from_millis(2000);
        let mut observed = false;
        while Instant::now() < deadline {
            if gc_requests_total() > baseline {
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            observed,
            "gc_requests_total() should advance after high allocation rate \
             (baseline {}, current {}) — cron monitor must call request_gc() \
             when alloc rate exceeds ALLOC_RATE_THRESHOLD",
            baseline,
            gc_requests_total(),
        );

        singleton.shutdown();
    }

    /// Unit test: rate calculation triggers GC when rate > threshold.
    #[test]
    fn test_monitor_state_rate_calculation() {
        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(0);
        let gc_threshold = AtomicUsize::new(1024 * 1024 * 1024); // 1 GB — won't trigger
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // Simulate time passing and allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(50_000, Ordering::Relaxed);

        // Second poll: rate = 50k / 0.1s = 500k/s > threshold
        let triggered =
            execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(triggered, "should request GC at 500k allocs/s");
        pool.shutdown();
    }

    /// Unit test: low rate does NOT trigger GC.
    #[test]
    fn test_monitor_state_below_threshold() {
        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(0);
        let gc_threshold = AtomicUsize::new(1024 * 1024 * 1024); // 1 GB — won't trigger
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // Simulate time passing and low allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(100, Ordering::Relaxed); // 100 / 0.1s = 1000/s < 100k threshold

        // Second poll — assert on return value instead of global flag (race-free)
        let triggered =
            execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(!triggered, "should NOT request GC at 1000 allocs/s");
        pool.shutdown();
    }

    /// Unit test: committed bytes >= threshold triggers GC.
    #[test]
    fn test_threshold_based_trigger() {
        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(8 * 1024 * 1024); // 8 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First poll: committed (8 MB) >= gc_threshold (4 MB) should trigger
        let triggered =
            execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(
            triggered,
            "should request GC when committed bytes exceed threshold"
        );
        pool.shutdown();
    }

    /// Unit test: committed bytes < threshold does NOT trigger GC.
    #[test]
    fn test_threshold_not_triggered_below() {
        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(2 * 1024 * 1024); // 2 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // committed (2 MB) < gc_threshold (4 MB) should NOT trigger
        let triggered =
            execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(
            !triggered,
            "should NOT request GC when committed bytes below threshold"
        );
        pool.shutdown();
    }

    /// Test that the cron pool singleton initializes with expected parameters.
    #[test]
    fn test_cron_work_pool_singleton() {
        let pool = cron_work_pool();
        assert_eq!(pool.min_threads(), CRON_MIN_WORKERS);
        assert_eq!(pool.max_threads(), CRON_MAX_WORKERS);
        assert_eq!(pool.initial_active(), CRON_INITIAL_ACTIVE);

        // Second call returns the same Arc (idempotent)
        let pool2 = cron_work_pool();
        assert!(Arc::ptr_eq(&pool, &pool2));
    }

    /// Test that the GC cron memory monitor runs on pool threads, not the
    /// cron scheduler thread.
    #[test]
    fn test_gc_cron_memory_monitor_runs_on_pool() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_threshold = Arc::new(AtomicUsize::new(1024 * 1024 * 1024)); // 1 GB

        let singleton = spawn_gc_cron(
            Arc::clone(&committed),
            Arc::clone(&alloc_count),
            Arc::clone(&gc_threshold),
        );

        // Schedule a one-shot task that records its thread name
        let thread_name = Arc::new(parking_lot::Mutex::new(String::new()));
        let tn = Arc::clone(&thread_name);
        let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = Arc::clone(&done);

        singleton
            .handle
            .schedule_once(0, "thread-name-probe", move || {
                *tn.lock() = std::thread::current()
                    .name()
                    .unwrap_or("unknown")
                    .to_string();
                d.store(true, Ordering::Release);
                true
            });

        // Wait for probe task to execute
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done.load(Ordering::Acquire) {
            if Instant::now() > deadline {
                panic!("Timeout waiting for cron pool probe task");
            }
            thread::sleep(Duration::from_millis(10));
        }

        let name = thread_name.lock().clone();
        assert!(
            name.starts_with("work-pool-"),
            "Expected task on work-pool-* thread, got '{}'",
            name
        );

        singleton.shutdown();
    }
}
