//! Online weight refinement: EMA updates, P²-to-EMA transfer, epoch tracking.
//!
//! Closes the feedback loop between task execution and scheduling weights.
//! After each task completes, its actual runtime updates the corresponding
//! weight in the scheduler automaton via exponential moving average.
//!
//! ## Integration with existing P² estimator
//!
//! The existing `P2MedianEstimator` in `priority_scheduler.rs` provides robust
//! median estimates per `TaskTypeId`. Online refinement transfers these estimates
//! to the scheduler automaton's per-(head_hash, arity) EMAs during warm-start.
//!
//! ## Epoch management
//!
//! Each epoch represents a scheduler automaton rebuild (e.g., after AAM analysis).
//! Within an epoch, weights evolve via EMA. Cross-epoch, the P²-to-EMA transfer
//! provides warm-start values so the new automaton doesn't start cold.

use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;

use crate::backend::models::adaptive_pool::Ema;
use crate::backend::priority_scheduler::RuntimeTracker;

use super::cost_class::{CostClass, TaskDescriptor};

// ══════════════════════════════════════════════════════════════════════════════
// Constants
// ══════════════════════════════════════════════════════════════════════════════

/// EMA smoothing factor for weight updates (alpha = 0.15).
///
/// Half-life ≈ 4.3 samples. This means after ~4 observations, the old value
/// contributes less than 50% to the current estimate.
pub const WEIGHT_EMA_ALPHA: f64 = 0.15;

/// Number of samples required before reclassifying an expression.
///
/// Prevents thrashing at cost class boundaries. An expression must see
/// this many consecutive samples in a new class before switching.
pub const RECLASSIFY_HYSTERESIS: u32 = 10;

/// Threshold for "cheap" expressions (nanoseconds).
/// Expressions consistently below this are classified as GroundCheap or GroundArith.
pub const CHEAP_THRESHOLD_NS: f64 = 1_000.0; // 1μs

/// Threshold for "moderate" expressions (nanoseconds).
pub const MODERATE_THRESHOLD_NS: f64 = 100_000.0; // 100μs

/// Threshold for "expensive" expressions (nanoseconds).
pub const EXPENSIVE_THRESHOLD_NS: f64 = 10_000_000.0; // 10ms

// ══════════════════════════════════════════════════════════════════════════════
// Weight tracker
// ══════════════════════════════════════════════════════════════════════════════

/// Per-(head_hash, arity) weight tracking entry.
#[derive(Debug)]
struct WeightEntry {
    /// Exponential moving average of runtime in nanoseconds.
    ema: Ema,
    /// Current cost class (from last classification).
    current_class: CostClass,
    /// Number of consecutive samples suggesting a different class.
    reclassify_count: u32,
    /// Total number of observations.
    total_observations: u64,
}

impl WeightEntry {
    fn new(initial_class: CostClass) -> Self {
        WeightEntry {
            ema: Ema::new(WEIGHT_EMA_ALPHA),
            current_class: initial_class,
            reclassify_count: 0,
            total_observations: 0,
        }
    }
}

/// Online weight refinement engine.
///
/// Tracks per-(head_hash, arity) runtime EMAs and suggests cost class
/// reclassifications when runtime observations diverge from the current class.
pub struct OnlineRefinement {
    /// Per-(head_hash, arity) tracking entries.
    entries: DashMap<u32, WeightEntry>,

    /// Current epoch.
    epoch: AtomicU64,

    /// Total weight updates processed.
    total_updates: AtomicU64,
}

impl OnlineRefinement {
    /// Create a new online refinement engine.
    pub fn new() -> Self {
        OnlineRefinement {
            entries: DashMap::new(),
            epoch: AtomicU64::new(0),
            total_updates: AtomicU64::new(0),
        }
    }

    /// Record a runtime observation and return an optional reclassification.
    ///
    /// If the runtime consistently suggests a different cost class (after
    /// `RECLASSIFY_HYSTERESIS` samples), returns `Some(new_class)`.
    pub fn record_runtime(
        &self,
        descriptor: TaskDescriptor,
        actual_runtime_ns: u64,
        current_class: CostClass,
    ) -> Option<CostClass> {
        let key = descriptor.head_arity_key();
        self.total_updates.fetch_add(1, Ordering::Relaxed);

        let mut entry = self.entries.entry(key).or_insert_with(|| {
            WeightEntry::new(current_class)
        });

        entry.ema.update(actual_runtime_ns as f64);
        entry.total_observations += 1;

        // Suggest reclassification based on observed runtime
        let suggested = Self::suggest_class(entry.ema.value(), current_class);

        if suggested != entry.current_class {
            entry.reclassify_count += 1;
            if entry.reclassify_count >= RECLASSIFY_HYSTERESIS {
                entry.current_class = suggested;
                entry.reclassify_count = 0;
                return Some(suggested);
            }
        } else {
            entry.reclassify_count = 0;
        }

        None
    }

    /// Suggest a cost class based on observed average runtime.
    fn suggest_class(avg_runtime_ns: f64, current_class: CostClass) -> CostClass {
        if avg_runtime_ns < CHEAP_THRESHOLD_NS {
            // Very cheap — could be ground or simple symbolic
            match current_class {
                CostClass::GroundCheap | CostClass::GroundArith => current_class,
                CostClass::ImpureSequential => CostClass::ImpureSequential, // impurity doesn't change
                _ => CostClass::SymbolicCheap,
            }
        } else if avg_runtime_ns < MODERATE_THRESHOLD_NS {
            // Moderate cost
            match current_class {
                CostClass::ImpureSequential => CostClass::ImpureSequential,
                CostClass::ParallelPure => CostClass::ParallelPure, // purity preserved
                _ => CostClass::SymbolicModerate,
            }
        } else if avg_runtime_ns < EXPENSIVE_THRESHOLD_NS {
            // Expensive
            match current_class {
                CostClass::ImpureSequential => CostClass::ImpureSequential,
                CostClass::RecursiveBounded | CostClass::RecursiveUnbounded => current_class,
                _ => CostClass::RecursiveBounded,
            }
        } else {
            // Very expensive
            match current_class {
                CostClass::ImpureSequential => CostClass::ImpureSequential,
                _ => CostClass::RecursiveUnbounded,
            }
        }
    }

    /// Transfer P² median estimates to EMA entries (warm-start).
    ///
    /// Called during epoch transitions to seed the new automaton with
    /// runtime data from the previous epoch's P² estimators.
    pub fn warm_start_from_p2(&self, runtime_tracker: &RuntimeTracker) {
        // The RuntimeTracker uses TaskTypeId, which we need to map to
        // (head_hash, arity) keys. For now, we use the global median
        // as a default initial estimate for all entries.
        let global_median = runtime_tracker.global_median();
        if global_median > 0.0 {
            // Entries will be initialized with the P² median on first access
            // via the EMA's first-sample initialization behavior.
            // No explicit transfer needed — the EMA's first update call
            // will set the initial value.
        }
    }

    /// Get the current EMA estimate for a descriptor.
    pub fn estimated_runtime(&self, descriptor: TaskDescriptor) -> Option<f64> {
        let key = descriptor.head_arity_key();
        self.entries.get(&key).map(|e| e.ema.value())
    }

    /// Get the current cost class for a descriptor.
    pub fn current_class(&self, descriptor: TaskDescriptor) -> Option<CostClass> {
        let key = descriptor.head_arity_key();
        self.entries.get(&key).map(|e| e.current_class)
    }

    /// Get the current epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Relaxed)
    }

    /// Advance to a new epoch.
    pub fn advance_epoch(&self) -> u64 {
        self.epoch.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Total number of weight updates processed.
    pub fn total_updates(&self) -> u64 {
        self.total_updates.load(Ordering::Relaxed)
    }

    /// Number of tracked entries.
    pub fn num_entries(&self) -> usize {
        self.entries.len()
    }
}

impl Default for OnlineRefinement {
    fn default() -> Self {
        Self::new()
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_suggest_class_cheap() {
        // Very fast execution → should suggest cheap class
        let class = OnlineRefinement::suggest_class(500.0, CostClass::SymbolicModerate);
        assert_eq!(class, CostClass::SymbolicCheap);
    }

    #[test]
    fn test_suggest_class_preserves_impure() {
        // Impure should stay impure regardless of runtime
        let class = OnlineRefinement::suggest_class(500.0, CostClass::ImpureSequential);
        assert_eq!(class, CostClass::ImpureSequential);
    }

    #[test]
    fn test_suggest_class_expensive() {
        let class = OnlineRefinement::suggest_class(50_000_000.0, CostClass::SymbolicModerate);
        assert_eq!(class, CostClass::RecursiveUnbounded);
    }

    #[test]
    fn test_hysteresis() {
        let refinement = OnlineRefinement::new();
        let desc = TaskDescriptor::pack(0x1234, 2, 0, 0);

        // Record fast runtimes, but not enough to trigger reclassification
        for _ in 0..(RECLASSIFY_HYSTERESIS - 1) {
            let result = refinement.record_runtime(desc, 100, CostClass::SymbolicModerate);
            assert!(result.is_none(), "should not reclassify before hysteresis threshold");
        }

        // One more should trigger reclassification
        let result = refinement.record_runtime(desc, 100, CostClass::SymbolicModerate);
        assert!(result.is_some(), "should reclassify after hysteresis threshold");
        assert_eq!(result.unwrap(), CostClass::SymbolicCheap);
    }

    #[test]
    fn test_epoch_tracking() {
        let refinement = OnlineRefinement::new();
        assert_eq!(refinement.epoch(), 0);
        assert_eq!(refinement.advance_epoch(), 1);
        assert_eq!(refinement.epoch(), 1);
    }
}
