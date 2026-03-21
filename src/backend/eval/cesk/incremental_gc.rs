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

use std::cell::Cell;

use crate::backend::models::MettaValueTrait;

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
}

impl Default for NurseryConfig {
    fn default() -> Self {
        Self {
            threshold_bytes: 256 * 1024,
            promotion_threshold: 2,
            max_objects_per_step: 1024,
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

    /// Whether nursery collection is enabled.
    pub enabled: bool,
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
            enabled: false, // Disabled by default — opt-in via configuration
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
            enabled: true,
        }
    }

    /// Record an allocation of the given size.
    ///
    /// Returns `true` if nursery collection should be triggered
    /// (allocation pressure exceeded threshold).
    #[inline]
    pub fn record_alloc(&mut self, bytes: usize) -> bool {
        if !self.enabled {
            return false;
        }
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
        self.enabled && self.bytes_since_collect >= self.config.threshold_bytes
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
            enabled: self.enabled,
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nursery_default_disabled() {
        let nursery = NurseryState::new();
        assert!(!nursery.enabled);
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
        assert!(stats.enabled);
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
    }
}
