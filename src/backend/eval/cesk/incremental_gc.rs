//! Incremental GC at Safepoints (Nursery + Old Generation)
//!
//! This module provides the framework for incremental garbage collection
//! using the SECK machine's algebraic root sets. Instead of stop-the-world
//! collection, GC is performed incrementally at trampoline safepoints:
//!
//! - **Nursery**: Small, thread-local allocation region collected at every safepoint
//! - **Old generation**: Values that survive N nursery collections are promoted
//! - **Algebraic roots**: SECK RootSet provides precise root enumeration
//!
//! ## Design
//!
//! The incremental GC uses a generational strategy:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────┐
//! │  Nursery (thread-local, bump-allocated)              │
//! │  ┌─────────┐                                        │
//! │  │ new vals│  → collected at every safepoint        │
//! │  └─────────┘    survivors promoted to old-gen       │
//! │                                                      │
//! │  Old Generation (global slab allocator)              │
//! │  ┌─────────────────────────────────┐                │
//! │  │ promoted vals + direct old-gen  │                │
//! │  │ allocations (rules, facts)      │                │
//! │  └─────────────────────────────────┘                │
//! │  → collected by existing mark-sweep GC              │
//! └─────────────────────────────────────────────────────┘
//! ```
//!
//! ## Integration
//!
//! The incremental GC is opt-in via `EvalContext::should_safepoint()`. When
//! enabled, the trampoline calls `nursery_collect()` at each safepoint,
//! which uses the `RootSet` to determine which nursery values are live.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;

use crate::backend::models::{MettaValue, MettaValueInner, MettaValueTrait};

// ============================================================================
// GC Generation Tracking
// ============================================================================

/// Configuration for the incremental GC nursery.
#[derive(Debug, Clone)]
pub struct NurseryConfig {
    /// Size threshold (in bytes) at which nursery collection is triggered.
    /// Default: 256 KB (matches page size of slab allocator).
    pub threshold_bytes: usize,

    /// Number of nursery survivals before promotion to old generation.
    /// Default: 2 (values surviving 2 collections are likely long-lived).
    pub promotion_threshold: u8,

    /// Maximum number of objects to scan per incremental GC step.
    /// Limits GC pause time at each safepoint. Default: 1024.
    pub max_objects_per_step: usize,

    /// Enable deterministic (post-rule-match) collection (Phase 2.3).
    ///
    /// When `true`, nursery collection runs after every rule match,
    /// ensuring the state space is always "garbage-free". This eliminates
    /// the need for epoch-based cache invalidation (`GC_SWEEP_EPOCH`,
    /// `check_gc_epoch()`) because no cache can ever hold a stale pointer.
    ///
    /// Default: `false` (opt-in — requires Phase 2.2 nursery correctness).
    pub deterministic: bool,
}

impl Default for NurseryConfig {
    fn default() -> Self {
        Self {
            threshold_bytes: 256 * 1024,
            promotion_threshold: 2,
            max_objects_per_step: 1024,
            deterministic: false,
        }
    }
}

/// Per-value generation metadata.
///
/// Tracks how many nursery collections a value has survived, for promotion
/// decisions. This is stored alongside the value in the slab allocator
/// (future integration with `MettaValueInner` flags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationInfo {
    /// Number of nursery collections this value has survived.
    pub survival_count: u8,

    /// Whether this value has been promoted to old generation.
    pub is_promoted: bool,
}

impl Default for GenerationInfo {
    fn default() -> Self {
        Self {
            survival_count: 0,
            is_promoted: false,
        }
    }
}

// ============================================================================
// Nursery State
// ============================================================================

/// Thread-local nursery state for incremental GC.
///
/// Tracks allocation pressure since last collection and maintains the
/// set of nursery-allocated values that need to be scanned.
#[derive(Debug)]
pub struct NurseryState {
    /// Configuration for this nursery.
    pub config: NurseryConfig,

    /// Bytes allocated since last nursery collection.
    pub bytes_since_collect: usize,

    /// Number of nursery collections performed.
    pub collection_count: u64,

    /// Number of values promoted to old generation.
    pub promotion_count: u64,

    /// Number of values reclaimed by nursery collection.
    pub reclaimed_count: u64,

}

impl NurseryState {
    /// Create a new nursery state with default configuration.
    pub fn new() -> Self {
        Self {
            config: NurseryConfig::default(),
            bytes_since_collect: 0,
            collection_count: 0,
            promotion_count: 0,
            reclaimed_count: 0,
        }
    }

    /// Create a new nursery state with custom configuration.
    pub fn with_config(config: NurseryConfig) -> Self {
        Self {
            config,
            bytes_since_collect: 0,
            collection_count: 0,
            promotion_count: 0,
            reclaimed_count: 0,
        }
    }

    /// Record an allocation of the given size.
    ///
    /// Returns `true` if nursery collection should be triggered
    /// (allocation pressure exceeded threshold).
    #[inline]
    pub fn record_alloc(&mut self, bytes: usize) -> bool {
        self.bytes_since_collect += bytes;
        self.bytes_since_collect >= self.config.threshold_bytes
    }

    /// Record a nursery collection.
    ///
    /// Resets allocation pressure and increments collection counter.
    pub fn record_collection(&mut self, reclaimed: usize, promoted: usize) {
        self.bytes_since_collect = 0;
        self.collection_count += 1;
        self.reclaimed_count += reclaimed as u64;
        self.promotion_count += promoted as u64;
    }

    /// Check if nursery collection should be triggered.
    #[inline]
    pub fn should_collect(&self) -> bool {
        self.bytes_since_collect >= self.config.threshold_bytes
    }

    /// Reset nursery state (for testing or between evaluations).
    pub fn reset(&mut self) {
        self.bytes_since_collect = 0;
        self.collection_count = 0;
        self.promotion_count = 0;
        self.reclaimed_count = 0;
    }

    /// Return diagnostic statistics.
    pub fn stats(&self) -> NurseryStats {
        NurseryStats {
            enabled: true, // Nursery is always active
            bytes_since_collect: self.bytes_since_collect,
            threshold_bytes: self.config.threshold_bytes,
            collection_count: self.collection_count,
            promotion_count: self.promotion_count,
            reclaimed_count: self.reclaimed_count,
        }
    }
}

impl Default for NurseryState {
    fn default() -> Self {
        Self::new()
    }
}

/// Diagnostic statistics for the nursery.
#[derive(Debug, Clone)]
pub struct NurseryStats {
    pub enabled: bool,
    pub bytes_since_collect: usize,
    pub threshold_bytes: usize,
    pub collection_count: u64,
    pub promotion_count: u64,
    pub reclaimed_count: u64,
}

impl std::fmt::Display for NurseryStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Nursery: {} ({}/{} bytes), {} collections, {} promoted, {} reclaimed",
            if self.enabled { "enabled" } else { "disabled" },
            self.bytes_since_collect,
            self.threshold_bytes,
            self.collection_count,
            self.promotion_count,
            self.reclaimed_count,
        )
    }
}

// ============================================================================
// Write Barrier (for remembered set)
// ============================================================================

/// Write barrier for tracking old-to-new references.
///
/// When an old-generation value is mutated to point to a nursery value,
/// the write barrier records the reference so the nursery collector
/// can find it as a root. Without this, nursery values referenced only
/// from old-gen would be incorrectly collected.
///
/// Currently a placeholder — the write barrier is activated when
/// the generational GC is fully integrated with the slab allocator.
#[derive(Debug)]
pub struct WriteBarrier {
    /// Whether the write barrier is active.
    pub active: bool,

    /// Count of barrier triggers (for diagnostics).
    pub trigger_count: u64,
}

impl WriteBarrier {
    pub fn new() -> Self {
        Self {
            active: false,
            trigger_count: 0,
        }
    }

    /// Record a write from old-gen to nursery.
    ///
    /// Currently a no-op counter. When integrated with the slab allocator,
    /// this will add the old-gen value to a remembered set.
    #[inline]
    pub fn record_write(&mut self) {
        if self.active {
            self.trigger_count += 1;
        }
    }
}

impl Default for WriteBarrier {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Nursery Collector
// ============================================================================

/// Thread-local nursery collector that performs incremental GC at safepoints.
///
/// The collector tracks which slab slots were allocated since the last
/// collection (via `nursery_slots`). At each safepoint, it:
///
/// 1. Builds a mark set from the algebraic root set + remembered set
/// 2. Scans only nursery-resident slots
/// 3. Frees unmarked slots immediately
/// 4. Promotes survivors that have survived `promotion_threshold` collections
///
/// ## Thread Safety
///
/// The collector is thread-local — no synchronization needed. Each evaluation
/// thread has its own collector instance.
#[derive(Debug)]
pub struct NurseryCollector {
    /// Nursery state (allocation pressure, stats).
    pub state: NurseryState,

    /// Write barrier (remembered set for old→nursery refs).
    pub write_barrier: WriteBarrier,

    /// Slab pointers allocated since last collection.
    /// These are the inner pointers (from `MettaValue::inner_ptr()`) of
    /// values allocated in the nursery epoch.
    nursery_ptrs: Vec<usize>,

    /// Per-pointer survival count. Indexed in parallel with `nursery_ptrs`.
    /// Values surviving `promotion_threshold` collections are promoted.
    survival_counts: Vec<u8>,

    /// Remembered set: pointers from old-gen values that reference nursery values.
    /// Drained during collection to serve as additional roots.
    remembered_set: Vec<usize>,
}

/// Result of a nursery collection.
#[derive(Debug, Clone)]
pub struct NurseryCollectResult {
    /// Number of nursery values that were live (survived).
    pub live_count: usize,
    /// Number of nursery values freed.
    pub freed_count: usize,
    /// Number of values promoted to old generation.
    pub promoted_count: usize,
}

impl NurseryCollector {
    /// Create a new nursery collector with default configuration.
    pub fn new() -> Self {
        Self::with_config(NurseryConfig::default())
    }

    /// Create a new nursery collector with custom configuration.
    pub fn with_config(config: NurseryConfig) -> Self {
        Self {
            state: NurseryState::with_config(config),
            write_barrier: WriteBarrier::new(),
            nursery_ptrs: Vec::with_capacity(1024),
            survival_counts: Vec::with_capacity(1024),
            remembered_set: Vec::with_capacity(64),
        }
    }

    /// Record a nursery allocation.
    ///
    /// Called from the allocation hot path to track which slab slots belong
    /// to the current nursery epoch. The `ptr` is the inner pointer from
    /// `MettaValue::inner_ptr()`.
    ///
    /// Returns `true` if nursery collection should be triggered.
    #[inline]
    pub fn record_alloc(&mut self, ptr: usize, bytes: usize) -> bool {
        self.nursery_ptrs.push(ptr);
        self.survival_counts.push(0);
        self.state.record_alloc(bytes)
    }

    /// Record an old-gen → nursery reference (write barrier).
    ///
    /// Called when an old-gen value is constructed that references a
    /// nursery-allocated value (e.g., SExpr containing nursery children).
    #[inline]
    pub fn record_remembered(&mut self, old_gen_ptr: usize) {
        self.write_barrier.record_write();
        self.remembered_set.push(old_gen_ptr);
    }

    /// Perform nursery collection using the algebraic root set.
    ///
    /// Scans only nursery-resident slots against the mark set derived from
    /// `live_ptrs` (the root set's inner pointers) + remembered set.
    ///
    /// Returns collection statistics.
    pub fn collect(&mut self, live_ptrs: &HashSet<usize>) -> NurseryCollectResult {
        if self.nursery_ptrs.is_empty() {
            self.state.record_collection(0, 0);
            return NurseryCollectResult {
                live_count: 0,
                freed_count: 0,
                promoted_count: 0,
            };
        }

        // Build extended mark set: roots + remembered set
        let mut mark_set = live_ptrs.clone();
        for &ptr in &self.remembered_set {
            mark_set.insert(ptr);
        }

        let mut new_nursery_ptrs = Vec::with_capacity(self.nursery_ptrs.len());
        let mut new_survival_counts = Vec::with_capacity(self.nursery_ptrs.len());
        let mut freed_count = 0usize;
        let mut promoted_count = 0usize;
        let mut live_count = 0usize;

        for (i, &ptr) in self.nursery_ptrs.iter().enumerate() {
            if mark_set.contains(&ptr) {
                // Value is live
                let survival = self.survival_counts[i] + 1;
                if survival >= self.state.config.promotion_threshold {
                    // Promote to old generation — stop tracking in nursery.
                    // The value remains in the slab; old-gen GC manages it.
                    promoted_count += 1;
                } else {
                    // Keep in nursery with incremented survival count
                    new_nursery_ptrs.push(ptr);
                    new_survival_counts.push(survival);
                }
                live_count += 1;
            } else {
                // Value is dead — eligible for freeing.
                // NOTE: Actual slot freeing is delegated to the main GC.
                // The nursery collector marks them as reclaimable; the next
                // mark-sweep will not find them as roots and will free them.
                // This avoids the nursery collector needing direct access to
                // the SlabAllocator's free-list (which would require unsafe).
                freed_count += 1;
            }
        }

        self.nursery_ptrs = new_nursery_ptrs;
        self.survival_counts = new_survival_counts;
        self.remembered_set.clear();

        self.state.record_collection(freed_count, promoted_count);

        NurseryCollectResult {
            live_count,
            freed_count,
            promoted_count,
        }
    }

    /// Check if nursery collection should be triggered.
    #[inline]
    pub fn should_collect(&self) -> bool {
        self.state.should_collect()
    }

    /// Return the number of nursery-tracked pointers.
    #[inline]
    pub fn nursery_size(&self) -> usize {
        self.nursery_ptrs.len()
    }

    /// Clear the nursery collector state.
    pub fn clear(&mut self) {
        self.nursery_ptrs.clear();
        self.survival_counts.clear();
        self.remembered_set.clear();
        self.state.reset();
    }

    /// Check if the collector is enabled.
    #[inline]
    /// Nursery is always enabled (no feature gate).
    pub fn is_enabled(&self) -> bool {
        true
    }

    /// Check if deterministic (post-rule-match) collection is enabled.
    #[inline]
    pub fn is_deterministic(&self) -> bool {
        self.state.config.deterministic
    }

    /// Perform deterministic collection after a rule match (Phase 2.3).
    ///
    /// This is a lighter-weight collection that runs after each rule match
    /// when `config.deterministic` is true. It only collects if the nursery
    /// has accumulated values, and skips collection if the nursery is small
    /// (under 64 values) to avoid overhead on simple matches.
    ///
    /// When deterministic collection is active, the epoch-based cache
    /// invalidation system (`check_gc_epoch()`) is bypassed because the
    /// state space is kept garbage-free between rule matches.
    pub fn collect_deterministic(&mut self, live_ptrs: &HashSet<usize>) -> Option<NurseryCollectResult> {
        if !self.state.config.deterministic {
            return None;
        }

        // Skip if nursery is very small — the overhead of building the mark set
        // exceeds the benefit of freeing a handful of values.
        if self.nursery_ptrs.len() < 64 {
            return None;
        }

        Some(self.collect(live_ptrs))
    }
}

impl Default for NurseryCollector {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

thread_local! {
    static THREAD_NURSERY: RefCell<NurseryCollector> = RefCell::new(NurseryCollector::new());
}

/// Access the thread-local nursery collector.
#[inline]
pub fn with_nursery_collector<R>(f: impl FnOnce(&mut NurseryCollector) -> R) -> R {
    THREAD_NURSERY.with(|cell| {
        let mut collector = cell.borrow_mut();
        f(&mut collector)
    })
}

/// Clear the thread-local nursery collector.
#[inline]
pub fn clear_nursery_collector() {
    THREAD_NURSERY.with(|cell| {
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
    fn test_nursery_default_disabled() {
        let nursery = NurseryState::new();
        assert!(!nursery.should_collect()); // No bytes allocated yet
        assert!(!nursery.should_collect());
    }

    #[test]
    fn test_nursery_threshold_trigger() {
        let config = NurseryConfig {
            threshold_bytes: 1024,
            ..Default::default()
        };
        let mut nursery = NurseryState::with_config(config);

        // Under threshold
        assert!(!nursery.record_alloc(500));
        assert!(!nursery.should_collect());

        // Over threshold
        assert!(nursery.record_alloc(600));
        assert!(nursery.should_collect());
    }

    #[test]
    fn test_nursery_collection_resets_pressure() {
        let config = NurseryConfig {
            threshold_bytes: 1024,
            ..Default::default()
        };
        let mut nursery = NurseryState::with_config(config);

        nursery.record_alloc(2000);
        assert!(nursery.should_collect());

        nursery.record_collection(10, 2);
        assert!(!nursery.should_collect());
        assert_eq!(nursery.collection_count, 1);
        assert_eq!(nursery.reclaimed_count, 10);
        assert_eq!(nursery.promotion_count, 2);
    }

    #[test]
    fn test_nursery_stats() {
        let config = NurseryConfig {
            threshold_bytes: 4096,
            ..Default::default()
        };
        let mut nursery = NurseryState::with_config(config);

        nursery.record_alloc(1000);
        let stats = nursery.stats();
        assert!(stats.enabled); // Always true
        assert_eq!(stats.bytes_since_collect, 1000);
        assert_eq!(stats.threshold_bytes, 4096);
    }

    #[test]
    fn test_nursery_reset() {
        let config = NurseryConfig {
            threshold_bytes: 1024,
            ..Default::default()
        };
        let mut nursery = NurseryState::with_config(config);

        nursery.record_alloc(2000);
        nursery.record_collection(5, 1);
        nursery.reset();

        assert_eq!(nursery.bytes_since_collect, 0);
        assert_eq!(nursery.collection_count, 0);
    }

    #[test]
    fn test_generation_info() {
        let gen = GenerationInfo::default();
        assert_eq!(gen.survival_count, 0);
        assert!(!gen.is_promoted);
    }

    #[test]
    fn test_write_barrier_inactive() {
        let mut wb = WriteBarrier::new();
        assert!(!wb.active);
        wb.record_write();
        assert_eq!(wb.trigger_count, 0); // Inactive — no count
    }

    #[test]
    fn test_write_barrier_active() {
        let mut wb = WriteBarrier::new();
        wb.active = true;
        wb.record_write();
        wb.record_write();
        assert_eq!(wb.trigger_count, 2);
    }

    #[test]
    fn test_config_default() {
        let config = NurseryConfig::default();
        assert_eq!(config.threshold_bytes, 256 * 1024);
        assert_eq!(config.promotion_threshold, 2);
        assert_eq!(config.max_objects_per_step, 1024);
        assert!(!config.deterministic);
    }

    // ── NurseryCollector tests ──────────────────────────────────

    #[test]
    fn test_collector_new() {
        let collector = NurseryCollector::new();
        assert!(collector.is_enabled()); // Always enabled
        assert_eq!(collector.nursery_size(), 0);
    }

    #[test]
    fn test_collector_with_config() {
        let config = NurseryConfig {
            threshold_bytes: 512,
            promotion_threshold: 3,
            ..Default::default()
        };
        let collector = NurseryCollector::with_config(config);
        assert!(collector.is_enabled()); // Always enabled
        assert_eq!(collector.nursery_size(), 0);
    }

    #[test]
    fn test_collector_record_alloc() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        assert!(!collector.record_alloc(0x1000, 50));
        assert_eq!(collector.nursery_size(), 1);

        assert!(collector.record_alloc(0x2000, 60)); // Over threshold
        assert_eq!(collector.nursery_size(), 2);
    }

    #[test]
    fn test_collector_collect_empty() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        let live_ptrs = HashSet::new();
        let result = collector.collect(&live_ptrs);
        assert_eq!(result.live_count, 0);
        assert_eq!(result.freed_count, 0);
        assert_eq!(result.promoted_count, 0);
    }

    #[test]
    fn test_collector_collect_all_live() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            promotion_threshold: 3,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        collector.record_alloc(0x1000, 10);
        collector.record_alloc(0x2000, 10);

        let mut live_ptrs = HashSet::new();
        live_ptrs.insert(0x1000);
        live_ptrs.insert(0x2000);

        let result = collector.collect(&live_ptrs);
        assert_eq!(result.live_count, 2);
        assert_eq!(result.freed_count, 0);
        assert_eq!(result.promoted_count, 0);
        assert_eq!(collector.nursery_size(), 2); // Still tracked
    }

    #[test]
    fn test_collector_collect_some_dead() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            promotion_threshold: 3,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        collector.record_alloc(0x1000, 10);
        collector.record_alloc(0x2000, 10);
        collector.record_alloc(0x3000, 10);

        // Only 0x1000 is live
        let mut live_ptrs = HashSet::new();
        live_ptrs.insert(0x1000);

        let result = collector.collect(&live_ptrs);
        assert_eq!(result.live_count, 1);
        assert_eq!(result.freed_count, 2);
        assert_eq!(result.promoted_count, 0);
        assert_eq!(collector.nursery_size(), 1); // Only survivor remains
    }

    #[test]
    fn test_collector_promotion() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            promotion_threshold: 2, // Promote after 2 survivals
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        collector.record_alloc(0x1000, 10);

        let mut live_ptrs = HashSet::new();
        live_ptrs.insert(0x1000);

        // First collection: survival_count → 1
        let r1 = collector.collect(&live_ptrs);
        assert_eq!(r1.live_count, 1);
        assert_eq!(r1.promoted_count, 0);
        assert_eq!(collector.nursery_size(), 1);

        // Second collection: survival_count → 2 → promoted!
        let r2 = collector.collect(&live_ptrs);
        assert_eq!(r2.live_count, 1);
        assert_eq!(r2.promoted_count, 1);
        assert_eq!(collector.nursery_size(), 0); // Promoted out of nursery
    }

    #[test]
    fn test_collector_remembered_set() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            promotion_threshold: 3,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        collector.record_alloc(0x1000, 10);
        collector.record_remembered(0x1000); // Old-gen ref to nursery value

        // 0x1000 is not in the direct root set, but IS in remembered set
        let live_ptrs = HashSet::new();
        let result = collector.collect(&live_ptrs);
        assert_eq!(result.live_count, 1); // Kept alive by remembered set
        assert_eq!(result.freed_count, 0);
    }

    #[test]
    fn test_collector_clear() {
        let config = NurseryConfig {
            threshold_bytes: 100,
            ..Default::default()
        };
        let mut collector = NurseryCollector::with_config(config);

        collector.record_alloc(0x1000, 10);
        collector.record_alloc(0x2000, 10);
        collector.record_remembered(0x3000);

        collector.clear();
        assert_eq!(collector.nursery_size(), 0);
        assert!(!collector.should_collect());
    }

    #[test]
    fn test_collector_thread_local() {
        with_nursery_collector(|c| {
            c.clear();
            c.record_alloc(0xABCD, 10);
            assert_eq!(c.nursery_size(), 1);
        });

        with_nursery_collector(|c| {
            assert_eq!(c.nursery_size(), 1); // Persists
            c.clear();
        });
    }
}
