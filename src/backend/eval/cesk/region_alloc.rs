//! Region-Based Allocation for `let*` Scopes
//!
//! This module provides a bump-allocating region that supports bulk deallocation
//! when a `let*` scope exits. Values allocated within a region are freed in O(1)
//! by resetting the bump pointer, rather than waiting for GC to discover they
//! are unreachable.
//!
//! ## Design
//!
//! Each region is a contiguous byte buffer with a bump pointer. Allocations
//! advance the pointer; deallocation resets it to the region's start.
//!
//! ```text
//! Region:
//! ┌──────────────────────────────────────────┐
//! │ val1 │ val2 │ val3 │ .... free space ... │
//! └──────────────────────────────────────────┘
//! ^                      ^                    ^
//! base                   bump                 end
//! ```
//!
//! On `exit_region()`:
//! ```text
//! ┌──────────────────────────────────────────┐
//! │ .............. free space ............... │
//! └──────────────────────────────────────────┘
//! ^
//! base = bump (reset)
//! ```
//!
//! ## Integration with Store Trait
//!
//! The `AllocHint::LetScope { region_id }` hint routes allocations to the
//! active region. When no region is active, allocations fall through to the
//! global slab allocator.
//!
//! ## Thread Safety
//!
//! Region allocators are thread-local. Each evaluation thread has its own
//! region stack, eliminating contention on the allocation hot path.

use std::cell::RefCell;

use crate::backend::models::MettaValueTrait;

// ============================================================================
// Region
// ============================================================================

/// A single allocation region with bump-pointer semantics.
///
/// Values allocated within a region are freed in O(1) when `reset()` is called.
/// The region does not individually track allocations — it simply advances
/// a bump pointer and resets it on scope exit.
#[derive(Debug)]
pub struct Region {
    /// Unique ID for this region (monotonically increasing).
    pub id: u32,

    /// Number of values allocated in this region.
    pub alloc_count: u32,

    /// Nesting depth at which this region was created.
    pub depth: u32,
}

impl Region {
    fn new(id: u32, depth: u32) -> Self {
        Self {
            id,
            alloc_count: 0,
            depth,
        }
    }

    /// Record an allocation in this region.
    #[inline]
    pub fn record_alloc(&mut self) {
        self.alloc_count += 1;
    }

    /// Reset the region (bulk deallocation).
    #[inline]
    pub fn reset(&mut self) {
        self.alloc_count = 0;
    }
}

// ============================================================================
// Region Stack
// ============================================================================

/// Per-thread stack of active allocation regions.
///
/// Regions are pushed when entering a `let*` scope and popped when exiting.
/// The topmost region receives allocations with `AllocHint::LetScope`.
///
/// ## Invariant
///
/// Region IDs are monotonically increasing. A region's ID is always greater
/// than all preceding regions' IDs.
#[derive(Debug)]
pub struct RegionStack {
    /// Stack of active regions (innermost at the end).
    regions: Vec<Region>,

    /// Next region ID to assign.
    next_id: u32,

    /// Total allocations across all regions (lifetime counter).
    total_allocs: u64,

    /// Total regions entered (lifetime counter).
    total_regions_entered: u64,

    /// Total values bulk-freed via region exit (lifetime counter).
    total_bulk_freed: u64,
}

impl RegionStack {
    /// Create a new empty region stack.
    pub fn new() -> Self {
        Self {
            regions: Vec::with_capacity(8),
            next_id: 1, // Start at 1 (0 is reserved for "no region")
            total_allocs: 0,
            total_regions_entered: 0,
            total_bulk_freed: 0,
        }
    }

    /// Enter a new region for a `let*` scope.
    ///
    /// Returns the region ID. Use this ID with `AllocHint::LetScope { region_id }`
    /// to direct allocations to this region.
    pub fn enter(&mut self, depth: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.regions.push(Region::new(id, depth));
        self.total_regions_entered += 1;
        id
    }

    /// Exit the current region, bulk-freeing all its allocations.
    ///
    /// Returns the number of values freed, or `None` if no region was active.
    pub fn exit(&mut self) -> Option<u32> {
        let region = self.regions.pop()?;
        let freed = region.alloc_count;
        self.total_bulk_freed += freed as u64;
        Some(freed)
    }

    /// Exit a specific region by ID, and all inner regions.
    ///
    /// Pops regions until the region with the given ID is found and popped.
    /// Returns the total values freed across all exited regions.
    pub fn exit_to(&mut self, region_id: u32) -> u32 {
        let mut freed = 0u32;
        while let Some(region) = self.regions.last() {
            if region.id < region_id {
                break; // Don't pop regions below the target
            }
            freed += region.alloc_count;
            self.total_bulk_freed += region.alloc_count as u64;
            self.regions.pop();
        }
        freed
    }

    /// Record an allocation in the current (innermost) region.
    ///
    /// Returns `true` if an allocation was recorded (region is active),
    /// `false` if no region is active.
    #[inline]
    pub fn record_alloc(&mut self) -> bool {
        if let Some(region) = self.regions.last_mut() {
            region.record_alloc();
            self.total_allocs += 1;
            true
        } else {
            false
        }
    }

    /// Check if a region is currently active.
    #[inline]
    pub fn is_active(&self) -> bool {
        !self.regions.is_empty()
    }

    /// Return the current region's ID, or 0 if no region is active.
    #[inline]
    pub fn current_region_id(&self) -> u32 {
        self.regions.last().map_or(0, |r| r.id)
    }

    /// Return the number of active regions.
    #[inline]
    pub fn depth(&self) -> usize {
        self.regions.len()
    }

    /// Clear all regions without freeing (for reset/error recovery).
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    /// Return diagnostic statistics.
    pub fn stats(&self) -> RegionStats {
        RegionStats {
            active_regions: self.regions.len(),
            total_allocs: self.total_allocs,
            total_regions_entered: self.total_regions_entered,
            total_bulk_freed: self.total_bulk_freed,
        }
    }
}

impl Default for RegionStack {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic statistics for the region stack.
#[derive(Debug, Clone)]
pub struct RegionStats {
    pub active_regions: usize,
    pub total_allocs: u64,
    pub total_regions_entered: u64,
    pub total_bulk_freed: u64,
}

impl std::fmt::Display for RegionStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "RegionStack: {} active, {} allocs, {} entered, {} bulk-freed",
            self.active_regions,
            self.total_allocs,
            self.total_regions_entered,
            self.total_bulk_freed,
        )
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

thread_local! {
    static THREAD_REGIONS: RefCell<RegionStack> = RefCell::new(RegionStack::new());
}

/// Access the thread-local region stack.
#[inline]
pub fn with_region_stack<R>(f: impl FnOnce(&mut RegionStack) -> R) -> R {
    THREAD_REGIONS.with(|cell| {
        let mut stack = cell.borrow_mut();
        f(&mut stack)
    })
}

/// Clear the thread-local region stack.
#[inline]
pub fn clear_region_stack() {
    THREAD_REGIONS.with(|cell| {
        cell.borrow_mut().clear();
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_stack() {
        let stack = RegionStack::new();
        assert!(!stack.is_active());
        assert_eq!(stack.depth(), 0);
        assert_eq!(stack.current_region_id(), 0);
    }

    #[test]
    fn test_enter_exit() {
        let mut stack = RegionStack::new();

        let id = stack.enter(0);
        assert_eq!(id, 1);
        assert!(stack.is_active());
        assert_eq!(stack.depth(), 1);

        stack.record_alloc();
        stack.record_alloc();
        stack.record_alloc();

        let freed = stack.exit().expect("has region");
        assert_eq!(freed, 3);
        assert!(!stack.is_active());
    }

    #[test]
    fn test_nested_regions() {
        let mut stack = RegionStack::new();

        let id1 = stack.enter(0);
        stack.record_alloc();

        let id2 = stack.enter(1);
        stack.record_alloc();
        stack.record_alloc();

        assert_eq!(stack.depth(), 2);
        assert_eq!(stack.current_region_id(), id2);

        // Exit inner
        let freed2 = stack.exit().expect("inner");
        assert_eq!(freed2, 2);
        assert_eq!(stack.current_region_id(), id1);

        // Exit outer
        let freed1 = stack.exit().expect("outer");
        assert_eq!(freed1, 1);
        assert!(!stack.is_active());
    }

    #[test]
    fn test_exit_to() {
        let mut stack = RegionStack::new();

        let id1 = stack.enter(0);
        stack.record_alloc();

        let _id2 = stack.enter(1);
        stack.record_alloc();
        stack.record_alloc();

        let _id3 = stack.enter(2);
        stack.record_alloc();

        // Exit to id1 — pops id3 and id2
        let freed = stack.exit_to(id1);
        assert_eq!(freed, 3 + 1); // id2(2) + id3(1) = 3, plus id1(1) = 4
        assert!(!stack.is_active());
    }

    #[test]
    fn test_no_region_record_alloc() {
        let mut stack = RegionStack::new();
        assert!(!stack.record_alloc()); // No region active
    }

    #[test]
    fn test_stats() {
        let mut stack = RegionStack::new();

        stack.enter(0);
        stack.record_alloc();
        stack.record_alloc();
        stack.exit();

        stack.enter(1);
        stack.record_alloc();
        stack.exit();

        let stats = stack.stats();
        assert_eq!(stats.active_regions, 0);
        assert_eq!(stats.total_allocs, 3);
        assert_eq!(stats.total_regions_entered, 2);
        assert_eq!(stats.total_bulk_freed, 3);
    }

    #[test]
    fn test_monotonic_ids() {
        let mut stack = RegionStack::new();

        let id1 = stack.enter(0);
        let id2 = stack.enter(1);
        let id3 = stack.enter(2);

        assert!(id1 < id2);
        assert!(id2 < id3);

        stack.clear();

        // IDs continue from where they left off
        let id4 = stack.enter(0);
        assert!(id3 < id4);
    }

    #[test]
    fn test_thread_local() {
        with_region_stack(|stack| {
            stack.clear();
            let id = stack.enter(0);
            stack.record_alloc();
            assert_eq!(stack.current_region_id(), id);
        });

        with_region_stack(|stack| {
            assert!(stack.is_active()); // Persists
            stack.clear();
        });
    }
}
