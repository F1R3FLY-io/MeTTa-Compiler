//! BEAM-Style Reductions-Based Fair Scheduling
//!
//! Each trampoline evaluation runs for a limited number of "reductions" (steps)
//! before yielding to the scheduler. This prevents long-running branches from
//! starving other branches in the work pool.
//!
//! ## Design
//!
//! A **reduction** is one trampoline iteration (pop work item, process, push
//! continuations/results). After `REDUCTION_BUDGET` reductions, the trampoline
//! saves its complete `SeckState` into a `SuspendedEval` and returns
//! `EvalOutcome::Yielded`. The worker re-enqueues the suspended evaluation
//! as a new task (with the same priority), allowing other waiting tasks to run.
//!
//! ## Reduction Budget
//!
//! Default: 8192 reductions (configurable via `METTATRON_REDUCTION_BUDGET`).
//! This matches roughly 2x the GC safepoint interval (4096), giving each
//! branch a fair time slice without excessive context-switching overhead.
//!
//! ## Worker Affinity
//!
//! When a trampoline yields, the suspended evaluation is re-enqueued to the
//! SAME worker's local deque (via the striped queue's `push_local`). This
//! preserves thread-local state (nursery, thunks, caches, binding arena)
//! because the same thread will resume execution.
//!
//! ## Yield Conditions
//!
//! The trampoline yields when ALL of:
//! 1. Reductions >= REDUCTION_BUDGET
//! 2. Continuation stack is non-empty (still work to do)
//! 3. Running on a worker thread (not the main thread)
//!
//! Condition 3 prevents yielding during single-threaded evaluation (REPL, tests).

use std::sync::OnceLock;

// ============================================================================
// Configuration
// ============================================================================

/// Default reduction budget per trampoline slice.
///
/// 8192 reductions ≈ 2ms of wall-clock time for typical PLN evaluation.
/// This gives each branch a fair time slice while keeping context-switch
/// overhead below 1% (yield cost ≈ 50ns, amortized over 8192 × 200ns steps).
pub const DEFAULT_REDUCTION_BUDGET: u32 = 8192;

static REDUCTION_BUDGET: OnceLock<u32> = OnceLock::new();

/// Get the configured reduction budget.
#[inline]
pub fn reduction_budget() -> u32 {
    *REDUCTION_BUDGET.get_or_init(|| {
        std::env::var("METTATRON_REDUCTION_BUDGET")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_REDUCTION_BUDGET)
    })
}

// ============================================================================
// Evaluation Outcome
// ============================================================================

/// Outcome of a trampoline evaluation step.
///
/// The trampoline returns `Complete` when evaluation finishes, or `Yielded`
/// when the reduction budget is exhausted and the evaluation should be
/// resumed later.
#[derive(Debug)]
pub enum EvalOutcome {
    /// Evaluation completed. Contains the final (results, environment).
    Complete(
        smallvec::SmallVec<[crate::backend::eval::trampoline::types::BoundValue; 2]>,
        crate::backend::environment::MettaEnvironment,
    ),

    /// Evaluation yielded after exhausting its reduction budget.
    /// Contains the suspended state for resumption.
    Yielded(SuspendedEval),
}

/// A suspended trampoline evaluation that can be resumed.
///
/// Contains the complete machine state at the yield point plus metadata
/// for priority scheduling and worker affinity.
#[derive(Debug)]
pub struct SuspendedEval {
    /// The work stack at the yield point.
    pub work_stack: Vec<super::super::trampoline::WorkItem>,
    /// The continuation stack at the yield point.
    pub continuations: Vec<super::super::trampoline::Continuation>,

    /// Evaluation depth at suspension (for priority scheduling).
    pub depth: u32,

    /// Number of reductions consumed so far (lifetime counter).
    pub total_reductions: u64,

    /// Worker ID that was executing this evaluation (for affinity).
    pub worker_id: Option<usize>,
}

// ============================================================================
// Reduction Counter
// ============================================================================

/// A reduction counter for tracking trampoline steps.
///
/// Incremented on each trampoline iteration. When it exceeds the budget,
/// the trampoline should check yield conditions.
#[derive(Debug)]
pub struct ReductionCounter {
    /// Reductions since last yield/start.
    current: u32,
    /// Total reductions across all slices.
    total: u64,
    /// Budget per slice.
    budget: u32,
}

impl ReductionCounter {
    /// Create a new reduction counter with the configured budget.
    pub fn new() -> Self {
        Self {
            current: 0,
            total: 0,
            budget: reduction_budget(),
        }
    }

    /// Create a new reduction counter with a custom budget.
    pub fn with_budget(budget: u32) -> Self {
        Self {
            current: 0,
            total: 0,
            budget,
        }
    }

    /// Increment the counter and check if the budget is exhausted.
    ///
    /// Returns `true` if the budget has been reached (yield should be considered).
    #[inline]
    pub fn tick(&mut self) -> bool {
        self.current += 1;
        self.total += 1;
        self.current >= self.budget
    }

    /// Reset the counter for a new slice (after yield/resume).
    #[inline]
    pub fn reset_slice(&mut self) {
        self.current = 0;
    }

    /// Get the current slice reduction count.
    #[inline]
    pub fn current(&self) -> u32 {
        self.current
    }

    /// Get the total reductions across all slices.
    #[inline]
    pub fn total(&self) -> u64 {
        self.total
    }

    /// Get the budget per slice.
    #[inline]
    pub fn budget(&self) -> u32 {
        self.budget
    }

    /// Add to the lifetime total (used when resuming from a prior yield).
    #[inline]
    pub fn add_total(&mut self, n: u64) {
        self.total += n;
    }
}

impl Default for ReductionCounter {
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
    fn test_default_budget() {
        assert_eq!(DEFAULT_REDUCTION_BUDGET, 8192);
    }

    #[test]
    fn test_counter_new() {
        let counter = ReductionCounter::new();
        assert_eq!(counter.current(), 0);
        assert_eq!(counter.total(), 0);
    }

    #[test]
    fn test_counter_tick() {
        let mut counter = ReductionCounter::with_budget(3);

        assert!(!counter.tick()); // 1
        assert!(!counter.tick()); // 2
        assert!(counter.tick()); // 3 — budget reached
        assert_eq!(counter.current(), 3);
        assert_eq!(counter.total(), 3);
    }

    #[test]
    fn test_counter_reset_slice() {
        let mut counter = ReductionCounter::with_budget(2);

        counter.tick(); // 1
        counter.tick(); // 2 — budget
        assert_eq!(counter.current(), 2);

        counter.reset_slice();
        assert_eq!(counter.current(), 0);
        assert_eq!(counter.total(), 2); // Total preserved

        assert!(!counter.tick()); // 1 (new slice)
        assert!(counter.tick()); // 2 — budget again
        assert_eq!(counter.total(), 4);
    }

    #[test]
    fn test_counter_with_budget() {
        let counter = ReductionCounter::with_budget(100);
        assert_eq!(counter.budget(), 100);
    }
}
