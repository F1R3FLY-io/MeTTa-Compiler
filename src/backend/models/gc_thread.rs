//! Background GC Thread for Snapshot-Based Async Mark-Sweep Collection
//!
//! This module provides a background GC thread that runs concurrently with
//! evaluation. The evaluation thread builds an immutable `GcSnapshot` and sends
//! it to the GC thread. The GC thread performs mark-sweep on the snapshot (using
//! its own mark bitmaps) and returns a `GcResponse` with dead values and the
//! snapshot epoch for TOCTOU filtering.
//!
//! ## Protocol (Fixes all three bugs from original design)
//!
//! ```text
//! Evaluation Thread                    GC Thread
//!       │                                  │
//!       │  ┌─ Between expressions ─┐       │
//!       │  │ 1. Process GcResponse │       │
//!       │  │    with EPOCH filter  │       │
//!       │  │ 2. Build GcSnapshot   │       │
//!       │  │    (copy page ptrs,   │       │
//!       │  │    free set, epoch,   │──────>│ 3. Receive owned snapshot
//!       │  │    roots, marks)      │       │ 4. Mark: trace from roots
//!       │  │ 3. Check pressure     │       │    (reads immutable values
//!       │  └───────────────────────┘       │     via stable page ptrs)
//!       │                                  │ 5. Sweep: build GcResponse
//!       │  ┌─ Evaluate next expr ──┐       │    (FULL sweep, no watermark)
//!       │  │ Allocates new values  │<──────│ 6. Send GcResponse + epoch
//!       │  │ GC runs concurrently  │       │ 7. Wait for next snapshot
//!       │  │ (SAFE: disjoint state)│       │
//!       │  └───────────────────────┘       │
//! ```
//!
//! ## Safety
//!
//! - **Bug 1 (TOCTOU)**: Fixed by epoch-based filtering. The eval thread skips
//!   dead set entries for slots re-allocated after the snapshot epoch.
//! - **Bug 2 (Data Race)**: Fixed by snapshot-based GC. The GC thread operates
//!   exclusively on the owned `GcSnapshot` — never touches live allocator state.
//!   MettaValues are immutable after creation, so reading value data is safe.
//! - **Bug 3 (Watermark)**: Fixed by full sweep. The GC sweeps ALL committed
//!   slots in the snapshot — no watermark restriction. Live byte counts are
//!   accurate for threshold calibration.

use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use super::gc_allocator::{mark_snapshot, sweep_snapshot, GcResponse, GcSnapshot};

// ============================================================================
// GC Request / Response
// ============================================================================

/// Request from evaluation thread to GC thread.
pub enum GcRequest {
    /// Perform a GC cycle with the given snapshot (owned by GC thread).
    Collect(GcSnapshot),
    /// Shut down the GC thread.
    Shutdown,
}

// SAFETY: GcSnapshot is Send (contains raw pointers but the eval thread
// does not access it after sending). See GcSnapshot's unsafe impl Send.
unsafe impl Send for GcRequest {}

// ============================================================================
// TryRecvGcResponse — Tri-State Result for Non-Blocking Receive
// ============================================================================

/// Result of a non-blocking GC response receive attempt.
///
/// Distinguishes between a response being available, no response yet
/// (GC thread still working), and the channel being disconnected
/// (GC thread crashed or shut down).
pub enum TryRecvGcResponse {
    /// GC cycle completed, response available.
    Response(GcResponse),
    /// No response available yet (GC thread still working).
    Empty,
    /// GC thread channel disconnected (thread crashed or shut down).
    Disconnected,
}

// ============================================================================
// GcThread
// ============================================================================

/// Handle to the background GC thread.
///
/// Spawned when `MettaState::new()` is called. Joined when `MettaState` is
/// dropped (sends `Shutdown` signal and waits).
pub struct GcThread {
    /// Channel to send requests to the GC thread.
    request_tx: mpsc::Sender<GcRequest>,
    /// Channel to receive responses from the GC thread.
    response_rx: mpsc::Receiver<GcResponse>,
    /// Thread handle for joining on drop.
    handle: Option<JoinHandle<()>>,
}

impl GcThread {
    /// Spawn a new GC thread.
    ///
    /// The GC thread receives owned `GcSnapshot`s, performs mark-sweep on them,
    /// and sends back `GcResponse`s. It never accesses the live `SlabAllocator`.
    pub fn spawn() -> Self {
        let (request_tx, request_rx) = mpsc::channel::<GcRequest>();
        let (response_tx, response_rx) = mpsc::channel::<GcResponse>();

        let handle = thread::Builder::new()
            .name("mettatron-gc".to_string())
            .spawn(move || {
                gc_thread_main(request_rx, response_tx);
            })
            .expect("failed to spawn GC thread");

        Self {
            request_tx,
            response_rx,
            handle: Some(handle),
        }
    }

    /// Request a GC cycle by sending an owned snapshot (non-blocking).
    ///
    /// The snapshot is transferred to the GC thread. The GC thread will
    /// perform mark-sweep on it and send back a `GcResponse`.
    pub fn request_gc(&self, snapshot: GcSnapshot) {
        let _ = self.request_tx.send(GcRequest::Collect(snapshot));
    }

    /// Try to receive a GC response (non-blocking).
    ///
    /// Returns a tri-state result distinguishing between a response being
    /// available, no response yet (GC thread still working), and the GC
    /// thread channel being disconnected (thread crashed or shut down).
    pub fn try_recv_response(&self) -> TryRecvGcResponse {
        match self.response_rx.try_recv() {
            Ok(response) => TryRecvGcResponse::Response(response),
            Err(mpsc::TryRecvError::Empty) => TryRecvGcResponse::Empty,
            Err(mpsc::TryRecvError::Disconnected) => TryRecvGcResponse::Disconnected,
        }
    }

    /// Block until the GC thread completes a cycle.
    ///
    /// Used for hard memory pressure when we must reclaim before continuing.
    pub fn recv_response_blocking(&self) -> Option<GcResponse> {
        match self.response_rx.recv() {
            Ok(response) => Some(response),
            Err(_) => None,
        }
    }

    /// Shut down the GC thread and wait for it to finish.
    pub fn shutdown(&mut self) {
        let _ = self.request_tx.send(GcRequest::Shutdown);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for GcThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ============================================================================
// GC Thread Main Loop
// ============================================================================

/// Main loop for the background GC thread.
///
/// Receives owned GcSnapshots, performs mark-sweep, and sends GcResponses.
/// Never accesses the live SlabAllocator — operates exclusively on snapshots.
fn gc_thread_main(request_rx: mpsc::Receiver<GcRequest>, response_tx: mpsc::Sender<GcResponse>) {
    loop {
        // Block waiting for the next request
        match request_rx.recv() {
            Ok(GcRequest::Collect(mut snapshot)) => {
                // Mark phase: trace from roots (operates on snapshot's mark bitmaps)
                mark_snapshot(&mut snapshot);

                // Sweep phase: build response (full sweep, no watermark)
                let response = sweep_snapshot(&snapshot);

                // Send response back to eval thread
                if response_tx.send(response).is_err() {
                    // Eval thread dropped its receiver — shut down
                    break;
                }
            }
            Ok(GcRequest::Shutdown) | Err(_) => {
                // Clean shutdown or channel closed
                break;
            }
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::super::gc_allocator::{GcFactory, SlabAllocator};
    use super::super::metta_value_trait::MettaValueFactory;
    use super::*;

    /// Helper to create a test allocator with 'static lifetime.
    fn test_alloc() -> &'static SlabAllocator {
        Box::leak(Box::new(SlabAllocator::new()))
    }

    #[test]
    fn test_gc_thread_spawn_and_shutdown() {
        let mut gc = GcThread::spawn();
        gc.shutdown();
        // Should not panic
    }

    #[test]
    fn test_gc_thread_collect_cycle() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let mut gc = GcThread::spawn();

        // Allocate values (slab-allocated types for NaN-boxing compatibility)
        let alive = factory.atom("alive");
        let _dead = factory.atom("dead");

        // Build snapshot and request GC
        let snapshot = alloc.build_snapshot(vec![alive]);
        gc.request_gc(snapshot);

        // Wait for response
        let response = gc
            .recv_response_blocking()
            .expect("should receive response");

        // Should have collected the dead value
        assert!(
            response.dead_values.len() >= 1,
            "expected at least 1 dead value, got {}",
            response.dead_values.len()
        );

        gc.shutdown();
    }

    #[test]
    fn test_gc_thread_multiple_cycles() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let mut gc = GcThread::spawn();

        let names_alive = ["a0", "a1", "a2", "a3", "a4"];
        let names_dead = ["d0", "d1", "d2", "d3", "d4"];
        for i in 0..5 {
            let alive = factory.atom(names_alive[i]);
            let _dead = factory.atom(names_dead[i]);

            let snapshot = alloc.build_snapshot(vec![alive]);
            gc.request_gc(snapshot);

            let response = gc
                .recv_response_blocking()
                .expect("should receive response");
            alloc.process_gc_response(&response);
        }

        gc.shutdown();
    }

    #[test]
    fn test_gc_thread_try_recv_nonblocking() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let mut gc = GcThread::spawn();

        // No request sent yet — try_recv should return Empty
        assert!(matches!(gc.try_recv_response(), TryRecvGcResponse::Empty));

        // Send a request
        let alive = factory.atom("alive");
        let snapshot = alloc.build_snapshot(vec![alive]);
        gc.request_gc(snapshot);

        // Wait a bit for the GC thread to process
        thread::sleep(Duration::from_millis(50));

        // Should now have a response
        let response = gc.try_recv_response();
        assert!(
            matches!(response, TryRecvGcResponse::Response(_)),
            "expected response after waiting"
        );

        gc.shutdown();
    }

    #[test]
    fn test_gc_thread_preserves_live_data() {
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let mut gc = GcThread::spawn();

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
        gc.request_gc(snapshot);

        let response = gc
            .recv_response_blocking()
            .expect("should receive response");
        alloc.process_gc_response(&response);

        // Root and all children should still be accessible
        let items = root.as_sexpr().expect("root is sexpr");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_atom(), Some("+"));
        assert_eq!(items[1].as_atom(), Some("one"));
        let inner = items[2].as_sexpr().expect("inner is sexpr");
        assert_eq!(inner[0].as_atom(), Some("*"));

        gc.shutdown();
    }

    #[test]
    fn test_gc_thread_drop_triggers_shutdown() {
        {
            let _gc = GcThread::spawn();
            // GcThread dropped here — should trigger shutdown
        }
        // Should not hang or panic
    }

    #[test]
    fn test_gc_thread_epoch_filtering() {
        // Test that epoch filtering prevents freeing re-allocated slots
        let alloc = test_alloc();
        let factory = GcFactory::new(alloc);
        let mut gc = GcThread::spawn();

        // Allocate a value, free it, then re-allocate from free list
        // (use slab-allocated types for NaN-boxing compatibility)
        let v1 = factory.atom("first");
        let v1_ptr = v1.inner_ptr() as *mut u8;

        // Free v1 to put it on the free list
        unsafe {
            alloc.free_value(v1_ptr);
        }

        // Build snapshot with empty roots BEFORE re-allocation
        // (simulates the TOCTOU scenario)
        let snapshot = alloc.build_snapshot(vec![]);
        let snapshot_epoch = snapshot.snapshot_epoch;

        // Now re-allocate from free list — this increments epoch
        let v2 = factory.atom("second");
        let v2_ptr = v2.inner_ptr() as *mut u8;
        assert_eq!(v1_ptr, v2_ptr, "should reuse the freed slot");

        // The slot epoch should be > snapshot epoch
        assert!(
            alloc.is_realloc_after_epoch(v2_ptr as *const u8, snapshot_epoch),
            "re-allocated slot should have epoch > snapshot epoch"
        );

        gc.request_gc(snapshot);
        let response = gc
            .recv_response_blocking()
            .expect("should receive response");

        // process_gc_response should filter out the re-allocated slot
        alloc.process_gc_response(&response);

        // v2 should still be valid (not freed by GC)
        assert_eq!(
            v2.as_atom(),
            Some("second"),
            "re-allocated value should survive GC"
        );

        gc.shutdown();
    }
}
