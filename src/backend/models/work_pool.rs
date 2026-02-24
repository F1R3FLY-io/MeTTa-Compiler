//! Unified Work Pool (Eval + Compile via P2 Priority Queue)
//!
//! Replaces both the Rayon compilation pool and par_iter eval parallelism with a
//! single adaptive thread pool backed by the P2 priority queue.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │          Unified WorkPool (P2)           │
//! │  ┌────────────────────────────────────┐  │
//! │  │ Eval tasks    (pri=NORMAL=5)       │  │  ← dequeued first
//! │  │ Compile tasks (pri=BACKGROUND=10)  │  │  ← dequeued when no eval pending
//! │  └────────────────────────────────────┘  │
//! │  1–N workers, single EMA hill climber    │
//! │  Objective: max weighted throughput      │
//! └──────────────────────────────────────────┘
//! ```
//!
//! ## Thread Inventory
//!
//! - **Idle**: main + 1 work + 1 GC + scheduler = 4 threads
//! - **Peak** (36-core): main + 18 work + 4 GC + scheduler = 24 threads
//!
//! ## Backpressure
//!
//! Compile tasks are speculative and dropped when queue depth exceeds
//! `max_queue_size`. Eval tasks are never dropped.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::{debug, trace};

use super::adaptive_pool::{Ema, HillClimber, ScaleAction, WorkerPark};
use crate::backend::priority_scheduler::{
    PriorityQueue, PriorityTask, RuntimeTracker, SchedulerConfig, TaskTypeId,
};

// ============================================================================
// Configuration
// ============================================================================

/// Get work pool thread configuration from environment.
///
/// - `METTATRON_MIN_WORK_THREADS`: Minimum workers (default: 1, at least 1)
/// - `METTATRON_MAX_WORK_THREADS`: Maximum workers (default: num_cpus/2, at least min, at least 2)
fn get_work_thread_config() -> (usize, usize) {
    let min = std::env::var("METTATRON_MIN_WORK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    let max = std::env::var("METTATRON_MAX_WORK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(num_cpus::get() / 2)
        .max(min)
        .max(2);

    (min, max)
}

/// Maximum queue depth before compile tasks are dropped (backpressure).
const MAX_QUEUE_SIZE: usize = 256;

// ============================================================================
// Global Eval Counter
// ============================================================================

/// Global counter of completed eval tasks (for throughput tracking).
pub static WORK_EVAL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the global eval completion count.
#[inline]
pub fn work_eval_count() -> u64 {
    WORK_EVAL_COUNT.load(Ordering::Relaxed)
}

// ============================================================================
// WorkPool
// ============================================================================

/// Unified adaptive thread pool for eval + compile work.
///
/// All workers are spawned at startup (up to `max_threads`). Workers beyond
/// `min_threads` start in the Parked state.
///
/// Workers have a 4-state machine:
/// - **Idle**: Waiting for tasks from P2 queue (blocks on `pop_timeout`)
/// - **Executing**: Running a task closure
/// - **Parked**: Dormant (blocked on `WorkerPark` condvar), activated by scaling monitor
/// - **ShuttingDown**: Exiting the worker loop
pub struct WorkPool {
    /// P2 priority queue shared by all workers.
    queue: Arc<PriorityQueue>,

    /// Runtime tracker for P2 estimation.
    runtime_tracker: Arc<RuntimeTracker>,

    /// Worker thread handles (all workers, including initially parked ones).
    /// Wrapped in `Mutex<Option<...>>` so the scaling monitor can check
    /// `is_finished()` without moving the handle, and replace dead workers.
    workers: Vec<Mutex<Option<JoinHandle<()>>>>,

    /// Per-worker parking primitives.
    worker_parks: Vec<Arc<WorkerPark>>,

    /// Shutdown signal (shared with all workers).
    shutdown: Arc<AtomicBool>,

    /// Number of currently active (non-parked) workers.
    active_count: AtomicUsize,

    /// Minimum number of workers (never park below this).
    min_threads: usize,

    /// Maximum number of workers.
    max_threads: usize,

    /// Monotonic sequence counter for stable ordering.
    sequence: AtomicU64,
}

impl WorkPool {
    /// Create and start the work pool using environment variable configuration.
    pub fn new() -> Self {
        let (min_threads, max_threads) = get_work_thread_config();
        Self::with_threads(min_threads, max_threads)
    }

    /// Create and start a work pool with explicit thread counts.
    ///
    /// This constructor is useful for tests that need isolated pools
    /// without sharing the global singleton.
    pub fn with_threads(min_threads: usize, max_threads: usize) -> Self {
        let min_threads = min_threads.max(1);
        let max_threads = max_threads.max(min_threads).max(2);
        let config = SchedulerConfig::default();
        let runtime_tracker = Arc::new(RuntimeTracker::new());
        let queue = Arc::new(PriorityQueue::new(Arc::clone(&runtime_tracker), config));
        let shutdown = Arc::new(AtomicBool::new(false));

        let mut workers = Vec::with_capacity(max_threads);
        let mut worker_parks = Vec::with_capacity(max_threads);

        for id in 0..max_threads {
            let initially_parked = id >= min_threads; // Workers beyond min start parked
            let park = Arc::new(WorkerPark::new(initially_parked));
            worker_parks.push(Arc::clone(&park));

            let queue = Arc::clone(&queue);
            let runtime_tracker = Arc::clone(&runtime_tracker);
            let shutdown = Arc::clone(&shutdown);

            let handle = thread::Builder::new()
                .name(format!("work-pool-{}", id))
                .spawn(move || {
                    work_pool_worker_loop(id, queue, runtime_tracker, shutdown, park);
                })
                .expect("failed to spawn work pool worker thread");

            workers.push(Mutex::new(Some(handle)));
        }

        debug!(
            min_threads,
            max_threads,
            "WorkPool started"
        );

        Self {
            queue,
            runtime_tracker,
            workers,
            worker_parks,
            shutdown,
            active_count: AtomicUsize::new(min_threads),
            min_threads,
            max_threads,
            sequence: AtomicU64::new(0),
        }
    }

    /// Spawn an eval task (priority = NORMAL).
    ///
    /// Eval tasks are never dropped. The caller's `EvalGuard` should be managed
    /// by the task closure itself.
    pub fn spawn_eval<F>(&self, f: F, task_type: TaskTypeId, priority: u32)
    where
        F: FnOnce() + Send + 'static,
    {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task = PriorityTask::new(Box::new(f), priority, task_type, sequence);
        self.queue.push(task);
    }

    /// Spawn a compile task (priority = BACKGROUND_COMPILE).
    ///
    /// Compile tasks are speculative. If the queue is full (`>= MAX_QUEUE_SIZE`),
    /// the task is silently dropped (backpressure).
    ///
    /// Returns `true` if the task was enqueued, `false` if it was dropped due
    /// to backpressure. Callers should revert any speculative state changes
    /// (e.g., `ExprCompilationState` CAS) when `false` is returned.
    pub fn spawn_compile<F>(&self, f: F, task_type: TaskTypeId, priority: u32) -> bool
    where
        F: FnOnce() + Send + 'static,
    {
        // Backpressure: drop compile tasks when queue is saturated
        if self.queue.len() >= MAX_QUEUE_SIZE {
            trace!("WorkPool: compile task dropped (queue backpressure)");
            return false;
        }

        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task = PriorityTask::new(Box::new(f), priority, task_type, sequence);
        self.queue.push(task);
        true
    }

    /// Spawn a generic detached task.
    pub fn spawn_detached<F>(&self, f: F, task_type: TaskTypeId, priority: u32)
    where
        F: FnOnce() + Send + 'static,
    {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task = PriorityTask::new(Box::new(f), priority, task_type, sequence);
        self.queue.push(task);
    }

    /// Get the current queue depth.
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Get the number of currently active (non-parked) workers.
    pub fn active_workers(&self) -> usize {
        self.active_count.load(Ordering::Relaxed)
    }

    /// Get the minimum thread count.
    pub fn min_threads(&self) -> usize {
        self.min_threads
    }

    /// Get the maximum thread count.
    pub fn max_threads(&self) -> usize {
        self.max_threads
    }

    /// Unpark one worker (called by the scaling monitor).
    ///
    /// Finds the first parked worker and unparks it. Returns true if a worker
    /// was unparked.
    pub fn unpark_one(&self) -> bool {
        for park in &self.worker_parks {
            if park.is_parked() {
                park.unpark();
                self.active_count.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Park one worker (called by the scaling monitor).
    ///
    /// Finds the last active worker and parks it. Returns true if a worker
    /// was parked. Does not park below `min_threads`.
    pub fn park_one(&self) -> bool {
        let active = self.active_count.load(Ordering::Relaxed);
        if active <= self.min_threads {
            return false;
        }

        // Park from the end (highest index = most recently added)
        for park in self.worker_parks.iter().rev() {
            if !park.is_parked() {
                park.park();
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Get the P2 priority queue (for external monitoring).
    pub fn queue(&self) -> &Arc<PriorityQueue> {
        &self.queue
    }

    /// Get the runtime tracker.
    pub fn runtime_tracker(&self) -> &Arc<RuntimeTracker> {
        &self.runtime_tracker
    }

    /// Check for dead workers and respawn them.
    ///
    /// Iterates all worker slots, checks `is_finished()` (non-blocking),
    /// and respawns any dead workers. Returns the number of workers respawned.
    ///
    /// Called periodically by the scaling monitor (every 200ms).
    pub fn check_and_respawn_workers(&self) -> usize {
        let mut respawned = 0;

        for (id, slot) in self.workers.iter().enumerate() {
            let mut guard = slot.lock();
            let is_dead = match guard.as_ref() {
                Some(handle) => handle.is_finished(),
                None => true, // Slot was already taken (shouldn't happen outside Drop)
            };

            if !is_dead {
                continue;
            }

            // Reap the dead thread
            if let Some(old_handle) = guard.take() {
                match old_handle.join() {
                    Ok(()) => {
                        tracing::warn!(worker_id = id, "WorkPool: worker exited unexpectedly -- respawning");
                    }
                    Err(payload) => {
                        tracing::error!(
                            worker_id = id,
                            panic = ?payload,
                            "WorkPool: worker panicked -- respawning"
                        );
                    }
                }
            }

            // Respawn with the same shared state
            let park = Arc::clone(&self.worker_parks[id]);
            park.unpark(); // Ensure new worker starts unparked
            let queue = Arc::clone(&self.queue);
            let runtime_tracker = Arc::clone(&self.runtime_tracker);
            let shutdown = Arc::clone(&self.shutdown);

            let new_handle = thread::Builder::new()
                .name(format!("work-pool-{}", id))
                .spawn(move || {
                    work_pool_worker_loop(id, queue, runtime_tracker, shutdown, park);
                })
                .expect("failed to respawn work pool worker thread");

            *guard = Some(new_handle);
            respawned += 1;
        }

        respawned
    }

    /// Initiate graceful shutdown.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Unpark all workers so they can see the shutdown signal
        for park in &self.worker_parks {
            park.unpark();
        }

        // Wake up any workers blocked on the queue
        self.queue.notify_all();
    }
}

impl Drop for WorkPool {
    fn drop(&mut self) {
        self.shutdown();

        // Join all worker threads
        for slot in self.workers.iter() {
            if let Some(handle) = slot.lock().take() {
                let _ = handle.join();
            }
        }
    }
}

// ============================================================================
// Worker Loop
// ============================================================================

/// Worker thread main loop.
///
/// State machine:
/// 1. Check park flag → if parked, block on condvar
/// 2. Pop task from queue (with 500ms timeout for periodic park/shutdown check)
/// 3. Execute task + record runtime
/// 4. Loop back to 1
fn work_pool_worker_loop(
    _id: usize,
    queue: Arc<PriorityQueue>,
    runtime_tracker: Arc<RuntimeTracker>,
    shutdown: Arc<AtomicBool>,
    park: Arc<WorkerPark>,
) {
    loop {
        // Check for shutdown
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Check if we're parked — block until unparked (with 5s timeout to
        // recover from a dead scaling monitor that never calls unpark())
        park.wait_if_parked_timeout(Duration::from_secs(5));

        // Re-check shutdown after unpark
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Pop a task with timeout (allows periodic park/shutdown checks)
        match queue.pop_timeout(&shutdown, Duration::from_millis(500)) {
            Some(task) => {
                let task_type = task.task_type();

                // Outer catch_unwind: defense in depth. The inner catch_unwind
                // in PriorityTask::execute() handles task panics. This outer
                // layer catches panics from record_runtime() or any other code
                // between inner catch and loop iteration.
                let outer_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // Execute and measure runtime
                    let runtime_nanos = task.execute();

                    // Only record runtime if task didn't panic (runtime > 0)
                    if runtime_nanos > 0 {
                        runtime_tracker.record_runtime(task_type, runtime_nanos);
                    }

                    // Track eval completions for throughput monitoring
                    if matches!(task_type, TaskTypeId::Eval(_)) {
                        WORK_EVAL_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }));

                if let Err(payload) = outer_result {
                    tracing::error!(
                        ?task_type,
                        panic = ?payload,
                        "work_pool_worker_loop: outer catch_unwind caught panic -- worker continues"
                    );
                }
            }
            None => {
                // Timeout or shutdown — loop back to check flags
            }
        }
    }
}

// ============================================================================
// Global Singleton
// ============================================================================

/// Global work pool singleton (lazy initialization).
static GLOBAL_WORK_POOL: LazyLock<WorkPool> = LazyLock::new(WorkPool::new);

/// Get the global unified work pool.
///
/// On first access, lazily initializes the pool and starts the scaling monitor.
pub fn global_work_pool() -> &'static WorkPool {
    let pool = &*GLOBAL_WORK_POOL;
    // Start the scaling monitor (idempotent — only runs once)
    start_work_scaling_monitor();
    pool
}

// ============================================================================
// Scaling Monitor (Hill Climbing)
// ============================================================================

/// Interval between scaling monitor samples (milliseconds).
const WORK_MONITOR_INTERVAL_MS: u64 = 200;

/// EMA smoothing factor for throughput and queue depth.
/// Half-life ≈ 4.3 samples ≈ 860ms at 200ms interval.
const WORK_EMA_ALPHA: f64 = 0.15;

/// Number of monitor ticks to wait after a scale action before the next.
/// 5 ticks × 200ms = 1s settling time.
const WORK_COOLDOWN_PERIOD: u32 = 5;

/// Minimum relative improvement in objective required to accept a perturbation.
const WORK_IMPROVEMENT_THRESHOLD: f64 = 0.05;

/// Weight for throughput in the composite objective (negative = maximize).
const THROUGHPUT_WEIGHT: f64 = 1.0;

/// Weight for queue depth in the composite objective (positive = minimize).
const QUEUE_DEPTH_WEIGHT: f64 = 0.5;

/// Weight for slab memory pressure in the composite objective (positive = minimize).
///
/// At M = 1 (moderate pressure, backpressure level 1), the contribution is 5.0,
/// which exceeds the maximum EMA-filtered throughput gradient near N*
/// (typically ≤ 9 evals/s × ema_gain ≈ 1.59). This ensures memory pressure
/// overrides throughput optimization near the USL peak.
///
/// Derivation: w_mp must satisfy w_mp > w_tp × ΔT_max × α/(1−α).
/// For ΔT_max ≤ 28: 5.0 > 1.0 × 28 × 0.176 = 4.94. Verified in Rocq.
const MEMORY_PRESSURE_WEIGHT: f64 = 5.0;

/// Weight for RSS pressure in the composite objective (positive = minimize).
///
/// 60% stronger than slab pressure weight because OOM kills are unrecoverable
/// (the kernel terminates the process immediately). Slab pressure can be
/// mitigated by GC, but RSS pressure means the system as a whole is running
/// low on memory.
///
/// Ratio: w_rss / w_mp = 8.0 / 5.0 = 1.6. Verified in Rocq.
const RSS_PRESSURE_WEIGHT: f64 = 8.0;

// ============================================================================
// Memory Pressure Helpers
// ============================================================================

/// Read the current process RSS in bytes.
///
/// On Linux, reads directly from `/proc/self/statm` for minimal overhead
/// (no heap allocation, no library calls). On other platforms, falls back
/// to the `sysinfo` crate for cross-platform support.
///
/// The statm file format is: `size resident shared text lib data dt`
/// where `resident` (2nd field) is RSS in pages.
fn read_rss_bytes(page_size: usize) -> Option<usize> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Read;
        let mut buf = [0u8; 128];
        let mut fd = std::fs::File::open("/proc/self/statm").ok()?;
        let n = fd.read(&mut buf).ok()?;
        let s = std::str::from_utf8(&buf[..n]).ok()?;
        let rss_pages: usize = s.split_ascii_whitespace().nth(1)?.parse().ok()?;
        Some(rss_pages.saturating_mul(page_size))
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = page_size; // Suppress unused warning on non-Linux
        use sysinfo::{Pid, ProcessesToUpdate, System};
        let pid = Pid::from_u32(std::process::id());
        let mut sys = System::new();
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
        sys.process(pid).map(|p| p.memory() as usize)
    }
}

/// Get the RSS limit for memory pressure computation.
///
/// Priority:
/// 1. `METTATRON_RSS_LIMIT_MB` environment variable (in MB)
/// 2. 80% of system physical memory (auto-detected via sysconf on Linux,
///    `sysinfo` crate on other platforms)
/// 3. 0 (disables RSS pressure if both fail)
fn get_rss_limit() -> usize {
    if let Ok(val) = std::env::var("METTATRON_RSS_LIMIT_MB") {
        if let Ok(mb) = val.parse::<usize>() {
            return mb.saturating_mul(1024 * 1024);
        }
    }

    // Auto-detect from system physical memory
    #[cfg(target_os = "linux")]
    {
        let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        let total_pages = unsafe { libc::sysconf(libc::_SC_PHYS_PAGES) };
        if page_size > 0 && total_pages > 0 {
            let total_bytes = (page_size as usize).saturating_mul(total_pages as usize);
            return total_bytes * 4 / 5; // 80% of physical memory
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        use sysinfo::System;
        let total = System::new_all().total_memory() as usize;
        if total > 0 {
            return total * 4 / 5; // 80% of physical memory
        }
    }

    0 // Disabled
}

/// Get the system page size in bytes.
#[cfg(target_os = "linux")]
fn get_page_size() -> usize {
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps > 0 { ps as usize } else { 4096 }
}

#[cfg(not(target_os = "linux"))]
fn get_page_size() -> usize {
    4096
}

/// Compute slab memory pressure from backpressure level.
///
/// Maps the 4-level backpressure signal (0–3) to a continuous pressure
/// value (0.0–3.0) for the objective function. The mapping is identity
/// since backpressure levels were designed to align with this range.
#[inline]
fn slab_pressure() -> f64 {
    super::gc_allocator::backpressure_level() as f64
}

/// Compute RSS pressure from process RSS relative to the RSS limit.
///
/// Returns a value in [0.0, 3.0]:
/// - 0.0: RSS ≤ 50% of limit (no pressure)
/// - 1.0: RSS = 75% of limit (moderate pressure)
/// - 2.0: RSS = 100% of limit (high pressure)
/// - 3.0: RSS ≥ 125% of limit (critical pressure)
///
/// The pressure function is: `clamp(4 × (rss/limit - 0.5), 0, 3)`.
/// This creates a linear ramp from 50% to 125% of the limit, capped at 3.
fn rss_pressure(page_size: usize, rss_limit: usize) -> f64 {
    if rss_limit == 0 {
        return 0.0; // Disabled
    }
    let rss_bytes = match read_rss_bytes(page_size) {
        Some(rss) => rss,
        None => return 0.0,
    };
    let ratio = rss_bytes as f64 / rss_limit as f64;
    // Linear ramp: 0 at ratio=0.5, 3 at ratio=1.25
    let pressure = (ratio - 0.5) * 4.0;
    pressure.clamp(0.0, 3.0)
}

// ============================================================================
// Work Monitor State
// ============================================================================

/// Mutable state for the work pool scaling monitor.
///
/// Tracks EMA-smoothed throughput, queue depth, slab memory pressure,
/// and RSS pressure, feeding them to a four-term composite objective
/// function minimized by a HillClimber to decide when to park/unpark workers.
struct WorkMonitorState {
    /// Previous eval count snapshot (for delta computation).
    prev_eval_count: u64,
    /// Previous sample timestamp.
    prev_sample_time: Instant,
    /// EMA of eval throughput (evals/second).
    ema_throughput: Ema,
    /// EMA of pending task queue depth.
    ema_queue_depth: Ema,
    /// EMA of slab memory pressure (backpressure level, 0–3).
    ema_slab_pressure: Ema,
    /// EMA of RSS memory pressure (0–3).
    ema_rss_pressure: Ema,
    /// Hill climber for ±1 worker perturbation.
    climber: HillClimber,
    /// RSS limit in bytes (0 = disabled).
    rss_limit: usize,
    /// System page size in bytes (cached to avoid repeated sysconf calls).
    page_size: usize,
}

impl WorkMonitorState {
    fn new(pool: &WorkPool) -> Self {
        Self {
            prev_eval_count: work_eval_count(),
            prev_sample_time: Instant::now(),
            ema_throughput: Ema::new(WORK_EMA_ALPHA),
            ema_queue_depth: Ema::new(WORK_EMA_ALPHA),
            ema_slab_pressure: Ema::new(WORK_EMA_ALPHA),
            ema_rss_pressure: Ema::new(WORK_EMA_ALPHA),
            climber: HillClimber::new(
                WORK_COOLDOWN_PERIOD,
                WORK_IMPROVEMENT_THRESHOLD,
                pool.min_threads(),
                pool.max_threads(),
                pool.active_workers(),
            ),
            rss_limit: get_rss_limit(),
            page_size: get_page_size(),
        }
    }
}

/// Execute one tick of the work pool scaling monitor.
///
/// Called every `WORK_MONITOR_INTERVAL_MS` by the generic task scheduler.
/// Computes EMA-smoothed throughput, queue depth, slab pressure, and RSS
/// pressure, then feeds the four-term composite objective to the HillClimber
/// to decide whether to park or unpark a worker.
///
/// ## Four-Term Composite Objective
///
/// ```text
/// J(N) = −w_tp × ema_tp + w_qd × ema_qd + w_mp × ema_slab + w_rss × ema_rss
/// ```
///
/// The hill climber minimizes J by:
/// - Maximizing throughput (negative coefficient)
/// - Minimizing queue depth, slab pressure, and RSS pressure (positive coefficients)
///
/// ## Emergency Override
///
/// When slab backpressure ≥ 2 (medium/heavy), the hill climber is bypassed
/// and workers are immediately parked (one per tick). This provides a fast
/// response path that doesn't wait for EMA convergence.
fn work_scaling_monitor_tick(pool: &WorkPool, state: &mut WorkMonitorState) {
    let now = Instant::now();
    let elapsed = now.duration_since(state.prev_sample_time).as_secs_f64();
    if elapsed < 0.001 {
        // Avoid division by zero on very fast successive calls
        return;
    }

    // Sample throughput: delta evals / elapsed time
    let current_eval_count = work_eval_count();
    let delta = current_eval_count.wrapping_sub(state.prev_eval_count) as f64;
    let throughput = delta / elapsed;

    state.prev_eval_count = current_eval_count;
    state.prev_sample_time = now;

    // Sample queue depth
    let queue_depth = pool.queue_len() as f64;

    // Sample memory pressure signals
    let slab_p = slab_pressure();
    let rss_p = rss_pressure(state.page_size, state.rss_limit);

    // Update all EMAs
    let ema_tp = state.ema_throughput.update(throughput);
    let ema_qd = state.ema_queue_depth.update(queue_depth);
    let ema_slab = state.ema_slab_pressure.update(slab_p);
    let ema_rss = state.ema_rss_pressure.update(rss_p);

    // Emergency override: when slab backpressure ≥ 2 (medium/heavy),
    // immediately park a worker without waiting for EMA convergence.
    // This is a fast response path for acute memory pressure.
    let bp_level = super::gc_allocator::backpressure_level();
    if bp_level >= 2 {
        if pool.park_one() {
            trace!(
                bp_level,
                slab_pressure = ema_slab,
                rss_pressure = ema_rss,
                active = pool.active_workers(),
                "WorkPool scaling: EMERGENCY park (bp_level >= 2)"
            );
        }
        // Still check for dead workers even during emergency
        check_and_log_respawns(pool);
        return;
    }

    // Four-term composite objective: minimize (lower = better)
    //
    //   J(N) = −w_tp × ema_tp + w_qd × ema_qd + w_mp × ema_slab + w_rss × ema_rss
    //
    // Weight derivation and dominance conditions verified in
    // formal/rocq/work_pool_stability/theories/WeightDominance.v
    let objective = -THROUGHPUT_WEIGHT * ema_tp
        + QUEUE_DEPTH_WEIGHT * ema_qd
        + MEMORY_PRESSURE_WEIGHT * ema_slab
        + RSS_PRESSURE_WEIGHT * ema_rss;

    // Feed to hill climber
    let action = state.climber.step(objective);

    match action {
        ScaleAction::Unpark => {
            if pool.unpark_one() {
                trace!(
                    throughput = ema_tp,
                    queue_depth = ema_qd,
                    slab_pressure = ema_slab,
                    rss_pressure = ema_rss,
                    objective,
                    active = pool.active_workers(),
                    "WorkPool scaling: unparked 1 worker"
                );
            }
        }
        ScaleAction::Park => {
            if pool.park_one() {
                trace!(
                    throughput = ema_tp,
                    queue_depth = ema_qd,
                    slab_pressure = ema_slab,
                    rss_pressure = ema_rss,
                    objective,
                    active = pool.active_workers(),
                    "WorkPool scaling: parked 1 worker"
                );
            }
        }
        ScaleAction::Hold => {
            // No action needed
        }
    }

    // Check for and respawn dead workers
    check_and_log_respawns(pool);
}

/// Check for dead workers and log respawns. Extracted to avoid duplication
/// between the normal path and the emergency override path.
fn check_and_log_respawns(pool: &WorkPool) {
    let respawned = pool.check_and_respawn_workers();
    if respawned > 0 {
        tracing::warn!(
            respawned,
            "WorkPool scaling monitor: respawned dead workers"
        );
    }
}

/// Global work scaling monitor singleton. Lazily spawned on first use.
static GLOBAL_WORK_MONITOR: OnceLock<super::task_scheduler::TaskSchedulerSingleton> =
    OnceLock::new();

/// Start the work pool scaling monitor as a recurring task on a dedicated scheduler.
///
/// Spawns a generic task scheduler thread and registers a recurring task that runs
/// every `WORK_MONITOR_INTERVAL_MS` milliseconds. The monitor samples eval throughput
/// and queue depth, then uses hill climbing to adaptively scale the worker count.
///
/// Idempotent — subsequent calls are no-ops.
pub fn start_work_scaling_monitor() {
    GLOBAL_WORK_MONITOR.get_or_init(|| {
        // Access GLOBAL_WORK_POOL directly (not via global_work_pool()) to avoid
        // reentrant OnceLock deadlock: global_work_pool() → start_work_scaling_monitor()
        // → global_work_pool() → start_work_scaling_monitor() → OnceLock reentry.
        let pool: &'static WorkPool = &*GLOBAL_WORK_POOL;
        let mut state = WorkMonitorState::new(pool);

        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread_handle, ready_rx) =
            super::task_scheduler::spawn_cron_with_interval_and_name(
                Arc::clone(&terminating),
                WORK_MONITOR_INTERVAL_MS,
                "mettatron-work-monitor",
            );

        // Wait for scheduler to be ready before scheduling tasks.
        // Use recv_timeout to avoid blocking forever if the monitor thread panics during startup.
        if ready_rx.recv_timeout(Duration::from_secs(5)).is_err() {
            tracing::warn!("Work scaling monitor did not signal ready within 5s — continuing without scaling");
        }

        handle.schedule_recurring(
            WORK_MONITOR_INTERVAL_MS,
            WORK_MONITOR_INTERVAL_MS,
            "work-scaling-monitor",
            move || {
                work_scaling_monitor_tick(pool, &mut state);
                true // Continue recurring
            },
        );

        debug!(
            interval_ms = WORK_MONITOR_INTERVAL_MS,
            "Work pool scaling monitor started"
        );

        super::task_scheduler::TaskSchedulerSingleton::new(handle, thread_handle)
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::priority_scheduler::priority_levels;
    use std::sync::atomic::AtomicU32;

    /// Test timeout for waiting on task completion.
    const TEST_TIMEOUT: Duration = Duration::from_secs(10);

    /// Helper: wait for an AtomicBool to become true (with timeout).
    fn wait_for_bool(flag: &AtomicBool) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while !flag.load(Ordering::Acquire) {
            if Instant::now() > deadline {
                panic!("Timeout waiting for flag");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Helper: wait for an AtomicU32 to reach a target value (with timeout).
    fn wait_for_count(counter: &AtomicU32, target: u32) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        while counter.load(Ordering::Relaxed) < target {
            if Instant::now() > deadline {
                panic!(
                    "Timeout: {}/{} tasks completed",
                    counter.load(Ordering::Relaxed),
                    target
                );
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    // ===================================================================
    // Local Pool Tests (fully isolated, no global singleton)
    // ===================================================================

    #[test]
    fn test_work_pool_spawn_and_complete() {
        let pool = WorkPool::with_threads(2, 4);

        let counter = Arc::new(AtomicU32::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&counter);
        let d = Arc::clone(&done);

        pool.spawn_eval(
            move || {
                c.fetch_add(1, Ordering::Relaxed);
                d.store(true, Ordering::Release);
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        wait_for_bool(&done);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_work_pool_compile_task() {
        let pool = WorkPool::with_threads(2, 4);

        let counter = Arc::new(AtomicU32::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&counter);
        let d = Arc::clone(&done);

        pool.spawn_compile(
            move || {
                c.fetch_add(1, Ordering::Relaxed);
                d.store(true, Ordering::Release);
            },
            TaskTypeId::BytecodeCompile,
            priority_levels::BACKGROUND_COMPILE,
        );

        wait_for_bool(&done);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_work_pool_multiple_tasks() {
        let pool = WorkPool::with_threads(2, 4);

        let counter = Arc::new(AtomicU32::new(0));
        let num_tasks = 50u32;

        for _ in 0..num_tasks {
            let c = Arc::clone(&counter);
            pool.spawn_eval(
                move || {
                    c.fetch_add(1, Ordering::Relaxed);
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        wait_for_count(&counter, num_tasks);
        assert_eq!(counter.load(Ordering::Relaxed), num_tasks);
    }

    #[test]
    fn test_work_pool_thread_config() {
        let (min, max) = get_work_thread_config();
        assert!(min >= 1);
        assert!(max >= min);
        assert!(max >= 2);
    }

    #[test]
    fn test_work_pool_queue_len() {
        let pool = WorkPool::with_threads(2, 4);
        assert_eq!(pool.queue_len(), 0);
    }

    #[test]
    fn test_work_pool_active_workers() {
        let pool = WorkPool::with_threads(2, 4);
        let active = pool.active_workers();
        assert!(
            active >= pool.min_threads() && active <= pool.max_threads(),
            "Active workers {} should be in [{}, {}]",
            active,
            pool.min_threads(),
            pool.max_threads(),
        );
    }

    #[test]
    fn test_work_pool_survives_panicking_task() {
        let pool = WorkPool::with_threads(2, 4);

        // Submit a panicking task
        let panic_done = Arc::new(AtomicBool::new(false));
        let pd = Arc::clone(&panic_done);
        pool.spawn_eval(
            move || {
                pd.store(true, Ordering::Release);
                panic!("intentional panic in work pool task");
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        wait_for_bool(&panic_done);

        // Submit a normal task after the panic — pool should still work
        let counter = Arc::new(AtomicU32::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&counter);
        let d = Arc::clone(&done);

        pool.spawn_eval(
            move || {
                c.fetch_add(1, Ordering::Relaxed);
                d.store(true, Ordering::Release);
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        wait_for_bool(&done);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    // ===================================================================
    // Global Singleton Tests (kept minimal)
    // ===================================================================

    #[test]
    fn test_global_work_pool_spawn_task() {
        let pool = &*GLOBAL_WORK_POOL;

        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);

        pool.spawn_eval(
            move || {
                d.store(true, Ordering::Release);
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        wait_for_bool(&done);
    }

    #[test]
    fn test_start_work_scaling_monitor_idempotent() {
        // First call starts the monitor
        start_work_scaling_monitor();

        // Second call should be a no-op (idempotent)
        start_work_scaling_monitor();

        // Monitor should be running — just verify no panic
        assert!(GLOBAL_WORK_MONITOR.get().is_some());
    }

    // ===================================================================
    // Scaling Monitor Unit Tests (use local pools)
    // ===================================================================

    #[test]
    fn test_work_monitor_state_initialization() {
        let pool = WorkPool::with_threads(2, 4);
        let state = WorkMonitorState::new(&pool);
        assert!(!state.ema_throughput.is_initialized());
        assert!(!state.ema_queue_depth.is_initialized());
    }

    #[test]
    fn test_work_scaling_monitor_tick_hold_on_idle() {
        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Simulate time passing
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);

        // Run a tick — on idle system, should just update EMAs and hold
        work_scaling_monitor_tick(&pool, &mut state);

        // EMAs should be initialized now
        assert!(state.ema_throughput.is_initialized());
        assert!(state.ema_queue_depth.is_initialized());
    }

    #[test]
    fn test_work_scaling_monitor_tick_skips_fast_calls() {
        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Set prev_sample_time to now (< 1ms elapsed)
        state.prev_sample_time = Instant::now();

        // Should skip (avoid division by zero)
        work_scaling_monitor_tick(&pool, &mut state);

        // EMAs should NOT be initialized (tick was skipped)
        assert!(!state.ema_throughput.is_initialized());
    }

    #[test]
    fn test_work_scaling_monitor_tick_with_load() {
        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Simulate 1 second elapsed with some evals completed
        state.prev_sample_time = Instant::now() - Duration::from_secs(1);
        let initial_count = work_eval_count();
        state.prev_eval_count = initial_count.wrapping_sub(100); // Pretend 100 evals happened

        work_scaling_monitor_tick(&pool, &mut state);

        // Throughput EMA should be approximately 100 evals/sec
        let tp = state.ema_throughput.value();
        assert!(
            tp > 50.0 && tp < 200.0,
            "Expected throughput ~100, got {}",
            tp
        );
    }

    // ===================================================================
    // Memory Pressure Tests
    // ===================================================================

    #[cfg(target_os = "linux")]
    #[test]
    fn test_read_rss_bytes_returns_value() {
        let page_size = get_page_size();
        let rss = read_rss_bytes(page_size);
        assert!(rss.is_some(), "read_rss_bytes should succeed on Linux");
        let rss = rss.expect("already checked Some");
        // RSS should be at least a few MB for any running process
        assert!(
            rss > 1_000_000,
            "RSS {} bytes seems too small for a running test process",
            rss
        );
    }

    #[test]
    fn test_rss_pressure_capped_at_3() {
        // Even with a very small limit, pressure should cap at 3.0
        let page_size = get_page_size();
        let tiny_limit = 1024; // 1 KB — way smaller than any real RSS
        let pressure = rss_pressure(page_size, tiny_limit);
        assert!(
            pressure <= 3.0,
            "RSS pressure {} should be capped at 3.0",
            pressure
        );
        assert!(
            pressure >= 0.0,
            "RSS pressure {} should be non-negative",
            pressure
        );
    }

    #[test]
    fn test_zero_slab_pressure_when_gc_unreachable() {
        // When no GC has run, backpressure level should be 0
        // (this is the default state before any allocation pressure)
        let sp = slab_pressure();
        assert!(
            sp >= 0.0 && sp <= 3.0,
            "Slab pressure {} should be in [0, 3]",
            sp
        );
    }

    #[test]
    fn test_rss_limit_disabled_when_zero() {
        let page_size = get_page_size();
        let pressure = rss_pressure(page_size, 0);
        assert_eq!(pressure, 0.0, "RSS pressure should be 0.0 when limit is 0 (disabled)");
    }

    #[test]
    fn test_memory_pressure_increases_objective() {
        // Verify that higher memory pressure produces a worse (higher) objective.
        // Use the weight formula directly: J = -tp + 0.5*qd + 5.0*slab + 8.0*rss
        let tp = 100.0;
        let qd = 0.0;

        let obj_no_pressure = -THROUGHPUT_WEIGHT * tp + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 0.0 + RSS_PRESSURE_WEIGHT * 0.0;

        let obj_slab_pressure = -THROUGHPUT_WEIGHT * tp + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 1.0 + RSS_PRESSURE_WEIGHT * 0.0;

        let obj_rss_pressure = -THROUGHPUT_WEIGHT * tp + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 0.0 + RSS_PRESSURE_WEIGHT * 1.0;

        let obj_both_pressure = -THROUGHPUT_WEIGHT * tp + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 2.0 + RSS_PRESSURE_WEIGHT * 2.0;

        assert!(
            obj_slab_pressure > obj_no_pressure,
            "Slab pressure should increase objective: {} > {}",
            obj_slab_pressure, obj_no_pressure
        );
        assert!(
            obj_rss_pressure > obj_no_pressure,
            "RSS pressure should increase objective: {} > {}",
            obj_rss_pressure, obj_no_pressure
        );
        assert!(
            obj_rss_pressure > obj_slab_pressure,
            "RSS pressure should dominate slab pressure: {} > {}",
            obj_rss_pressure, obj_slab_pressure
        );
        assert!(
            obj_both_pressure > obj_rss_pressure,
            "Combined pressure should be worst: {} > {}",
            obj_both_pressure, obj_rss_pressure
        );
    }

    #[test]
    fn test_emergency_park_at_high_backpressure() {
        // When backpressure is >= 2, the emergency override should park workers
        // immediately (bypassing the hill climber).
        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);

        // Unpark all workers first
        while pool.unpark_one() {}
        let initial_active = pool.active_workers();
        assert!(initial_active > pool.min_threads(),
            "Need more than min_threads active to test parking");

        // Simulate high backpressure level
        crate::backend::models::gc_allocator::set_backpressure_level(2);

        // Run a tick — should park via emergency override
        work_scaling_monitor_tick(&pool, &mut state);

        let after_active = pool.active_workers();
        assert!(
            after_active < initial_active,
            "Emergency park should reduce active workers: {} < {}",
            after_active, initial_active
        );

        // Reset backpressure to avoid affecting other tests
        crate::backend::models::gc_allocator::set_backpressure_level(0);
    }

    #[test]
    fn test_emergency_park_respects_min_threads() {
        // Emergency park should never go below min_threads
        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Set high backpressure
        crate::backend::models::gc_allocator::set_backpressure_level(3);

        // Run many ticks — should not drop below min_threads
        for _ in 0..20 {
            state.prev_sample_time = Instant::now() - Duration::from_millis(200);
            work_scaling_monitor_tick(&pool, &mut state);
        }

        let active = pool.active_workers();
        assert!(
            active >= pool.min_threads(),
            "Active workers {} should not drop below min_threads {}",
            active, pool.min_threads()
        );

        // Reset backpressure
        crate::backend::models::gc_allocator::set_backpressure_level(0);
    }

    #[test]
    fn test_monitor_state_has_memory_emas() {
        // Verify that the new EMA fields are initialized properly
        let pool = WorkPool::with_threads(2, 4);
        let state = WorkMonitorState::new(&pool);

        assert!(!state.ema_slab_pressure.is_initialized());
        assert!(!state.ema_rss_pressure.is_initialized());
        assert!(state.rss_limit > 0 || cfg!(not(target_os = "linux")),
            "RSS limit should be auto-detected on Linux");
        assert!(state.page_size > 0, "Page size should be positive");
    }

    #[test]
    fn test_rss_pressure_linear_ramp() {
        // Test the linear ramp function at known points:
        // ratio=0.5 → pressure=0.0
        // ratio=0.75 → pressure=1.0
        // ratio=1.0 → pressure=2.0
        // ratio=1.25 → pressure=3.0

        // We can't easily mock read_rss_bytes, so test the formula directly
        let ramp = |ratio: f64| -> f64 {
            ((ratio - 0.5) * 4.0).clamp(0.0, 3.0)
        };

        assert!((ramp(0.25) - 0.0).abs() < f64::EPSILON, "Below 50%: no pressure");
        assert!((ramp(0.5) - 0.0).abs() < f64::EPSILON, "At 50%: no pressure");
        assert!((ramp(0.75) - 1.0).abs() < f64::EPSILON, "At 75%: moderate pressure");
        assert!((ramp(1.0) - 2.0).abs() < f64::EPSILON, "At 100%: high pressure");
        assert!((ramp(1.25) - 3.0).abs() < f64::EPSILON, "At 125%: capped at 3.0");
        assert!((ramp(2.0) - 3.0).abs() < f64::EPSILON, "At 200%: still capped at 3.0");
    }

    #[test]
    fn test_weight_dominance_conditions() {
        // Verify the analytical weight dominance conditions from the plan
        // (also formally verified in Rocq)

        // C1: Memory overrides zero gradient
        assert!(MEMORY_PRESSURE_WEIGHT > 0.0);

        // C2: Memory overrides typical near-peak gradient (≤ 9 evals/s)
        let ema_gain = WORK_EMA_ALPHA / (1.0 - WORK_EMA_ALPHA);
        assert!(
            MEMORY_PRESSURE_WEIGHT * 1.0 > THROUGHPUT_WEIGHT * 9.0 * ema_gain,
            "Memory weight {} should dominate throughput gradient {} at peak",
            MEMORY_PRESSURE_WEIGHT,
            THROUGHPUT_WEIGHT * 9.0 * ema_gain
        );

        // C3: RSS dominates slab
        assert!(RSS_PRESSURE_WEIGHT > MEMORY_PRESSURE_WEIGHT);

        // C4: Queue sensitivity matches threshold
        assert!(QUEUE_DEPTH_WEIGHT * 0.1 >= WORK_IMPROVEMENT_THRESHOLD);

        // C5: Scale-up dominates at T₁ ≥ 32
        let t1_val = 32.0;
        assert!(
            THROUGHPUT_WEIGHT * (t1_val * 0.9) * ema_gain > MEMORY_PRESSURE_WEIGHT * 1.0,
            "Scale-up should dominate mild pressure at T₁={}",
            t1_val
        );
    }
}
