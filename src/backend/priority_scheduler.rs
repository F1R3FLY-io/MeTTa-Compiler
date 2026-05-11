//! Priority Scheduler with P² Runtime Estimation
//!
//! This module provides a priority-based thread pool scheduler featuring:
//! - Min-heap priority queue (lower score = higher priority)
//! - P² algorithm for task runtime estimation (running median)
//! - Time decay to prevent starvation of low-priority tasks
//!
//! # Priority Score Formula
//!
//! ```text
//! score = base_priority + (estimated_runtime * runtime_weight) - (age * decay_rate)
//! ```
//!
//! Where:
//! - `base_priority`: User-specified (0 = highest priority)
//! - `estimated_runtime`: P² median of past task runtimes for similar tasks
//! - `age`: Time since task was enqueued (seconds)
//! - Lower score = scheduled first (min-heap)

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use dashmap::DashMap;
use parking_lot::{Condvar, Mutex};
use xxhash_rust::xxh3::Xxh3;

use crate::backend::bytecode::cache::hash_metta_value;
use crate::backend::models::MettaValue;

// ============================================================================
// P² Median Estimator
// ============================================================================

/// P² algorithm for dynamic median estimation without storing observations.
///
/// Reference: Jain, R. and Chlamtac, I. "The P² algorithm for dynamic
/// calculation of quantiles and histograms without storing observations."
/// Communications of the ACM 28, no. 10 (1985): 1076-1085.
///
/// Properties:
/// - O(1) space: only 5 markers stored
/// - O(1) time per observation
/// - Accurate for most distributions
#[derive(Debug, Clone)]
pub struct P2MedianEstimator {
    /// Marker heights (q_i): actual quantile value estimates
    /// q[0] = min, q[2] = median estimate, q[4] = max
    heights: [f64; 5],

    /// Marker positions (n_i): integer indices in sorted observation sequence
    positions: [i32; 5],

    /// Desired positions (n'_i): target positions as real values
    desired_positions: [f64; 5],

    /// Position increments (dn'_i): values added after each observation
    /// For median (p=0.5): [0.0, 0.25, 0.5, 0.75, 1.0]
    increments: [f64; 5],

    /// Number of observations seen
    count: u32,
}

impl Default for P2MedianEstimator {
    fn default() -> Self {
        Self::new()
    }
}

impl P2MedianEstimator {
    /// Create a new P² estimator for median (p=0.5)
    pub fn new() -> Self {
        Self {
            heights: [0.0; 5],
            positions: [1, 2, 3, 4, 5],
            desired_positions: [1.0, 2.0, 3.0, 4.0, 5.0],
            increments: [0.0, 0.25, 0.5, 0.75, 1.0], // For p=0.5 median
            count: 0,
        }
    }

    /// Add a new observation (e.g., runtime in nanoseconds)
    /// O(1) time complexity
    pub fn add_observation(&mut self, x: f64) {
        if self.count < 5 {
            // Initialization: store first 5 observations
            self.heights[self.count as usize] = x;
            self.count += 1;
            if self.count == 5 {
                // Sort initial observations to establish markers
                self.heights
                    .sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
            }
            return;
        }

        // Find marker bracket k where q[k] <= x < q[k+1]
        let k = self.find_bracket(x);

        // Update extreme markers
        if x < self.heights[0] {
            self.heights[0] = x;
        } else if x > self.heights[4] {
            self.heights[4] = x;
        }

        // Increment positions for markers above k
        for i in (k + 1)..5 {
            self.positions[i] += 1;
        }

        // Update desired positions
        for i in 0..5 {
            self.desired_positions[i] += self.increments[i];
        }

        // Adjust middle markers (1, 2, 3) using P² formula
        for i in 1..4 {
            let d = self.desired_positions[i] - self.positions[i] as f64;
            if (d >= 1.0 && self.positions[i + 1] - self.positions[i] > 1)
                || (d <= -1.0 && self.positions[i - 1] - self.positions[i] < -1)
            {
                let d_sign = if d >= 0.0 { 1 } else { -1 };
                let q_new = self.parabolic_adjustment(i, d_sign);

                // Check bounds: q[i-1] < q_new < q[i+1]
                if self.heights[i - 1] < q_new && q_new < self.heights[i + 1] {
                    self.heights[i] = q_new;
                } else {
                    // Fallback to linear interpolation
                    self.heights[i] = self.linear_adjustment(i, d_sign);
                }
                self.positions[i] += d_sign;
            }
        }

        self.count += 1;
    }

    /// Piecewise-parabolic (P²) adjustment formula
    fn parabolic_adjustment(&self, i: usize, d: i32) -> f64 {
        let n_i = self.positions[i] as f64;
        let n_im1 = self.positions[i - 1] as f64;
        let n_ip1 = self.positions[i + 1] as f64;
        let q_i = self.heights[i];
        let q_im1 = self.heights[i - 1];
        let q_ip1 = self.heights[i + 1];
        let d = d as f64;

        q_i + d / (n_ip1 - n_im1)
            * ((n_i - n_im1 + d) * (q_ip1 - q_i) / (n_ip1 - n_i)
                + (n_ip1 - n_i - d) * (q_i - q_im1) / (n_i - n_im1))
    }

    /// Linear adjustment fallback
    fn linear_adjustment(&self, i: usize, d: i32) -> f64 {
        let idx = if d >= 0 { i + 1 } else { i - 1 };
        let n_i = self.positions[i] as f64;
        let n_other = self.positions[idx] as f64;
        let q_i = self.heights[i];
        let q_other = self.heights[idx];

        q_i + d as f64 * (q_other - q_i) / (n_other - n_i)
    }

    /// Find bracket index k where heights[k] <= x < heights[k+1]
    fn find_bracket(&self, x: f64) -> usize {
        for k in 0..4 {
            if x < self.heights[k + 1] {
                return k;
            }
        }
        3 // x >= heights[4]
    }

    /// Get the current median estimate
    #[inline]
    pub fn median(&self) -> f64 {
        if self.count < 5 {
            // Not enough data, return simple average or 0
            if self.count == 0 {
                return 0.0;
            }
            self.heights[..self.count as usize].iter().sum::<f64>() / self.count as f64
        } else {
            self.heights[2] // Median marker
        }
    }

    /// Get observation count
    #[inline]
    pub fn count(&self) -> u32 {
        self.count
    }
}

// ============================================================================
// Task Type Classification
// ============================================================================

/// Task type identifier for runtime tracking.
///
/// Used to group similar tasks for P² runtime estimation.
/// Different task types have different runtime characteristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskTypeId {
    /// Evaluation task with expression hash for similarity grouping
    Eval(u64),
    /// Bytecode compilation task
    BytecodeCompile,
    /// JIT compilation task (stage 1 or 2)
    JitCompile,
    /// Generic/unclassified task
    Generic,
}

impl TaskTypeId {
    /// Create task type ID from a MettaValue expression
    pub fn from_expr(expr: &MettaValue) -> Self {
        TaskTypeId::Eval(hash_metta_value(expr))
    }

    /// Create a generic task type
    pub fn generic() -> Self {
        TaskTypeId::Generic
    }
}

// ============================================================================
// Runtime Tracker
// ============================================================================

/// Global runtime tracker for all task types.
///
/// Uses DashMap for lock-free concurrent access to per-task-type estimators.
pub struct RuntimeTracker {
    /// Per-task-type P² estimators
    /// Key: TaskTypeId hash, Value: P² estimator wrapped in Mutex
    estimators: DashMap<u64, Mutex<P2MedianEstimator>>,

    /// Global P² estimator for fallback (when task type has insufficient data)
    global_estimator: Mutex<P2MedianEstimator>,
}

impl Default for RuntimeTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeTracker {
    pub fn new() -> Self {
        Self {
            estimators: DashMap::new(),
            global_estimator: Mutex::new(P2MedianEstimator::new()),
        }
    }

    /// Record a task runtime observation
    pub fn record_runtime(&self, task_type: TaskTypeId, runtime_nanos: u64) {
        let runtime = runtime_nanos as f64;

        // Update per-task-type estimator
        let hash = self.hash_task_type(&task_type);
        self.estimators
            .entry(hash)
            .or_insert_with(|| Mutex::new(P2MedianEstimator::new()))
            .lock()
            .add_observation(runtime);

        // Also update global estimator
        self.global_estimator.lock().add_observation(runtime);
    }

    /// Get estimated runtime for a task type
    pub fn estimated_runtime(&self, task_type: TaskTypeId) -> f64 {
        let hash = self.hash_task_type(&task_type);

        if let Some(estimator) = self.estimators.get(&hash) {
            let est = estimator.lock();
            if est.count() >= 5 {
                return est.median();
            }
        }

        // Fallback to global median
        self.global_estimator.lock().median()
    }

    /// Get the global median estimate
    pub fn global_median(&self) -> f64 {
        self.global_estimator.lock().median()
    }

    fn hash_task_type(&self, task_type: &TaskTypeId) -> u64 {
        let mut h = Xxh3::new();
        task_type.hash(&mut h);
        h.finish()
    }
}

// ============================================================================
// Scheduler Configuration
// ============================================================================

/// Configuration for the priority scheduler.
#[derive(Debug, Clone, Copy)]
pub struct SchedulerConfig {
    /// Weight applied to estimated runtime in priority calculation.
    /// Higher values prioritize shorter tasks (SJF-like behavior).
    /// Default: 1.0
    pub runtime_weight: f64,

    /// Rate at which priority decays over time (per second).
    /// Higher values prioritize older tasks (prevents starvation).
    /// Default: 0.1 (priority decreases by 0.1 per second of waiting)
    pub decay_rate: f64,

    /// Maximum number of tasks in the priority queue before backpressure.
    /// Default: num_threads * 16
    pub max_queue_size: usize,
}

/// Get thread count from METTATRON_NUM_THREADS env var, falling back to num_cpus::get().
fn get_configured_thread_count() -> usize {
    std::env::var("METTATRON_NUM_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(num_cpus::get)
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        let num_cpus = get_configured_thread_count();
        Self {
            runtime_weight: 1.0,
            decay_rate: 0.1,
            max_queue_size: num_cpus * 16,
        }
    }
}

// ============================================================================
// Priority Levels
// ============================================================================

/// Predefined priority levels for common task types.
pub mod priority_levels {
    /// Interactive/real-time evaluation (highest priority)
    pub const INTERACTIVE: u32 = 0;

    /// Normal evaluation tasks
    pub const NORMAL: u32 = 5;

    /// Background compilation (bytecode/JIT)
    pub const BACKGROUND_COMPILE: u32 = 10;

    /// Low-priority background tasks
    pub const LOW: u32 = 20;

    /// Lowest priority (batch processing, cleanup)
    pub const BATCH: u32 = 50;
}

// ============================================================================
// Priority Task
// ============================================================================

/// A task wrapper that carries priority information.
pub struct PriorityTask {
    /// The actual task closure to execute
    task: Box<dyn FnOnce() + Send + 'static>,

    /// User-specified base priority (0 = highest)
    base_priority: u32,

    /// Task type for runtime estimation
    task_type: TaskTypeId,

    /// Timestamp when task was enqueued (for age-based decay)
    enqueued_at: Instant,

    /// Unique sequence number for stable ordering
    sequence: u64,

    /// WFST-assigned cost class (if the expression was classified).
    /// When `Some`, the transducer-assigned priority overrides `base_priority`
    /// in the scoring function.
    wfst_cost_class: Option<super::scheduler::CostClass>,

    /// WFST-assigned task descriptor (for weight update on completion).
    wfst_descriptor: Option<super::scheduler::TaskDescriptor>,
}

impl PriorityTask {
    pub fn new(
        task: Box<dyn FnOnce() + Send + 'static>,
        base_priority: u32,
        task_type: TaskTypeId,
        sequence: u64,
    ) -> Self {
        Self {
            task,
            base_priority,
            task_type,
            enqueued_at: Instant::now(),
            sequence,
            wfst_cost_class: None,
            wfst_descriptor: None,
        }
    }

    /// Create a WFST-classified task.
    ///
    /// The cost class and descriptor are used by the scoring function to
    /// override the base priority with the transducer-assigned priority,
    /// and by the weight update on completion.
    pub fn new_classified(
        task: Box<dyn FnOnce() + Send + 'static>,
        base_priority: u32,
        task_type: TaskTypeId,
        sequence: u64,
        cost_class: super::scheduler::CostClass,
        descriptor: super::scheduler::TaskDescriptor,
    ) -> Self {
        Self {
            task,
            base_priority,
            task_type,
            enqueued_at: Instant::now(),
            sequence,
            wfst_cost_class: Some(cost_class),
            wfst_descriptor: Some(descriptor),
        }
    }

    /// Calculate the effective priority score.
    ///
    /// When WFST classification is available:
    ///   score = wfst_priority + (ema_runtime * runtime_weight) - (age * decay_rate)
    /// Otherwise (fallback to P²):
    ///   score = base_priority + (p2_runtime * runtime_weight) - (age * decay_rate)
    ///
    /// Lower score = scheduled first (min-heap)
    pub fn score(&self, runtime_tracker: &RuntimeTracker, config: &SchedulerConfig) -> f64 {
        // Use WFST-assigned priority when available, otherwise base_priority
        let base = if let Some(cost_class) = self.wfst_cost_class {
            let automaton = super::scheduler::global_scheduler();
            let action = automaton.transduce(cost_class);
            action.priority_class as f64
        } else {
            self.base_priority as f64
        };

        // Use WFST EMA runtime estimate when available, otherwise P² estimate
        let estimated_runtime = if let Some(descriptor) = self.wfst_descriptor {
            let automaton = super::scheduler::global_scheduler();
            automaton
                .estimated_runtime(descriptor)
                .unwrap_or_else(|| runtime_tracker.estimated_runtime(self.task_type))
        } else {
            runtime_tracker.estimated_runtime(self.task_type)
        };

        let runtime_component = (estimated_runtime / 1_000_000_000.0) * config.runtime_weight;

        // Age component (time decay to prevent starvation)
        let age_secs = self.enqueued_at.elapsed().as_secs_f64();
        let age_component = age_secs * config.decay_rate;

        // Final score: lower is higher priority
        base + runtime_component - age_component
    }

    /// Execute the task and return runtime in nanoseconds.
    ///
    /// If the task panics, the panic is caught and logged. The worker thread
    /// continues normally. Returns `0` for panicked tasks so the P² estimator
    /// is not polluted with meaningless runtime data.
    pub fn execute(self) -> u64 {
        let start = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(self.task));
        match result {
            Ok(()) => start.elapsed().as_nanos() as u64,
            Err(payload) => {
                tracing::error!(
                    task_type = ?self.task_type,
                    panic = ?payload,
                    "PriorityTask panicked -- worker continues"
                );
                0
            }
        }
    }

    /// Get task type for runtime tracking
    pub fn task_type(&self) -> TaskTypeId {
        self.task_type
    }

    /// Get the WFST cost class (if classified).
    pub fn wfst_cost_class(&self) -> Option<super::scheduler::CostClass> {
        self.wfst_cost_class
    }

    /// Get the WFST task descriptor (if classified).
    pub fn wfst_descriptor(&self) -> Option<super::scheduler::TaskDescriptor> {
        self.wfst_descriptor
    }
}

// ============================================================================
// Scored Task (for heap ordering)
// ============================================================================

/// Wrapper for heap ordering (min-heap behavior via reversed comparison)
struct ScoredTask {
    task: PriorityTask,
    score: f64,
}

impl Ord for ScoredTask {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap (lower score = higher priority)
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| self.task.sequence.cmp(&other.task.sequence))
    }
}

impl PartialOrd for ScoredTask {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Eq for ScoredTask {}

impl PartialEq for ScoredTask {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.task.sequence == other.task.sequence
    }
}

// ============================================================================
// Priority Queue
// ============================================================================

/// Thread-safe priority queue using a single parking_lot::Mutex over BinaryHeap.
///
/// Uses a single lock for both the heap and the "is-empty" condition, eliminating
/// a TOCTOU race that existed when count and heap had separate locks. The condvar
/// waits on the heap mutex, which is released during wait() and reacquired on
/// wake — so pushers can acquire the lock while a popper is parked.
pub struct PriorityQueue {
    heap: Mutex<BinaryHeap<ScoredTask>>,
    runtime_tracker: Arc<RuntimeTracker>,
    config: SchedulerConfig,
    /// Condition variable signaled when the queue transitions from empty to non-empty.
    not_empty: Condvar,
}

impl PriorityQueue {
    pub fn new(runtime_tracker: Arc<RuntimeTracker>, config: SchedulerConfig) -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
            runtime_tracker,
            config,
            not_empty: Condvar::new(),
        }
    }

    /// Push a task onto the priority queue.
    pub fn push(&self, task: PriorityTask) {
        let score = task.score(&self.runtime_tracker, &self.config);
        let mut heap = self.heap.lock();
        heap.push(ScoredTask { task, score });
        self.not_empty.notify_one();
    }

    /// Pop the highest-priority task (blocking).
    ///
    /// Blocks until a task is available or shutdown is signaled.
    pub fn pop_blocking(&self, shutdown: &AtomicBool) -> Option<PriorityTask> {
        let mut heap = self.heap.lock();
        while heap.is_empty() {
            if shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
            self.not_empty.wait(&mut heap);
            if shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
        }
        heap.pop().map(|st| st.task)
    }

    /// Pop the highest-priority task with timeout.
    ///
    /// Blocks until a task is available, timeout expires, or shutdown is signaled.
    /// Returns `None` on timeout or shutdown.
    pub fn pop_timeout(&self, shutdown: &AtomicBool, timeout: Duration) -> Option<PriorityTask> {
        let mut heap = self.heap.lock();
        while heap.is_empty() {
            if shutdown.load(AtomicOrdering::Relaxed) {
                return None;
            }
            if self.not_empty.wait_for(&mut heap, timeout).timed_out() {
                return None;
            }
            if shutdown.load(AtomicOrdering::Relaxed) {
                return None;
            }
        }
        heap.pop().map(|st| st.task)
    }

    /// Pop without blocking (try).
    pub fn try_pop(&self) -> Option<PriorityTask> {
        let mut heap = self.heap.lock();
        heap.pop().map(|st| st.task)
    }

    /// Get queue length.
    pub fn len(&self) -> usize {
        self.heap.lock().len()
    }

    /// Check if queue is empty.
    pub fn is_empty(&self) -> bool {
        self.heap.lock().is_empty()
    }

    /// Notify all waiting workers (for shutdown).
    pub fn notify_all(&self) {
        self.not_empty.notify_all();
    }
}

// ============================================================================
// Result Receiver
// ============================================================================

/// A receiver for the result of a spawned task.
pub struct ResultReceiver<T> {
    receiver: Receiver<T>,
}

impl<T> ResultReceiver<T> {
    /// Block until the result is available
    pub fn recv(self) -> Result<T, RecvError> {
        self.receiver.recv().map_err(|_| RecvError)
    }

    /// Try to receive without blocking
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        match self.receiver.try_recv() {
            Ok(v) => Ok(v),
            Err(crossbeam_channel::TryRecvError::Empty) => Err(TryRecvError::Empty),
            Err(crossbeam_channel::TryRecvError::Disconnected) => Err(TryRecvError::Disconnected),
        }
    }
}

/// Error returned when receiving from a closed channel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecvError;

impl std::fmt::Display for RecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "receiving from a closed channel")
    }
}

impl std::error::Error for RecvError {}

/// Error returned when try_recv fails
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TryRecvError {
    /// No result available yet
    Empty,
    /// The task was dropped or pool shut down
    Disconnected,
}

impl std::fmt::Display for TryRecvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TryRecvError::Empty => write!(f, "no result available yet"),
            TryRecvError::Disconnected => write!(f, "task was dropped or pool shut down"),
        }
    }
}

impl std::error::Error for TryRecvError {}

// ============================================================================
// Priority Thread Pool Statistics
// ============================================================================

/// Statistics about the priority thread pool
#[derive(Debug, Clone)]
pub struct PriorityPoolStats {
    /// Current queue length
    pub queue_length: usize,
    /// Global median runtime estimate (nanoseconds)
    pub global_median_runtime: f64,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;
    use std::thread;

    #[test]
    fn test_p2_median_accuracy() {
        let mut estimator = P2MedianEstimator::new();

        // Add 1000 observations from uniform distribution [0, 100]
        for i in 0..1000 {
            estimator.add_observation((i % 100) as f64);
        }

        // Median should be approximately 49.5
        let median = estimator.median();
        assert!(
            (median - 49.5).abs() < 10.0,
            "Median {} not close to 49.5",
            median
        );
    }

    #[test]
    fn test_p2_with_exponential_like_data() {
        let mut estimator = P2MedianEstimator::new();

        // Simulate exponential-like runtimes (many small, few large)
        for i in 0..500 {
            let value = if i % 10 == 0 {
                1000.0 // Occasional long task
            } else {
                (i % 50) as f64 // Short tasks
            };
            estimator.add_observation(value);
        }

        // Should have a reasonable median
        let median = estimator.median();
        assert!(
            median > 0.0 && median < 1000.0,
            "Median {} out of range",
            median
        );
    }

    #[test]
    fn test_runtime_tracker() {
        let tracker = RuntimeTracker::new();

        // Record some runtimes
        for i in 0..10 {
            tracker.record_runtime(TaskTypeId::Generic, i * 100_000);
        }

        // Should have a non-zero estimate
        let estimate = tracker.estimated_runtime(TaskTypeId::Generic);
        assert!(estimate > 0.0, "Expected non-zero estimate");
    }

    #[test]
    fn test_priority_task_execute_catches_panic() {
        let task = PriorityTask::new(
            Box::new(|| panic!("intentional panic in PriorityTask")),
            0,
            TaskTypeId::Generic,
            0,
        );

        // Should return 0 (not propagate the panic)
        let runtime = task.execute();
        assert_eq!(runtime, 0, "Panicked task should return runtime of 0");
    }

    #[test]
    fn test_pop_timeout_returns_task() {
        let tracker = Arc::new(RuntimeTracker::new());
        let queue = PriorityQueue::new(Arc::clone(&tracker), SchedulerConfig::default());
        let shutdown = AtomicBool::new(false);
        let seq = AtomicU64::new(0);

        let task = PriorityTask::new(
            Box::new(|| {}),
            0,
            TaskTypeId::Generic,
            seq.fetch_add(1, AtomicOrdering::Relaxed),
        );
        queue.push(task);

        let result = queue.pop_timeout(&shutdown, Duration::from_millis(100));
        assert!(result.is_some(), "Should pop an available task");
    }

    #[test]
    fn test_pop_timeout_expires_on_empty() {
        let tracker = Arc::new(RuntimeTracker::new());
        let queue = PriorityQueue::new(Arc::clone(&tracker), SchedulerConfig::default());
        let shutdown = AtomicBool::new(false);

        let start = std::time::Instant::now();
        let result = queue.pop_timeout(&shutdown, Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(result.is_none(), "Should return None on timeout");
        assert!(
            elapsed >= Duration::from_millis(40),
            "Should wait approximately the timeout period, elapsed: {:?}",
            elapsed,
        );
    }

    #[test]
    fn test_pop_timeout_returns_on_shutdown() {
        let tracker = Arc::new(RuntimeTracker::new());
        let queue = Arc::new(PriorityQueue::new(
            Arc::clone(&tracker),
            SchedulerConfig::default(),
        ));
        let shutdown = Arc::new(AtomicBool::new(false));

        let queue_clone = Arc::clone(&queue);
        let shutdown_clone = Arc::clone(&shutdown);

        let handle = thread::spawn(move || {
            queue_clone.pop_timeout(&shutdown_clone, Duration::from_secs(10))
        });

        // Give thread time to enter the wait
        thread::sleep(Duration::from_millis(50));

        // Signal shutdown and notify
        shutdown.store(true, AtomicOrdering::SeqCst);
        queue.notify_all();

        let result = handle.join().expect("Thread panicked");
        assert!(result.is_none(), "Should return None on shutdown");
    }
}
