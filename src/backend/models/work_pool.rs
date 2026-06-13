//! Split Work Pools (Eval Pool + Compile Pool)
//!
//! Two separate `WorkPool` instances provide CPU isolation between eval and
//! compile tasks:
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │       Eval Pool (P2, adaptive)           │
//! │  ┌────────────────────────────────────┐  │
//! │  │ Eval tasks    (pri=NORMAL=5)       │  │
//! │  └────────────────────────────────────┘  │
//! │  1–N workers, EMA hill climber           │
//! │  Objective: max weighted throughput      │
//! └──────────────────────────────────────────┘
//!
//! ┌──────────────────────────────────────────┐
//! │    Compile Pool (fixed, 4 workers)       │
//! │  ┌────────────────────────────────────┐  │
//! │  │ Compile tasks (pri=BACKGROUND=10)  │  │
//! │  └────────────────────────────────────┘  │
//! │  Fixed 4 workers, no scaling monitor     │
//! │  Backpressure: drop at queue cap (256)   │
//! └──────────────────────────────────────────┘
//! ```
//!
//! ## Thread Inventory
//!
//! - **Idle**: main + 4 eval + 4 compile + 1 GC + scheduler = 11 threads
//! - **Peak** (36-core): main + 72 eval + 4 compile + 4 GC + scheduler = 82 threads
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
/// - `METTATRON_MAX_WORK_THREADS`: Maximum workers (default: num_cpus*2, at least min, at least 2)
fn get_work_thread_config() -> (usize, usize) {
    let min = std::env::var("METTATRON_MIN_WORK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    let max = std::env::var("METTATRON_MAX_WORK_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(num_cpus::get() * 2)
        .max(min)
        .max(2);

    (min, max)
}

/// Maximum queue depth before compile tasks are dropped (backpressure).
const MAX_QUEUE_SIZE: usize = 256;

/// CPU utilization ratio below which a worker is considered "blocked".
///
/// Workers with `cpu_delta / wall_delta < 0.5` are spending more than half
/// their time sleeping/waiting (locks, condvars, I/O, GC) rather than computing.
const BLOCKED_RATIO_THRESHOLD: f64 = 0.5;

/// Minimum wall-time delta (in nanoseconds) before computing a CPU ratio.
///
/// Avoids division-by-zero and noisy ratios from very short intervals.
/// 1ms is well above VDSO clock resolution (~25ns) and below the monitor
/// interval (200ms).
const MIN_WALL_DELTA_NS: u64 = 1_000_000; // 1ms

// ============================================================================
// Global Eval Counter
// ============================================================================

/// Global counter of completed eval tasks (for throughput tracking).
#[cfg(any(feature = "trace", feature = "track-stats"))]
pub static WORK_EVAL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Read the global eval completion count.
#[cfg(any(feature = "trace", feature = "track-stats"))]
#[inline]
pub fn work_eval_count() -> u64 {
    WORK_EVAL_COUNT.load(Ordering::Relaxed)
}

// ============================================================================
// Trace Infrastructure (eval-trace feature only)
// ============================================================================

/// Global trace collector for work pool events.
///
/// The work pool doesn't have access to an `EvalContext`, so we stash a `Weak`
/// reference here. Set once per session from `eval_with_trace()`.
#[cfg(feature = "trace")]
static WORK_POOL_TRACE_COLLECTOR: OnceLock<std::sync::Weak<crate::backend::trace::TraceCollector>> =
    OnceLock::new();

/// Register the trace collector so work pool events can be emitted.
///
/// Called from `eval_with_trace()` on the first traced evaluation. Uses
/// `OnceLock` so it is safe to call multiple times — only the first call wins.
#[cfg(feature = "trace")]
pub fn set_work_pool_trace_collector(
    collector: &std::sync::Arc<crate::backend::trace::TraceCollector>,
) {
    let _ = WORK_POOL_TRACE_COLLECTOR.set(std::sync::Arc::downgrade(collector));
}

/// Execute a closure with the work pool trace collector, if available.
///
/// No-op if no collector was registered or it has been dropped.
#[cfg(feature = "trace")]
#[inline]
fn with_work_pool_trace(f: impl FnOnce(&crate::backend::trace::TraceCollector)) {
    if let Some(weak) = WORK_POOL_TRACE_COLLECTOR.get() {
        if let Some(arc) = weak.upgrade() {
            f(&arc);
        }
    }
}

/// Get an `Arc<TraceCollector>` from the global work pool trace collector.
///
/// Returns `Some(Arc<TraceCollector>)` if a trace collector was registered
/// and the session is still active (Weak upgrades successfully). Returns
/// `None` if tracing is not active or the collector has been dropped.
///
/// Used by `ParallelBranchContext` to hold a strong reference to the trace
/// collector for the duration of parallel branch evaluation.
#[cfg(feature = "trace")]
#[inline]
pub fn get_work_pool_trace_collector(
) -> Option<std::sync::Arc<crate::backend::trace::TraceCollector>> {
    WORK_POOL_TRACE_COLLECTOR
        .get()
        .and_then(|weak| weak.upgrade())
}

/// Map a `TaskTypeId` to a human-readable task kind string for trace events.
#[cfg(feature = "trace")]
fn task_type_kind_str(task_type: &TaskTypeId) -> &'static str {
    match task_type {
        TaskTypeId::Eval(_) => "eval",
        TaskTypeId::BytecodeCompile | TaskTypeId::JitCompile => "compile",
        TaskTypeId::Generic => "detached",
    }
}

/// Return `(active_workers, max_workers)` from the global work pool.
///
/// Used by the worker loop to include pool-level stats in trace events
/// without threading additional parameters through `spawn_all_workers`.
#[cfg(feature = "trace")]
fn global_eval_pool_stats() -> (u32, u32) {
    let pool = &*GLOBAL_EVAL_POOL;
    (pool.active_workers() as u32, pool.max_threads() as u32)
}

// ============================================================================
// Per-Worker CPU State (Blocked-Worker Detection)
// ============================================================================

/// Per-worker CPU utilization state, published by the worker and read by the monitor.
///
/// Each field is on its own 64-byte cache line to avoid false sharing between
/// the worker (writer) and the scaling monitor (reader).
#[repr(C, align(64))]
pub struct WorkerCpuState {
    /// Cumulative CPU time in nanoseconds (worker writes, monitor reads).
    cpu_nanos: AtomicU64,
    _pad0: [u8; 56],

    /// Wall-clock nanos at the time of the last CPU time update (worker writes, monitor reads).
    wall_nanos: AtomicU64,
    _pad1: [u8; 56],

    /// Task completion counter / heartbeat (worker writes, monitor reads).
    task_count: AtomicU64,
    _pad2: [u8; 56],
}

impl WorkerCpuState {
    /// Create a new zeroed CPU state.
    fn new() -> Self {
        Self {
            cpu_nanos: AtomicU64::new(0),
            _pad0: [0; 56],
            wall_nanos: AtomicU64::new(0),
            _pad1: [0; 56],
            task_count: AtomicU64::new(0),
            _pad2: [0; 56],
        }
    }

    /// Publish the current CPU and wall-clock time, and increment the task counter.
    ///
    /// Called by the worker after each `task.execute()`.
    #[inline]
    fn publish(&self) {
        let cpu_ns = get_thread_cpu_nanos();
        let wall_ns = wall_nanos_now();
        self.cpu_nanos.store(cpu_ns, Ordering::Relaxed);
        self.wall_nanos.store(wall_ns, Ordering::Relaxed);
        self.task_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Publish the initial CPU and wall-clock time at worker startup (no task count increment).
    #[inline]
    fn publish_initial(&self) {
        let cpu_ns = get_thread_cpu_nanos();
        let wall_ns = wall_nanos_now();
        self.cpu_nanos.store(cpu_ns, Ordering::Relaxed);
        self.wall_nanos.store(wall_ns, Ordering::Relaxed);
    }
}

/// Get the current thread's cumulative CPU time in nanoseconds.
///
/// Uses `clock_gettime(CLOCK_THREAD_CPUTIME_ID)` which is VDSO-accelerated
/// on Linux 6.x (~25ns per call, no syscall overhead).
#[cfg(target_os = "linux")]
#[inline]
fn get_thread_cpu_nanos() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Fallback: returns 0 on non-Linux (CPU ratio detection disabled, heartbeat-only).
#[cfg(not(target_os = "linux"))]
#[inline]
fn get_thread_cpu_nanos() -> u64 {
    0
}

/// Get the current monotonic wall-clock time in nanoseconds.
///
/// Uses `CLOCK_MONOTONIC` via VDSO for consistency with CPU time measurement.
#[cfg(target_os = "linux")]
#[inline]
fn wall_nanos_now() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts);
    }
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// Fallback: uses `Instant` on non-Linux.
#[cfg(not(target_os = "linux"))]
#[inline]
fn wall_nanos_now() -> u64 {
    // Use a thread-local epoch to convert Instant to nanos
    use std::cell::Cell;
    thread_local! {
        static EPOCH: Cell<Option<Instant>> = const { Cell::new(None) };
    }
    EPOCH.with(|e| {
        let epoch = match e.get() {
            Some(ep) => ep,
            None => {
                let ep = Instant::now();
                e.set(Some(ep));
                ep
            }
        };
        epoch.elapsed().as_nanos() as u64
    })
}

// ============================================================================
// Overflow Worker
// ============================================================================

/// An overflow worker thread spawned dynamically to compensate for blocked
/// core-pool workers. Each overflow worker has its own shutdown signal and
/// CPU state for monitoring.
struct OverflowWorker {
    /// Thread handle for joining on shutdown.
    handle: JoinHandle<()>,
    /// Per-overflow-worker shutdown signal (set by `drain_overflow`).
    self_shutdown: Arc<AtomicBool>,
    /// CPU state for this overflow worker (shared with monitor).
    cpu_state: Arc<WorkerCpuState>,
}

// ============================================================================
// WorkPool
// ============================================================================

/// Unified adaptive thread pool for eval + compile work.
///
/// The pool pre-allocates worker slots and parking primitives at construction.
/// OS threads are spawned asynchronously (global pool) or synchronously (test
/// pools via `with_threads`). Workers [0..initial_active) start active;
/// [initial_active..max_threads) start parked on their condvar and do not
/// touch the task queue until the scaling monitor unparks them.
///
/// Worker state machine:
/// - **Idle**: Waiting for tasks from P2 queue (blocks on `pop_timeout`)
/// - **Executing**: Running a task closure
/// - **Parked**: Dormant (blocked on `WorkerPark` condvar), activated by scaling monitor
/// - **ShuttingDown**: Exiting the worker loop
pub struct WorkPool {
    /// P2 priority queue shared by all workers.
    queue: Arc<PriorityQueue>,

    /// Runtime tracker for P2 estimation.
    runtime_tracker: Arc<RuntimeTracker>,

    /// Worker thread handles. `None` before async init spawns the thread,
    /// or after a dead worker is reaped and before respawn.
    workers: Vec<Mutex<Option<JoinHandle<()>>>>,

    /// Per-worker parking primitives.
    worker_parks: Vec<Arc<WorkerPark>>,

    /// Shutdown signal (shared with all workers).
    shutdown: Arc<AtomicBool>,

    /// Number of currently active (non-parked) workers.
    active_count: AtomicUsize,

    /// Target number of initially active (non-parked) workers.
    initial_active: usize,

    /// Minimum number of workers (never park below this).
    min_threads: usize,

    /// Maximum number of workers.
    max_threads: usize,

    /// Monotonic sequence counter for stable ordering.
    sequence: AtomicU64,

    /// Per-worker CPU utilization state (parallel to `worker_parks`).
    /// Workers publish CPU time after each task; the monitor reads for
    /// blocked-worker detection.
    worker_cpu_states: Vec<Arc<WorkerCpuState>>,

    /// Overflow worker thread handles (beyond max_threads).
    /// These are spawned dynamically when blocked workers reduce effective
    /// parallelism below the hill climber's target.
    overflow_workers: Mutex<Vec<OverflowWorker>>,

    /// Current overflow thread count (atomically updated for lock-free reads).
    overflow_count: AtomicUsize,
}

impl WorkPool {
    /// Create a work pool using environment variable configuration.
    ///
    /// Allocates structures only — no OS threads are spawned. Use
    /// `start_async_init()` (global singleton) or `spawn_all_workers()`
    /// (tests) to actually start workers.
    pub fn new() -> Self {
        let (min_threads, max_threads) = get_work_thread_config();
        Self::allocate(min_threads, max_threads)
    }

    /// Pre-allocate structures without spawning any OS threads.
    ///
    /// Creates the priority queue, runtime tracker, worker park primitives,
    /// and empty worker slots. Returns immediately.  Delegates to
    /// `allocate_with_initial` with `initial = max(min, 8)`.
    fn allocate(min_threads: usize, max_threads: usize) -> Self {
        let min = min_threads.max(1);
        let max = max_threads.max(min).max(2);
        // Start at half the CPU count (or at least 8) for faster cold-start.
        // On a 36-core machine: initial = max(1, 18).max(8) = 18.
        // Combined with geometric stepping, reaching full CPU count takes
        // only 2 unpark actions (18→36) instead of 28 (8→36) with ±1 stepping.
        let num_cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(8);
        let initial = min.max(num_cpus / 2).max(8).min(max);
        Self::allocate_with_initial(min, max, initial)
    }

    /// Pre-allocate structures without spawning any OS threads.
    ///
    /// Creates the priority queue, runtime tracker, worker park primitives,
    /// and empty worker slots. Returns immediately.
    ///
    /// `initial_active` is clamped to `[min_threads, max_threads]` and controls
    /// how many workers start unparked.
    fn allocate_with_initial(
        min_threads: usize,
        max_threads: usize,
        initial_active: usize,
    ) -> Self {
        let min_threads = min_threads.max(1);
        let max_threads = max_threads.max(min_threads).max(2);
        let initial_active = initial_active.clamp(min_threads, max_threads);
        let config = SchedulerConfig::default();
        let runtime_tracker = Arc::new(RuntimeTracker::new());
        let queue = Arc::new(PriorityQueue::new(Arc::clone(&runtime_tracker), config));
        let shutdown = Arc::new(AtomicBool::new(false));

        // Pre-allocate WorkerPark structs (cheap: Mutex<bool> + Condvar)
        let worker_parks: Vec<Arc<WorkerPark>> = (0..max_threads)
            .map(|id| Arc::new(WorkerPark::new(id >= initial_active)))
            .collect();

        // Pre-allocate per-worker CPU state (for blocked-worker detection)
        let worker_cpu_states: Vec<Arc<WorkerCpuState>> = (0..max_threads)
            .map(|_| Arc::new(WorkerCpuState::new()))
            .collect();

        // Pre-allocate worker slots as None (no OS threads yet)
        let workers: Vec<Mutex<Option<JoinHandle<()>>>> =
            (0..max_threads).map(|_| Mutex::new(None)).collect();

        Self {
            queue,
            runtime_tracker,
            workers,
            worker_parks,
            shutdown,
            active_count: AtomicUsize::new(initial_active),
            initial_active,
            min_threads,
            max_threads,
            sequence: AtomicU64::new(0),
            worker_cpu_states,
            overflow_workers: Mutex::new(Vec::new()),
            overflow_count: AtomicUsize::new(0),
        }
    }

    /// Spawn all max_threads worker OS threads into pre-allocated slots.
    ///
    /// Workers [0..initial_active) start unparked and immediately drain the
    /// task queue. Workers [initial_active..max_threads) start parked — their
    /// WorkerPark has `parked = true`, so they block on `wait_if_parked_timeout()`
    /// at line 413 of the worker loop. This bounds active workers to
    /// `active_count` (= initial_active).
    ///
    /// Uses `std::thread::scope` for parallel spawning when max_threads > 4.
    fn spawn_all_workers(&self) {
        if self.max_threads <= 4 {
            for id in 0..self.max_threads {
                self.spawn_worker(id);
            }
        } else {
            thread::scope(|s| {
                for id in 0..self.max_threads {
                    s.spawn(move || self.spawn_worker(id));
                }
            });
        }
        debug!(
            initial_active = self.initial_active,
            max = self.max_threads,
            "WorkPool: all workers spawned"
        );
    }

    /// Spawn a single worker thread into slot `id`.
    fn spawn_worker(&self, id: usize) {
        let park = Arc::clone(&self.worker_parks[id]);
        let queue = Arc::clone(&self.queue);
        let runtime_tracker = Arc::clone(&self.runtime_tracker);
        let shutdown = Arc::clone(&self.shutdown);
        let cpu_state = Arc::clone(&self.worker_cpu_states[id]);

        let handle = thread::Builder::new()
            .name(format!("work-pool-{}", id))
            // **Stack-safety defense-in-depth (2026-05-15)**: explicit 8 MB stack.
            // Matches Linux glibc default but is now guaranteed across platforms
            // (macOS default is ~512 KB on some thread types — would not be
            // adequate for the trampoline's bounded-but-non-trivial stack frame
            // budget). After Phases 1-5, the trampoline's recursion is eliminated
            // and this excess capacity is purely forward-compatibility headroom.
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                work_pool_worker_loop(id, queue, runtime_tracker, shutdown, park, cpu_state)
            })
            .expect("failed to spawn work pool worker thread");
        *self.workers[id].lock() = Some(handle);
    }

    /// Create and start a work pool with explicit thread counts.
    ///
    /// Spawns all workers synchronously before returning. This constructor
    /// is useful for tests that need isolated pools without sharing the
    /// global singleton.
    pub fn with_threads(min_threads: usize, max_threads: usize) -> Self {
        let pool = Self::allocate(min_threads, max_threads);
        pool.spawn_all_workers();
        pool
    }

    /// Create and start a work pool with explicit thread counts and initial
    /// active worker count.
    ///
    /// Like `with_threads`, but allows specifying exactly how many workers
    /// start unparked. `initial_active` is clamped to `[min, max]`.
    pub fn with_threads_initial(
        min_threads: usize,
        max_threads: usize,
        initial_active: usize,
    ) -> Self {
        let pool = Self::allocate_with_initial(min_threads, max_threads, initial_active);
        pool.spawn_all_workers();
        pool
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

        #[cfg(feature = "trace")]
        {
            let queue_depth = self.queue.len() as u32;
            let active_workers = self.active_workers() as u32;
            let max_workers = self.max_threads() as u32;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolTaskEnqueued {
                        task_kind: "eval".to_string(),
                        priority,
                        queue_depth,
                        active_workers,
                        max_workers,
                    },
                );
            });
        }
    }

    /// Spawn a WFST-classified eval task.
    ///
    /// Like `spawn_eval`, but the task carries a WFST cost class and descriptor
    /// for automata-based scheduling. The transducer-assigned priority overrides
    /// the base priority in the scoring function, and the descriptor enables
    /// per-expression weight tracking on completion.
    pub fn spawn_eval_classified<F>(
        &self,
        f: F,
        task_type: TaskTypeId,
        priority: u32,
        cost_class: crate::backend::scheduler::CostClass,
        descriptor: crate::backend::scheduler::TaskDescriptor,
    ) where
        F: FnOnce() + Send + 'static,
    {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task = PriorityTask::new_classified(
            Box::new(f),
            priority,
            task_type,
            sequence,
            cost_class,
            descriptor,
        );
        self.queue.push(task);

        #[cfg(feature = "trace")]
        {
            let queue_depth = self.queue.len() as u32;
            let active_workers = self.active_workers() as u32;
            let max_workers = self.max_threads() as u32;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolTaskEnqueued {
                        task_kind: format!("eval[{}]", cost_class),
                        priority,
                        queue_depth,
                        active_workers,
                        max_workers,
                    },
                );
            });
        }
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

            #[cfg(feature = "trace")]
            {
                let queue_depth = self.queue.len() as u32;
                let active_workers = self.active_workers() as u32;
                with_work_pool_trace(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::WorkPoolTaskDropped {
                            task_kind: "compile".to_string(),
                            queue_depth,
                            active_workers,
                        },
                    );
                });
            }

            return false;
        }

        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let task = PriorityTask::new(Box::new(f), priority, task_type, sequence);
        self.queue.push(task);

        #[cfg(feature = "trace")]
        {
            let queue_depth = self.queue.len() as u32;
            let active_workers = self.active_workers() as u32;
            let max_workers = self.max_threads() as u32;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolTaskEnqueued {
                        task_kind: "compile".to_string(),
                        priority,
                        queue_depth,
                        active_workers,
                        max_workers,
                    },
                );
            });
        }

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

        #[cfg(feature = "trace")]
        {
            let queue_depth = self.queue.len() as u32;
            let active_workers = self.active_workers() as u32;
            let max_workers = self.max_threads() as u32;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolTaskEnqueued {
                        task_kind: "detached".to_string(),
                        priority,
                        queue_depth,
                        active_workers,
                        max_workers,
                    },
                );
            });
        }
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

    /// Get the target number of initially active workers.
    pub fn initial_active(&self) -> usize {
        self.initial_active
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

    /// Unpark up to `n` workers. Returns the number actually unparked.
    ///
    /// Scans worker parks from lowest index, unparking each parked worker
    /// until `n` have been unparked or no parked workers remain.
    pub fn unpark_n(&self, n: usize) -> usize {
        let mut unparked = 0;
        for park in &self.worker_parks {
            if unparked >= n {
                break;
            }
            if park.is_parked() {
                park.unpark();
                self.active_count.fetch_add(1, Ordering::Relaxed);
                unparked += 1;
            }
        }
        unparked
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

    /// Park up to `n` workers. Returns the number actually parked.
    ///
    /// Scans worker parks from highest index, parking each active worker
    /// until `n` have been parked, `min_threads` is reached, or no active
    /// workers remain.
    pub fn park_n(&self, n: usize) -> usize {
        let mut parked = 0;
        for park in self.worker_parks.iter().rev() {
            if parked >= n {
                break;
            }
            let active = self.active_count.load(Ordering::Relaxed);
            if active <= self.min_threads {
                break;
            }
            if !park.is_parked() {
                park.park();
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                parked += 1;
            }
        }
        parked
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
                None => false, // Not yet spawned (async init in progress) — skip
            };

            if !is_dead {
                continue;
            }

            // Reap the dead thread
            if let Some(old_handle) = guard.take() {
                match old_handle.join() {
                    Ok(()) => {
                        tracing::warn!(
                            worker_id = id,
                            "WorkPool: worker exited unexpectedly -- respawning"
                        );
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
            let cpu_state = Arc::clone(&self.worker_cpu_states[id]);

            let new_handle = thread::Builder::new()
                .name(format!("work-pool-{}", id))
                // **Stack-safety defense-in-depth**: see `spawn_worker` for rationale.
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    work_pool_worker_loop(id, queue, runtime_tracker, shutdown, park, cpu_state);
                })
                .expect("failed to respawn work pool worker thread");

            *guard = Some(new_handle);
            respawned += 1;
        }

        respawned
    }

    /// Initiate graceful shutdown (signal only — does not join workers).
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);

        // Unpark all workers so they can see the shutdown signal
        for park in &self.worker_parks {
            park.unpark();
        }

        // Wake up any workers blocked on the queue
        self.queue.notify_all();
    }

    /// Signal shutdown and join all worker threads (blocks until workers exit).
    ///
    /// Unlike `shutdown()` which only signals, this method waits for all
    /// in-flight tasks to complete. Useful when the pool is behind an `Arc`
    /// and `Drop` won't run until the last reference is released.
    pub fn shutdown_and_join(&self) {
        self.shutdown();

        // Signal overflow workers to drain
        {
            let overflow = self.overflow_workers.lock();
            for ow in overflow.iter() {
                ow.self_shutdown.store(true, Ordering::Relaxed);
            }
        }

        // Join core workers
        for slot in self.workers.iter() {
            if let Some(handle) = slot.lock().take() {
                let _ = handle.join();
            }
        }

        // Join overflow workers
        let mut overflow = self.overflow_workers.lock();
        for ow in overflow.drain(..) {
            let _ = ow.handle.join();
        }
    }
}

/// Compute how many overflow workers may be spawned without exceeding the cap.
#[inline]
fn overflow_spawn_quota(requested: usize, live_overflow: usize, max_overflow: usize) -> usize {
    requested.min(max_overflow.saturating_sub(live_overflow))
}

// ============================================================================
// Overflow Pool Methods
// ============================================================================

impl WorkPool {
    /// Get the current number of active overflow threads.
    #[inline]
    pub fn overflow_count(&self) -> usize {
        self.overflow_count.load(Ordering::Relaxed)
    }

    /// Maximum number of overflow threads allowed.
    ///
    /// Capped at `max_threads` so total possible workers = `2 * max_threads`.
    #[inline]
    pub fn max_overflow(&self) -> usize {
        self.max_threads
    }

    /// Get a reference to the per-worker CPU states (for monitor access).
    pub fn worker_cpu_states(&self) -> &[Arc<WorkerCpuState>] {
        &self.worker_cpu_states
    }

    /// Spawn `count` overflow worker threads.
    ///
    /// Overflow workers share the same `PriorityQueue` as core workers but
    /// have their own shutdown signals and CPU states for independent lifecycle
    /// management. They self-drain after 1s of idleness.
    pub fn spawn_overflow(&self, count: usize) {
        let mut overflow = self.overflow_workers.lock();
        let live_overflow = self.overflow_count.load(Ordering::Acquire);
        let spawn_count = overflow_spawn_quota(count, live_overflow, self.max_overflow());
        if spawn_count == 0 {
            trace!(
                requested = count,
                live_overflow,
                max_overflow = self.max_overflow(),
                "WorkPool: overflow spawn skipped at cap"
            );
            return;
        }

        let base_id = self.max_threads + overflow.len();

        for i in 0..spawn_count {
            let queue = Arc::clone(&self.queue);
            let runtime_tracker = Arc::clone(&self.runtime_tracker);
            let global_shutdown = Arc::clone(&self.shutdown);
            let self_shutdown = Arc::new(AtomicBool::new(false));
            let cpu_state = Arc::new(WorkerCpuState::new());
            let overflow_count = &self.overflow_count as *const AtomicUsize as usize;
            let self_shutdown_clone = Arc::clone(&self_shutdown);
            let cpu_state_clone = Arc::clone(&cpu_state);
            let worker_id = base_id + i;

            // SAFETY: overflow_count points to a field of the WorkPool behind
            // GLOBAL_EVAL_POOL (LazyLock, 'static lifetime). The AtomicUsize
            // outlives any overflow worker thread. For test pools, Drop joins
            // all overflow threads before the pool is freed.
            let overflow_count_ref = unsafe { &*(overflow_count as *const AtomicUsize) };

            let handle = thread::Builder::new()
                .name(format!("work-pool-overflow-{}", worker_id))
                // **Stack-safety defense-in-depth**: see `spawn_worker` for rationale.
                .stack_size(8 * 1024 * 1024)
                .spawn(move || {
                    overflow_worker_loop(
                        worker_id,
                        queue,
                        runtime_tracker,
                        global_shutdown,
                        self_shutdown_clone,
                        cpu_state_clone,
                        overflow_count_ref,
                    );
                })
                .expect("failed to spawn overflow worker thread");

            self.overflow_count.fetch_add(1, Ordering::Relaxed);

            overflow.push(OverflowWorker {
                handle,
                self_shutdown,
                cpu_state,
            });
        }

        trace!(
            spawned = spawn_count,
            total_overflow = self.overflow_count.load(Ordering::Relaxed),
            "WorkPool: spawned overflow workers"
        );
    }

    /// Signal `count` overflow workers to drain (oldest first).
    ///
    /// Workers exit after completing their current task or after the next
    /// queue pop timeout (500ms). Finished handles are reaped lazily by
    /// `reap_finished_overflow()`.
    pub fn drain_overflow(&self, count: usize) {
        let overflow = self.overflow_workers.lock();
        let mut drained = 0;
        for ow in overflow.iter() {
            if drained >= count {
                break;
            }
            if !ow.self_shutdown.load(Ordering::Relaxed) {
                ow.self_shutdown.store(true, Ordering::Relaxed);
                drained += 1;
            }
        }
        if drained > 0 {
            trace!(drained, "WorkPool: signaled overflow workers to drain");
        }
    }

    /// Signal all overflow workers to drain.
    pub fn drain_all_overflow(&self) {
        let overflow = self.overflow_workers.lock();
        for ow in overflow.iter() {
            ow.self_shutdown.store(true, Ordering::Relaxed);
        }
    }

    /// Reap (join) overflow workers that have finished.
    ///
    /// Called at the end of each monitor tick to clean up joined handles
    /// and their associated resources. Non-blocking: only joins threads
    /// whose `is_finished()` returns true.
    pub fn reap_finished_overflow(&self) {
        let mut overflow = self.overflow_workers.lock();
        let before = overflow.len();

        // Partition: retain live workers, collect finished ones
        let mut i = 0;
        while i < overflow.len() {
            if overflow[i].handle.is_finished() {
                let ow = overflow.swap_remove(i);
                let _ = ow.handle.join();
                // Note: overflow_count is decremented by the worker loop itself
            } else {
                i += 1;
            }
        }

        let reaped = before - overflow.len();
        if reaped > 0 {
            trace!(
                reaped,
                remaining = overflow.len(),
                "WorkPool: reaped finished overflow workers"
            );
        }
    }

    /// Collect CPU states from all live overflow workers.
    ///
    /// Returns a snapshot of `Arc<WorkerCpuState>` for each overflow worker
    /// that has not been signaled to shut down. Used by the monitor for
    /// blocked-worker detection across the full worker population.
    pub fn overflow_cpu_states(&self) -> Vec<Arc<WorkerCpuState>> {
        let overflow = self.overflow_workers.lock();
        overflow
            .iter()
            .filter(|ow| !ow.self_shutdown.load(Ordering::Relaxed))
            .map(|ow| Arc::clone(&ow.cpu_state))
            .collect()
    }
}

// ============================================================================
// Overflow Worker Loop
// ============================================================================

/// Overflow worker thread main loop.
///
/// Simpler than the core worker loop — no park/unpark, no trace events.
/// Self-drains after 2 consecutive idle timeouts (1s total) with no work.
fn overflow_worker_loop(
    _id: usize,
    queue: Arc<PriorityQueue>,
    runtime_tracker: Arc<RuntimeTracker>,
    shutdown: Arc<AtomicBool>,
    self_shutdown: Arc<AtomicBool>,
    cpu_state: Arc<WorkerCpuState>,
    overflow_count: &AtomicUsize,
) {
    // Publish initial CPU state at startup.
    cpu_state.publish_initial();

    let mut consecutive_idle = 0u32;

    loop {
        if shutdown.load(Ordering::Relaxed) || self_shutdown.load(Ordering::Relaxed) {
            break;
        }

        match queue.pop_timeout(&shutdown, Duration::from_millis(500)) {
            Some(task) => {
                consecutive_idle = 0;
                let task_type = task.task_type();
                let wfst_descriptor = task.wfst_descriptor();

                let outer_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let runtime_nanos = task.execute();
                    if runtime_nanos > 0 {
                        runtime_tracker.record_runtime(task_type, runtime_nanos);
                        // WFST weight update: feed actual runtime to scheduler automaton
                        if let Some(descriptor) = wfst_descriptor {
                            crate::backend::scheduler::global_scheduler()
                                .update_weight(descriptor, runtime_nanos);
                        }
                    }
                    cpu_state.publish();

                    #[cfg(any(feature = "trace", feature = "track-stats"))]
                    if matches!(task_type, TaskTypeId::Eval(_)) {
                        WORK_EVAL_COUNT.fetch_add(1, Ordering::Relaxed);
                    }
                }));

                if let Err(payload) = outer_result {
                    tracing::error!(
                        worker_id = _id,
                        ?task_type,
                        panic = ?payload,
                        "overflow_worker_loop: catch_unwind caught panic -- worker continues"
                    );
                }
            }
            None => {
                consecutive_idle += 1;
                // Self-drain: if idle for 2 consecutive timeouts (1s total)
                // with no work, this overflow thread is no longer needed.
                if consecutive_idle >= 2 {
                    break;
                }
            }
        }
    }

    overflow_count.fetch_sub(1, Ordering::Relaxed);
}

impl Drop for WorkPool {
    fn drop(&mut self) {
        self.shutdown();

        // Signal all overflow workers to drain
        {
            let overflow = self.overflow_workers.lock();
            for ow in overflow.iter() {
                ow.self_shutdown.store(true, Ordering::Relaxed);
            }
        }

        // Join all core worker threads
        for slot in self.workers.iter() {
            if let Some(handle) = slot.lock().take() {
                let _ = handle.join();
            }
        }

        // Join all overflow worker threads
        let mut overflow = self.overflow_workers.lock();
        for ow in overflow.drain(..) {
            let _ = ow.handle.join();
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
    cpu_state: Arc<WorkerCpuState>,
) {
    // Publish initial CPU/wall time at thread startup so the monitor
    // has a valid baseline before any tasks execute.
    cpu_state.publish_initial();

    // Track whether this worker was parked at the end of the previous iteration,
    // so we can detect park→unpark and unpark→park transitions for trace events.
    #[cfg(feature = "trace")]
    let mut was_parked = park.is_parked();

    loop {
        // Check for shutdown
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Detect parking: if we're about to block, emit a WorkPoolWorkerParked event.
        #[cfg(feature = "trace")]
        {
            let currently_parked = park.is_parked();
            if currently_parked && !was_parked {
                let queue_depth = queue.len() as u32;
                with_work_pool_trace(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::WorkPoolWorkerParked {
                            worker_id: _id as u32,
                            queue_depth,
                        },
                    );
                });
            }
            was_parked = currently_parked;
        }

        // Check if we're parked — block until unparked (with 5s timeout to
        // recover from a dead scaling monitor that never calls unpark())
        park.wait_if_parked_timeout(Duration::from_secs(5));

        // Detect resumption: if we were parked and now aren't, emit WorkPoolWorkerResumed.
        #[cfg(feature = "trace")]
        {
            let currently_parked = park.is_parked();
            if was_parked && !currently_parked {
                let (active_workers, _max) = global_eval_pool_stats();
                let queue_depth = queue.len() as u32;
                with_work_pool_trace(|tc| {
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        0,
                        trace_format::TraceValue::Unit,
                        vec![],
                        None,
                        trace_format::TraceEventKind::WorkPoolWorkerResumed {
                            worker_id: _id as u32,
                            queue_depth,
                            active_workers,
                        },
                    );
                });
            }
            was_parked = currently_parked;
        }

        // Re-check shutdown after unpark
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Pop a task with timeout (allows periodic park/shutdown checks)
        match queue.pop_timeout(&shutdown, Duration::from_millis(500)) {
            Some(task) => {
                let task_type = task.task_type();
                let wfst_descriptor = task.wfst_descriptor();

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
                        // WFST weight update: feed actual runtime to scheduler automaton
                        if let Some(descriptor) = wfst_descriptor {
                            crate::backend::scheduler::global_scheduler()
                                .update_weight(descriptor, runtime_nanos);
                        }
                    }

                    // Publish CPU time + heartbeat for blocked-worker detection.
                    // The monitor reads these atomics every 200ms to compute
                    // per-worker cpu_delta/wall_delta ratios.
                    cpu_state.publish();

                    // Emit WorkPoolTaskCompleted trace event
                    #[cfg(feature = "trace")]
                    {
                        let (active_workers, _max) = global_eval_pool_stats();
                        let queue_depth = queue.len() as u32;
                        let kind_str = task_type_kind_str(&task_type);
                        with_work_pool_trace(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                trace_format::TraceValue::Unit,
                                vec![],
                                None,
                                trace_format::TraceEventKind::WorkPoolTaskCompleted {
                                    task_kind: kind_str.to_string(),
                                    runtime_nanos,
                                    queue_depth,
                                    active_workers,
                                },
                            );
                        });
                    }

                    // Track eval completions for throughput monitoring
                    #[cfg(any(feature = "trace", feature = "track-stats"))]
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
                // PriorityQueue empty/timeout — loop back to park/shutdown check.
            }
        }
    }
}

// ============================================================================
// Global Singleton
// ============================================================================

/// Global eval pool singleton (lazy initialization).
///
/// Handles eval tasks only. Compile tasks are routed to `GLOBAL_COMPILE_POOL`.
/// `LazyLock::new(WorkPool::new)` calls `allocate()` only — no OS threads
/// are spawned. Workers are started asynchronously via `start_async_init()`.
static GLOBAL_EVAL_POOL: LazyLock<WorkPool> = LazyLock::new(WorkPool::new);

/// Global async init guard — ensures `spawn_all_workers()` runs exactly once.
static WORK_POOL_INIT: OnceLock<()> = OnceLock::new();

impl WorkPool {
    /// Spawn all workers synchronously (idempotent).
    ///
    /// Ensures `active_workers() > 0` by the time the first evaluation
    /// reaches a parallel gate. Workers are spawned in parallel via
    /// `thread::scope` when max_threads > 4, so wall-clock time is
    /// ~1 thread-creation time regardless of pool size.
    fn start_init(&'static self) {
        WORK_POOL_INIT.get_or_init(|| {
            self.spawn_all_workers();
        });
    }
}

/// Get the global eval pool (for evaluation tasks).
///
/// On first access, lazily initializes the pool (allocation only), kicks off
/// async background worker spawning, and starts the scaling monitor.
pub fn global_eval_pool() -> &'static WorkPool {
    let pool = &*GLOBAL_EVAL_POOL;
    pool.start_init();
    // Start the scaling monitor (idempotent — only runs once)
    start_work_scaling_monitor();
    pool
}

/// Backward-compatible alias for `global_eval_pool()`.
#[inline]
pub fn global_work_pool() -> &'static WorkPool {
    global_eval_pool()
}

// ============================================================================
// Global Compile Pool (Fixed-Size, Separate from Eval)
// ============================================================================

/// Number of fixed worker threads for the compile pool.
///
/// Fixed at 4 to prevent compile tasks from stealing CPU from eval workers.
/// The compile pool has no scaling monitor — its size is constant.
const COMPILE_POOL_WORKERS: usize = 4;

/// Global compile pool singleton (lazy initialization).
///
/// Separate from the eval pool to prevent compile task CPU contention from
/// degrading eval throughput. Fixed at `COMPILE_POOL_WORKERS` threads with
/// no scaling monitor.
static GLOBAL_COMPILE_POOL: LazyLock<WorkPool> = LazyLock::new(|| {
    WorkPool::with_threads_initial(
        COMPILE_POOL_WORKERS, // min = 4
        COMPILE_POOL_WORKERS, // max = 4 (fixed)
        COMPILE_POOL_WORKERS, // all active from start
    )
});

/// Get the global compile pool (for bytecode/JIT compilation tasks).
///
/// Separate from the eval pool to prevent CPU contention. Fixed at
/// `COMPILE_POOL_WORKERS` threads with no dynamic scaling.
pub fn global_compile_pool() -> &'static WorkPool {
    &*GLOBAL_COMPILE_POOL
}

/// Eagerly initialize all global thread pools at application startup.
///
/// Forces initialization of the eval pool (with async background worker
/// spawning), compile pool, GC pool, and scaling monitor. Call from `main()`
/// before any evaluation to start workers warming up during arg parsing.
///
/// This is optional — all pools self-initialize on first access via
/// `global_eval_pool()` / `global_compile_pool()` / `global_gc_pool()`,
/// so the Rholang integration entry point also triggers initialization.
/// Calling this from `main()` just starts it sooner.
pub fn init_thread_pools() {
    let _ = global_eval_pool(); // Triggers LazyLock + start_async_init + scaling monitor
    let _ = global_compile_pool(); // Triggers LazyLock for compile pool
    let _ = super::gc_pool::global_gc_pool(); // Triggers OnceLock for GC pool
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
///
/// Increased from 0.5 to 2.0 so that a growing queue can counterbalance
/// memory pressure (w_mp=5.0). At queue_depth=10 pending tasks, the
/// contribution is 2.0 × 10 = 20.0, which equals w_mp × M at bp_level=1
/// slab_pressure=2.0. This prevents the death spiral where memory pressure
/// parks all workers → queue grows → more parking.
const QUEUE_DEPTH_WEIGHT: f64 = 2.0;

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

    #[cfg(target_os = "macos")]
    {
        let _ = page_size; // Suppress unused warning on macOS
        use std::mem;
        let mut info: libc::mach_task_basic_info_data_t = unsafe { mem::zeroed() };
        let mut count = (mem::size_of::<libc::mach_task_basic_info_data_t>()
            / mem::size_of::<libc::natural_t>())
            as libc::mach_msg_type_number_t;
        let kr = unsafe {
            libc::task_info(
                libc::mach_task_self(),
                libc::MACH_TASK_BASIC_INFO,
                &mut info as *mut _ as libc::task_info_t,
                &mut count,
            )
        };
        if kr == libc::KERN_SUCCESS {
            Some(info.resident_size as usize)
        } else {
            None
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = page_size; // Suppress unused warning on unsupported platforms
        None // RSS monitoring disabled on unsupported platforms
    }
}

/// Get the RSS limit for memory pressure computation.
///
/// Priority:
/// 1. `METTATRON_RSS_LIMIT_MB` environment variable (in MB)
/// 2. 80% of system physical memory (auto-detected via sysconf on Linux,
///    `sysctl` on macOS)
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

    #[cfg(target_os = "macos")]
    {
        let mut memsize: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let mib = [libc::CTL_HW, libc::HW_MEMSIZE];
        let ret = unsafe {
            libc::sysctl(
                mib.as_ptr() as *mut _,
                2,
                &mut memsize as *mut _ as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 && memsize > 0 {
            return (memsize as usize) * 4 / 5; // 80% of physical memory
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // No system memory detection — RSS limit disabled
    }

    0 // Disabled
}

/// Get the system page size in bytes.
#[cfg(target_os = "linux")]
fn get_page_size() -> usize {
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if ps > 0 {
        ps as usize
    } else {
        4096
    }
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

/// Per-worker CPU snapshot for blocked-worker detection.
///
/// Stores the previous sample's CPU and wall-clock nanoseconds so the
/// monitor can compute `cpu_delta / wall_delta` per worker.
#[derive(Clone)]
#[allow(dead_code)] // prev_blocked and last_cpu_ratio are read only with eval-trace feature
struct WorkerCpuSnapshot {
    prev_cpu_nanos: u64,
    prev_wall_nanos: u64,
    prev_task_count: u64,
    /// Number of consecutive ticks this worker has been considered stalled
    /// (heartbeat-only detection for non-Linux platforms).
    ticks_stalled: u32,
    /// Whether this worker was blocked in the previous tick (for transition detection).
    prev_blocked: bool,
    /// CPU ratio at last detection (for trace emission).
    last_cpu_ratio: f64,
}

impl WorkerCpuSnapshot {
    fn new() -> Self {
        Self {
            prev_cpu_nanos: 0,
            prev_wall_nanos: 0,
            prev_task_count: 0,
            ticks_stalled: 0,
            prev_blocked: false,
            last_cpu_ratio: 1.0,
        }
    }
}

/// Mutable state for the work pool scaling monitor.
///
/// Tracks EMA-smoothed throughput, queue depth, slab memory pressure,
/// and RSS pressure, feeding them to a four-term composite objective
/// function minimized by a HillClimber to decide when to park/unpark workers.
struct WorkMonitorState {
    /// Previous eval count snapshot (for delta computation).
    #[cfg(any(feature = "trace", feature = "track-stats"))]
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
    /// Per-worker CPU snapshots for blocked-worker detection (parallel to worker_parks).
    worker_snapshots: Vec<WorkerCpuSnapshot>,
    /// Number of core-pool workers detected as blocked in the last tick.
    blocked_worker_count: usize,
}

impl WorkMonitorState {
    fn new(pool: &WorkPool) -> Self {
        Self {
            #[cfg(any(feature = "trace", feature = "track-stats"))]
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
                pool.initial_active(), // stable value, doesn't depend on spawn progress
            ),
            rss_limit: get_rss_limit(),
            page_size: get_page_size(),
            worker_snapshots: (0..pool.max_threads())
                .map(|_| WorkerCpuSnapshot::new())
                .collect(),
            blocked_worker_count: 0,
        }
    }

    /// Detect blocked workers by comparing CPU time deltas to wall time deltas.
    ///
    /// For each non-parked core-pool worker, reads the current CPU and wall
    /// nanoseconds (Relaxed atomics), computes the ratio, and marks workers
    /// with ratio < BLOCKED_RATIO_THRESHOLD as blocked.
    ///
    /// On non-Linux platforms, falls back to heartbeat-only stall detection
    /// (workers with no task completions for >= 2 ticks are considered stalled).
    ///
    /// Returns a Vec of blocked worker indices (for trace emission).
    fn detect_blocked_workers(&mut self, pool: &WorkPool) -> Vec<u32> {
        let cpu_states = pool.worker_cpu_states();
        let mut blocked = 0;
        let mut blocked_indices = Vec::new();

        for (i, snap) in self.worker_snapshots.iter_mut().enumerate() {
            // Skip parked workers — they're blocked by design, not by contention
            if pool.worker_parks[i].is_parked() {
                snap.ticks_stalled = 0;
                // Transition: was blocked, now parked → treat as unblocked
                if snap.prev_blocked {
                    snap.prev_blocked = false;
                    #[cfg(feature = "trace")]
                    {
                        let worker_id = i as u32;
                        with_work_pool_trace(|tc| {
                            tc.emit_converted(
                                trace_format::TraceTier::TreeWalker,
                                0,
                                trace_format::TraceValue::Unit,
                                vec![],
                                None,
                                trace_format::TraceEventKind::WorkPoolWorkerUnblocked { worker_id },
                            );
                        });
                    }
                }
                continue;
            }

            let state = &cpu_states[i];
            let curr_cpu = state.cpu_nanos.load(Ordering::Relaxed);
            let curr_wall = state.wall_nanos.load(Ordering::Relaxed);
            let curr_tasks = state.task_count.load(Ordering::Relaxed);

            // Worker hasn't published yet (just spawned, no tasks executed).
            // Treat it like a stalled heartbeat: increment ticks_stalled and
            // count as blocked after 2 ticks, matching the non-Linux heartbeat
            // fallback threshold. This handles the case where OS thread
            // scheduling delays prevent publish_initial() from running.
            if curr_wall == 0 {
                snap.ticks_stalled += 1;
                if snap.ticks_stalled >= 2 {
                    blocked += 1;
                    blocked_indices.push(i as u32);
                    snap.prev_blocked = true;
                }
                continue;
            }

            let task_delta = curr_tasks.wrapping_sub(snap.prev_task_count);
            let mut is_blocked = false;

            #[cfg(target_os = "linux")]
            {
                let cpu_delta = curr_cpu.saturating_sub(snap.prev_cpu_nanos);
                let wall_delta = curr_wall.saturating_sub(snap.prev_wall_nanos);

                if wall_delta >= MIN_WALL_DELTA_NS {
                    let ratio = cpu_delta as f64 / wall_delta as f64;
                    snap.last_cpu_ratio = ratio;
                    if ratio < BLOCKED_RATIO_THRESHOLD && task_delta == 0 {
                        is_blocked = true;
                    }
                }
            }

            #[cfg(not(target_os = "linux"))]
            {
                if task_delta == 0 {
                    snap.ticks_stalled += 1;
                    if snap.ticks_stalled >= 2 {
                        is_blocked = true;
                        snap.last_cpu_ratio = 0.0;
                    }
                } else {
                    snap.ticks_stalled = 0;
                    snap.last_cpu_ratio = 1.0;
                }
            }

            if is_blocked {
                blocked += 1;
                blocked_indices.push(i as u32);
            }

            // Emit per-worker transition events
            #[cfg(feature = "trace")]
            {
                if is_blocked && !snap.prev_blocked {
                    // Transition: unblocked → blocked
                    let worker_id = i as u32;
                    let cpu_ratio = snap.last_cpu_ratio;
                    with_work_pool_trace(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::WorkPoolWorkerBlocked {
                                worker_id,
                                cpu_ratio,
                            },
                        );
                    });
                } else if !is_blocked && snap.prev_blocked {
                    // Transition: blocked → unblocked
                    let worker_id = i as u32;
                    with_work_pool_trace(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            0,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::WorkPoolWorkerUnblocked { worker_id },
                        );
                    });
                }
            }

            snap.prev_blocked = is_blocked;

            // Update snapshot for next tick
            snap.prev_cpu_nanos = curr_cpu;
            snap.prev_wall_nanos = curr_wall;
            snap.prev_task_count = curr_tasks;
        }

        self.blocked_worker_count = blocked;
        blocked_indices
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
/// ## Graduated Memory Pressure Response
///
/// Instead of an emergency override that bypasses the hill climber, slab memory
/// pressure is amplified by a graduated multiplier (1x at bp_level 0-1, 2x at
/// bp_level 2, 4x at bp_level 3). This allows queue depth to counterbalance
/// memory pressure, preventing the death spiral where memory pressure parks all
/// workers → queue grows → more parking.
fn work_scaling_monitor_tick(pool: &WorkPool, state: &mut WorkMonitorState) {
    let now = Instant::now();
    let elapsed = now.duration_since(state.prev_sample_time).as_secs_f64();
    if elapsed < 0.001 {
        // Avoid division by zero on very fast successive calls
        return;
    }

    // Sample throughput: delta evals / elapsed time
    #[cfg(any(feature = "trace", feature = "track-stats"))]
    let (throughput_raw, delta_evals_raw, current_eval_count) = {
        let current_eval_count = work_eval_count();
        let delta_evals_raw = current_eval_count.wrapping_sub(state.prev_eval_count);
        let throughput_raw = delta_evals_raw as f64 / elapsed;
        state.prev_eval_count = current_eval_count;
        (throughput_raw, delta_evals_raw, current_eval_count)
    };
    state.prev_sample_time = now;

    // Sample queue depth
    let queue_len_raw = pool.queue_len();
    let queue_depth = queue_len_raw as f64;

    // Idle detection: no evals completed AND no work queued.
    // When idle, freeze EMAs (preserve last active signal) and skip all
    // scaling phases. This prevents EMA decay during idle periods from
    // confusing the hill climber into parking workers.
    #[cfg(any(feature = "trace", feature = "track-stats"))]
    let is_idle = delta_evals_raw == 0 && queue_len_raw == 0;
    #[cfg(not(any(feature = "trace", feature = "track-stats")))]
    let is_idle = queue_len_raw == 0;

    // Sample memory pressure signals
    let slab_p = slab_pressure();
    let rss_p = rss_pressure(state.page_size, state.rss_limit);

    // Emit WorkPoolMonitorTick — raw samples before any EMA/decision processing
    #[cfg(feature = "trace")]
    {
        // current_eval_count is available via any(eval-trace, track-stats) gate
        let trace_eval_count = current_eval_count;
        let rss_bytes = read_rss_bytes(state.page_size).unwrap_or(0) as u64;
        let bp_lvl = super::gc_allocator::backpressure_level();
        with_work_pool_trace(|tc| {
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                0,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::WorkPoolMonitorTick {
                    current_eval_count: trace_eval_count,
                    elapsed_ns: (elapsed * 1_000_000_000.0) as u64,
                    queue_len: queue_len_raw as u32,
                    bp_level: bp_lvl as u32,
                    rss_bytes,
                },
            );
        });
    }

    // When idle: skip EMA updates and all scaling phases.
    // Still run housekeeping (respawns + overflow reaping).
    if is_idle {
        #[cfg(feature = "trace")]
        {
            let active_workers_after = pool.active_workers() as u32;
            let instantaneous_qd = pool.queue_len() as u32;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolScaleEvent {
                        action: "hold".to_string(),
                        active_workers_after,
                        min_workers: pool.min_threads() as u32,
                        max_workers: pool.max_threads() as u32,
                        queue_depth: instantaneous_qd,
                        ema_throughput: state.ema_throughput.value(),
                        ema_queue_depth: state.ema_queue_depth.value(),
                        ema_slab_pressure: state.ema_slab_pressure.value(),
                        ema_rss_pressure: state.ema_rss_pressure.value(),
                        objective: 0.0,
                        emergency: false,
                        hc_direction: state.climber.direction(),
                        hc_cooldown_remaining: state.climber.cooldown_remaining(),
                        hc_prev_objective: state.climber.prev_objective(),
                        hc_improvement: 0.0,
                        raw_throughput: 0.0,
                        raw_slab_pressure: slab_p,
                        raw_rss_pressure: rss_p,
                        bp_level: super::gc_allocator::backpressure_level() as u32,
                        slab_amplifier: 1.0,
                        term_throughput: 0.0,
                        term_queue_depth: 0.0,
                        term_slab_pressure: 0.0,
                        term_rss_pressure: 0.0,
                        blocked_worker_count: state.blocked_worker_count as u32,
                        overflow_count: pool.overflow_count() as u32,
                        decision_phase: "idle_skip".to_string(),
                        delta_evals: 0,
                        elapsed_seconds: elapsed,
                    },
                );
            });
        }

        // Drain any overflow workers when idle — no work for them to do
        if pool.overflow_count() > 0 {
            pool.drain_all_overflow();
        }
        check_and_log_respawns(pool);
        pool.reap_finished_overflow();
        return;
    }

    // Update all EMAs (only when active — frozen during idle to preserve signal)
    #[cfg(any(feature = "trace", feature = "track-stats"))]
    let ema_tp = state.ema_throughput.update(throughput_raw);
    #[cfg(not(any(feature = "trace", feature = "track-stats")))]
    let ema_tp = state.ema_throughput.value(); // no throughput data, use current EMA
    let ema_qd = state.ema_queue_depth.update(queue_depth);
    let ema_slab = state.ema_slab_pressure.update(slab_p);
    let ema_rss = state.ema_rss_pressure.update(rss_p);

    // Graduated slab pressure amplification
    let bp_level = super::gc_allocator::backpressure_level();
    let slab_amplifier = match bp_level {
        0 | 1 => 1.0,
        2 => 2.0,
        _ => 4.0, // bp_level 3 (heavy)
    };

    // Compute individual objective terms (used by both Phase 1 and Phase 4 trace blocks).
    // Use EMA queue depth (not instantaneous) — EMA retains memory of recent queueing
    // even when the queue transiently empties, providing counter-pressure against parking.
    let term_throughput = -THROUGHPUT_WEIGHT * ema_tp;
    let term_queue_depth = QUEUE_DEPTH_WEIGHT * ema_qd;
    let term_slab_pressure = MEMORY_PRESSURE_WEIGHT * slab_amplifier * ema_slab;
    let term_rss_pressure = RSS_PRESSURE_WEIGHT * ema_rss;

    // Snapshot hill climber state BEFORE any mutations (used for trace emission)
    #[cfg(feature = "trace")]
    let hc_direction = state.climber.direction();
    #[cfg(feature = "trace")]
    let hc_cooldown = state.climber.cooldown_remaining();
    #[cfg(feature = "trace")]
    let hc_prev_obj = state.climber.prev_objective();

    // ====================================================================
    // Phase 1: Blocked-worker detection
    // ====================================================================
    #[cfg(feature = "trace")]
    let blocked_indices = state.detect_blocked_workers(pool);
    #[cfg(not(feature = "trace"))]
    state.detect_blocked_workers(pool);

    // Emit WorkPoolBlockedWorkersDetected when any workers are blocked
    #[cfg(feature = "trace")]
    {
        if state.blocked_worker_count > 0 {
            let active_workers = pool.active_workers() as u32;
            let blocked_count = state.blocked_worker_count as u32;
            let indices = blocked_indices;
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolBlockedWorkersDetected {
                        blocked_count,
                        active_workers,
                        blocked_indices: indices,
                    },
                );
            });
        }
    }

    // ====================================================================
    // Phase 2: Compensatory activation (emergency bypass)
    // ====================================================================
    let active = pool.active_workers();
    let blocked = state.blocked_worker_count;
    let overflow = pool.overflow_count();
    let unblocked_core = active.saturating_sub(blocked);
    let total_unblocked = unblocked_core + overflow;
    let target = state.climber.current_active();
    let queue_len = pool.queue_len();

    // Track compensatory actions for trace emission
    #[cfg(feature = "trace")]
    let mut comp_core_unparked: u32 = 0;
    #[cfg(feature = "trace")]
    let mut comp_overflow_spawned: u32 = 0;
    #[cfg(feature = "trace")]
    let mut comp_overflow_drained: u32 = 0;
    #[cfg(feature = "trace")]
    let mut comp_deficit: u32 = 0;
    #[cfg(feature = "trace")]
    let mut comp_rss_veto = false;
    #[cfg(feature = "trace")]
    let mut comp_any_action = false;

    if queue_len == 0 {
        if overflow > 0 {
            #[cfg(feature = "trace")]
            {
                comp_overflow_drained = overflow as u32;
                comp_any_action = true;
            }
            pool.drain_all_overflow();
            trace!(overflow, "WorkPool: draining all overflow (queue empty)");
        }
    } else if total_unblocked < target {
        let deficit = target - total_unblocked;
        #[cfg(feature = "trace")]
        {
            comp_deficit = deficit as u32;
        }

        let mut compensated = 0;
        while compensated < deficit {
            if pool.unpark_one() {
                compensated += 1;
            } else {
                break;
            }
        }
        #[cfg(feature = "trace")]
        {
            comp_core_unparked = compensated as u32;
        }

        let remaining = deficit - compensated;
        let overflow_allowed = rss_p < 2.0;
        #[cfg(feature = "trace")]
        {
            comp_rss_veto = !overflow_allowed;
        }
        if remaining > 0 && overflow_allowed {
            let max_overflow = pool.max_overflow();
            let can_spawn = max_overflow.saturating_sub(overflow);
            let to_spawn = remaining.min(can_spawn);
            if to_spawn > 0 {
                pool.spawn_overflow(to_spawn);
                #[cfg(feature = "trace")]
                {
                    comp_overflow_spawned = to_spawn as u32;
                }
                trace!(
                    to_spawn,
                    blocked,
                    deficit,
                    compensated,
                    "WorkPool: spawned overflow to compensate for blocked workers"
                );
            }
        }
        #[cfg(feature = "trace")]
        {
            comp_any_action = true;
        }
    }

    // Drain excess overflow when blocking resolves
    if overflow > 0 && total_unblocked > target {
        let excess = (total_unblocked - target).min(overflow);
        #[cfg(feature = "trace")]
        {
            comp_overflow_drained = excess as u32;
            comp_any_action = true;
        }
        pool.drain_overflow(excess);
    }

    // Emit WorkPoolCompensatoryAction when any compensatory action was taken
    #[cfg(feature = "trace")]
    {
        if comp_any_action {
            with_work_pool_trace(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::WorkPoolCompensatoryAction {
                        core_unparked: comp_core_unparked,
                        overflow_spawned: comp_overflow_spawned,
                        overflow_drained: comp_overflow_drained,
                        target: target as u32,
                        deficit: comp_deficit,
                        rss_veto: comp_rss_veto,
                    },
                );
            });
        }
    }

    // ====================================================================
    // Phase 3: Hill climber (normal throughput/queue-depth optimization)
    // ====================================================================
    let objective = term_throughput + term_queue_depth + term_slab_pressure + term_rss_pressure;

    // Feed to hill climber
    let decision = state.climber.step(objective);

    // Compute improvement after step
    #[cfg(feature = "trace")]
    let hc_improvement = hc_prev_obj - objective;

    // Determine action string for trace event
    #[cfg(feature = "trace")]
    let action_str = match decision.action {
        ScaleAction::Unpark => "unpark",
        ScaleAction::Park => "park",
        ScaleAction::Hold => "hold",
    };

    match decision.action {
        ScaleAction::Unpark => {
            let actually_unparked = pool.unpark_n(decision.count);
            if actually_unparked > 0 {
                trace!(
                    throughput = ema_tp,
                    queue_depth = ema_qd,
                    slab_pressure = ema_slab,
                    rss_pressure = ema_rss,
                    objective,
                    requested = decision.count,
                    actually_unparked,
                    active = pool.active_workers(),
                    step_size = state.climber.step_size(),
                    "WorkPool scaling: unparked workers"
                );
            }
        }
        ScaleAction::Park => {
            let actually_parked = pool.park_n(decision.count);
            if actually_parked > 0 {
                trace!(
                    throughput = ema_tp,
                    queue_depth = ema_qd,
                    slab_pressure = ema_slab,
                    rss_pressure = ema_rss,
                    objective,
                    requested = decision.count,
                    actually_parked,
                    active = pool.active_workers(),
                    step_size = state.climber.step_size(),
                    "WorkPool scaling: parked workers"
                );
            }
        }
        ScaleAction::Hold => {
            // No action needed
        }
    }

    // Emit WorkPoolScaleEvent for every tick (including Hold) so the full
    // timeline of the monitor's decision-making is visible in the trace.
    #[cfg(feature = "trace")]
    {
        let active_workers_after = pool.active_workers() as u32;
        let instantaneous_qd = pool.queue_len() as u32;
        with_work_pool_trace(|tc| {
            tc.emit_converted(
                trace_format::TraceTier::TreeWalker,
                0,
                trace_format::TraceValue::Unit,
                vec![],
                None,
                trace_format::TraceEventKind::WorkPoolScaleEvent {
                    action: action_str.to_string(),
                    active_workers_after,
                    min_workers: pool.min_threads() as u32,
                    max_workers: pool.max_threads() as u32,
                    queue_depth: instantaneous_qd,
                    ema_throughput: ema_tp,
                    ema_queue_depth: ema_qd,
                    ema_slab_pressure: ema_slab,
                    ema_rss_pressure: ema_rss,
                    objective,
                    emergency: false,
                    hc_direction,
                    hc_cooldown_remaining: hc_cooldown,
                    hc_prev_objective: hc_prev_obj,
                    hc_improvement,
                    raw_throughput: throughput_raw,
                    raw_slab_pressure: slab_p,
                    raw_rss_pressure: rss_p,
                    bp_level: bp_level as u32,
                    slab_amplifier,
                    term_throughput,
                    term_queue_depth,
                    term_slab_pressure,
                    term_rss_pressure,
                    blocked_worker_count: state.blocked_worker_count as u32,
                    overflow_count: pool.overflow_count() as u32,
                    decision_phase: "phase3_hill_climber".to_string(),
                    delta_evals: delta_evals_raw,
                    elapsed_seconds: elapsed,
                },
            );
        });
    }

    // Check for and respawn dead workers, then reap finished overflow
    check_and_log_respawns(pool);
    pool.reap_finished_overflow();
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
        // Access GLOBAL_EVAL_POOL directly (not via global_work_pool()) to avoid
        // reentrant OnceLock deadlock: global_work_pool() → start_work_scaling_monitor()
        // → global_work_pool() → start_work_scaling_monitor() → OnceLock reentry.
        let pool: &'static WorkPool = &*GLOBAL_EVAL_POOL;
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
            tracing::warn!(
                "Work scaling monitor did not signal ready within 5s — continuing without scaling"
            );
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

    /// Mutex to serialize tests that read/write global `backpressure_level`.
    /// Prevents parallel test contamination (one test setting bp=3 while
    /// another expects bp=0 inside `work_scaling_monitor_tick`).
    static BP_TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let pool = global_work_pool(); // Triggers async init

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
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Simulate time passing
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);

        // Run a tick — on idle system (no evals, no queue), EMAs should be
        // frozen (not updated) to prevent throughput EMA decay from confusing
        // the hill climber into parking workers.
        work_scaling_monitor_tick(&pool, &mut state);

        // EMAs should NOT be initialized (idle tick skips EMA updates)
        assert!(
            !state.ema_throughput.is_initialized(),
            "Idle tick should skip EMA updates to preserve signal"
        );
        assert!(
            !state.ema_queue_depth.is_initialized(),
            "Idle tick should skip EMA updates to preserve signal"
        );
    }

    #[test]
    fn test_work_scaling_monitor_tick_skips_fast_calls() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        let pool = WorkPool::with_threads(2, 4);
        let mut state = WorkMonitorState::new(&pool);

        // Set prev_sample_time to now (< 1ms elapsed)
        state.prev_sample_time = Instant::now();

        // Should skip (avoid division by zero)
        work_scaling_monitor_tick(&pool, &mut state);

        // EMAs should NOT be initialized (tick was skipped)
        assert!(!state.ema_throughput.is_initialized());
    }

    #[cfg(feature = "track-stats")]
    #[test]
    fn test_work_scaling_monitor_tick_with_load() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

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
        assert_eq!(
            pressure, 0.0,
            "RSS pressure should be 0.0 when limit is 0 (disabled)"
        );
    }

    #[test]
    fn test_memory_pressure_increases_objective() {
        // Verify that higher memory pressure produces a worse (higher) objective.
        // Use the weight formula directly: J = -tp + 0.5*qd + 5.0*slab + 8.0*rss
        let tp = 100.0;
        let qd = 0.0;

        let obj_no_pressure = -THROUGHPUT_WEIGHT * tp
            + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 0.0
            + RSS_PRESSURE_WEIGHT * 0.0;

        let obj_slab_pressure = -THROUGHPUT_WEIGHT * tp
            + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 1.0
            + RSS_PRESSURE_WEIGHT * 0.0;

        let obj_rss_pressure = -THROUGHPUT_WEIGHT * tp
            + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 0.0
            + RSS_PRESSURE_WEIGHT * 1.0;

        let obj_both_pressure = -THROUGHPUT_WEIGHT * tp
            + QUEUE_DEPTH_WEIGHT * qd
            + MEMORY_PRESSURE_WEIGHT * 2.0
            + RSS_PRESSURE_WEIGHT * 2.0;

        assert!(
            obj_slab_pressure > obj_no_pressure,
            "Slab pressure should increase objective: {} > {}",
            obj_slab_pressure,
            obj_no_pressure
        );
        assert!(
            obj_rss_pressure > obj_no_pressure,
            "RSS pressure should increase objective: {} > {}",
            obj_rss_pressure,
            obj_no_pressure
        );
        assert!(
            obj_rss_pressure > obj_slab_pressure,
            "RSS pressure should dominate slab pressure: {} > {}",
            obj_rss_pressure,
            obj_slab_pressure
        );
        assert!(
            obj_both_pressure > obj_rss_pressure,
            "Combined pressure should be worst: {} > {}",
            obj_both_pressure,
            obj_rss_pressure
        );
    }

    #[test]
    fn test_monitor_state_has_memory_emas() {
        // Verify that the new EMA fields are initialized properly
        let pool = WorkPool::with_threads(2, 4);
        let state = WorkMonitorState::new(&pool);

        assert!(!state.ema_slab_pressure.is_initialized());
        assert!(!state.ema_rss_pressure.is_initialized());
        assert!(
            state.rss_limit > 0 || cfg!(not(target_os = "linux")),
            "RSS limit should be auto-detected on Linux"
        );
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
        let ramp = |ratio: f64| -> f64 { ((ratio - 0.5) * 4.0).clamp(0.0, 3.0) };

        assert!(
            (ramp(0.25) - 0.0).abs() < f64::EPSILON,
            "Below 50%: no pressure"
        );
        assert!(
            (ramp(0.5) - 0.0).abs() < f64::EPSILON,
            "At 50%: no pressure"
        );
        assert!(
            (ramp(0.75) - 1.0).abs() < f64::EPSILON,
            "At 75%: moderate pressure"
        );
        assert!(
            (ramp(1.0) - 2.0).abs() < f64::EPSILON,
            "At 100%: high pressure"
        );
        assert!(
            (ramp(1.25) - 3.0).abs() < f64::EPSILON,
            "At 125%: capped at 3.0"
        );
        assert!(
            (ramp(2.0) - 3.0).abs() < f64::EPSILON,
            "At 200%: still capped at 3.0"
        );
    }

    /// Verify that tasks queued before workers exist are drained once
    /// workers come online (simulates async init window).
    #[test]
    fn test_async_init_tasks_drain() {
        // Create pool via allocate() — no OS threads yet
        let pool = WorkPool::allocate(2, 4);

        let num_tasks = 20u32;
        let counter = Arc::new(AtomicU32::new(0));

        // Submit tasks while no workers exist — they sit in the queue
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

        assert_eq!(
            pool.queue_len(),
            num_tasks as usize,
            "Tasks should be queued"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0, "No tasks executed yet");

        // Now spawn workers — they should drain all queued tasks
        pool.spawn_all_workers();

        wait_for_count(&counter, num_tasks);
        assert_eq!(counter.load(Ordering::Relaxed), num_tasks);
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

    // ===================================================================
    // with_threads_initial Tests
    // ===================================================================

    #[test]
    fn test_with_threads_initial() {
        let pool = WorkPool::with_threads_initial(1, 8, 2);

        assert_eq!(pool.min_threads(), 1);
        assert_eq!(pool.max_threads(), 8);
        assert_eq!(pool.initial_active(), 2);
        assert_eq!(pool.active_workers(), 2);

        // Verify tasks still execute on the pool
        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);
        pool.spawn_eval(
            move || {
                d.store(true, Ordering::Release);
            },
            TaskTypeId::Generic,
            priority_levels::LOW,
        );
        wait_for_bool(&done);
    }

    #[test]
    fn test_with_threads_initial_clamps_to_min() {
        // initial (0) gets clamped up to min (2)
        let pool = WorkPool::with_threads_initial(2, 8, 0);
        assert_eq!(pool.initial_active(), 2);
        assert_eq!(pool.active_workers(), 2);
    }

    #[test]
    fn test_with_threads_initial_clamps_to_max() {
        // initial (100) gets clamped down to max (4)
        let pool = WorkPool::with_threads_initial(1, 4, 100);
        assert_eq!(pool.initial_active(), 4);
        assert_eq!(pool.active_workers(), 4);
    }

    // ===================================================================
    // Blocked-Worker Detection + Overflow Pool Tests
    // ===================================================================

    #[test]
    fn test_worker_cpu_state_published() {
        // Spawn pool, submit tasks, verify CPU state atomics are non-zero after execution.
        let pool = WorkPool::with_threads(2, 4);

        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);
        pool.spawn_eval(
            move || {
                // Do some work so CPU time advances
                let mut sum = 0u64;
                for i in 0..10_000 {
                    sum = sum.wrapping_add(i);
                }
                std::hint::black_box(sum);
                d.store(true, Ordering::Release);
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        wait_for_bool(&done);

        // Give a moment for the CPU state publish to complete
        thread::sleep(Duration::from_millis(50));

        // At least one worker should have non-zero wall_nanos (from publish_initial
        // at startup or from publish after task execution)
        let cpu_states = pool.worker_cpu_states();
        let any_published = cpu_states
            .iter()
            .any(|s| s.wall_nanos.load(Ordering::Relaxed) > 0);
        assert!(
            any_published,
            "At least one worker should have published CPU state"
        );

        // At least one worker should have task_count > 0
        let any_tasks = cpu_states
            .iter()
            .any(|s| s.task_count.load(Ordering::Relaxed) > 0);
        assert!(any_tasks, "At least one worker should have task_count > 0");
    }

    #[test]
    fn test_blocked_worker_detection() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // Submit a task that sleeps (simulating blocking), verify monitor
        // detects blocked_worker_count > 0.
        let pool = WorkPool::with_threads_initial(2, 2, 2);
        let mut state = WorkMonitorState::new(&pool);

        // Warm up: run one normal tick so snapshots initialize
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);
        work_scaling_monitor_tick(&pool, &mut state);

        // Submit a blocking task
        let blocking = Arc::new(AtomicBool::new(false));
        let b = Arc::clone(&blocking);
        pool.spawn_eval(
            move || {
                b.store(true, Ordering::Release);
                thread::sleep(Duration::from_secs(2));
            },
            TaskTypeId::Generic,
            priority_levels::NORMAL,
        );

        // Wait for the blocking task to start
        wait_for_bool(&blocking);

        // Wait for the monitor interval to pass, then tick
        thread::sleep(Duration::from_millis(250));
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);
        state.detect_blocked_workers(&pool);

        // On Linux: should detect at least one blocked worker (sleeping = low CPU ratio)
        // On non-Linux: heartbeat detection needs 2 ticks to trigger
        #[cfg(target_os = "linux")]
        assert!(
            state.blocked_worker_count > 0,
            "Expected blocked workers detected, got {}",
            state.blocked_worker_count
        );
    }

    #[test]
    fn test_compensatory_unpark() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // Create pool with 2 active, 2 parked. Block active workers with
        // a mutex so detect_blocked_workers sees low CPU utilization (or
        // stalled heartbeat), then verify compensatory logic unparks
        // parked workers to compensate.
        let pool = WorkPool::with_threads_initial(1, 4, 2);
        let mut state = WorkMonitorState::new(&pool);
        assert_eq!(pool.active_workers(), 2);

        // Block both active workers on a mutex
        let blocker = Arc::new(std::sync::Mutex::new(()));
        let _guard = blocker.lock().expect("lock blocker");
        for _ in 0..2 {
            let b = Arc::clone(&blocker);
            pool.spawn_eval(
                move || {
                    // Use poison-tolerant lock: if the test assertion fails and the
                    // test thread panics while holding `_guard`, the mutex becomes
                    // poisoned. Workers must tolerate this to avoid cascade panics.
                    let _lock = b.lock().unwrap_or_else(|e| e.into_inner());
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        // Wait for all workers to publish initial CPU state and pick up blocking tasks.
        // Workers call publish_initial() at startup, setting wall_nanos > 0. Without
        // this, detect_blocked_workers skips workers with curr_wall == 0, and the
        // compensatory logic never fires (macOS thread scheduling can be slow).
        {
            let cpu_states = pool.worker_cpu_states();
            for _ in 0..200 {
                let all_published = cpu_states
                    .iter()
                    .all(|s| s.wall_nanos.load(Ordering::Relaxed) > 0);
                if all_published {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            // Also wait for the blocking tasks to be dequeued
            thread::sleep(Duration::from_millis(100));
        }

        // Submit additional tasks that also block on the same mutex.
        // This ensures tasks stay in the queue (or block if dequeued by a
        // spuriously-unparked worker), keeping queue_len > 0 for the
        // compensatory logic check.
        let counter = Arc::new(AtomicU32::new(0));
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            let b = Arc::clone(&blocker);
            pool.spawn_eval(
                move || {
                    let _lock = b.lock().unwrap_or_else(|e| e.into_inner());
                    c.fetch_add(1, Ordering::Relaxed);
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        // Run multiple ticks so detect_blocked_workers converges.
        // Needs wall_delta >= MIN_WALL_DELTA_NS with low CPU ratio on Linux,
        // or 2+ ticks with 0 task completions on non-Linux heartbeat fallback.
        // Under heavy concurrent test load (3790+ parallel tests), macOS thread
        // scheduling can delay worker startup and task pickup, so we use many
        // ticks with larger artificial time gaps to ensure convergence.
        for _ in 0..40 {
            state.prev_sample_time = Instant::now() - Duration::from_millis(300);
            work_scaling_monitor_tick(&pool, &mut state);
            thread::sleep(Duration::from_millis(50));

            // Early exit: check if compensatory logic already fired
            if pool.active_workers() > 2 || pool.overflow_count() > 0 {
                break;
            }
        }

        // Compensatory logic should have unparked workers (from parked pool)
        // OR spawned overflow to cover the deficit
        let active_after = pool.active_workers();
        let overflow_after = pool.overflow_count();
        assert!(
            active_after > 2 || overflow_after > 0,
            "Compensatory logic should have unparked workers or spawned overflow: \
             active={}, overflow={}, blocked={}, expected active > 2 or overflow > 0",
            active_after,
            overflow_after,
            state.blocked_worker_count
        );

        // Release the blocker so all tasks can complete
        drop(_guard);
        wait_for_count(&counter, 5);
    }

    #[test]
    fn test_overflow_spawn_on_deficit() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // Set max_threads=2, block both workers with a mutex, submit more
        // tasks, verify overflow threads are spawned.
        let pool = WorkPool::with_threads_initial(2, 2, 2);
        let mut state = WorkMonitorState::new(&pool);

        // Block both core workers on a mutex so they appear stalled
        let blocker = Arc::new(std::sync::Mutex::new(()));
        let _guard = blocker.lock().expect("lock blocker");
        for _ in 0..2 {
            let b = Arc::clone(&blocker);
            pool.spawn_eval(
                move || {
                    let _lock = b.lock().expect("lock in worker");
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        // Wait briefly for workers to pick up the blocking tasks
        thread::sleep(Duration::from_millis(50));

        // Submit additional tasks that will queue behind the blocked workers
        let counter = Arc::new(AtomicU32::new(0));
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            pool.spawn_eval(
                move || {
                    c.fetch_add(1, Ordering::Relaxed);
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        // Run multiple ticks to allow detect_blocked_workers to converge.
        // On Linux: needs wall_delta >= MIN_WALL_DELTA_NS (10ms) with low CPU ratio.
        // On non-Linux: needs 2+ ticks with 0 task completions.
        for _ in 0..4 {
            state.prev_sample_time = Instant::now() - Duration::from_millis(200);
            work_scaling_monitor_tick(&pool, &mut state);
            thread::sleep(Duration::from_millis(50));
        }

        // Overflow should have been spawned (or workers unparked) to compensate
        let oc = pool.overflow_count();
        assert!(
            oc > 0,
            "Overflow workers should have been spawned when all core workers are blocked: \
             overflow_count={}, blocked_count={}, active={}",
            oc,
            state.blocked_worker_count,
            pool.active_workers()
        );

        // Release the blocker so all tasks can complete
        drop(_guard);

        // Wait for the counter tasks to complete
        wait_for_count(&counter, 5);
    }

    #[test]
    fn test_overflow_drain_on_unblock() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // After overflow is spawned, simulate unblocking, verify overflow
        // threads are drained.
        let pool = WorkPool::with_threads_initial(2, 2, 2);

        // Manually spawn some overflow
        pool.spawn_overflow(2);
        assert_eq!(pool.overflow_count(), 2);

        let mut state = WorkMonitorState::new(&pool);
        // Simulate: no workers blocked, all are unblocked
        state.blocked_worker_count = 0;

        // Tick with empty queue — should drain all overflow
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);
        work_scaling_monitor_tick(&pool, &mut state);

        // Give overflow workers time to receive drain signal and exit
        thread::sleep(Duration::from_millis(600));
        pool.reap_finished_overflow();

        // All overflow should be drained
        let oc = pool.overflow_count();
        assert_eq!(
            oc, 0,
            "All overflow should be drained when queue is empty: overflow_count={}",
            oc
        );
    }

    #[test]
    fn test_no_overflow_when_queue_empty() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // Block a worker but leave queue empty, verify no overflow threads
        // are spawned.
        let pool = WorkPool::with_threads_initial(2, 2, 2);
        let mut state = WorkMonitorState::new(&pool);

        // Simulate a blocked worker
        state.blocked_worker_count = 1;

        // No tasks submitted — queue is empty

        // Run a tick
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);
        work_scaling_monitor_tick(&pool, &mut state);

        // No overflow should have been spawned
        assert_eq!(
            pool.overflow_count(),
            0,
            "No overflow should be spawned when queue is empty"
        );
    }

    #[test]
    fn test_memory_pressure_vetoes_overflow() {
        let _bp_guard = BP_TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        // Set slab backpressure >= 3, verify no overflow spawned even
        // with blocked workers. The emergency path fires at bp_level >= 3
        // (bp_level == 2 is graduated, feeding slab_amplifier into the
        // hill climber objective, not an emergency bypass).
        let pool = WorkPool::with_threads_initial(2, 4, 2);
        let mut state = WorkMonitorState::new(&pool);

        // Submit tasks so queue is non-empty
        let counter = Arc::new(AtomicU32::new(0));
        for _ in 0..5 {
            let c = Arc::clone(&counter);
            pool.spawn_eval(
                move || {
                    c.fetch_add(1, Ordering::Relaxed);
                },
                TaskTypeId::Generic,
                priority_levels::NORMAL,
            );
        }

        // Simulate blocked workers
        state.blocked_worker_count = 2;

        // Set high backpressure (emergency threshold is bp_level >= 3)
        crate::backend::models::gc_allocator::set_backpressure_level(3);

        // Run a tick — emergency path should fire (park + drain), NOT spawn overflow
        state.prev_sample_time = Instant::now() - Duration::from_millis(200);
        work_scaling_monitor_tick(&pool, &mut state);

        assert_eq!(
            pool.overflow_count(),
            0,
            "No overflow should be spawned during emergency memory pressure"
        );

        // Reset backpressure
        crate::backend::models::gc_allocator::set_backpressure_level(0);

        wait_for_count(&counter, 5);
    }

    #[test]
    fn test_overflow_max_cap() {
        // Verify overflow count is bounded at max_overflow (= max_threads).
        let pool = WorkPool::with_threads_initial(2, 2, 2);

        // Try to spawn more overflow than the cap
        pool.spawn_overflow(2); // max_overflow = 2 for max_threads = 2
        assert_eq!(pool.overflow_count(), 2);

        // spawn_overflow itself enforces the cap, not only the monitor call site.
        pool.spawn_overflow(1);
        assert_eq!(pool.overflow_count(), 2);

        // Clean up: drain all
        pool.drain_all_overflow();
        thread::sleep(Duration::from_millis(600));
        pool.reap_finished_overflow();
    }

    #[test]
    fn test_overflow_spawn_quota_caps_requested() {
        assert_eq!(overflow_spawn_quota(0, 0, 2), 0);
        assert_eq!(overflow_spawn_quota(3, 0, 2), 2);
        assert_eq!(overflow_spawn_quota(3, 1, 2), 1);
        assert_eq!(overflow_spawn_quota(3, 2, 2), 0);
        assert_eq!(overflow_spawn_quota(3, 5, 2), 0);
    }
}
