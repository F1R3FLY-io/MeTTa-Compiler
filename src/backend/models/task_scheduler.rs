//! Generic Lock-Free Reactive State Machine Task Scheduler
//!
//! Extracted from `gc_cron.rs` — a generic, reusable cron-like scheduler built on
//! a lock-free reactive state machine with MPSC channel-based dynamic task scheduling.
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
//! | State machine       | Thread-local, no sharing   | ✅ Yes     |
//! | Task queue          | BinaryHeap (thread-local)  | ✅ Yes     |

use std::cmp::{Ord, Ordering, PartialOrd};
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use parking_lot::Mutex;
use tracing::{error, trace, warn};

use super::work_pool::WorkPool;
use crate::backend::priority_scheduler::{priority_levels, TaskTypeId};

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
// CronDispatchState — Shared state for worker-pool dispatched recurring tasks
// ============================================================================

/// Shared state for a recurring task dispatched to a worker pool.
///
/// Wraps the mutable closure for shared access across worker dispatches and
/// provides an in-flight guard to prevent overlapping executions of the same
/// recurring task.
pub(crate) struct CronDispatchState {
    /// Mutable closure wrapped for shared access across worker dispatches.
    shared_task: Mutex<Box<dyn FnMut() -> bool + Send>>,
    /// In-flight guard: prevents overlapping executions of the same recurring task.
    in_flight: AtomicBool,
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

    /// Shared dispatch state for worker pool execution of recurring tasks.
    /// `None` for first dispatch or one-shot tasks. Set by `dispatch_to_pool()`.
    pub(crate) dispatch_state: Option<Arc<CronDispatchState>>,
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

    /// One-shot ready signal sender (sent at start of run()).
    ready_tx: Option<Sender<()>>,

    /// Optional worker pool for dispatching task execution off the cron thread.
    /// When `Some`, tasks are dispatched to pool workers instead of executing
    /// inline on the scheduler thread.
    worker_pool: Option<Arc<WorkPool>>,
}

impl CronStateMachine {
    /// Default poll interval: 100ms
    pub const DEFAULT_POLL_INTERVAL_MS: u64 = 100;

    /// Create a new CronStateMachine.
    ///
    /// # Arguments
    /// * `task_rx` - Receiver for new tasks from the channel
    /// * `terminating` - Atomic flag for graceful shutdown
    /// * `poll_interval_ms` - Maximum sleep duration between polls
    /// * `ready_tx` - Optional one-shot channel to signal when event loop starts
    /// * `worker_pool` - Optional worker pool for off-thread task execution
    pub fn new(
        task_rx: Receiver<ScheduledTask>,
        terminating: Arc<AtomicBool>,
        poll_interval_ms: u64,
        ready_tx: Option<Sender<()>>,
        worker_pool: Option<Arc<WorkPool>>,
    ) -> Self {
        Self {
            state: CronState::CheckEvents,
            queue: BinaryHeap::new(),
            task_rx,
            poll_interval_ms,
            terminating,
            channel_disconnected: false,
            ready_tx,
            worker_pool,
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
        // Signal ready INSIDE the event loop (not before it starts).
        // This ensures tasks scheduled after receiving the ready signal
        // will be processed by this event loop iteration.
        if let Some(tx) = self.ready_tx.take() {
            let _ = tx.send(());
        }

        // Drive state machine until terminal state.
        // The loop body is wrapped in catch_unwind for resilience:
        // if poll_event() or transition() panics (e.g., from a task's
        // FnMut returning a bad value, or an unexpected state), the
        // scheduler recovers to CheckEvents and continues.
        while self.state != CronState::Terminated {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let event = self.poll_event();
                self.transition(event);
            }));

            if let Err(payload) = result {
                error!(
                    panic = ?payload,
                    "CronStateMachine::run() caught panic -- resetting to CheckEvents"
                );
                self.state = CronState::CheckEvents;
                // Sleep briefly to prevent tight panic loops
                thread::sleep(Duration::from_millis(100));
            }
        }
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
        self.state = match (self.state, event) {
            // === Termination (highest priority, from any state) ===
            (_, CronEvent::TerminationRequested) => {
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
            (CronState::ExecutingTask, CronEvent::TaskCompleted { .. }) => {
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
    ///
    /// When a worker pool is configured, dispatches to the pool; otherwise
    /// executes inline on the cron thread.
    fn execute_one_task(&mut self) {
        let Some(task) = self.queue.pop() else {
            return;
        };

        if let Some(pool) = &self.worker_pool {
            self.dispatch_to_pool(Arc::clone(pool), task);
        } else {
            Self::execute_inline(task, &mut self.queue);
        }
    }

    /// Execute a task inline on the current thread (exception-safe).
    fn execute_inline(mut task: ScheduledTask, queue: &mut BinaryHeap<ScheduledTask>) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (task.task)()));

        match result {
            Ok(true) => {
                // Re-queue if recurring
                if let Some(interval) = task.metadata.recurrence_interval() {
                    task.scheduled_time_ms = now_ms() + interval;
                    queue.push(task);
                }
            }
            Ok(false) => {
                // Task returned false — not re-queuing
            }
            Err(e) => {
                error!(task_name = task.metadata.name(), panic = ?e, "Task panicked");
            }
        }
    }

    /// Dispatch a task to the worker pool for off-thread execution.
    ///
    /// **Recurring tasks**: The `FnMut` closure is wrapped in a shared
    /// `CronDispatchState` (on first dispatch) with an `AtomicBool` in-flight
    /// guard. If the previous execution is still in-flight, the task is
    /// re-queued at `now + interval` without dispatching.
    ///
    /// **One-shot tasks**: Dispatched directly as a `FnOnce` — no shared state.
    fn dispatch_to_pool(&mut self, pool: Arc<WorkPool>, task: ScheduledTask) {
        let interval = task.metadata.recurrence_interval();

        if let Some(interval_ms) = interval {
            // --- Recurring task: use shared dispatch state ---
            let dispatch = task.dispatch_state.unwrap_or_else(|| {
                Arc::new(CronDispatchState {
                    shared_task: Mutex::new(task.task),
                    in_flight: AtomicBool::new(false),
                })
            });

            // Check in-flight guard — skip if still running
            if dispatch
                .in_flight
                .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
                .is_err()
            {
                // Previous execution still running — re-queue without dispatching
                trace!(
                    task_name = task.metadata.name(),
                    "Skipping overlapping recurring task execution"
                );
                let requeued = ScheduledTask {
                    scheduled_time_ms: now_ms() + interval_ms,
                    metadata: task.metadata,
                    task: Box::new(|| true), // Placeholder — real closure is in dispatch_state
                    dispatch_state: Some(Arc::clone(&dispatch)),
                };
                self.queue.push(requeued);
                return;
            }

            // Dispatch to worker pool
            let dispatch_for_worker = Arc::clone(&dispatch);
            let task_name = task.metadata.name().to_string();
            pool.spawn_eval(
                move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let mut guard = dispatch_for_worker.shared_task.lock();
                        (guard)()
                    }));

                    // Clear in-flight flag
                    dispatch_for_worker
                        .in_flight
                        .store(false, AtomicOrdering::Release);

                    match result {
                        Ok(true) => {} // Will be re-queued by cron thread
                        Ok(false) => {
                            trace!(task_name, "Recurring task returned false — stopping");
                        }
                        Err(e) => {
                            error!(task_name, panic = ?e, "Worker pool task panicked");
                        }
                    }
                },
                TaskTypeId::Generic,
                priority_levels::LOW,
            );

            // Re-queue for next interval
            let requeued = ScheduledTask {
                scheduled_time_ms: now_ms() + interval_ms,
                metadata: task.metadata,
                task: Box::new(|| true), // Placeholder — real closure is in dispatch_state
                dispatch_state: Some(dispatch),
            };
            self.queue.push(requeued);
        } else {
            // --- One-shot task: dispatch directly ---
            let mut task_fn = task.task;
            let task_name = task.metadata.name().to_string();
            pool.spawn_eval(
                move || {
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (task_fn)()));
                    match result {
                        Ok(_) => {}
                        Err(e) => {
                            error!(task_name, panic = ?e, "Worker pool one-shot task panicked");
                        }
                    }
                },
                TaskTypeId::Generic,
                priority_levels::LOW,
            );
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
            thread::sleep(Duration::from_millis(sleep_ms));
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
            dispatch_state: None,
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
/// - `Receiver<()>` that signals when the scheduler is ready
pub fn spawn_cron(
    terminating: Arc<AtomicBool>,
) -> (
    CronHandle,
    JoinHandle<()>,
    Receiver<()>,
) {
    spawn_cron_with_interval(terminating, CronStateMachine::DEFAULT_POLL_INTERVAL_MS)
}

/// Spawn with custom poll interval.
///
/// Returns:
/// - `CronHandle` for submitting tasks (clone-able, thread-safe, lock-free)
/// - `JoinHandle` for the cron thread
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
    Receiver<()>,
) {
    spawn_cron_with_interval_and_name(terminating, poll_interval_ms, "mettatron-task-scheduler")
}

/// Spawn with custom poll interval and thread name.
///
/// Returns:
/// - `CronHandle` for submitting tasks (clone-able, thread-safe, lock-free)
/// - `JoinHandle` for the cron thread
/// - `Receiver<()>` that signals when the scheduler is ready
pub fn spawn_cron_with_interval_and_name(
    terminating: Arc<AtomicBool>,
    poll_interval_ms: u64,
    thread_name: &str,
) -> (
    CronHandle,
    JoinHandle<()>,
    Receiver<()>,
) {
    spawn_cron_with_interval_name_and_pool(terminating, poll_interval_ms, thread_name, None)
}

/// Spawn with a worker pool for off-thread task execution.
///
/// The cron thread handles scheduling decisions; actual task execution is
/// dispatched to the worker pool. See `CronStateMachine::dispatch_to_pool`.
///
/// Returns:
/// - `CronHandle` for submitting tasks (clone-able, thread-safe, lock-free)
/// - `JoinHandle` for the cron thread
/// - `Receiver<()>` that signals when the scheduler is ready
pub fn spawn_cron_with_pool(
    terminating: Arc<AtomicBool>,
    poll_interval_ms: u64,
    thread_name: &str,
    worker_pool: Arc<WorkPool>,
) -> (
    CronHandle,
    JoinHandle<()>,
    Receiver<()>,
) {
    spawn_cron_with_interval_name_and_pool(
        terminating,
        poll_interval_ms,
        thread_name,
        Some(worker_pool),
    )
}

/// Internal spawn function accepting all optional parameters.
fn spawn_cron_with_interval_name_and_pool(
    terminating: Arc<AtomicBool>,
    poll_interval_ms: u64,
    thread_name: &str,
    worker_pool: Option<Arc<WorkPool>>,
) -> (
    CronHandle,
    JoinHandle<()>,
    Receiver<()>,
) {
    // Lock-free unbounded MPSC channel for tasks
    let (task_tx, task_rx) = unbounded::<ScheduledTask>();

    // One-shot channel to signal when the scheduler is ready
    let (ready_tx, ready_rx) = unbounded::<()>();

    let terminating_clone = Arc::clone(&terminating);

    let thread_handle = thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            // Pass ready_tx to state machine - signal will be sent inside run()
            let mut sm = CronStateMachine::new(
                task_rx,
                terminating_clone,
                poll_interval_ms,
                Some(ready_tx),
                worker_pool,
            );

            sm.run();
        })
        .expect("Failed to spawn cron state machine thread");

    let handle = CronHandle {
        task_tx,
        terminating,
    };

    (handle, thread_handle, ready_rx)
}

// ============================================================================
// TaskSchedulerSingleton — Generic wrapper for a scheduler + thread
// ============================================================================

/// Generic task scheduler singleton wrapping `CronStateMachine` with lifetime management.
///
/// Immutable after initialization — `CronHandle` is `Clone + Send`,
/// so no `Mutex` is needed for submission.
pub struct TaskSchedulerSingleton {
    /// Handle for dynamic task scheduling (Clone + Send, lock-free).
    pub handle: CronHandle,
    /// Thread join handle (for graceful shutdown).
    thread_handle: Mutex<Option<JoinHandle<()>>>,
    /// Optional worker pool for coordinated shutdown.
    worker_pool: Option<Arc<WorkPool>>,
}

impl TaskSchedulerSingleton {
    /// Create a new singleton from a handle and thread (no worker pool).
    pub fn new(handle: CronHandle, thread_handle: JoinHandle<()>) -> Self {
        Self {
            handle,
            thread_handle: Mutex::new(Some(thread_handle)),
            worker_pool: None,
        }
    }

    /// Create a new singleton with a worker pool for coordinated shutdown.
    ///
    /// Shutdown order: cron thread is joined first (stops new dispatches),
    /// then the worker pool is shut down (drains in-flight tasks).
    pub fn with_pool(
        handle: CronHandle,
        thread_handle: JoinHandle<()>,
        pool: Arc<WorkPool>,
    ) -> Self {
        Self {
            handle,
            thread_handle: Mutex::new(Some(thread_handle)),
            worker_pool: Some(pool),
        }
    }

    /// Request graceful shutdown and join the scheduler thread.
    ///
    /// If a worker pool is attached, shuts it down and joins its workers
    /// after the cron thread exits (ensures no new dispatches occur after
    /// the cron thread is joined, and all in-flight pool tasks complete).
    pub fn shutdown(&self) {
        self.handle.request_shutdown();
        // Join cron thread first — prevents new dispatches to the pool
        let mut guard = self.thread_handle.lock();
        if let Some(handle) = guard.take() {
            let _ = handle.join();
        }
        drop(guard);
        // Then shut down the worker pool and join all workers to drain in-flight tasks
        if let Some(pool) = &self.worker_pool {
            pool.shutdown_and_join();
        }
    }
}

impl Drop for TaskSchedulerSingleton {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BinaryHeap;
    use std::sync::atomic::{AtomicU64 as StdAtomicU64, Ordering};
    use std::time::Instant;

    /// Test that state transitions work correctly.
    #[test]
    fn test_state_transitions() {
        let (_, rx) = unbounded::<ScheduledTask>();
        let terminating = Arc::new(AtomicBool::new(false));

        let sm = CronStateMachine::new(rx, terminating.clone(), 100, None, None);

        // Initial state is CheckEvents
        assert_eq!(sm.current_state(), CronState::CheckEvents);
    }

    /// Test that termination signal works from any state.
    #[test]
    fn test_termination_from_any_state() {
        let (_, rx) = unbounded::<ScheduledTask>();
        let terminating = Arc::new(AtomicBool::new(false));

        let mut sm = CronStateMachine::new(rx, terminating.clone(), 100, None, None);

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
        let (handle, thread, _ready) = spawn_cron(Arc::clone(&terminating));

        let counter = Arc::new(StdAtomicU64::new(0));

        // Submit tasks from multiple threads concurrently (lock-free)
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let h = handle.clone();
                let c = Arc::clone(&counter);
                thread::spawn(move || {
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
        thread::sleep(Duration::from_millis(500));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        // All 1000 tasks should have executed
        assert_eq!(counter.load(Ordering::Relaxed), 1000);
    }

    /// Test that recurring tasks are requeued.
    #[test]
    fn test_recurring_task() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule recurring task every 50ms
        handle.schedule_recurring(0, 50, "counter", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait ~275ms - should execute ~5-6 times
        thread::sleep(Duration::from_millis(275));

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
        let (handle, thread, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule recurring task that stops after 3 executions
        handle.schedule_recurring(0, 20, "limited-counter", move || {
            let count = c.fetch_add(1, Ordering::Relaxed) + 1;
            count < 3 // Return false on 3rd execution
        });

        // Wait longer than needed for all executions
        thread::sleep(Duration::from_millis(200));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 3, "Expected exactly 3 executions, got {}", count);
    }

    /// Test that one-shot tasks execute exactly once.
    #[test]
    fn test_one_shot_task() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Schedule one-shot task
        handle.schedule_once(0, "one-shot", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait for execution
        thread::sleep(Duration::from_millis(100));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Expected exactly 1 execution, got {}", count);
    }

    /// Test that panicking tasks are caught and don't crash the scheduler.
    #[test]
    fn test_panic_safety() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, ready_rx) =
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

        // Poll until normal task executes (with timeout — 2s for CI/slow machines)
        let deadline = Instant::now() + Duration::from_secs(2);
        while counter.load(Ordering::Relaxed) == 0 {
            if Instant::now() > deadline {
                panic!("Timeout waiting for normal task to execute");
            }
            thread::sleep(Duration::from_millis(10));
        }

        handle.request_shutdown();
        thread
            .join()
            .expect("Cron thread should not panic from task panic");

        // Normal task should have executed despite the panic
        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Normal task should have executed");
    }

    /// Test that channel disconnection terminates the scheduler when queue is empty.
    #[test]
    fn test_channel_disconnect_empty_queue() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) =
            spawn_cron_with_interval(Arc::clone(&terminating), 10);

        // Don't schedule any tasks, just drop the handle
        drop(handle);

        // Scheduler should terminate since queue is empty and channel is disconnected
        thread.join().expect("Cron thread panicked");
    }

    /// Test that channel disconnection keeps scheduler running when tasks remain.
    #[test]
    fn test_channel_disconnect_with_tasks() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) =
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
        thread::sleep(Duration::from_millis(200));

        // Scheduler should terminate after task completes
        terminating.store(true, AtomicOrdering::Release);
        thread.join().expect("Cron thread panicked");

        let count = counter.load(Ordering::Relaxed);
        assert_eq!(count, 1, "Delayed task should have executed");
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
            dispatch_state: None,
        });
        heap.push(ScheduledTask {
            scheduled_time_ms: 100,
            metadata: TaskMetadata::OneShot,
            task: Box::new(|| true),
            dispatch_state: None,
        });
        heap.push(ScheduledTask {
            scheduled_time_ms: 200,
            metadata: TaskMetadata::OneShot,
            task: Box::new(|| true),
            dispatch_state: None,
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
        let (handle, thread, ready_rx) = spawn_cron(Arc::clone(&terminating));

        // Wait for the scheduler to be ready before scheduling tasks.
        ready_rx.recv().expect("Cron thread failed to start");

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

        // Poll until both tasks execute (with timeout — 2s for CI/slow machines)
        let deadline = Instant::now() + Duration::from_secs(2);
        while counter.load(Ordering::Relaxed) < 2 {
            if Instant::now() > deadline {
                panic!(
                    "Timeout waiting for tasks: {} of 2 executed",
                    counter.load(Ordering::Relaxed)
                );
            }
            thread::sleep(Duration::from_millis(10));
        }

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");

        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    /// Test shutdown flag propagation.
    #[test]
    fn test_shutdown_flag() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) = spawn_cron(Arc::clone(&terminating));

        // Initially not shutting down
        assert!(!handle.is_shutting_down());

        // Request shutdown
        handle.request_shutdown();

        // Should now be shutting down
        assert!(handle.is_shutting_down());

        thread.join().expect("Cron thread panicked");
    }

    /// Test TaskSchedulerSingleton lifecycle.
    #[test]
    fn test_scheduler_singleton_lifecycle() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread_handle, ready_rx) = spawn_cron(Arc::clone(&terminating));

        ready_rx.recv().expect("Cron thread failed to start");

        let singleton = TaskSchedulerSingleton::new(handle, thread_handle);

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        singleton.handle.schedule_once(0, "test-task", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait for task
        let deadline = Instant::now() + Duration::from_secs(2);
        while counter.load(Ordering::Relaxed) == 0 {
            if Instant::now() > deadline {
                panic!("Timeout waiting for task");
            }
            thread::sleep(Duration::from_millis(10));
        }

        singleton.shutdown();
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    /// Test custom thread naming.
    #[test]
    fn test_custom_thread_name() {
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, _ready) = spawn_cron_with_interval_and_name(
            Arc::clone(&terminating),
            10,
            "my-custom-scheduler",
        );

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");
    }

    // ===================================================================
    // Worker Pool Dispatch Tests
    // ===================================================================

    /// Test that one-shot tasks dispatched to a worker pool execute on pool
    /// threads, not the cron thread.
    #[test]
    fn test_cron_dispatches_to_worker_pool() {
        use super::super::work_pool::WorkPool;

        let pool = Arc::new(WorkPool::with_threads_initial(1, 4, 2));
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, ready_rx) = spawn_cron_with_pool(
            Arc::clone(&terminating),
            10,
            "test-cron-pool",
            Arc::clone(&pool),
        );
        ready_rx.recv().expect("Cron thread failed to start");

        let thread_name = Arc::new(parking_lot::Mutex::new(String::new()));
        let tn = Arc::clone(&thread_name);
        let done = Arc::new(AtomicBool::new(false));
        let d = Arc::clone(&done);

        handle.schedule_once(0, "thread-name-check", move || {
            *tn.lock() = thread::current()
                .name()
                .unwrap_or("unknown")
                .to_string();
            d.store(true, AtomicOrdering::Release);
            true
        });

        // Wait for task to execute
        let deadline = Instant::now() + Duration::from_secs(2);
        while !done.load(AtomicOrdering::Acquire) {
            if Instant::now() > deadline {
                panic!("Timeout waiting for pool-dispatched task");
            }
            thread::sleep(Duration::from_millis(10));
        }

        let name = thread_name.lock().clone();
        assert!(
            name.starts_with("work-pool-"),
            "Expected task on work-pool-* thread, got '{}'",
            name
        );

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");
    }

    /// Test that recurring tasks on the worker pool execute multiple times.
    #[test]
    fn test_recurring_task_on_worker_pool() {
        use super::super::work_pool::WorkPool;

        let pool = Arc::new(WorkPool::with_threads_initial(1, 4, 2));
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, ready_rx) = spawn_cron_with_pool(
            Arc::clone(&terminating),
            10,
            "test-recurring-pool",
            Arc::clone(&pool),
        );
        ready_rx.recv().expect("Cron thread failed to start");

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Recurring task every 50ms
        handle.schedule_recurring(0, 50, "pool-counter", move || {
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait ~300ms — should execute several times
        thread::sleep(Duration::from_millis(300));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");
        pool.shutdown();

        let count = counter.load(Ordering::Relaxed);
        assert!(
            count >= 3,
            "Expected at least 3 recurring executions on pool, got {}",
            count
        );
    }

    /// Test that the in-flight guard prevents overlapping executions of
    /// recurring tasks dispatched to the worker pool.
    #[test]
    fn test_in_flight_guard_prevents_overlap() {
        use super::super::work_pool::WorkPool;

        let pool = Arc::new(WorkPool::with_threads_initial(1, 4, 2));
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, ready_rx) = spawn_cron_with_pool(
            Arc::clone(&terminating),
            10,
            "test-inflight-guard",
            Arc::clone(&pool),
        );
        ready_rx.recv().expect("Cron thread failed to start");

        let counter = Arc::new(StdAtomicU64::new(0));
        let c = Arc::clone(&counter);

        // Task takes 200ms but is scheduled every 10ms — should NOT overlap
        handle.schedule_recurring(0, 10, "slow-task", move || {
            c.fetch_add(1, Ordering::Relaxed);
            thread::sleep(Duration::from_millis(200));
            true
        });

        // Wait 500ms — without the guard we'd see ~50 concurrent executions.
        // With the guard, each 200ms execution blocks the next, so ~2-3 max.
        thread::sleep(Duration::from_millis(500));

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");
        pool.shutdown();

        let count = counter.load(Ordering::Relaxed);
        assert!(
            count <= 4,
            "Expected at most 4 non-overlapping executions, got {} (overlap likely)",
            count
        );
        assert!(
            count >= 1,
            "Expected at least 1 execution, got {}",
            count
        );
    }

    /// Test that the cron thread is not blocked by worker pool tasks.
    /// A long task on the pool should not prevent a short task from completing.
    #[test]
    fn test_cron_thread_not_blocked_by_worker_tasks() {
        use super::super::work_pool::WorkPool;

        let pool = Arc::new(WorkPool::with_threads_initial(2, 4, 2));
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread, ready_rx) = spawn_cron_with_pool(
            Arc::clone(&terminating),
            10,
            "test-not-blocked",
            Arc::clone(&pool),
        );
        ready_rx.recv().expect("Cron thread failed to start");

        let short_done = Arc::new(AtomicBool::new(false));
        let sd = Arc::clone(&short_done);

        // Schedule a long-running task
        handle.schedule_once(0, "long-task", move || {
            thread::sleep(Duration::from_millis(500));
            true
        });

        // Schedule a short task slightly after
        handle.schedule_once(10, "short-task", move || {
            sd.store(true, AtomicOrdering::Release);
            true
        });

        // The short task should complete well before the long task
        let deadline = Instant::now() + Duration::from_millis(200);
        while !short_done.load(AtomicOrdering::Acquire) {
            if Instant::now() > deadline {
                panic!("Short task blocked by long task — cron thread may be blocked");
            }
            thread::sleep(Duration::from_millis(5));
        }

        handle.request_shutdown();
        thread.join().expect("Cron thread panicked");
        pool.shutdown();
    }

    /// Test that shutdown drains the cron worker pool (in-flight tasks complete).
    #[test]
    fn test_shutdown_drains_cron_pool() {
        use super::super::work_pool::WorkPool;

        let pool = Arc::new(WorkPool::with_threads_initial(1, 4, 2));
        let terminating = Arc::new(AtomicBool::new(false));
        let (handle, thread_handle, ready_rx) = spawn_cron_with_pool(
            Arc::clone(&terminating),
            10,
            "test-shutdown-drain",
            Arc::clone(&pool),
        );
        ready_rx.recv().expect("Cron thread failed to start");

        let started = Arc::new(AtomicBool::new(false));
        let counter = Arc::new(StdAtomicU64::new(0));
        let s = Arc::clone(&started);
        let c = Arc::clone(&counter);

        // Schedule a task that signals when it starts and takes a moment to complete
        handle.schedule_once(0, "drain-task", move || {
            s.store(true, AtomicOrdering::Release);
            thread::sleep(Duration::from_millis(100));
            c.fetch_add(1, Ordering::Relaxed);
            true
        });

        // Wait until the task has actually started executing on the pool
        let deadline = Instant::now() + Duration::from_secs(2);
        while !started.load(AtomicOrdering::Acquire) {
            if Instant::now() > deadline {
                panic!("Timeout waiting for drain-task to start");
            }
            thread::sleep(Duration::from_millis(5));
        }

        let singleton = TaskSchedulerSingleton::with_pool(handle, thread_handle, pool);
        singleton.shutdown();

        // Task should have completed before shutdown returned
        assert_eq!(
            counter.load(Ordering::Relaxed),
            1,
            "In-flight task should complete before shutdown returns"
        );
    }
}
