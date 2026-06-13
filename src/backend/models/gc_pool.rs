//! Adaptive GC Pool — Multi-Worker Mark-Sweep + Session Release
//!
//! Replaces the single `GcThread` + single `session_release_thread` with a
//! priority-channeled worker pool (1–4 workers) managed by EMA hill climbing.
//!
//! ## Architecture
//!
//! ```text
//! ┌──────────────────────────────────────────┐
//! │          Adaptive GC Pool                │
//! │  HIGH channel: Collect(GcSnapshot)       │
//! │  LOW  channel: SessionRelease { ids }    │
//! │  1–4 workers, hill climb: alloc/free     │
//! └──────────────────────────────────────────┘
//! ```
//!
//! ## Priority
//!
//! Workers first try to dequeue from the HIGH channel (non-blocking). If empty,
//! they dequeue from the LOW channel (with 500ms timeout for periodic park/shutdown
//! checks). Mark-sweep GC collections (`Collect`) take priority over session
//! releases because they reclaim the largest amounts of memory.
//!
//! ## Quiescent-State Coordination
//!
//! All TLA+-verified sync primitives (`GC_IN_PROGRESS`, `ACTIVE_EVALUATORS`,
//! condvars) are preserved unchanged. Session release work items perform the
//! same quiescence waiting as the original `session_release_thread_main()`.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crossbeam_channel::{self, Receiver, Sender};
use tracing::{debug, warn};

use super::adaptive_pool::WorkerPark;
use super::gc_allocator::{mark_snapshot, sweep_snapshot, GcResponse, GcSnapshot};

// ============================================================================
// Configuration
// ============================================================================

/// Minimum GC pool workers (never park below this).
const MIN_GC_WORKERS: usize = 1;

/// Maximum GC pool workers.
const MAX_GC_WORKERS: usize = 4;

/// Timeout for LOW channel dequeue (allows periodic park/shutdown checks).
const LOW_CHANNEL_TIMEOUT: Duration = Duration::from_millis(500);

// ============================================================================
// GC Work Items
// ============================================================================

/// Work item for the adaptive GC pool.
pub enum GcWorkItem {
    /// High priority: Perform mark-sweep GC on an owned snapshot.
    /// The response is sent back through the pool's response channel.
    Collect(GcSnapshot),

    /// Low priority: Release sessions after quiescence.
    /// The worker waits for ACTIVE_EVALUATORS == 0, acquires GC_IN_PROGRESS,
    /// traces surviving set, and releases sessions.
    SessionRelease {
        /// Context IDs to release.
        context_ids: Vec<u32>,
    },

    /// Shutdown signal for workers.
    Shutdown,
}

// SAFETY: GcSnapshot is Send (contains raw pointers but ownership is transferred).
unsafe impl Send for GcWorkItem {}

// ============================================================================
// Adaptive GC Pool
// ============================================================================

/// Adaptive thread pool for GC mark-sweep and session release work.
///
/// Uses two crossbeam channels (HIGH and LOW priority) with per-worker parking.
/// The pool is managed by a hill climber in `gc_cron.rs` that adjusts worker
/// count based on allocation/free rate ratio.
pub struct AdaptiveGcPool {
    /// High-priority channel: GC collections.
    high_tx: Sender<GcWorkItem>,

    /// Low-priority channel: session releases.
    low_tx: Sender<GcWorkItem>,

    /// Stored receiver clones for respawning workers.
    high_rx: Receiver<GcWorkItem>,
    low_rx: Receiver<GcWorkItem>,

    /// Response channel: GC responses from mark-sweep workers.
    /// The sender is stored to keep the channel alive (workers clone it)
    /// and for respawning workers.
    response_tx: Sender<GcResponse>,
    response_rx: Receiver<GcResponse>,

    /// Worker thread handles. Wrapped in `Mutex<Option<...>>` so the
    /// memory monitor can check `is_finished()` without moving the handle,
    /// and replace dead workers.
    workers: Vec<parking_lot::Mutex<Option<JoinHandle<()>>>>,

    /// Per-worker parking primitives.
    worker_parks: Vec<Arc<WorkerPark>>,

    /// Shutdown signal (shared with all workers).
    shutdown: Arc<AtomicBool>,

    /// Number of currently active (non-parked) workers.
    active_count: AtomicUsize,

    /// Serializes worker park/unpark/respawn transitions with active-count
    /// accounting. The worker-local parked flag is protected by `WorkerPark`;
    /// this lock makes the pool-level aggregate update one logical transition.
    scale_lock: parking_lot::Mutex<()>,

    /// Minimum workers (never park below this).
    min_workers: usize,

    /// Maximum workers.
    max_workers: usize,

    /// Counter of completed session releases (for throughput tracking).
    #[cfg(feature = "track-stats")]
    session_release_count: AtomicU32,
}

impl AdaptiveGcPool {
    /// Create and start the adaptive GC pool with default worker counts.
    pub fn new() -> Self {
        Self::with_workers(MIN_GC_WORKERS, MAX_GC_WORKERS)
    }

    /// Create and start an adaptive GC pool with explicit worker counts.
    ///
    /// This constructor is useful for tests that need isolated pools
    /// without sharing the global singleton.
    pub fn with_workers(min_workers: usize, max_workers: usize) -> Self {
        let min_workers = min_workers.max(1);
        let max_workers = max_workers.max(min_workers);
        let (high_tx, high_rx) = crossbeam_channel::unbounded::<GcWorkItem>();
        let (low_tx, low_rx) = crossbeam_channel::unbounded::<GcWorkItem>();
        let (response_tx, response_rx) = crossbeam_channel::unbounded::<GcResponse>();

        let shutdown = Arc::new(AtomicBool::new(false));

        let mut workers = Vec::with_capacity(max_workers);
        let mut worker_parks = Vec::with_capacity(max_workers);

        for id in 0..max_workers {
            let initially_parked = id >= min_workers;
            let park = Arc::new(WorkerPark::new(initially_parked));
            worker_parks.push(Arc::clone(&park));

            let high_rx = high_rx.clone();
            let low_rx = low_rx.clone();
            let response_tx = response_tx.clone();
            let shutdown = Arc::clone(&shutdown);

            let handle = thread::Builder::new()
                .name(format!("mettatron-gc-pool-{}", id))
                .spawn(move || {
                    gc_pool_worker_loop(id, high_rx, low_rx, response_tx, shutdown, park);
                })
                .expect("failed to spawn GC pool worker thread");

            workers.push(parking_lot::Mutex::new(Some(handle)));
        }

        debug!(min_workers, max_workers, "AdaptiveGcPool started");

        Self {
            high_tx,
            low_tx,
            high_rx,
            low_rx,
            response_tx,
            response_rx,
            workers,
            worker_parks,
            shutdown,
            active_count: AtomicUsize::new(min_workers),
            scale_lock: parking_lot::Mutex::new(()),
            min_workers,
            max_workers,
            #[cfg(feature = "track-stats")]
            session_release_count: AtomicU32::new(0),
        }
    }

    /// Submit a high-priority GC collection work item.
    pub fn submit_high(&self, item: GcWorkItem) {
        let _ = self.high_tx.send(item);
    }

    /// Submit a low-priority session release work item.
    pub fn submit_low(&self, item: GcWorkItem) {
        let _ = self.low_tx.send(item);
    }

    /// Try to receive a GC response (non-blocking).
    pub fn try_recv_response(&self) -> Option<GcResponse> {
        self.response_rx.try_recv().ok()
    }

    /// Block until a GC response is available.
    pub fn recv_response_blocking(&self) -> Option<GcResponse> {
        self.response_rx.recv().ok()
    }

    /// Get the number of currently active (non-parked) workers.
    pub fn active_workers(&self) -> usize {
        self.active_count.load(Ordering::Relaxed)
    }

    /// Get the minimum worker count.
    pub fn min_workers(&self) -> usize {
        self.min_workers
    }

    /// Get the maximum worker count.
    pub fn max_workers(&self) -> usize {
        self.max_workers
    }

    /// Get the count of completed session releases.
    #[cfg(feature = "track-stats")]
    pub fn session_release_count(&self) -> u32 {
        self.session_release_count.load(Ordering::Relaxed)
    }

    /// Increment the session release counter (called by workers).
    #[cfg(feature = "track-stats")]
    fn bump_session_release_count(&self) {
        self.session_release_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Unpark one worker (called by the GC scaling monitor).
    pub fn unpark_one(&self) -> bool {
        let _scale = self.scale_lock.lock();
        for park in &self.worker_parks {
            if park.try_unpark() {
                self.active_count.fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Unpark up to `n` workers. Returns the number actually unparked.
    pub fn unpark_n(&self, n: usize) -> usize {
        let _scale = self.scale_lock.lock();
        let mut unparked = 0;
        for park in &self.worker_parks {
            if unparked >= n {
                break;
            }
            if park.try_unpark() {
                self.active_count.fetch_add(1, Ordering::Relaxed);
                unparked += 1;
            }
        }
        unparked
    }

    /// Park one worker (called by the GC scaling monitor).
    /// Does not park below `min_workers`.
    pub fn park_one(&self) -> bool {
        let _scale = self.scale_lock.lock();
        let active = self.active_count.load(Ordering::Relaxed);
        if active <= self.min_workers {
            return false;
        }

        for park in self.worker_parks.iter().rev() {
            if park.try_park() {
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                return true;
            }
        }
        false
    }

    /// Park up to `n` workers. Returns the number actually parked.
    /// Does not park below `min_workers`.
    pub fn park_n(&self, n: usize) -> usize {
        let _scale = self.scale_lock.lock();
        let mut parked = 0;
        for park in self.worker_parks.iter().rev() {
            if parked >= n {
                break;
            }
            let active = self.active_count.load(Ordering::Relaxed);
            if active <= self.min_workers {
                break;
            }
            if park.try_park() {
                self.active_count.fetch_sub(1, Ordering::Relaxed);
                parked += 1;
            }
        }
        parked
    }

    /// Check for dead workers and respawn them.
    ///
    /// Iterates all worker slots, checks `is_finished()` (non-blocking),
    /// and respawns any dead workers. Returns the number of workers respawned.
    ///
    /// Called periodically by the memory monitor in `gc_cron.rs`.
    pub fn check_and_respawn_workers(&self) -> usize {
        let mut respawned = 0;

        for (id, slot) in self.workers.iter().enumerate() {
            let mut guard = slot.lock();
            let is_dead = match guard.as_ref() {
                Some(handle) => handle.is_finished(),
                None => true,
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
                            "AdaptiveGcPool: worker exited unexpectedly -- respawning"
                        );
                    }
                    Err(payload) => {
                        tracing::error!(
                            worker_id = id,
                            panic = ?payload,
                            "AdaptiveGcPool: worker panicked -- respawning"
                        );
                    }
                }
            }

            // Respawn with cloned shared state
            let park = Arc::clone(&self.worker_parks[id]);
            let _scale = self.scale_lock.lock();
            if park.try_unpark() {
                self.active_count.fetch_add(1, Ordering::Relaxed);
            }
            let high_rx = self.high_rx.clone();
            let low_rx = self.low_rx.clone();
            let response_tx = self.response_tx.clone();
            let shutdown = Arc::clone(&self.shutdown);

            let new_handle = thread::Builder::new()
                .name(format!("mettatron-gc-pool-{}", id))
                .spawn(move || {
                    gc_pool_worker_loop(id, high_rx, low_rx, response_tx, shutdown, park);
                })
                .expect("failed to respawn GC pool worker thread");

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

        // Send shutdown signals to both channels
        for _ in 0..self.max_workers {
            let _ = self.high_tx.send(GcWorkItem::Shutdown);
        }
    }
}

impl Drop for AdaptiveGcPool {
    fn drop(&mut self) {
        self.shutdown();
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

/// Worker thread main loop for the adaptive GC pool.
///
/// Priority dequeue:
/// 1. Check park flag → if parked, block on condvar
/// 2. Try HIGH channel (non-blocking) → if item, execute
/// 3. Try LOW channel (with timeout) → if item, execute
/// 4. Loop back to 1
fn gc_pool_worker_loop(
    _id: usize,
    high_rx: Receiver<GcWorkItem>,
    low_rx: Receiver<GcWorkItem>,
    response_tx: Sender<GcResponse>,
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

        // Priority dequeue: HIGH first (non-blocking), then LOW (with timeout)
        let item = match high_rx.try_recv() {
            Ok(item) => item,
            Err(_) => {
                // No high-priority work — try low-priority with timeout
                match low_rx.recv_timeout(LOW_CHANNEL_TIMEOUT) {
                    Ok(item) => item,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        };

        match item {
            GcWorkItem::Collect(mut snapshot) => {
                // Acquire read lock on PAGE_LIFECYCLE_LOCK: permits concurrent
                // mark/sweep operations but blocks release_empty_pages() (which
                // takes a write lock) from munmapping pages we're traversing.
                let _page_guard = super::gc_allocator::PAGE_LIFECYCLE_LOCK.read();

                // Wrap mark+sweep in catch_unwind. On panic: log, drop snapshot,
                // continue loop. The caller will timeout waiting for a response.
                let collect_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    // Mark phase: trace from roots (operates on snapshot's mark bitmaps)
                    mark_snapshot(&mut snapshot);

                    // Sweep phase: build response (full sweep, no watermark)
                    sweep_snapshot(&snapshot)
                }));

                // Release read lock before sending response — munmap can proceed
                // once mark/sweep is done.
                drop(_page_guard);

                match collect_result {
                    Ok(response) => {
                        // Send response back to eval thread.
                        // Channel disconnect is NOT fatal — the worker should
                        // keep processing other items and respect the shutdown signal.
                        if response_tx.send(response).is_err() {
                            warn!("GC pool: response channel disconnected -- worker continues");
                        }
                    }
                    Err(payload) => {
                        tracing::error!(
                            panic = ?payload,
                            "GC pool: mark/sweep panicked -- dropping snapshot, worker continues"
                        );
                    }
                }
            }

            GcWorkItem::SessionRelease { context_ids } => {
                // Wrap session release in catch_unwind. On panic: log, continue.
                let release_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_session_release(&context_ids);
                }));

                if let Err(payload) = &release_result {
                    tracing::error!(
                        panic = ?payload,
                        "GC pool: session release panicked -- worker continues"
                    );
                }

                // Track for throughput monitoring (only on success)
                #[cfg(feature = "track-stats")]
                if release_result.is_ok() {
                    if let Some(pool) = GLOBAL_GC_POOL.get() {
                        pool.bump_session_release_count();
                    }
                }
            }

            GcWorkItem::Shutdown => {
                break;
            }
        }
    }
}

// ============================================================================
// Session Release Execution
// ============================================================================

/// Execute a batch of session releases with quiescence coordination.
///
/// This is the same protocol as the original `session_release_thread_main()`:
/// 1. Wait for ACTIVE_EVALUATORS == 0
/// 2. CAS on GC_IN_PROGRESS (mutual exclusion with maybe_quiescent_gc)
/// 3. Double-check no eval snuck in
/// 4. Trace surviving set
/// 5. Release sessions
/// Maximum time to wait for quiescence before retrying.
/// Prevents deadlock in multi-threaded test environments where
/// `ACTIVE_EVALUATORS` may never reach 0 because other test threads
/// are continuously evaluating.
const QUIESCENCE_TIMEOUT: Duration = Duration::from_secs(5);

/// Maximum number of quiescence retries before re-enqueuing.
/// 10 retries × 5s timeout = 50s max wait before giving up and re-enqueuing.
const MAX_QUIESCENCE_RETRIES: u32 = 10;

fn execute_session_release(context_ids: &[u32]) {
    use super::gc_allocator::{
        gc_cycle_in_flight, global_allocator, wait_for_gc_cycle_idle, GcInProgressGuard,
        ACTIVE_EVALUATORS, GC_IN_PROGRESS, GC_PROGRESS_CONDVAR, GC_PROGRESS_MUTEX,
        INHIBITOR_CONDVAR, INHIBITOR_MUTEX, QUIESCENT_CONDVAR, QUIESCENT_MUTEX,
        SESSION_RELEASE_INHIBITORS,
    };

    let mut retries = 0u32;

    'quiescence: loop {
        // Bounded retry: after MAX_QUIESCENCE_RETRIES, re-enqueue and return.
        // This prevents infinite spin when evaluators are permanently active
        // (e.g., long-running parallel workloads).
        if retries >= MAX_QUIESCENCE_RETRIES {
            warn!(
                retries = retries,
                context_ids = ?context_ids,
                "Quiescence not reached after {} retries ({}s) -- re-enqueuing session release",
                MAX_QUIESCENCE_RETRIES,
                MAX_QUIESCENCE_RETRIES as u64 * QUIESCENCE_TIMEOUT.as_secs(),
            );
            // Re-enqueue so sessions are not leaked
            if let Some(pool) = GLOBAL_GC_POOL.get() {
                pool.submit_low(GcWorkItem::SessionRelease {
                    context_ids: context_ids.to_vec(),
                });
            }
            return;
        }
        retries += 1;

        // === Wait for quiescent state (with timeout) ===
        // H10 dual-counter protocol: session-release waits for BOTH
        //   ACTIVE_EVALUATORS == 0 (no eval in progress)
        //   SESSION_RELEASE_INHIBITORS == 0 (no main-thread result-protection holds)
        // Mark-sweep checks only ACTIVE_EVALUATORS, so it can proceed during
        // long top-level evals while session-release waits its turn.
        {
            let mut lock = QUIESCENT_MUTEX.lock();
            while ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
                let result = QUIESCENT_CONDVAR.wait_for(&mut lock, QUIESCENCE_TIMEOUT);
                if result.timed_out() && ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
                    // Quiescence not reached within timeout — retry the
                    // outer loop which re-checks. This prevents indefinite
                    // blocking when other threads keep evaluators active
                    // (e.g., parallel test harness).
                    continue 'quiescence;
                }
            }
        }
        {
            let mut lock = INHIBITOR_MUTEX.lock();
            while SESSION_RELEASE_INHIBITORS.load(Ordering::Acquire) > 0 {
                let result = INHIBITOR_CONDVAR.wait_for(&mut lock, QUIESCENCE_TIMEOUT);
                if result.timed_out() && SESSION_RELEASE_INHIBITORS.load(Ordering::Acquire) > 0 {
                    continue 'quiescence;
                }
            }
        }
        // Re-verify ACTIVE_EVALUATORS is still 0 after waiting on inhibitors
        // (an evaluator could have entered between the two waits).
        if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0 {
            continue 'quiescence;
        }

        // A session release physically frees session-owned slots. Do not let it
        // overlap an outstanding mark/sweep response: that response's dead set
        // was computed against the pre-release slot state and can otherwise
        // double-free a slot the session release has already returned.
        if !wait_for_gc_cycle_idle(QUIESCENCE_TIMEOUT) {
            continue 'quiescence;
        }
        if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0
            || SESSION_RELEASE_INHIBITORS.load(Ordering::Acquire) > 0
        {
            continue 'quiescence;
        }

        // === Acquire GC_IN_PROGRESS via RAII guard (CAS) ===
        // The guard clears GC_IN_PROGRESS on drop (including panics),
        // ensuring evaluators are never permanently blocked.
        let gc_guard = loop {
            match GcInProgressGuard::try_enter() {
                Some(guard) => break guard,
                None => {
                    // Another thread holds GC_IN_PROGRESS — park until released.
                    // Uses timeout to avoid permanent hang from lost condvar
                    // notifications (race between CAS failure and lock acquire).
                    let mut lock = GC_PROGRESS_MUTEX.lock();
                    while GC_IN_PROGRESS.load(Ordering::Acquire) {
                        let result = GC_PROGRESS_CONDVAR.wait_for(&mut lock, QUIESCENCE_TIMEOUT);
                        if result.timed_out() && GC_IN_PROGRESS.load(Ordering::Acquire) {
                            // GC_IN_PROGRESS still held after timeout — retry
                            // the outer quiescence loop (which has bounded retries)
                            drop(lock);
                            continue 'quiescence;
                        }
                    }
                }
            }
        };

        // Double-check: no eval or result-protection hold snuck in between
        // the quiescence waits and the GC_IN_PROGRESS CAS. The waits above
        // are not atomic with respect to a new top-level session entering
        // GcHoldGuard, so both counters must be rechecked after the guard is
        // acquired.
        if ACTIVE_EVALUATORS.load(Ordering::Acquire) > 0
            || SESSION_RELEASE_INHIBITORS.load(Ordering::Acquire) > 0
        {
            drop(gc_guard); // clears GC_IN_PROGRESS + notifies waiters
            continue 'quiescence;
        }
        if gc_cycle_in_flight() {
            drop(gc_guard);
            continue 'quiescence;
        }

        // Serialize with the periodic exec-counter sync. If a sync task has
        // already begun scanning live slots, wait for any pending compile root
        // registration to finish before computing the surviving set.
        let _counter_flush_guard = super::gc_cron::COUNTER_FLUSH_LOCK.lock();

        // === Safe: trace roots at quiescent point ===
        let alloc = global_allocator();
        let mut surviving = alloc.trace_surviving_set();

        // Safety net: merge safepoint + environment live set into surviving set.
        // Mirrors the filter in process_gc_response. Prevents freeing values that
        // are reachable from current safepoint roots or environment roots but were
        // missed by trace_surviving_set() due to transiently dead Weak references
        // in the root registry (e.g., from deferred_shared_drops.clear()).
        let (safepoint_live, _env_complete) = super::gc_allocator::trace_safepoint_live_set();
        if let (Some(live), _) = (safepoint_live, _env_complete) {
            for ptr in live.iter() {
                surviving.insert(*ptr);
            }
        }

        // Release sessions while GC_IN_PROGRESS is still held. Session
        // release frees slots via free_list.push(). If we released the
        // guard first, evaluators would wake up and could pop()+
        // write_slot_bytes() on a slot that push() is concurrently
        // inserting, causing a CAS-loop race on FreeNode::next that
        // corrupts the Treiber stack.
        for context_id in context_ids {
            alloc.release_session_with_surviving(*context_id, &surviving);
        }

        // Drop the RAII guard: clears GC_IN_PROGRESS and wakes evaluators.
        // All frees are complete, so pop()+write_slot_bytes() is safe.
        drop(gc_guard);

        break 'quiescence;
    }
}

// ============================================================================
// Global Singleton
// ============================================================================

/// Global adaptive GC pool singleton. Lazily spawned on first use.
static GLOBAL_GC_POOL: OnceLock<AdaptiveGcPool> = OnceLock::new();

/// Get the global adaptive GC pool, spawning it if needed.
pub fn global_gc_pool() -> &'static AdaptiveGcPool {
    GLOBAL_GC_POOL.get_or_init(AdaptiveGcPool::new)
}

// ============================================================================
// Tests
// ============================================================================

// (cfg-gate) These tests exercise the slab adaptive GC pool internals directly
// (GcFactory/SlabAllocator, live-data preservation across pooled collectors).
// Under `--features index-gc` the active store is the index arena and the
// process decodes index handles, so slab values produced here aren't
// interpretable by that runtime. Slab-internal — slab build only.
#[cfg(all(test, not(feature = "index-gc")))]
mod tests {
    use super::super::gc_allocator::{GcFactory, SlabAllocator};
    use super::super::metta_value_trait::MettaValueFactory;
    use super::*;

    /// Helper to create a test allocator with 'static lifetime.
    fn test_alloc() -> &'static SlabAllocator {
        Box::leak(Box::new(SlabAllocator::new()))
    }

    #[test]
    fn test_gc_pool_spawn_and_shutdown() {
        let pool = AdaptiveGcPool::new();
        assert!(pool.active_workers() >= pool.min_workers());
        assert!(pool.active_workers() <= pool.max_workers());
        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_collect_cycle() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let pool = AdaptiveGcPool::new();

        // Allocate values (slab-allocated types for NaN-boxing compatibility)
        let alive = factory.atom("alive");
        let _dead = factory.atom("dead");

        // Build snapshot and submit
        let snapshot = alloc.build_snapshot(vec![alive]);
        pool.submit_high(GcWorkItem::Collect(snapshot));

        // Wait for response
        let response = pool
            .recv_response_blocking()
            .expect("should receive GC response");

        assert!(
            response.dead_values.len() >= 1,
            "expected at least 1 dead value, got {}",
            response.dead_values.len()
        );

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_multiple_cycles() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let pool = AdaptiveGcPool::new();

        let names_alive = ["a0", "a1", "a2", "a3", "a4"];
        let names_dead = ["d0", "d1", "d2", "d3", "d4"];
        for i in 0..5 {
            let alive = factory.atom(names_alive[i]);
            let _dead = factory.atom(names_dead[i]);

            let snapshot = alloc.build_snapshot(vec![alive]);
            pool.submit_high(GcWorkItem::Collect(snapshot));

            let response = pool
                .recv_response_blocking()
                .expect("should receive GC response");
            alloc.process_gc_response(&response);
        }

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_try_recv_nonblocking() {
        let pool = AdaptiveGcPool::new();

        // No work submitted — try_recv should return None
        assert!(pool.try_recv_response().is_none());

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_preserves_live_data() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let pool = AdaptiveGcPool::new();

        // Create a complex live structure (slab-allocated types for NaN-boxing compatibility)
        let root = factory.sexpr(vec![
            factory.atom("+"),
            factory.atom("one"),
            factory.sexpr(vec![
                factory.atom("*"),
                factory.atom("two"),
                factory.atom("three"),
            ]),
        ]);

        let snapshot = alloc.build_snapshot(vec![root]);
        pool.submit_high(GcWorkItem::Collect(snapshot));

        let response = pool
            .recv_response_blocking()
            .expect("should receive GC response");
        alloc.process_gc_response(&response);

        // Root and all children should still be accessible
        let items = root.as_sexpr().expect("root is sexpr");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_atom(), Some("+"));
        assert_eq!(items[1].as_atom(), Some("one"));
        let inner = items[2].as_sexpr().expect("inner is sexpr");
        assert_eq!(inner[0].as_atom(), Some("*"));

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_park_unpark() {
        let pool = AdaptiveGcPool::new();
        let initial_active = pool.active_workers();

        // Try to unpark one
        if initial_active < pool.max_workers() {
            assert!(pool.unpark_one());
            assert_eq!(pool.active_workers(), initial_active + 1);

            // Park it back
            assert!(pool.park_one());
            assert_eq!(pool.active_workers(), initial_active);
        }

        // Cannot park below min
        while pool.active_workers() > pool.min_workers() {
            assert!(pool.park_one());
        }
        assert!(!pool.park_one());

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_active_count_tracks_successful_park_transitions() {
        let pool = AdaptiveGcPool::with_workers(2, 4);
        assert_eq!(pool.active_workers(), 2);

        assert_eq!(pool.unpark_n(10), 2);
        assert_eq!(pool.active_workers(), 4);
        assert_eq!(pool.unpark_n(10), 0);
        assert_eq!(pool.active_workers(), 4);

        assert_eq!(pool.park_n(10), 2);
        assert_eq!(pool.active_workers(), 2);
        assert_eq!(pool.park_n(10), 0);
        assert_eq!(pool.active_workers(), 2);

        pool.shutdown();
    }

    #[test]
    fn test_gc_pool_global_singleton() {
        let pool = global_gc_pool();
        assert!(pool.active_workers() >= pool.min_workers());
    }

    /// Phase 7c: Verify GC pool workers survive a panicking task.
    ///
    /// Submits a GC collect cycle with a valid snapshot after the pool has been
    /// running — ensures the worker loop's `catch_unwind` keeps workers alive.
    #[test]
    fn test_gc_pool_survives_collect_cycle() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let pool = AdaptiveGcPool::with_workers(1, 2);

        // First cycle — normal
        let alive = factory.long(1);
        let snapshot = alloc.build_snapshot(vec![alive]);
        pool.submit_high(GcWorkItem::Collect(snapshot));
        let resp = pool
            .recv_response_blocking()
            .expect("should receive first GC response");
        alloc.process_gc_response(&resp);

        // Second cycle — still works (workers survived)
        let alive2 = factory.long(2);
        let snapshot2 = alloc.build_snapshot(vec![alive2]);
        pool.submit_high(GcWorkItem::Collect(snapshot2));
        let resp2 = pool
            .recv_response_blocking()
            .expect("should receive second GC response");
        alloc.process_gc_response(&resp2);

        pool.shutdown();
    }
}
