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
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::adaptive_pool::{Ema, HillClimber, ScaleAction};
use super::gc_allocator::{gc_values_freed_total, request_gc};
use super::gc_pool::global_gc_pool;
use super::task_scheduler::TaskSchedulerSingleton;

// Re-export generic types that external code depends on
pub use super::task_scheduler::{
    CronHandle, CronState, CronStateMachine, CronEvent,
    TaskMetadata, ScheduledTask, UnixTimestampMs, now_ms,
    spawn_cron_with_interval,
};

// ============================================================================
// GC-specific constants
// ============================================================================

/// Memory monitor poll interval (100ms).
const MONITOR_INTERVAL_MS: u64 = 100;

/// Allocation rate threshold (allocs/s) to set gc_requested.
/// When exceeded, the cron sets the flag so the trampoline's `maybe_gc()`
/// triggers a GC cycle at the next check.
const ALLOC_RATE_THRESHOLD: u64 = 100_000;

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
        Self::with_params(pool.min_workers(), pool.max_workers(), pool.active_workers())
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
    use std::sync::atomic::AtomicBool;
    use super::task_scheduler::spawn_cron_with_interval_and_name;

    let terminating = Arc::new(AtomicBool::new(false));
    let (handle, thread_handle, ready_rx) = spawn_cron_with_interval_and_name(
        Arc::clone(&terminating),
        CronStateMachine::DEFAULT_POLL_INTERVAL_MS,
        "mettatron-gc-cron",
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
    handle.schedule_recurring(MONITOR_INTERVAL_MS, MONITOR_INTERVAL_MS, "memory-monitor", move || {
        execute_memory_monitor(&committed_clone, &alloc_clone, &threshold_clone, &mut monitor, gc_pool);
        true // always reschedule
    });

    TaskSchedulerSingleton::new(handle, thread_handle)
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
) {
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

    if should_gc {
        request_gc();
    }

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
        let action = monitor.gc_climber.step(smoothed);

        match action {
            ScaleAction::Unpark => { gc_pool.unpark_one(); }
            ScaleAction::Park => { gc_pool.park_one(); }
            ScaleAction::Hold => {}
        }
    }

    monitor.prev_alloc_count = current_alloc_count;
    monitor.prev_poll_time = now;

    // Check for and respawn dead GC workers
    let respawned = gc_pool.check_and_respawn_workers();
    if respawned > 0 {
        tracing::warn!(
            respawned,
            "GC memory monitor: respawned dead GC workers"
        );
    }
}


// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::gc_allocator::is_gc_requested;
    use super::super::gc_pool::AdaptiveGcPool;
    use std::sync::atomic::Ordering;
    use std::time::Duration;
    use std::thread;

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
    /// Uses a polling loop with a deadline because `GC_REQUESTED` is a global
    /// flag that may be cleared by `maybe_trigger_gc()` in parallel tests.
    /// The monitor re-sets it on each poll, so we check repeatedly.
    #[test]
    fn test_gc_requested_on_high_alloc_rate() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

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

        // Poll until GC_REQUESTED is set (with timeout).
        // The monitor fires every 100ms and will re-set the flag if cleared
        // by other parallel tests calling maybe_trigger_gc().
        let deadline = Instant::now() + Duration::from_millis(500);
        let mut observed = false;
        while Instant::now() < deadline {
            if is_gc_requested() {
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        assert!(
            observed,
            "GC_REQUESTED should be true after high allocation rate (within 500ms)"
        );

        singleton.shutdown();
    }

    /// Unit test: rate calculation triggers GC when rate > threshold.
    #[test]
    fn test_monitor_state_rate_calculation() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

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
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(is_gc_requested(), "should request GC at 500k allocs/s");
        pool.shutdown();
    }

    /// Unit test: low rate does NOT trigger GC.
    #[test]
    fn test_monitor_state_below_threshold() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(0);
        let gc_threshold = AtomicUsize::new(1024 * 1024 * 1024); // 1 GB — won't trigger
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // Clear again after first poll — parallel tests may set GC_REQUESTED
        // between our initial clear and the first poll.
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        // Simulate time passing and low allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(100, Ordering::Relaxed); // 100 / 0.1s = 1000/s < 100k threshold

        // Second poll
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        assert!(
            !is_gc_requested(),
            "should NOT request GC at 1000 allocs/s"
        );
        pool.shutdown();
    }

    /// Unit test: committed bytes >= threshold triggers GC.
    #[test]
    fn test_threshold_based_trigger() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(8 * 1024 * 1024); // 8 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // committed (8 MB) >= gc_threshold (4 MB) should trigger
        assert!(
            is_gc_requested(),
            "should request GC when committed bytes exceed threshold"
        );
        pool.shutdown();
    }

    /// Unit test: committed bytes < threshold does NOT trigger GC.
    #[test]
    fn test_threshold_not_triggered_below() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(2 * 1024 * 1024); // 2 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::with_params(1, 2, 1);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // Clear immediately before poll — minimize race window with parallel tests
        // that may set GC_REQUESTED via their own cron monitors.
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        // First poll: set baseline (should NOT set GC_REQUESTED since committed < threshold)
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // Clear again and re-poll to get a clean read.
        // This double-poll pattern isolates us from parallel tests: the first poll
        // sets baselines, the second poll computes rates from zero delta + checks
        // committed < threshold. We clear right before the second poll.
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::SeqCst);
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor, &pool);

        // committed (2 MB) < gc_threshold (4 MB) should NOT trigger
        assert!(
            !is_gc_requested(),
            "should NOT request GC when committed bytes below threshold"
        );
        pool.shutdown();
    }
}
