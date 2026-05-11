//! Lock-Free Striped Task Queue with Per-Worker Work-Stealing
//!
//! Replaces the single-mutex `PriorityQueue` for the eval pool with a
//! striped deque architecture using `crossbeam-deque` (Chase-Lev algorithm).
//!
//! ## Architecture
//!
//! ```text
//! Worker 0: [local deque] ← push/pop from bottom (LIFO, cache-warm)
//! Worker 1: [local deque] ← push/pop from bottom
//! Worker 2: [local deque] ← push/pop from bottom
//!   ...
//! Worker N: [local deque] ← push/pop from bottom
//!
//! Global injector: [crossbeam Injector] ← external pushes, stolen by idle workers
//! ```
//!
//! Each worker has a `crossbeam_deque::Worker<T>` (single-producer, single-consumer
//! from the bottom). Other workers hold `Stealer<T>` references for work-stealing
//! from the top (FIFO — oldest tasks stolen first, preserving fairness).
//!
//! ## Performance
//!
//! - Local push/pop: O(1), no contention (single-thread access)
//! - Steal: O(1) amortized, CAS-based (may retry on contention)
//! - Inject: O(1) amortized, CAS-based
//! - No mutex on any hot path
//!
//! ## Integration
//!
//! The `StripedQueue` is a standalone data structure that can be used by the
//! work pool as an alternative to `PriorityQueue`. Integration with `WorkPool`
//! is done by replacing the queue field and updating worker loops.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crossbeam_deque::{Injector, Steal, Stealer, Worker};
use parking_lot::{Condvar, Mutex};

// ============================================================================
// Task Wrapper
// ============================================================================

/// A task in the striped queue.
///
/// Wraps a boxed closure (same as the existing work pool task format)
/// with optional metadata for diagnostics.
pub struct StripedTask {
    /// The task closure to execute.
    pub work: Box<dyn FnOnce() + Send + 'static>,
    /// Priority level (lower = higher priority). Used for injector ordering.
    pub priority: u32,
}

impl std::fmt::Debug for StripedTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripedTask")
            .field("priority", &self.priority)
            .finish()
    }
}

// ============================================================================
// Striped Queue
// ============================================================================

/// Lock-free striped task queue with per-worker work-stealing deques.
///
/// Each worker has a local deque for push/pop (no contention). Tasks from
/// external sources go to the global injector. Idle workers steal from
/// other workers' deques or from the injector.
pub struct StripedQueue {
    /// Global injector for tasks from external (non-worker) threads.
    injector: Injector<StripedTask>,

    /// Stealers for each worker's local deque.
    /// Worker i steals from stealers[j] for j != i.
    stealers: Vec<Stealer<StripedTask>>,

    /// Number of workers (deques).
    num_workers: usize,

    /// Total task count across all deques + injector (approximate).
    task_count: AtomicUsize,

    /// Condvar for waking blocked workers when tasks arrive.
    notify_lock: Mutex<()>,
    notify_cvar: Condvar,

    /// Shutdown flag.
    shutdown: AtomicBool,
}

impl StripedQueue {
    /// Create a new striped queue with the given number of workers.
    ///
    /// Returns `(queue, workers)` where `workers` is a Vec of per-worker
    /// `Worker<StripedTask>` deques. Each worker must be given to exactly
    /// one thread.
    pub fn new(num_workers: usize) -> (Self, Vec<Worker<StripedTask>>) {
        let mut workers = Vec::with_capacity(num_workers);
        let mut stealers = Vec::with_capacity(num_workers);

        for _ in 0..num_workers {
            let w = Worker::new_lifo(); // LIFO for cache locality
            stealers.push(w.stealer());
            workers.push(w);
        }

        let queue = Self {
            injector: Injector::new(),
            stealers,
            num_workers,
            task_count: AtomicUsize::new(0),
            notify_lock: Mutex::new(()),
            notify_cvar: Condvar::new(),
            shutdown: AtomicBool::new(false),
        };

        (queue, workers)
    }

    /// Push a task from an external (non-worker) thread.
    ///
    /// The task goes to the global injector and will be stolen by an idle worker.
    pub fn push_external(&self, task: StripedTask) {
        self.injector.push(task);
        self.task_count.fetch_add(1, Ordering::Relaxed);
        // Wake one blocked worker
        self.notify_cvar.notify_one();
    }

    /// Push a task to a specific worker's local deque.
    ///
    /// Used when a worker wants to add work to its own queue (e.g., from
    /// `parallel_branch_eval` running on that worker's thread).
    pub fn push_local(worker: &Worker<StripedTask>, task: StripedTask, task_count: &AtomicUsize) {
        worker.push(task);
        task_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Pop a task from a worker's local deque.
    ///
    /// Returns `Some(task)` if the local deque has work, `None` otherwise.
    #[inline]
    pub fn pop_local(worker: &Worker<StripedTask>) -> Option<StripedTask> {
        worker.pop()
    }

    /// Try to steal a task from other workers or the injector.
    ///
    /// Steal order:
    /// 1. Round-robin through other workers' deques (starting at a rotating offset)
    /// 2. Global injector
    ///
    /// Returns `Some(task)` on success, `None` if all sources are empty.
    pub fn try_steal(&self, worker_id: usize, worker: &Worker<StripedTask>) -> Option<StripedTask> {
        // Try stealing from other workers (round-robin from rotating start)
        let start = worker_id.wrapping_add(1) % self.num_workers;
        for offset in 0..self.num_workers {
            let target = (start + offset) % self.num_workers;
            if target == worker_id {
                continue; // Don't steal from self
            }
            loop {
                match self.stealers[target].steal() {
                    Steal::Success(task) => {
                        self.task_count.fetch_sub(1, Ordering::Relaxed);
                        return Some(task);
                    }
                    Steal::Retry => continue,
                    Steal::Empty => break,
                }
            }
        }

        // Try the global injector
        loop {
            match self.injector.steal_batch_and_pop(worker) {
                Steal::Success(task) => {
                    self.task_count.fetch_sub(1, Ordering::Relaxed);
                    return Some(task);
                }
                Steal::Retry => continue,
                Steal::Empty => return None,
            }
        }
    }

    /// Pop a task, blocking if none available.
    ///
    /// Order: local deque → steal from others → injector → block on condvar.
    /// Returns `None` only when shutdown is signaled.
    pub fn pop_blocking(
        &self,
        worker_id: usize,
        worker: &Worker<StripedTask>,
    ) -> Option<StripedTask> {
        loop {
            // Fast path: local deque
            if let Some(task) = Self::pop_local(worker) {
                self.task_count.fetch_sub(1, Ordering::Relaxed);
                return Some(task);
            }

            // Try stealing
            if let Some(task) = self.try_steal(worker_id, worker) {
                return Some(task);
            }

            // Check shutdown
            if self.shutdown.load(Ordering::Relaxed) {
                return None;
            }

            // Block on condvar with timeout (1ms to re-check)
            let guard = self.notify_lock.lock();
            // Double-check after acquiring lock
            if self.shutdown.load(Ordering::Relaxed) {
                return None;
            }
            // Brief check of local and injector before sleeping
            if let Some(task) = Self::pop_local(worker) {
                self.task_count.fetch_sub(1, Ordering::Relaxed);
                return Some(task);
            }
            self.notify_cvar
                .wait_for(&mut { guard }, std::time::Duration::from_millis(1));
        }
    }

    /// Signal all workers to shut down.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Release);
        self.notify_cvar.notify_all();
    }

    /// Return approximate total task count.
    #[inline]
    pub fn task_count(&self) -> usize {
        self.task_count.load(Ordering::Relaxed)
    }

    /// Return the number of workers.
    #[inline]
    pub fn num_workers(&self) -> usize {
        self.num_workers
    }

    /// Check if shutdown has been signaled.
    #[inline]
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Get a reference to the task count atomic (for push_local).
    #[inline]
    pub fn task_count_ref(&self) -> &AtomicUsize {
        &self.task_count
    }
}

// ============================================================================
// Thread-Local Worker ID
// ============================================================================

thread_local! {
    /// The current worker's ID (set by the worker loop at startup).
    /// `None` for non-worker threads (REPL, compile pool, main thread).
    static CURRENT_WORKER_ID: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Set the current thread's worker ID.
pub fn set_worker_id(id: usize) {
    CURRENT_WORKER_ID.with(|cell| cell.set(Some(id)));
}

/// Get the current thread's worker ID, or None for non-worker threads.
#[inline]
pub fn current_worker_id() -> Option<usize> {
    CURRENT_WORKER_ID.with(|cell| cell.get())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    use std::sync::Arc;

    fn make_task(priority: u32) -> StripedTask {
        StripedTask {
            work: Box::new(|| {}),
            priority,
        }
    }

    #[test]
    fn test_create_queue() {
        let (queue, workers) = StripedQueue::new(4);
        assert_eq!(queue.num_workers(), 4);
        assert_eq!(workers.len(), 4);
        assert_eq!(queue.task_count(), 0);
    }

    #[test]
    fn test_push_pop_local() {
        let (queue, workers) = StripedQueue::new(2);
        let w0 = &workers[0];

        StripedQueue::push_local(w0, make_task(5), queue.task_count_ref());
        assert_eq!(queue.task_count(), 1);

        let task = StripedQueue::pop_local(w0);
        assert!(task.is_some());
        assert_eq!(task.expect("has task").priority, 5);
    }

    #[test]
    fn test_push_external_and_steal() {
        let (queue, workers) = StripedQueue::new(2);

        queue.push_external(make_task(10));
        assert_eq!(queue.task_count(), 1);

        // Worker 0 steals from injector
        let task = queue.try_steal(0, &workers[0]);
        assert!(task.is_some());
        assert_eq!(task.expect("stolen").priority, 10);
    }

    #[test]
    fn test_steal_from_other_worker() {
        let (queue, workers) = StripedQueue::new(2);

        // Push to worker 0's local deque
        StripedQueue::push_local(&workers[0], make_task(7), queue.task_count_ref());

        // Worker 1 steals from worker 0
        let task = queue.try_steal(1, &workers[1]);
        assert!(task.is_some());
        assert_eq!(task.expect("stolen from w0").priority, 7);
    }

    #[test]
    fn test_empty_steal() {
        let (queue, workers) = StripedQueue::new(2);
        let task = queue.try_steal(0, &workers[0]);
        assert!(task.is_none());
    }

    #[test]
    fn test_lifo_local() {
        let (queue, workers) = StripedQueue::new(1);
        let w = &workers[0];

        StripedQueue::push_local(w, make_task(1), queue.task_count_ref());
        StripedQueue::push_local(w, make_task(2), queue.task_count_ref());
        StripedQueue::push_local(w, make_task(3), queue.task_count_ref());

        // LIFO: most recent first
        assert_eq!(StripedQueue::pop_local(w).expect("t3").priority, 3);
        assert_eq!(StripedQueue::pop_local(w).expect("t2").priority, 2);
        assert_eq!(StripedQueue::pop_local(w).expect("t1").priority, 1);
    }

    #[test]
    fn test_shutdown() {
        let (queue, _workers) = StripedQueue::new(2);
        assert!(!queue.is_shutdown());
        queue.shutdown();
        assert!(queue.is_shutdown());
    }

    #[test]
    fn test_worker_id() {
        assert!(current_worker_id().is_none());
        set_worker_id(42);
        assert_eq!(current_worker_id(), Some(42));
        // Reset for other tests
        CURRENT_WORKER_ID.with(|cell| cell.set(None));
    }

    #[test]
    fn test_multithreaded_push_steal() {
        let (queue, workers) = StripedQueue::new(4);
        let queue = Arc::new(queue);
        let counter = Arc::new(AtomicU32::new(0));

        // Distribute workers to threads
        let handles: Vec<_> = workers
            .into_iter()
            .enumerate()
            .map(|(id, worker)| {
                let q = Arc::clone(&queue);
                let c = Arc::clone(&counter);
                std::thread::spawn(move || {
                    // Each worker pushes 10 tasks to its local deque
                    for i in 0..10 {
                        StripedQueue::push_local(&worker, make_task(i as u32), q.task_count_ref());
                    }

                    // Then pop all local + steal from others
                    let mut count = 0;
                    while StripedQueue::pop_local(&worker).is_some() {
                        count += 1;
                    }
                    while q.try_steal(id, &worker).is_some() {
                        count += 1;
                    }
                    c.fetch_add(count, Ordering::Relaxed);
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread panicked");
        }

        // All 40 tasks should have been consumed
        assert_eq!(counter.load(Ordering::Relaxed), 40);
    }
}
