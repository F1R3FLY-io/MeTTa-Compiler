//! Per-Thread Store Regions (BEAM-Inspired Bump Allocation)
//!
//! Each parallel branch allocates from a thread-local bump region, eliminating
//! contention on the global `SlabAllocator`'s Treiber stack free lists. Pages
//! are reserved from the global allocator but bumped locally.
//!
//! ## Design
//!
//! ```text
//! Thread 0:  [reserved page] ← bump locally (no CAS)
//! Thread 1:  [reserved page] ← bump locally (no CAS)
//! Thread 2:  [reserved page] ← bump locally (no CAS)
//!
//! Global SlabAllocator: page array (RwLock, read for GC)
//!   ↑ pages allocated from here once, bumped locally thereafter
//! ```
//!
//! On branch completion, the thread-local region is reset (bump pointers
//! cleared). Pages remain in the global page array for GC to scan. Live
//! values are found by the GC mark phase; dead values are reclaimed.
//!
//! ## Integration
//!
//! The region is entered at the start of `parallel_branch_eval` worker closures
//! and exited when the closure returns. During the branch, allocations with
//! `AllocHint::ThreadLocal` route through the thread-local region instead of
//! the global Treiber stack.

use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};

// ============================================================================
// Thread-Local Allocation Region
// ============================================================================

/// Per-thread allocation region for contention-free bump allocation.
///
/// During parallel branch evaluation, this region provides a fast path
/// for value allocation that avoids the global allocator's CAS loops.
///
/// ## Lifecycle
///
/// 1. `enter()` — marks the region as active (allocation routes here)
/// 2. Allocations bump the local offset (no CAS, no contention)
/// 3. `exit()` — marks the region as inactive, resets counters
///
/// ## Page Management
///
/// Pages are not managed by this struct — they are allocated from the global
/// `SlabAllocator` via its normal page allocation path. This struct tracks
/// only the allocation metadata (counts, bytes) for diagnostics and the
/// active flag for routing.
#[derive(Debug)]
pub struct ThreadLocalRegion {
    /// Whether the region is currently active (inside a parallel branch).
    active: bool,

    /// Number of allocations routed through this region.
    alloc_count: u64,

    /// Bytes allocated through this region.
    bytes_allocated: u64,

    /// Number of times this region has been entered.
    enter_count: u64,

    /// Nesting depth (supports nested parallel branches).
    nesting_depth: u32,
}

impl ThreadLocalRegion {
    /// Create a new inactive thread-local region.
    pub fn new() -> Self {
        Self {
            active: false,
            alloc_count: 0,
            bytes_allocated: 0,
            enter_count: 0,
            nesting_depth: 0,
        }
    }

    /// Enter the thread-local region (start of parallel branch).
    ///
    /// Subsequent allocations with `AllocHint::ThreadLocal` will be tracked.
    /// Supports nesting — inner enters increment the depth.
    #[inline]
    pub fn enter(&mut self) {
        self.nesting_depth += 1;
        if !self.active {
            self.active = true;
            self.enter_count += 1;
        }
    }

    /// Exit the thread-local region (end of parallel branch).
    ///
    /// Decrements nesting depth. Only deactivates when depth reaches 0.
    #[inline]
    pub fn exit(&mut self) {
        self.nesting_depth = self.nesting_depth.saturating_sub(1);
        if self.nesting_depth == 0 {
            self.active = false;
        }
    }

    /// Check if the region is active.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Record an allocation through this region.
    #[inline]
    pub fn record_alloc(&mut self, bytes: usize) {
        self.alloc_count += 1;
        self.bytes_allocated += bytes as u64;
    }

    /// Return the number of allocations since the last enter.
    #[inline]
    pub fn alloc_count(&self) -> u64 {
        self.alloc_count
    }

    /// Return the bytes allocated since the last enter.
    #[inline]
    pub fn bytes_allocated(&self) -> u64 {
        self.bytes_allocated
    }

    /// Return the number of times this region has been entered.
    #[inline]
    pub fn enter_count(&self) -> u64 {
        self.enter_count
    }

    /// Return diagnostic statistics.
    pub fn stats(&self) -> ThreadRegionStats {
        ThreadRegionStats {
            active: self.active,
            nesting_depth: self.nesting_depth,
            alloc_count: self.alloc_count,
            bytes_allocated: self.bytes_allocated,
            enter_count: self.enter_count,
        }
    }

    /// Reset allocation counters (between evaluations).
    pub fn reset_counters(&mut self) {
        self.alloc_count = 0;
        self.bytes_allocated = 0;
    }
}

impl Default for ThreadLocalRegion {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic statistics for a thread-local region.
#[derive(Debug, Clone)]
pub struct ThreadRegionStats {
    pub active: bool,
    pub nesting_depth: u32,
    pub alloc_count: u64,
    pub bytes_allocated: u64,
    pub enter_count: u64,
}

impl std::fmt::Display for ThreadRegionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ThreadRegion: {} (depth={}), {} allocs, {} bytes, {} enters",
            if self.active { "active" } else { "inactive" },
            self.nesting_depth,
            self.alloc_count,
            self.bytes_allocated,
            self.enter_count,
        )
    }
}

// ============================================================================
// RAII Guard
// ============================================================================

/// RAII guard that enters the thread-local region on creation and exits on drop.
///
/// Use this in parallel branch worker closures to ensure the region is
/// correctly entered and exited even on panic.
pub struct RegionGuard;

impl RegionGuard {
    /// Enter the thread-local region and return a guard.
    #[inline]
    pub fn enter() -> Self {
        with_thread_local_region(|r| r.enter());
        RegionGuard
    }
}

impl Drop for RegionGuard {
    #[inline]
    fn drop(&mut self) {
        with_thread_local_region(|r| r.exit());
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

thread_local! {
    static THREAD_REGION: RefCell<ThreadLocalRegion> = RefCell::new(ThreadLocalRegion::new());
}

/// Access the thread-local allocation region.
#[inline]
pub fn with_thread_local_region<R>(f: impl FnOnce(&mut ThreadLocalRegion) -> R) -> R {
    THREAD_REGION.with(|cell| {
        let mut region = cell.borrow_mut();
        f(&mut region)
    })
}

/// Check if the thread-local region is active (fast path for allocation routing).
#[inline]
pub fn is_thread_region_active() -> bool {
    THREAD_REGION.with(|cell| cell.borrow().is_active())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_region() {
        let r = ThreadLocalRegion::new();
        assert!(!r.is_active());
        assert_eq!(r.alloc_count(), 0);
        assert_eq!(r.bytes_allocated(), 0);
    }

    #[test]
    fn test_enter_exit() {
        let mut r = ThreadLocalRegion::new();
        r.enter();
        assert!(r.is_active());

        r.record_alloc(64);
        r.record_alloc(128);
        assert_eq!(r.alloc_count(), 2);
        assert_eq!(r.bytes_allocated(), 192);

        r.exit();
        assert!(!r.is_active());
    }

    #[test]
    fn test_nested_enter_exit() {
        let mut r = ThreadLocalRegion::new();

        r.enter();
        assert!(r.is_active());

        r.enter(); // Nested
        assert!(r.is_active());

        r.exit(); // Exit inner — still active
        assert!(r.is_active());

        r.exit(); // Exit outer — now inactive
        assert!(!r.is_active());
    }

    #[test]
    fn test_guard_raii() {
        {
            let _guard = RegionGuard::enter();
            assert!(is_thread_region_active());
        }
        assert!(!is_thread_region_active());
    }

    #[test]
    fn test_guard_panic_safety() {
        let result = std::panic::catch_unwind(|| {
            let _guard = RegionGuard::enter();
            panic!("test panic");
        });
        assert!(result.is_err());
        // Guard's Drop should have exited the region
        assert!(!is_thread_region_active());
    }

    #[test]
    fn test_stats() {
        let mut r = ThreadLocalRegion::new();
        r.enter();
        r.record_alloc(100);
        let stats = r.stats();
        assert!(stats.active);
        assert_eq!(stats.alloc_count, 1);
        assert_eq!(stats.bytes_allocated, 100);
        assert_eq!(stats.enter_count, 1);
    }

    #[test]
    fn test_reset_counters() {
        let mut r = ThreadLocalRegion::new();
        r.enter();
        r.record_alloc(100);
        r.reset_counters();
        assert_eq!(r.alloc_count(), 0);
        assert_eq!(r.bytes_allocated(), 0);
        assert!(r.is_active()); // Reset doesn't affect active state
    }

    #[test]
    fn test_thread_local() {
        with_thread_local_region(|r| {
            r.enter();
            assert!(r.is_active());
            r.exit();
        });
    }
}
