//! Custom thread pool for MeTTa evaluation
//!
//! This module provides a persistent thread pool optimized for parallel evaluation.
//! Unlike Tokio's spawn_blocking, workers stay alive and read tasks from a channel,
//! eliminating per-task spawn overhead (~100-500ns → ~10-50ns).
//!
//! # Design
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │                      EvalThreadPool                         │
//! │                                                             │
//! │  ┌─────────┐    ┌──────────────────────────────────────┐   │
//! │  │ Sender  │───>│  Bounded Channel (num_threads * 4)   │   │
//! │  └─────────┘    └──────────────────────────────────────┘   │
//! │                           │                                 │
//! │          ┌────────────────┼────────────────┐               │
//! │          ▼                ▼                ▼               │
//! │    ┌──────────┐    ┌──────────┐    ┌──────────┐           │
//! │    │ Worker 1 │    │ Worker 2 │    │ Worker N │           │
//! │    │ (recv)   │    │ (recv)   │    │ (recv)   │           │
//! │    └──────────┘    └──────────┘    └──────────┘           │
//! └─────────────────────────────────────────────────────────────┘
//! ```
//!
//! Workers block on `recv()` and stay alive for the lifetime of the pool.
//! Tasks are distributed via crossbeam's lock-free bounded channel.

use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::thread::{self, JoinHandle};

/// Global eval thread pool instance
static GLOBAL_POOL: LazyLock<EvalThreadPool> = LazyLock::new(|| {
    let num_threads = num_cpus::get();
    EvalThreadPool::new(num_threads)
});

/// Get the global eval thread pool
pub fn global_eval_pool() -> &'static EvalThreadPool {
    &GLOBAL_POOL
}

/// A boxed task that can be sent across threads
type BoxedTask = Box<dyn FnOnce() + Send + 'static>;

/// A persistent thread pool optimized for MeTTa evaluation
///
/// Workers stay alive and read tasks from a bounded channel,
/// eliminating per-task spawn overhead.
pub struct EvalThreadPool {
    sender: Sender<BoxedTask>,
    workers: Vec<JoinHandle<()>>,
    shutdown: Arc<AtomicBool>,
    num_threads: usize,
}

impl EvalThreadPool {
    /// Create a new thread pool with the specified number of worker threads
    ///
    /// Workers are spawned immediately and block on the channel, ready for work.
    pub fn new(num_threads: usize) -> Self {
        let (sender, receiver) = bounded::<BoxedTask>(num_threads * 4);
        let receiver = Arc::new(receiver);
        let shutdown = Arc::new(AtomicBool::new(false));

        let workers: Vec<_> = (0..num_threads)
            .map(|id| {
                let rx = Arc::clone(&receiver);
                let shutdown = Arc::clone(&shutdown);
                thread::Builder::new()
                    .name(format!("eval-worker-{}", id))
                    .spawn(move || {
                        worker_loop(rx, shutdown);
                    })
                    .expect("failed to spawn eval worker thread")
            })
            .collect();

        Self {
            sender,
            workers,
            shutdown,
            num_threads,
        }
    }

    /// Get the number of worker threads in the pool
    pub fn num_threads(&self) -> usize {
        self.num_threads
    }

    /// Spawn a task on the thread pool and get a receiver for the result
    ///
    /// The task will be executed by one of the worker threads.
    /// Returns a oneshot receiver that will contain the result when ready.
    ///
    /// # Panics
    ///
    /// Panics if the thread pool has been shut down.
    pub fn spawn<F, R>(&self, f: F) -> ResultReceiver<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        let (result_sender, result_receiver) = crossbeam_channel::bounded(1);

        let task = Box::new(move || {
            let result = f();
            // Ignore send errors - receiver may have been dropped
            let _ = result_sender.send(result);
        });

        self.sender
            .send(task)
            .expect("eval thread pool has been shut down");

        ResultReceiver {
            receiver: result_receiver,
        }
    }

    /// Spawn a task without waiting for a result (fire-and-forget)
    ///
    /// Useful when the result is not needed, avoiding channel allocation overhead.
    pub fn spawn_detached<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let task = Box::new(f);
        self.sender
            .send(task)
            .expect("eval thread pool has been shut down");
    }

    /// Shut down the thread pool, waiting for all workers to finish
    ///
    /// After shutdown, any calls to `spawn` will panic.
    pub fn shutdown(self) {
        // Signal shutdown
        self.shutdown.store(true, Ordering::SeqCst);

        // Drop sender to close channel
        drop(self.sender);

        // Wait for all workers to finish
        for worker in self.workers {
            let _ = worker.join();
        }
    }
}

/// Worker thread main loop
fn worker_loop(receiver: Arc<Receiver<BoxedTask>>, shutdown: Arc<AtomicBool>) {
    loop {
        // Check for shutdown
        if shutdown.load(Ordering::SeqCst) {
            break;
        }

        // Block on receiving a task
        match receiver.recv() {
            Ok(task) => {
                // Execute the task
                task();
            }
            Err(_) => {
                // Channel closed, exit
                break;
            }
        }
    }
}

/// A receiver for the result of a spawned task
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_spawn_and_recv() {
        let pool = EvalThreadPool::new(2);

        let result = pool.spawn(|| 42);
        assert_eq!(result.recv().unwrap(), 42);

        pool.shutdown();
    }

    #[test]
    fn test_parallel_tasks() {
        let pool = EvalThreadPool::new(4);
        let counter = Arc::new(AtomicUsize::new(0));

        let receivers: Vec<_> = (0..100)
            .map(|_| {
                let counter = Arc::clone(&counter);
                pool.spawn(move || {
                    counter.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        // Wait for all tasks
        for rx in receivers {
            rx.recv().unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 100);

        pool.shutdown();
    }

    #[test]
    fn test_spawn_detached() {
        let pool = EvalThreadPool::new(2);
        let counter = Arc::new(AtomicUsize::new(0));

        for _ in 0..10 {
            let counter = Arc::clone(&counter);
            pool.spawn_detached(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            });
        }

        // Give tasks time to complete
        std::thread::sleep(std::time::Duration::from_millis(100));

        assert_eq!(counter.load(Ordering::SeqCst), 10);

        pool.shutdown();
    }

    #[test]
    fn test_global_pool() {
        let result = global_eval_pool().spawn(|| 123);
        assert_eq!(result.recv().unwrap(), 123);
    }
}
