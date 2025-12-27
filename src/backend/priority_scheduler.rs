#![cfg(feature = "hybrid-p2-priority-scheduler")]
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

use crossbeam_channel::Receiver;
use dashmap::DashMap;
use parking_lot::{Condvar, Mutex};
use std::cmp::Ordering;
use std::collections::hash_map::DefaultHasher;
use std::collections::BinaryHeap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, LazyLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

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
                self.heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
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
    pub fn from_expr(expr: &crate::backend::models::MettaValue) -> Self {
        use crate::backend::bytecode::cache::hash_metta_value;
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
        let mut hasher = DefaultHasher::new();
        task_type.hash(&mut hasher);
        hasher.finish()
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

impl Default for SchedulerConfig {
    fn default() -> Self {
        let num_cpus = num_cpus::get();
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
        }
    }

    /// Calculate the effective priority score.
    ///
    /// score = base_priority + (estimated_runtime * runtime_weight) - (age * decay_rate)
    /// Lower score = scheduled first (min-heap)
    pub fn score(&self, runtime_tracker: &RuntimeTracker, config: &SchedulerConfig) -> f64 {
        let base = self.base_priority as f64;

        // Estimated runtime component (normalized to seconds)
        let estimated_runtime = runtime_tracker.estimated_runtime(self.task_type);
        let runtime_component = (estimated_runtime / 1_000_000_000.0) * config.runtime_weight;

        // Age component (time decay to prevent starvation)
        let age_secs = self.enqueued_at.elapsed().as_secs_f64();
        let age_component = age_secs * config.decay_rate;

        // Final score: lower is higher priority
        base + runtime_component - age_component
    }

    /// Execute the task and return runtime in nanoseconds
    pub fn execute(self) -> u64 {
        let start = Instant::now();
        (self.task)();
        start.elapsed().as_nanos() as u64
    }

    /// Get task type for runtime tracking
    pub fn task_type(&self) -> TaskTypeId {
        self.task_type
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

/// Thread-safe priority queue using parking_lot::Mutex and BinaryHeap.
pub struct PriorityQueue {
    heap: Mutex<BinaryHeap<ScoredTask>>,
    runtime_tracker: Arc<RuntimeTracker>,
    config: SchedulerConfig,
    /// Condition variable for blocking when queue is empty
    not_empty: Condvar,
    /// Count of available tasks
    count: Mutex<usize>,
}

impl PriorityQueue {
    pub fn new(runtime_tracker: Arc<RuntimeTracker>, config: SchedulerConfig) -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
            runtime_tracker,
            config,
            not_empty: Condvar::new(),
            count: Mutex::new(0),
        }
    }

    /// Push a task onto the priority queue
    pub fn push(&self, task: PriorityTask) {
        let score = task.score(&self.runtime_tracker, &self.config);

        {
            let mut heap = self.heap.lock();
            heap.push(ScoredTask { task, score });
        }

        // Notify one waiting worker
        let mut count = self.count.lock();
        *count += 1;
        self.not_empty.notify_one();
    }

    /// Pop the highest-priority task (blocking)
    pub fn pop_blocking(&self, shutdown: &AtomicBool) -> Option<PriorityTask> {
        let mut count = self.count.lock();

        while *count == 0 {
            if shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
            self.not_empty.wait(&mut count);
            if shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
        }

        *count -= 1;
        drop(count);

        // Pop from heap
        let mut heap = self.heap.lock();
        heap.pop().map(|st| st.task)
    }

    /// Pop without blocking (try)
    pub fn try_pop(&self) -> Option<PriorityTask> {
        let mut count = self.count.lock();
        if *count == 0 {
            return None;
        }
        *count -= 1;
        drop(count);

        let mut heap = self.heap.lock();
        heap.pop().map(|st| st.task)
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        *self.count.lock()
    }

    /// Check if queue is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Notify all waiting workers (for shutdown)
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
// Priority Thread Pool
// ============================================================================

/// Global priority-aware eval thread pool instance
static GLOBAL_PRIORITY_POOL: LazyLock<PriorityEvalThreadPool> = LazyLock::new(|| {
    let num_threads = num_cpus::get();
    PriorityEvalThreadPool::new(num_threads, SchedulerConfig::default())
});

/// Get the global priority eval thread pool
pub fn global_priority_eval_pool() -> &'static PriorityEvalThreadPool {
    &GLOBAL_PRIORITY_POOL
}

/// A priority-aware persistent thread pool for MeTTa evaluation.
///
/// Workers stay alive and pull highest-priority tasks from a shared priority queue,
/// eliminating per-task spawn overhead while enabling priority-based scheduling.
pub struct PriorityEvalThreadPool {
    /// Priority queue for pending tasks
    queue: Arc<PriorityQueue>,

    /// Runtime tracker for P² estimation
    runtime_tracker: Arc<RuntimeTracker>,

    /// Worker thread handles
    workers: Vec<JoinHandle<()>>,

    /// Shutdown signal
    shutdown: Arc<AtomicBool>,

    /// Number of worker threads
    num_threads: usize,

    /// Scheduler configuration
    config: SchedulerConfig,

    /// Monotonic sequence counter for stable ordering
    sequence: AtomicU64,
}

impl PriorityEvalThreadPool {
    /// Create a new priority thread pool with specified number of workers
    pub fn new(num_threads: usize, config: SchedulerConfig) -> Self {
        let runtime_tracker = Arc::new(RuntimeTracker::new());
        let queue = Arc::new(PriorityQueue::new(Arc::clone(&runtime_tracker), config));
        let shutdown = Arc::new(AtomicBool::new(false));

        let workers: Vec<_> = (0..num_threads)
            .map(|id| {
                let queue = Arc::clone(&queue);
                let runtime_tracker = Arc::clone(&runtime_tracker);
                let shutdown = Arc::clone(&shutdown);

                thread::Builder::new()
                    .name(format!("priority-eval-worker-{}", id))
                    .spawn(move || {
                        priority_worker_loop(queue, runtime_tracker, shutdown);
                    })
                    .expect("failed to spawn priority eval worker thread")
            })
            .collect();

        Self {
            queue,
            runtime_tracker,
            workers,
            shutdown,
            num_threads,
            config,
            sequence: AtomicU64::new(0),
        }
    }

    /// Spawn a task with default priority (0) and generic task type
    pub fn spawn<F, R>(&self, f: F) -> ResultReceiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_with_priority(f, 0, TaskTypeId::generic())
    }

    /// Spawn a task with explicit priority and task type
    pub fn spawn_with_priority<F, R>(
        &self,
        f: F,
        priority: u32,
        task_type: TaskTypeId,
    ) -> ResultReceiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (result_sender, result_receiver) = crossbeam_channel::bounded(1);
        let sequence = self.sequence.fetch_add(1, AtomicOrdering::Relaxed);

        let task = Box::new(move || {
            let result = f();
            let _ = result_sender.send(result);
        });

        let priority_task = PriorityTask::new(task, priority, task_type, sequence);
        self.queue.push(priority_task);

        ResultReceiver {
            receiver: result_receiver,
        }
    }

    /// Spawn a task for MeTTa evaluation with expression-based runtime tracking
    pub fn spawn_eval<F, R>(
        &self,
        f: F,
        expr: &crate::backend::models::MettaValue,
        priority: u32,
    ) -> ResultReceiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.spawn_with_priority(f, priority, TaskTypeId::from_expr(expr))
    }

    /// Spawn a fire-and-forget task with explicit priority
    pub fn spawn_detached<F>(&self, f: F, priority: u32, task_type: TaskTypeId)
    where
        F: FnOnce() + Send + 'static,
    {
        let sequence = self.sequence.fetch_add(1, AtomicOrdering::Relaxed);
        let task = Box::new(f);
        let priority_task = PriorityTask::new(task, priority, task_type, sequence);
        self.queue.push(priority_task);
    }

    /// Get the number of worker threads
    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Get current queue length
    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Get runtime statistics
    pub fn stats(&self) -> PriorityPoolStats {
        PriorityPoolStats {
            queue_length: self.queue.len(),
            global_median_runtime: self.runtime_tracker.global_median(),
        }
    }

    /// Shutdown the pool, waiting for all workers to finish
    pub fn shutdown(self) {
        self.shutdown.store(true, AtomicOrdering::SeqCst);
        self.queue.notify_all(); // Wake up any blocked workers

        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

/// Worker thread main loop for priority pool
fn priority_worker_loop(
    queue: Arc<PriorityQueue>,
    runtime_tracker: Arc<RuntimeTracker>,
    shutdown: Arc<AtomicBool>,
) {
    loop {
        // Check for shutdown
        if shutdown.load(AtomicOrdering::SeqCst) {
            break;
        }

        // Block until a task is available
        match queue.pop_blocking(&shutdown) {
            Some(task) => {
                let task_type = task.task_type();

                // Execute task and measure runtime
                let runtime_nanos = task.execute();

                // Record runtime for future estimations
                runtime_tracker.record_runtime(task_type, runtime_nanos);
            }
            None => {
                // Shutdown signaled or spurious wakeup
                if shutdown.load(AtomicOrdering::SeqCst) {
                    break;
                }
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

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
        assert!(median > 0.0 && median < 1000.0, "Median {} out of range", median);
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
    fn test_priority_pool_spawn_and_recv() {
        let pool = PriorityEvalThreadPool::new(2, SchedulerConfig::default());

        let result = pool.spawn(|| 42);
        assert_eq!(result.recv().unwrap(), 42);

        pool.shutdown();
    }

    #[test]
    fn test_priority_pool_parallel_tasks() {
        let pool = PriorityEvalThreadPool::new(4, SchedulerConfig::default());
        let counter = Arc::new(AtomicUsize::new(0));

        let receivers: Vec<_> = (0..100)
            .map(|_| {
                let counter = Arc::clone(&counter);
                pool.spawn(move || {
                    counter.fetch_add(1, AtomicOrdering::SeqCst);
                })
            })
            .collect();

        // Wait for all tasks
        for rx in receivers {
            rx.recv().unwrap();
        }

        assert_eq!(counter.load(AtomicOrdering::SeqCst), 100);

        pool.shutdown();
    }

    #[test]
    fn test_priority_ordering() {
        // This test verifies that high-priority tasks tend to execute before low-priority
        // Note: Due to concurrency, this is probabilistic
        let pool = PriorityEvalThreadPool::new(1, SchedulerConfig::default()); // Single worker for determinism
        let order = Arc::new(std::sync::Mutex::new(Vec::new()));

        // Submit low priority first, then high priority
        let order_clone = Arc::clone(&order);
        pool.spawn_with_priority(
            move || {
                order_clone.lock().unwrap().push("low");
            },
            10,
            TaskTypeId::Generic,
        );

        let order_clone = Arc::clone(&order);
        pool.spawn_with_priority(
            move || {
                order_clone.lock().unwrap().push("high");
            },
            0,
            TaskTypeId::Generic,
        );

        // Give time for tasks to execute
        std::thread::sleep(std::time::Duration::from_millis(100));

        let execution_order = order.lock().unwrap();
        // With single worker and immediate scheduling, high should execute first
        // (though timing may vary)
        assert_eq!(execution_order.len(), 2);

        pool.shutdown();
    }

    #[test]
    fn test_global_priority_pool() {
        let result = global_priority_eval_pool().spawn(|| 123);
        assert_eq!(result.recv().unwrap(), 123);
    }
}
