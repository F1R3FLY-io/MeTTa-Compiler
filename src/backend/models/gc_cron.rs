//! Lock-free reactive state machine scheduler for GC cron tasks.
//!
//! Ported from libgrammstein's `CronStateMachine` — a proper lock-free reactive
//! state machine with MPSC channel-based dynamic task scheduling via `CronHandle`.
//!
//! ## Design Goals
//!
//! 1. **Explicit states with clear transitions** - States and events are enumerated
//! 2. **Event-driven architecture** - No polling loops with scattered conditionals
//! 3. **Lock-free task submission** - MPSC channel for concurrent task submission
//! 4. **Thread-local min-heap** - Priority queue owned by the scheduler thread
//! 5. **Graceful shutdown** - Termination signal checked at every state transition
//!
//! ## State Machine Design
//!
//! ```text
//!                               ┌─────────────────────────────────────────┐
//!                               │                                         │
//!                               ▼                                         │
//!     ┌─────────┐  channel has  ┌──────────────┐  task due    ┌──────────────────┐
//!     │  Idle   │ ────────────▶ │ DrainChannel │ ───────────▶ │ ExecutingTask    │
//!     └─────────┘   messages    └──────────────┘              └──────────────────┘
//!          │                          │                              │
//!          │                          │ channel empty                │ task complete
//!          │                          │ & no tasks due               │ (requeue if
//!          │                          ▼                              │  recurring)
//!          │                    ┌──────────────┐                     │
//!          │                    │   Sleeping   │◀────────────────────┘
//!          │                    └──────────────┘
//!          │                          │
//!          │                          │ timer expired
//!          │                          ▼
//!          │                    ┌──────────────┐
//!          └───────────────────▶│ CheckEvents  │◀──── (loop back)
//!                               └──────────────┘
//!                                     │
//!                                     │ terminating == true
//!                                     ▼
//!                               ┌──────────────┐
//!                               │  Terminated  │
//!                               └──────────────┘
//! ```
//!
//! ## Lock-Free Guarantees
//!
//! | Component           | Synchronization            | Lock-Free? |
//! |---------------------|----------------------------|------------|
//! | Task submission     | crossbeam-channel (MPSC)   | ✅ Yes     |
//! | Termination flag    | AtomicBool                 | ✅ Yes     |
//! | Statistics          | AtomicU64                  | ✅ Yes     |
//! | State machine       | Thread-local, no sharing   | ✅ Yes     |
//! | Task queue          | BinaryHeap (thread-local)  | ✅ Yes     |
//!
//! ## GC Integration
//!
//! The `GcCronSingleton` wraps the generic `CronStateMachine` with GC-specific
//! tasks:
//!
//! 1. **Memory monitor** (100ms interval): Reads atomics, computes allocation
//!    rate (allocs/s), calls `request_gc()` if rate exceeds threshold OR
//!    committed bytes exceed `gc_threshold`.
//!
//! 2. **Stats reporter** (5s interval, optional): Logs memory statistics.
//!    Enabled by `METTA_GC_STATS=1` environment variable.

use std::cmp::{Ord, Ordering, PartialOrd};
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use tracing::{debug, error, info, warn};

use super::gc_allocator::request_gc;

// ============================================================================
// GC-specific constants
// ============================================================================

/// Memory monitor poll interval (100ms).
const MONITOR_INTERVAL_MS: u64 = 100;

/// Stats reporter interval (5000ms = 5s).
const STATS_INTERVAL_MS: u64 = 5000;

/// Allocation rate threshold (allocs/s) to set gc_requested.
/// When exceeded, the cron sets the flag so the trampoline's `maybe_gc()`
/// triggers a GC cycle at the next check.
const ALLOC_RATE_THRESHOLD: u64 = 100_000;

// ============================================================================
// Time utilities
// ============================================================================

/// Unix timestamp in milliseconds for scheduling precision.
pub type UnixTimestampMs = u64;

/// Get current Unix timestamp in milliseconds.
#[inline]
pub fn now_ms() -> UnixTimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time went backwards")
        .as_millis() as u64
}

// ============================================================================
// CronState — Reactive State Machine States
// ============================================================================

/// State machine states for the cron scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronState {
    /// Initial state - evaluate event sources.
    CheckEvents,
    /// Draining incoming tasks from the channel.
    DrainChannel,
    /// Executing a task that is due.
    ExecutingTask,
    /// Sleeping until next task or poll interval.
    Sleeping,
    /// Terminal state - graceful shutdown complete.
    Terminated,
}

// ============================================================================
// CronEvent — Events that drive state transitions
// ============================================================================

/// Events that drive state transitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronEvent {
    /// New task(s) available in channel.
    TaskReceived,
    /// Sleep timer expired.
    TimerExpired,
    /// A task in the queue is past its deadline.
    TaskDue,
    /// Task execution completed (with success flag).
    TaskCompleted {
        /// Whether the task returned true (success).
        success: bool,
        /// Whether the task should be requeued (recurring).
        should_requeue: bool,
    },
    /// External termination signal.
    TerminationRequested,
    /// All senders dropped, channel closed.
    ChannelDisconnected,
    /// No events pending (idle).
    NoEvents,
}

// ============================================================================
// TaskMetadata — Metadata for scheduled tasks
// ============================================================================

/// Metadata for a scheduled task.
#[derive(Clone, Debug)]
pub enum TaskMetadata {
    /// One-shot task - executed once and discarded.
    OneShot,

    /// Recurring task - re-queued after completion.
    Recurring {
        /// Interval in milliseconds between executions.
        interval_ms: u64,
    },

    /// Named task with custom metadata.
    Named {
        /// Task name for logging/debugging.
        name: String,
        /// Recurrence interval (None = one-shot).
        recurring_interval_ms: Option<u64>,
    },
}

impl TaskMetadata {
    /// Get the recurrence interval if this task recurs.
    #[inline]
    pub fn recurrence_interval(&self) -> Option<u64> {
        match self {
            TaskMetadata::OneShot => None,
            TaskMetadata::Recurring { interval_ms } => Some(*interval_ms),
            TaskMetadata::Named {
                recurring_interval_ms,
                ..
            } => *recurring_interval_ms,
        }
    }

    /// Get the task name for logging.
    pub fn name(&self) -> &str {
        match self {
            TaskMetadata::OneShot => "one-shot",
            TaskMetadata::Recurring { .. } => "recurring",
            TaskMetadata::Named { name, .. } => name,
        }
    }
}

// ============================================================================
// ScheduledTask — Task entry in the priority queue
// ============================================================================

/// A scheduled task with execution time and callback.
pub struct ScheduledTask {
    /// Unix timestamp (ms) when task should execute.
    pub scheduled_time_ms: UnixTimestampMs,

    /// Task metadata (one-shot, recurring, etc.).
    pub metadata: TaskMetadata,

    /// The task to execute. Returns `true` if successful.
    pub task: Box<dyn FnMut() -> bool + Send>,
}

impl std::fmt::Debug for ScheduledTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledTask")
            .field("scheduled_time_ms", &self.scheduled_time_ms)
            .field("metadata", &self.metadata)
            .field("task", &"<fn>")
            .finish()
    }
}

// Implement ordering for min-heap (earliest timestamp first)
impl Ord for ScheduledTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse for min-heap (BinaryHeap is max-heap by default)
        other.scheduled_time_ms.cmp(&self.scheduled_time_ms)
    }
}

impl PartialOrd for ScheduledTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ScheduledTask {
    fn eq(&self, other: &Self) -> bool {
        self.scheduled_time_ms == other.scheduled_time_ms
    }
}

impl Eq for ScheduledTask {}

// ============================================================================
// CronStats — Lock-free statistics
// ============================================================================

/// Statistics for the cron manager (lock-free).
#[derive(Default)]
pub struct CronStats {
    /// Total tasks executed.
    pub tasks_executed: AtomicU64,
    /// Tasks that returned false (failed).
    pub tasks_failed: AtomicU64,
    /// Tasks that panicked.
    pub tasks_panicked: AtomicU64,
    /// State transitions performed.
    pub transitions: AtomicU64,
}

impl CronStats {
    /// Record a successful task execution.
    #[inline]
    fn record_success(&self) {
        self.tasks_executed.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// Record a failed task execution.
    #[inline]
    fn record_failure(&self) {
        self.tasks_executed.fetch_add(1, AtomicOrdering::Relaxed);
        self.tasks_failed.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// Record a panicked task.
    #[inline]
    fn record_panic(&self) {
        self.tasks_executed.fetch_add(1, AtomicOrdering::Relaxed);
        self.tasks_panicked.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// Record a state transition.
    #[inline]
    fn record_transition(&self) {
        self.transitions.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// Get snapshot of current statistics.
    pub fn snapshot(&self) -> CronStatsSnapshot {
        CronStatsSnapshot {
            tasks_executed: self.tasks_executed.load(AtomicOrdering::Relaxed),
            tasks_failed: self.tasks_failed.load(AtomicOrdering::Relaxed),
            tasks_panicked: self.tasks_panicked.load(AtomicOrdering::Relaxed),
            transitions: self.transitions.load(AtomicOrdering::Relaxed),
        }
    }
}

/// Immutable snapshot of cron statistics.
#[derive(Debug, Clone, Copy)]
pub struct CronStatsSnapshot {
    /// Total tasks executed.
    pub tasks_executed: u64,
    /// Tasks that returned false (failed).
    pub tasks_failed: u64,
    /// Tasks that panicked.
    pub tasks_panicked: u64,
    /// State transitions performed.
    pub transitions: u64,
}

// ============================================================================
// CronStateMachine — Lock-free reactive state machine scheduler
// ============================================================================

/// Lock-free reactive state machine scheduler.
///
/// Explicit states and transitions make control flow clear:
/// - No scattered termination checks
/// - No nested conditionals
/// - Easy to reason about and test
///
/// Uses only lock-free primitives:
/// - AtomicBool for termination
/// - Lock-free MPSC channel for task submission
/// - Thread-local BinaryHeap (no sharing)
pub struct CronStateMachine {
    /// Current state.
    state: CronState,

    /// Task queue (owned by this thread - no sharing).
    queue: BinaryHeap<ScheduledTask>,

    /// Receiver for new tasks (lock-free MPSC).
    task_rx: Receiver<ScheduledTask>,

    /// Poll interval in milliseconds.
    poll_interval_ms: u64,

    /// Termination flag (atomic - no lock).
    terminating: Arc<AtomicBool>,

    /// Channel disconnected flag (set once, never cleared).
    channel_disconnected: bool,

    /// Statistics (atomic counters - no lock).
    stats: Arc<CronStats>,

    /// One-shot ready signal sender (sent at start of run()).
    ready_tx: Option<Sender<()>>,
}

impl CronStateMachine {
    /// Default poll interval: 100ms
    pub const DEFAULT_POLL_INTERVAL_MS: u64 = 100;

    /// Create a new CronStateMachine.
    ///
    /// # Arguments
    /// * `task_rx` - Receiver for new tasks from the channel
    /// * `terminating` - Atomic flag for graceful shutdown
    /// * `stats` - Shared statistics counters
    /// * `poll_interval_ms` - Maximum sleep duration between polls
    /// * `ready_tx` - Optional one-shot channel to signal when event loop starts
    pub fn new(
        task_rx: Receiver<ScheduledTask>,
        terminating: Arc<AtomicBool>,
        stats: Arc<CronStats>,
        poll_interval_ms: u64,
        ready_tx: Option<Sender<()>>,
    ) -> Self {
        Self {
            state: CronState::CheckEvents,
            queue: BinaryHeap::new(),
            task_rx,
            poll_interval_ms,
            terminating,
            channel_disconnected: false,
            stats,
            ready_tx,
        }
    }

    /// Run the state machine until termination (call from dedicated thread).
    ///
    /// This is the main entry point - drives the state machine to completion.
    ///
    /// **Important**: The ready signal is sent at the START of this method,
    /// INSIDE the event loop, ensuring that any tasks scheduled after the
    /// caller receives the ready signal will be processed by this event loop.
    pub fn run(&mut self) {
        info!(
            poll_interval_ms = self.poll_interval_ms,
            "CronStateMachine started (lock-free reactive design)"
        );

        // Signal ready INSIDE the event loop (not before it starts).
        // This ensures tasks scheduled after receiving the ready signal
        // will be processed by this event loop iteration.
        if let Some(tx) = self.ready_tx.take() {
            let _ = tx.send(());
        }

        // Drive state machine until terminal state
        while self.state != CronState::Terminated {
            let event = self.poll_event();
            self.transition(event);
        }

        info!(
            tasks_executed = self.stats.tasks_executed.load(AtomicOrdering::Relaxed),
            tasks_failed = self.stats.tasks_failed.load(AtomicOrdering::Relaxed),
            tasks_panicked = self.stats.tasks_panicked.load(AtomicOrdering::Relaxed),
            transitions = self.stats.transitions.load(AtomicOrdering::Relaxed),
            "CronStateMachine terminated"
        );
    }

    /// Poll for the next event based on current state.
    ///
    /// This is the "sense" phase of the reactive loop.
    ///
    /// **Important**: Due tasks are checked BEFORE termination to ensure
    /// that tasks which are already due get executed even if termination
    /// is requested while the scheduler was sleeping.
    fn poll_event(&mut self) -> CronEvent {
        // Check for due tasks FIRST - they have priority over termination
        // This ensures tasks that became due while sleeping get executed
        if let Some(task) = self.queue.peek() {
            if task.scheduled_time_ms <= now_ms() {
                return CronEvent::TaskDue;
            }
        }

        // Now check termination (atomic load)
        if self.terminating.load(AtomicOrdering::Acquire) {
            return CronEvent::TerminationRequested;
        }

        match self.state {
            CronState::CheckEvents => self.poll_check_events(),
            CronState::DrainChannel => self.poll_drain_channel(),
            CronState::ExecutingTask => unreachable!("ExecutingTask polls internally"),
            CronState::Sleeping => CronEvent::TimerExpired, // Just woke up
            CronState::Terminated => unreachable!("Cannot poll from Terminated"),
        }
    }

    /// Poll events in CheckEvents state.
    fn poll_check_events(&mut self) -> CronEvent {
        // Priority: termination > channel messages > due tasks > sleep
        if self.terminating.load(AtomicOrdering::Acquire) {
            return CronEvent::TerminationRequested;
        }

        // Check channel (non-blocking)
        match self.task_rx.try_recv() {
            Ok(task) => {
                self.queue.push(task);
                return CronEvent::TaskReceived;
            }
            Err(TryRecvError::Disconnected) if !self.channel_disconnected => {
                self.channel_disconnected = true;
                return CronEvent::ChannelDisconnected;
            }
            _ => {}
        }

        // Check for due tasks
        if let Some(task) = self.queue.peek() {
            if task.scheduled_time_ms <= now_ms() {
                return CronEvent::TaskDue;
            }
        }

        // Nothing to do
        CronEvent::NoEvents
    }

    /// Poll in DrainChannel state - continue draining or signal done.
    fn poll_drain_channel(&mut self) -> CronEvent {
        match self.task_rx.try_recv() {
            Ok(task) => {
                self.queue.push(task);
                CronEvent::TaskReceived
            }
            Err(TryRecvError::Empty) => {
                // Done draining, check for due tasks
                if let Some(task) = self.queue.peek() {
                    if task.scheduled_time_ms <= now_ms() {
                        return CronEvent::TaskDue;
                    }
                }
                CronEvent::NoEvents
            }
            Err(TryRecvError::Disconnected) => {
                self.channel_disconnected = true;
                CronEvent::ChannelDisconnected
            }
        }
    }

    /// State transition function: (State, Event) -> State
    ///
    /// This is the core of the reactive design - a pure function
    /// from (state, event) to new state with side effects.
    fn transition(&mut self, event: CronEvent) {
        self.stats.record_transition();
        let old_state = self.state;

        self.state = match (self.state, event) {
            // === Termination (highest priority, from any state) ===
            (_, CronEvent::TerminationRequested) => {
                let _ = old_state; // suppress unused warning
                CronState::Terminated
            }

            // === CheckEvents transitions ===
            (CronState::CheckEvents, CronEvent::TaskReceived) => {
                CronState::DrainChannel
            }
            (CronState::CheckEvents, CronEvent::TaskDue) => {
                self.execute_one_task();
                CronState::CheckEvents // Re-check after execution
            }
            (CronState::CheckEvents, CronEvent::NoEvents) => CronState::Sleeping,
            (CronState::CheckEvents, CronEvent::ChannelDisconnected) => {
                if self.queue.is_empty() {
                    CronState::Terminated
                } else {
                    CronState::CheckEvents
                }
            }

            // === DrainChannel transitions ===
            (CronState::DrainChannel, CronEvent::TaskReceived) => {
                // Stay in drain state until channel is empty
                CronState::DrainChannel
            }
            (CronState::DrainChannel, CronEvent::TaskDue) => {
                self.execute_one_task();
                CronState::CheckEvents
            }
            (CronState::DrainChannel, CronEvent::NoEvents) => CronState::Sleeping,
            (CronState::DrainChannel, CronEvent::ChannelDisconnected) => {
                CronState::CheckEvents
            }

            // === Sleeping transitions ===
            (CronState::Sleeping, CronEvent::TimerExpired) => CronState::CheckEvents,
            (CronState::Sleeping, CronEvent::TaskDue) => {
                // Task became due while sleeping - execute it
                self.execute_one_task();
                CronState::CheckEvents
            }

            // === ExecutingTask transitions ===
            (CronState::ExecutingTask, CronEvent::TaskCompleted { success, .. }) => {
                if success {
                    self.stats.record_success();
                } else {
                    self.stats.record_failure();
                }
                // Requeuing handled in execute_one_task
                CronState::CheckEvents
            }

            // === Invalid transitions ===
            (CronState::Terminated, _) => {
                unreachable!("Cannot transition from Terminated")
            }

            // Catch-all for unexpected combinations
            (state, event) => {
                warn!(?state, ?event, "Unexpected transition");
                CronState::CheckEvents
            }
        };

        // Handle sleeping state entry (side effect)
        if self.state == CronState::Sleeping {
            self.do_sleep();
        }
    }

    /// Execute one due task (exception-safe).
    fn execute_one_task(&mut self) {
        let Some(mut task) = self.queue.pop() else {
            return;
        };

        // Execute with panic catching
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (task.task)()));

        match result {
            Ok(true) => {
                self.stats.record_success();
                // Re-queue if recurring
                if let Some(interval) = task.metadata.recurrence_interval() {
                    task.scheduled_time_ms = now_ms() + interval;
                    self.queue.push(task);
                }
            }
            Ok(false) => {
                self.stats.record_failure();
                debug!(task_name = task.metadata.name(), "Task returned false, not re-queuing");
            }
            Err(e) => {
                self.stats.record_panic();
                error!(task_name = task.metadata.name(), panic = ?e, "Task panicked");
            }
        }
    }

    /// Sleep for the poll interval.
    fn do_sleep(&self) {
        // Calculate sleep duration: min(poll_interval, time_to_next_task)
        let sleep_ms = if let Some(task) = self.queue.peek() {
            let now = now_ms();
            if task.scheduled_time_ms <= now {
                0 // Task already due
            } else {
                (task.scheduled_time_ms - now).min(self.poll_interval_ms)
            }
        } else {
            self.poll_interval_ms
        };

        if sleep_ms > 0 {
            std::thread::sleep(Duration::from_millis(sleep_ms));
        }
    }

    /// Get number of pending tasks.
    pub fn pending_count(&self) -> usize {
        self.queue.len()
    }

    /// Get current state (for testing/debugging).
    pub fn current_state(&self) -> CronState {
        self.state
    }
}

// ============================================================================
// CronHandle — Lock-free task submission handle
// ============================================================================

/// Handle for submitting tasks to a CronStateMachine (thread-safe, lock-free).
///
/// Uses a lock-free MPSC channel - no mutexes or locks.
/// Clone this handle to submit tasks from multiple threads.
#[derive(Clone)]
pub struct CronHandle {
    /// Sender end of lock-free task channel.
    task_tx: Sender<ScheduledTask>,

    /// Reference to termination flag for shutdown coordination.
    terminating: Arc<AtomicBool>,
}

impl CronHandle {
    /// Schedule a task at a specific Unix timestamp (ms).
    ///
    /// This is a lock-free operation using crossbeam-channel.
    /// Returns `true` if the task was submitted, `false` if channel disconnected.
    pub fn schedule_at<F>(&self, time_ms: UnixTimestampMs, metadata: TaskMetadata, task: F) -> bool
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let scheduled_task = ScheduledTask {
            scheduled_time_ms: time_ms,
            metadata,
            task: Box::new(task),
        };
        self.task_tx.send(scheduled_task).is_ok()
    }

    /// Schedule a task after a delay (ms).
    pub fn schedule_after<F>(&self, delay_ms: u64, metadata: TaskMetadata, task: F) -> bool
    where
        F: FnMut() -> bool + Send + 'static,
    {
        self.schedule_at(now_ms() + delay_ms, metadata, task)
    }

    /// Schedule a recurring task.
    ///
    /// The task will execute after `initial_delay_ms` and then every `interval_ms`
    /// as long as it returns `true`. If the task returns `false`, it will not be
    /// rescheduled.
    pub fn schedule_recurring<F>(
        &self,
        initial_delay_ms: u64,
        interval_ms: u64,
        name: &str,
        task: F,
    ) -> bool
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let metadata = TaskMetadata::Named {
            name: name.to_string(),
            recurring_interval_ms: Some(interval_ms),
        };
        self.schedule_after(initial_delay_ms, metadata, task)
    }

    /// Schedule a one-shot task after a delay.
    pub fn schedule_once<F>(&self, delay_ms: u64, name: &str, task: F) -> bool
    where
        F: FnMut() -> bool + Send + 'static,
    {
        let metadata = TaskMetadata::Named {
            name: name.to_string(),
            recurring_interval_ms: None,
        };
        self.schedule_after(delay_ms, metadata, task)
    }

    /// Request graceful shutdown of the state machine.
    ///
    /// This is a lock-free atomic store.
    pub fn request_shutdown(&self) {
        self.terminating.store(true, AtomicOrdering::Release);
    }

    /// Check if shutdown has been requested.
    pub fn is_shutting_down(&self) -> bool {
        self.terminating.load(AtomicOrdering::Acquire)
    }
}

// ============================================================================
// spawn_cron / spawn_cron_with_interval — Generic spawning
// ============================================================================

/// Spawn the cron state machine on a dedicated thread.
///
/// Returns:
/// - `CronHandle` for submitting tasks (clone-able, thread-safe, lock-free)
/// - `JoinHandle` for the cron thread
/// - `Arc<CronStats>` for reading statistics (lock-free)
/// - `Receiver<()>` that signals when the scheduler is ready
pub fn spawn_cron(
    terminating: Arc<AtomicBool>,
) -> (
    CronHandle,
    JoinHandle<()>,
    Arc<CronStats>,
    Receiver<()>,
) {
    spawn_cron_with_interval(terminating, CronStateMachine::DEFAULT_POLL_INTERVAL_MS)
}

/// Spawn with custom poll interval.
///
/// Returns:
/// - `CronHandle` for submitting tasks (clone-able, thread-safe, lock-free)
/// - `JoinHandle` for the cron thread
/// - `Arc<CronStats>` for reading statistics (lock-free)
/// - `Receiver<()>` that signals when the scheduler is ready
///
/// # Ready Signal
///
/// The returned `Receiver<()>` provides a one-shot signal that the scheduler is ready.
/// Call `ready_rx.recv()` to block until the cron thread has entered its event loop.
/// This prevents race conditions where tasks are scheduled before the scheduler is ready.
pub fn spawn_cron_with_interval(
    terminating: Arc<AtomicBool>,
    poll_interval_ms: u64,
) -> (
    CronHandle,
    JoinHandle<()>,
    Arc<CronStats>,
    Receiver<()>,
) {
    // Lock-free unbounded MPSC channel for tasks
    let (task_tx, task_rx) = unbounded::<ScheduledTask>();

    // One-shot channel to signal when the scheduler is ready
    let (ready_tx, ready_rx) = unbounded::<()>();

    let stats = Arc::new(CronStats::default());
    let stats_clone = Arc::clone(&stats);
    let terminating_clone = Arc::clone(&terminating);

    let thread_handle = std::thread::Builder::new()
        .name("mettatron-gc-cron".to_string())
        .spawn(move || {
            // Pass ready_tx to state machine - signal will be sent inside run()
            let mut sm = CronStateMachine::new(
                task_rx,
                terminating_clone,
                stats_clone,
                poll_interval_ms,
                Some(ready_tx),
            );

            sm.run();
        })
        .expect("Failed to spawn cron state machine thread");

    let handle = CronHandle {
        task_tx,
        terminating,
    };

    (handle, thread_handle, stats, ready_rx)
}

// ============================================================================
// MonitorState — Per-Poll Tracking for Memory Monitor
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
// GcCronSingleton — GC-specific wrapper
// ============================================================================

/// GC cron manager singleton wrapping `CronStateMachine` with GC-specific tasks.
///
/// Immutable after initialization — `CronHandle` is `Clone + Send` and
/// `CronStats` uses atomics, so no `Mutex` is needed.
pub struct GcCronSingleton {
    /// Handle for dynamic task scheduling (Clone + Send, lock-free).
    pub handle: CronHandle,
    /// Statistics (lock-free atomic counters).
    pub stats: Arc<CronStats>,
    /// Thread join handle (for graceful shutdown).
    thread_handle: Mutex<Option<JoinHandle<()>>>,
}

impl GcCronSingleton {
    /// Request graceful shutdown and join the cron thread.
    pub fn shutdown(&self) {
        self.handle.request_shutdown();
        if let Ok(mut guard) = self.thread_handle.lock() {
            if let Some(handle) = guard.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for GcCronSingleton {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// spawn_gc_cron — GC-specific entry point
// ============================================================================

/// Spawn the GC cron manager on a dedicated thread with pre-configured tasks.
///
/// This function:
/// 1. Spawns a `CronStateMachine` via `spawn_cron()`
/// 2. Waits for the ready signal to ensure the event loop is running
/// 3. Schedules the **memory monitor** task (100ms recurring)
/// 4. Optionally schedules the **stats reporter** task (5s recurring, `METTA_GC_STATS=1`)
/// 5. Returns a `GcCronSingleton` for lifetime management
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
    let terminating = Arc::new(AtomicBool::new(false));
    let (handle, thread_handle, stats, ready_rx) = spawn_cron(Arc::clone(&terminating));

    // Wait for the event loop to start before scheduling tasks.
    // This prevents a race where tasks are submitted before the receiver is live.
    ready_rx.recv().expect("Cron thread failed to start");

    // Schedule memory monitor (recurring, 100ms)
    let committed_clone = Arc::clone(&committed_bytes);
    let alloc_clone = Arc::clone(&alloc_count);
    let threshold_clone = Arc::clone(&gc_threshold);
    let mut monitor = MonitorState::new();

    // Initial delay matches the interval so the first poll has a meaningful
    // baseline (prev_alloc_count / prev_poll_time are set at construction).
    handle.schedule_recurring(MONITOR_INTERVAL_MS, MONITOR_INTERVAL_MS, "memory-monitor", move || {
        execute_memory_monitor(&committed_clone, &alloc_clone, &threshold_clone, &mut monitor);
        true // always reschedule
    });

    // Schedule stats reporter (recurring, 5s) if METTA_GC_STATS=1
    if std::env::var("METTA_GC_STATS").is_ok() {
        let committed_clone2 = Arc::clone(&committed_bytes);
        let alloc_clone2 = Arc::clone(&alloc_count);

        handle.schedule_recurring(STATS_INTERVAL_MS, STATS_INTERVAL_MS, "stats-reporter", move || {
            execute_stats_reporter(&committed_clone2, &alloc_clone2);
            true
        });
    }

    GcCronSingleton {
        handle,
        stats,
        thread_handle: Mutex::new(Some(thread_handle)),
    }
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

    // Back-pressure computation: throttle allocation when GC can't keep up
    let bp_level = if threshold > 0 {
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
    super::gc_allocator::set_backpressure_level(bp_level);

    if should_gc {
        request_gc();
    }

    monitor.prev_alloc_count = current_alloc_count;
    monitor.prev_poll_time = now;
}

/// Stats reporter task: logs memory statistics to stderr.
fn execute_stats_reporter(
    committed_bytes: &AtomicUsize,
    alloc_count: &AtomicU64,
) {
    let committed = committed_bytes.load(AtomicOrdering::Relaxed);
    let allocs = alloc_count.load(AtomicOrdering::Relaxed);

    info!(
        committed_mb = format_args!("{:.1}", committed as f64 / (1024.0 * 1024.0)),
        total_allocs = allocs,
        "GC cron stats"
    );
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::gc_allocator::is_gc_requested;
    use std::sync::atomic::{AtomicU64 as StdAtomicU64, Ordering};

    // ========================================================================
    // Ported from libgrammstein CronStateMachine tests
    // ========================================================================

    /// Test that state transitions work correctly.
    #[test]
    fn test_state_transitions() {
        let (_, rx) = unbounded::<ScheduledTask>();
        let terminating = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(CronStats::default());

        let sm = CronStateMachine::new(rx, terminating.clone(), stats.clone(), 100, None);

        // Initial state is CheckEvents
        assert_eq!(sm.current_state(), CronState::CheckEvents);
    }

    /// Test that termination signal works from any state.
    #[test]
    fn test_termination_from_any_state() {
        let (_, rx) = unbounded::<ScheduledTask>();
        let terminating = Arc::new(AtomicBool::new(false));
        let stats = Arc::new(CronStats::default());

        let mut sm = CronStateMachine::new(rx, terminating.clone(), stats.clone(), 100, None);

        // Request termination
        terminating.store(true, AtomicOrdering::Release);

        // Should immediately transition to Terminated
        sm.run();
        assert_eq!(sm.current_state(), CronState::Terminated);
    }

    /// Test concurrent task submission from multiple threads.
    #[test]
    fn test_concurrent_task_submission() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, stats, _ready) = spawn_cron(Arc::clone(&terminating));

        let counter = Arc::new(StdAtomicU64::new(0));

        // Submit tasks from multiple threads concurrently (lock-free)
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let h = handle.clone();
                let c = Arc::clone(&counter);
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        h.schedule_after(
                            0,
                            TaskMetadata::OneShot,
                            {
                                let c = Arc::clone(&c);
                                move || {
                                    c.fetch_add(1, Ordering::Relaxed);
                                    true
                                }
                            },
                        );
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("Thread panicked");
        }

        // Wait for tasks to execute
        std::thread::sleep(Duration::from_millis(500));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        // All 1000 tasks should have executed
        assert_eq!(counter.load(Ordering::Relaxed), 1000);
        assert_eq!(stats.tasks_executed.load(Ordering::Relaxed), 1000);

        // State machine should have performed many transitions
        assert!(stats.transitions.load(Ordering::Relaxed) > 0);
    }

    /// Test that recurring tasks are requeued.
    #[test]
    fn test_recurring_task() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule recurring task every 50ms
        handle.schedule_recurring(0, 50, "counter", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait ~275ms - should execute ~5-6 times
        std::thread::sleep(Duration::from_millis(275));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert!(
            count >= 4 && count <= 7,
            "Expected 4-7 executions, got {}",
            count
        );
    }

    /// Test that recurring tasks stop when returning false.
    #[test]
    fn test_recurring_task_stops_on_false() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule recurring task that stops after 3 executions
        handle.schedule_recurring(0, 20, "limited-counter", move || {
            let count = c.fetch_add(1, Ordering::Relaxed) + 1;
            count < 3 // Return false on 3rd execution
        });

        // Wait longer than needed for all executions
        std::thread::sleep(Duration::from_millis(200));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 3, "Expected exactly 3 executions, got {}", count);
    }

    /// Test that one-shot tasks execute exactly once.
    #[test]
    fn test_one_shot_task() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule one-shot task
        handle.schedule_once(0, "one-shot", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait for execution
        std::thread::sleep(Duration::from_millis(100));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Expected exactly 1 execution, got {}", count);
    }

    /// Test that panicking tasks are caught and don't crash the scheduler.
    #[test]
    fn test_panic_safety() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, stats, ready_rx) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        // Wait for scheduler to be ready (prevents race condition where tasks are
        // scheduled before the cron thread has entered its event loop)
        ready_rx.recv().expect("Cron thread failed to start");

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule a task that panics
        handle.schedule_once(0, "panicking", || {
            panic!("This task intentionally panics");
        });

        // Schedule a normal task after the panicking one
        handle.schedule_once(50, "normal", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Poll until normal task executes (with timeout)
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        while counter.load(Ordering::Relaxed) == 0 {
            if std::time::Instant::now() > deadline {
                panic!("Timeout waiting for normal task to execute");
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        handle.request_shutdown();
        thread
            .join()
            .expect("Cron thread should not panic from task panic");

        // Normal task should have executed despite the panic
        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Normal task should have executed");

        // Stats should show one panic
        assert_eq!(stats.tasks_panicked.load(Ordering::Relaxed), 1);
    }

    /// Test that channel disconnection terminates the scheduler when queue is empty.
    #[test]
    fn test_channel_disconnect_empty_queue() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        // Don't schedule any tasks, just drop the handle
        drop(handle);

        // Scheduler should terminate since queue is empty and channel is disconnected
        thread.join().expect("Cron thread panicked");

        // Should have at least one transition (to check events, then to terminated)
        assert!(stats.transitions.load(Ordering::Relaxed) >= 1);
    }

    /// Test that channel disconnection keeps scheduler running when tasks remain.
    #[test]
    fn test_channel_disconnect_with_tasks() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule a delayed task
        handle.schedule_once(100, "delayed", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Drop handle immediately (disconnects channel)
        drop(handle);

        // Wait for the delayed task to execute
        std::thread::sleep(Duration::from_millis(200));

        // Scheduler should terminate after task completes
        terminating.store(true, AtomicOrdering::Release);
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Delayed task should have executed");
    }

    /// Test statistics snapshot.
    #[test]
    fn test_stats_snapshot() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, stats, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        // Schedule tasks
        for _ in 0..5 {
            handle.schedule_once(0, "success", || true);
        }
        for _ in 0..3 {
            handle.schedule_once(0, "failure", || false);
        }

        // Wait for execution
        std::thread::sleep(Duration::from_millis(100));

        // Get snapshot
        let snapshot = stats.snapshot();

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        // Verify snapshot
        assert_eq!(snapshot.tasks_executed, 8);
        assert_eq!(snapshot.tasks_failed, 3);
        assert_eq!(snapshot.tasks_panicked, 0);
    }

    /// Test task metadata types.
    #[test]
    fn test_task_metadata() {
        // OneShot
        let one_shot = TaskMetadata::OneShot;
        assert_eq!(one_shot.name(), "one-shot");
        assert_eq!(one_shot.recurrence_interval(), None);

        // Recurring
        let recurring = TaskMetadata::Recurring { interval_ms: 1000 };
        assert_eq!(recurring.name(), "recurring");
        assert_eq!(recurring.recurrence_interval(), Some(1000));

        // Named one-shot
        let named = TaskMetadata::Named {
            name: "custom".to_string(),
            recurring_interval_ms: None,
        };
        assert_eq!(named.name(), "custom");
        assert_eq!(named.recurrence_interval(), None);

        // Named recurring
        let named_recurring = TaskMetadata::Named {
            name: "checkpoint".to_string(),
            recurring_interval_ms: Some(5000),
        };
        assert_eq!(named_recurring.name(), "checkpoint");
        assert_eq!(named_recurring.recurrence_interval(), Some(5000));
    }

    /// Test scheduled task ordering (min-heap behavior).
    #[test]
    fn test_task_ordering() {
        let mut heap = BinaryHeap::new();

        // Add tasks in random order
        heap.push(ScheduledTask {
            scheduled_time_ms: 300,
            metadata: TaskMetadata::OneShot,
            task: Box::new(|| true),
        });
        heap.push(ScheduledTask {
            scheduled_time_ms: 100,
            metadata: TaskMetadata::OneShot,
            task: Box::new(|| true),
        });
        heap.push(ScheduledTask {
            scheduled_time_ms: 200,
            metadata: TaskMetadata::OneShot,
            task: Box::new(|| true),
        });

        // Should pop in ascending order (earliest first)
        assert_eq!(heap.pop().expect("should have task").scheduled_time_ms, 100);
        assert_eq!(heap.pop().expect("should have task").scheduled_time_ms, 200);
        assert_eq!(heap.pop().expect("should have task").scheduled_time_ms, 300);
    }

    /// Test handle cloning and concurrent use.
    #[test]
    fn test_handle_cloning() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) = spawn_cron(Arc::clone(&terminating));

        let counter = Arc::new(StdAtomicU64::new(0));

        // Clone handle multiple times
        let handle1 = handle.clone();
        let handle2 = handle.clone();

        let c1 = Arc::clone(&counter);
        let c2 = Arc::clone(&counter);

        // Submit from different handles
        handle1.schedule_once(0, "from-handle1", move || {
            c1.fetch_add(1, Ordering::Relaxed);
            true
        });
        handle2.schedule_once(0, "from-handle2", move || {
            c2.fetch_add(1, Ordering::Relaxed);
            true
        });

        std::thread::sleep(Duration::from_millis(100));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    /// Test shutdown flag propagation.
    #[test]
    fn test_shutdown_flag() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _stats, _ready) = spawn_cron(Arc::clone(&terminating));

        // Initially not shutting down
        assert!(!handle.is_shutting_down());

        // Request shutdown
        handle.request_shutdown();

        // Should now be shutting down
        assert!(handle.is_shutting_down());

        thread.join().expect("Cron thread panicked");
    }

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
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        let mut observed = false;
        while std::time::Instant::now() < deadline {
            if is_gc_requested() {
                observed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
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
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        // Simulate time passing and allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(50_000, Ordering::Relaxed);

        // Second poll: rate = 50k / 0.1s = 500k/s > threshold
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        assert!(is_gc_requested(), "should request GC at 500k allocs/s");
    }

    /// Unit test: low rate does NOT trigger GC.
    #[test]
    fn test_monitor_state_below_threshold() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(0);
        let gc_threshold = AtomicUsize::new(1024 * 1024 * 1024); // 1 GB — won't trigger
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        // Simulate time passing and low allocations
        monitor.prev_poll_time = Instant::now() - Duration::from_millis(100);
        alloc_count.store(100, Ordering::Relaxed); // 100 / 0.1s = 1000/s < 100k threshold

        // Second poll
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        assert!(
            !is_gc_requested(),
            "should NOT request GC at 1000 allocs/s"
        );
    }

    /// Unit test: committed bytes >= threshold triggers GC.
    #[test]
    fn test_threshold_based_trigger() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(8 * 1024 * 1024); // 8 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        // committed (8 MB) >= gc_threshold (4 MB) should trigger
        assert!(
            is_gc_requested(),
            "should request GC when committed bytes exceed threshold"
        );
    }

    /// Unit test: committed bytes < threshold does NOT trigger GC.
    #[test]
    fn test_threshold_not_triggered_below() {
        // Clear any prior GC request from other tests
        super::super::gc_allocator::GC_REQUESTED.store(false, Ordering::Relaxed);

        let alloc_count = AtomicU64::new(0);
        let committed = AtomicUsize::new(2 * 1024 * 1024); // 2 MB committed
        let gc_threshold = AtomicUsize::new(4 * 1024 * 1024); // 4 MB threshold
        let mut monitor = MonitorState::new();

        // First poll: set baseline
        execute_memory_monitor(&committed, &alloc_count, &gc_threshold, &mut monitor);

        // committed (2 MB) < gc_threshold (4 MB) should NOT trigger
        assert!(
            !is_gc_requested(),
            "should NOT request GC when committed bytes below threshold"
        );
    }
}
