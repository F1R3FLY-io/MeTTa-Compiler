//! GC Cron Manager — Periodic Memory Monitoring & Rate Detection
//!
//! A lock-free reactive state machine scheduler (modeled after libgrammstein's
//! `CronStateMachine`) that runs on a dedicated thread. It periodically reads
//! atomic memory counters from the `SlabAllocator` and sets a `gc_requested`
//! flag when allocation rate is high.
//!
//! ## Architecture
//!
//! The cron manager does NOT own or borrow the allocator. It reads only atomic
//! counters (`committed_bytes_atomic`, `alloc_count_atomic`) shared via `Arc`.
//! It writes only the `gc_requested: Arc<AtomicBool>` flag.
//!
//! ## Scheduled Tasks
//!
//! 1. **Memory monitor** (100ms interval): Reads atomics, computes allocation
//!    rate (allocs/s), sets `gc_requested` if rate exceeds threshold.
//!
//! 2. **Stats reporter** (5s interval, optional): Logs memory statistics.
//!    Enabled by `METTA_GC_STATS=1` environment variable.
//!
//! ## Thread Safety
//!
//! - Eval thread writes atomics, cron reads with `Relaxed` ordering.
//! - Cron writes `gc_requested`, eval reads with `Relaxed` ordering.
//! - No locks, no channels for the hot-path data flow.
//! - Shutdown uses `terminating: Arc<AtomicBool>` (Release/Acquire).

use std::collections::BinaryHeap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

// ============================================================================
// Configuration
// ============================================================================

/// Memory monitor poll interval.
const MONITOR_INTERVAL: Duration = Duration::from_millis(100);

/// Stats reporter interval.
const STATS_INTERVAL: Duration = Duration::from_secs(5);

/// Allocation rate threshold (allocs/s) to set gc_requested.
/// When exceeded, the cron sets the flag so the trampoline's `maybe_gc()`
/// triggers a GC cycle at the next check.
const ALLOC_RATE_THRESHOLD: u64 = 100_000;

// ============================================================================
// CronTask — Scheduled Task Entry
// ============================================================================

/// A scheduled task in the cron manager's priority queue.
struct CronTask {
    /// When this task is next due to run.
    next_run: Instant,
    /// Task identifier (for matching).
    id: CronTaskId,
    /// How often to repeat (None = one-shot). Stored for future extensibility
    /// (one-shot tasks, dynamic rescheduling).
    #[allow(dead_code)]
    interval: Option<Duration>,
}

/// Ordered by next_run (earliest first) for the BinaryHeap.
impl Ord for CronTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse because BinaryHeap is a max-heap; we want min-heap
        other.next_run.cmp(&self.next_run)
    }
}

impl PartialOrd for CronTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for CronTask {
    fn eq(&self, other: &Self) -> bool {
        self.next_run == other.next_run && self.id == other.id
    }
}

impl Eq for CronTask {}

/// Task identifiers for the GC cron manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CronTaskId {
    /// Periodic memory monitoring (100ms).
    MemoryMonitor,
    /// Optional stats reporting (5s).
    StatsReporter,
}

// ============================================================================
// CronState — Reactive State Machine States
// ============================================================================

/// States for the reactive cron state machine.
///
/// `ExecuteTask` and `Sleeping` are defined for completeness of the state machine
/// specification but are handled inline during transitions (not as entry states).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum CronState {
    /// Check for due tasks.
    CheckTasks,
    /// Execute the next due task.
    ExecuteTask,
    /// Sleep until the next task is due or termination.
    Sleeping,
    /// Terminal state — exit the event loop.
    Terminated,
}

/// Events that drive state transitions.
///
/// `TaskCompleted` is defined for completeness of the event model but task
/// completion is currently handled inline by rescheduling after `TaskDue`.
#[derive(Debug)]
#[allow(dead_code)]
enum CronEvent {
    /// A task is due for execution.
    TaskDue(CronTaskId),
    /// No tasks are due; sleep until the next one.
    NoTasksDue { sleep_duration: Duration },
    /// Task execution completed; reschedule if recurring.
    TaskCompleted(CronTaskId),
    /// Termination was requested.
    TerminationRequested,
}

// ============================================================================
// MonitorState — Per-Poll Tracking
// ============================================================================

/// Tracking state for the memory monitor task.
struct MonitorState {
    /// Allocation count at the previous poll.
    prev_alloc_count: u64,
    /// Timestamp of the previous poll.
    prev_poll_time: Instant,
}

impl MonitorState {
    fn new() -> Self {
        Self {
            prev_alloc_count: 0,
            prev_poll_time: Instant::now(),
        }
    }
}

// ============================================================================
// GcCronHandle — Public API
// ============================================================================

/// Handle to the GC cron manager thread.
///
/// Used to shut down the cron manager gracefully.
pub struct GcCronHandle {
    /// Termination flag shared with the cron thread.
    terminating: Arc<AtomicBool>,
    /// Thread handle for joining.
    handle: Option<JoinHandle<()>>,
}

impl GcCronHandle {
    /// Request graceful shutdown of the cron manager.
    ///
    /// Sets the termination flag and waits for the cron thread to exit.
    pub fn request_shutdown(&mut self) {
        self.terminating.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for GcCronHandle {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

// ============================================================================
// spawn_gc_cron — Entry Point
// ============================================================================

/// Spawn the GC cron manager on a dedicated thread.
///
/// The cron manager periodically reads atomic counters from the `SlabAllocator`
/// and sets `gc_requested` when allocation rate is high.
///
/// # Arguments
///
/// * `committed_bytes` - Atomic committed bytes counter (read-only for cron).
/// * `alloc_count` - Atomic allocation counter (read-only for cron).
/// * `gc_requested` - Atomic flag set by cron when GC is needed (write for cron).
/// * `terminating` - Shared termination flag.
///
/// # Returns
///
/// A `GcCronHandle` for shutting down the cron manager.
pub fn spawn_gc_cron(
    committed_bytes: Arc<AtomicUsize>,
    alloc_count: Arc<AtomicU64>,
    gc_requested: Arc<AtomicBool>,
    terminating: Arc<AtomicBool>,
) -> GcCronHandle {
    let term_clone = Arc::clone(&terminating);

    let handle = thread::Builder::new()
        .name("mettatron-gc-cron".to_string())
        .spawn(move || {
            cron_event_loop(committed_bytes, alloc_count, gc_requested, terminating);
        })
        .expect("failed to spawn GC cron thread");

    GcCronHandle {
        terminating: term_clone,
        handle: Some(handle),
    }
}

// ============================================================================
// cron_event_loop — Reactive State Machine
// ============================================================================

/// Main event loop for the GC cron manager.
///
/// Implements a reactive state machine with explicit states and events:
///
/// ```text
/// CheckTasks ──TaskDue──> ExecuteTask ──TaskCompleted──> CheckTasks
///     │                                                       │
///     └──NoTasksDue──> Sleeping ──(wake)──> CheckTasks        │
///     │                                                       │
///     └──TerminationRequested──> Terminated                   │
///     └───────────────────────────────────────────────────────┘
/// ```
fn cron_event_loop(
    committed_bytes: Arc<AtomicUsize>,
    alloc_count: Arc<AtomicU64>,
    gc_requested: Arc<AtomicBool>,
    terminating: Arc<AtomicBool>,
) {
    let stats_enabled = std::env::var("METTA_GC_STATS").is_ok();

    // Initialize the task queue
    let mut task_queue = BinaryHeap::new();
    let now = Instant::now();

    // Schedule memory monitor (recurring, 100ms)
    task_queue.push(CronTask {
        next_run: now + MONITOR_INTERVAL,
        id: CronTaskId::MemoryMonitor,
        interval: Some(MONITOR_INTERVAL),
    });

    // Schedule stats reporter (recurring, 5s) if enabled
    if stats_enabled {
        task_queue.push(CronTask {
            next_run: now + STATS_INTERVAL,
            id: CronTaskId::StatsReporter,
            interval: Some(STATS_INTERVAL),
        });
    }

    let mut monitor_state = MonitorState::new();
    let mut state = CronState::CheckTasks;

    // State machine event loop
    loop {
        let event = match state {
            CronState::CheckTasks => {
                // Check for termination first
                if terminating.load(Ordering::Acquire) {
                    CronEvent::TerminationRequested
                } else if let Some(task) = task_queue.peek() {
                    let now = Instant::now();
                    if task.next_run <= now {
                        // Task is due — pop and execute
                        let task = task_queue.pop().expect("peeked task should be poppable");
                        CronEvent::TaskDue(task.id)
                    } else {
                        // Sleep until next task
                        let sleep_duration = task.next_run - now;
                        CronEvent::NoTasksDue { sleep_duration }
                    }
                } else {
                    // No tasks — shouldn't happen, but sleep a bit
                    CronEvent::NoTasksDue { sleep_duration: MONITOR_INTERVAL }
                }
            }

            CronState::ExecuteTask => {
                // This state is transient — we already popped the task in CheckTasks.
                // The actual execution happens inline below when we transition.
                unreachable!("ExecuteTask is handled inline during TaskDue transition");
            }

            CronState::Sleeping => {
                // We already slept during the NoTasksDue transition.
                // Just go back to checking tasks.
                CronEvent::TaskDue(CronTaskId::MemoryMonitor) // dummy, won't be used
            }

            CronState::Terminated => break,
        };

        // State transitions based on events
        state = match event {
            CronEvent::TaskDue(task_id) => {
                // Execute the task (panic-safe)
                let completed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    match task_id {
                        CronTaskId::MemoryMonitor => {
                            execute_memory_monitor(
                                &committed_bytes,
                                &alloc_count,
                                &gc_requested,
                                &mut monitor_state,
                            );
                        }
                        CronTaskId::StatsReporter => {
                            execute_stats_reporter(&committed_bytes, &alloc_count);
                        }
                    }
                }));

                if completed.is_err() {
                    eprintln!("[gc-cron] task {:?} panicked, continuing", task_id);
                }

                // Reschedule recurring tasks
                let interval = match task_id {
                    CronTaskId::MemoryMonitor => Some(MONITOR_INTERVAL),
                    CronTaskId::StatsReporter => Some(STATS_INTERVAL),
                };
                if let Some(interval) = interval {
                    task_queue.push(CronTask {
                        next_run: Instant::now() + interval,
                        id: task_id,
                        interval: Some(interval),
                    });
                }

                CronState::CheckTasks
            }

            CronEvent::NoTasksDue { sleep_duration } => {
                // Smart sleep: wake early if termination is requested.
                // Use small sleep chunks so we respond to termination quickly.
                let sleep_end = Instant::now() + sleep_duration;
                let chunk = Duration::from_millis(50);
                while Instant::now() < sleep_end {
                    if terminating.load(Ordering::Acquire) {
                        break;
                    }
                    let remaining = sleep_end.saturating_duration_since(Instant::now());
                    thread::sleep(remaining.min(chunk));
                }
                CronState::CheckTasks
            }

            CronEvent::TaskCompleted(_) => CronState::CheckTasks,

            CronEvent::TerminationRequested => CronState::Terminated,
        };
    }
}

// ============================================================================
// Task Implementations
// ============================================================================

/// Memory monitor task: reads atomic counters and sets gc_requested if
/// allocation rate exceeds threshold.
fn execute_memory_monitor(
    _committed_bytes: &AtomicUsize,
    alloc_count: &AtomicU64,
    gc_requested: &AtomicBool,
    monitor: &mut MonitorState,
) {
    let now = Instant::now();
    let current_alloc_count = alloc_count.load(Ordering::Relaxed);
    let elapsed = now.duration_since(monitor.prev_poll_time);

    if elapsed.as_nanos() > 0 {
        let delta = current_alloc_count.saturating_sub(monitor.prev_alloc_count);
        let rate = (delta as f64 / elapsed.as_secs_f64()) as u64;

        if rate > ALLOC_RATE_THRESHOLD {
            // High allocation rate — request GC
            gc_requested.store(true, Ordering::Relaxed);
        }
    }

    monitor.prev_alloc_count = current_alloc_count;
    monitor.prev_poll_time = now;
}

/// Stats reporter task: logs memory statistics to stderr.
fn execute_stats_reporter(
    committed_bytes: &AtomicUsize,
    alloc_count: &AtomicU64,
) {
    let committed = committed_bytes.load(Ordering::Relaxed);
    let allocs = alloc_count.load(Ordering::Relaxed);

    eprintln!(
        "[gc-cron] committed={:.1} MB  total_allocs={}",
        committed as f64 / (1024.0 * 1024.0),
        allocs,
    );
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_and_shutdown() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_requested = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        let mut handle = spawn_gc_cron(
            committed,
            alloc_count,
            gc_requested,
            terminating,
        );

        // Should shut down cleanly
        handle.request_shutdown();
    }

    #[test]
    fn test_gc_requested_on_high_alloc_rate() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_requested = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        let mut handle = spawn_gc_cron(
            Arc::clone(&committed),
            Arc::clone(&alloc_count),
            Arc::clone(&gc_requested),
            Arc::clone(&terminating),
        );

        // Simulate high allocation rate: 1M allocs in 100ms
        alloc_count.store(1_000_000, Ordering::Relaxed);

        // Wait for at least one monitor poll
        thread::sleep(Duration::from_millis(250));

        // gc_requested should be set
        assert!(
            gc_requested.load(Ordering::Relaxed),
            "gc_requested should be true after high allocation rate"
        );

        handle.request_shutdown();
    }

    #[test]
    fn test_no_gc_requested_on_low_alloc_rate() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_requested = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        let mut handle = spawn_gc_cron(
            Arc::clone(&committed),
            Arc::clone(&alloc_count),
            Arc::clone(&gc_requested),
            Arc::clone(&terminating),
        );

        // Simulate low allocation rate: 10 allocs
        alloc_count.store(10, Ordering::Relaxed);

        // Wait for at least one monitor poll
        thread::sleep(Duration::from_millis(250));

        // gc_requested should NOT be set
        assert!(
            !gc_requested.load(Ordering::Relaxed),
            "gc_requested should be false for low allocation rate"
        );

        handle.request_shutdown();
    }

    #[test]
    fn test_drop_triggers_shutdown() {
        let committed = Arc::new(AtomicUsize::new(0));
        let alloc_count = Arc::new(AtomicU64::new(0));
        let gc_requested = Arc::new(AtomicBool::new(false));
        let terminating = Arc::new(AtomicBool::new(false));

        {
            let _handle = spawn_gc_cron(
                committed,
                alloc_count,
                gc_requested,
                terminating,
            );
            // _handle dropped here — should trigger shutdown
        }
        // Should not hang or panic
    }

    #[test]
    fn test_monitor_state_rate_calculation() {
        let alloc_count = AtomicU64::new(0);
        let gc_requested = AtomicBool::new(false);
        let committed = AtomicUsize::new(0);
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_requested, &mut monitor);

        // Simulate time passing and allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(50_000, Ordering::Relaxed);

        // Second poll: rate = 50k / 0.1s = 500k/s > threshold
        execute_memory_monitor(&committed, &alloc_count, &gc_requested, &mut monitor);

        assert!(gc_requested.load(Ordering::Relaxed),
            "should request GC at 500k allocs/s");
    }

    #[test]
    fn test_monitor_state_below_threshold() {
        let alloc_count = AtomicU64::new(0);
        let gc_requested = AtomicBool::new(false);
        let committed = AtomicUsize::new(0);
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_requested, &mut monitor);

        // Simulate time passing and low allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(100, Ordering::Relaxed); // 100 / 0.1s = 1000/s < 100k threshold

        // Second poll
        execute_memory_monitor(&committed, &alloc_count, &gc_requested, &mut monitor);

        assert!(!gc_requested.load(Ordering::Relaxed),
            "should NOT request GC at 1000 allocs/s");
    }

    #[test]
    fn test_cron_task_ordering() {
        let now = Instant::now();
        let mut heap = BinaryHeap::new();

        heap.push(CronTask {
            next_run: now + Duration::from_millis(200),
            id: CronTaskId::StatsReporter,
            interval: Some(STATS_INTERVAL),
        });
        heap.push(CronTask {
            next_run: now + Duration::from_millis(100),
            id: CronTaskId::MemoryMonitor,
            interval: Some(MONITOR_INTERVAL),
        });

        // Min-heap: earliest task should be popped first
        let first = heap.pop().expect("should have task");
        assert_eq!(first.id, CronTaskId::MemoryMonitor);

        let second = heap.pop().expect("should have task");
        assert_eq!(second.id, CronTaskId::StatsReporter);
    }
}
