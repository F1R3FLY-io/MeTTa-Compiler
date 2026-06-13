//! Shared Abstractions for Adaptive Thread Pools
//!
//! Provides core building blocks used by both the unified WorkPool and the
//! Adaptive GC Pool:
//!
//! - **`Ema`** — Exponential moving average with configurable smoothing factor
//! - **`HillClimber`** — ±1 perturbation-based hill climbing optimizer
//! - **`ScaleAction`** — Decision enum for thread scaling actions
//! - **`WorkerPark`** — Per-worker parking primitive (Mutex<bool> + Condvar)

use parking_lot::{Condvar, Mutex};
use std::time::Duration;

// ============================================================================
// Exponential Moving Average (EMA)
// ============================================================================

/// Exponential moving average with configurable smoothing factor.
///
/// The EMA update formula is:
/// ```text
/// ema = alpha * sample + (1 - alpha) * ema
/// ```
///
/// Where alpha ∈ (0, 1] controls responsiveness:
/// - Higher alpha → more responsive to recent values, noisier
/// - Lower alpha → smoother, more lag
///
/// Half-life in samples: `ln(2) / ln(1 / (1 - alpha))`
/// For alpha=0.15: half-life ≈ 4.3 samples
#[derive(Debug, Clone)]
pub struct Ema {
    /// Smoothing factor (0 < alpha ≤ 1).
    alpha: f64,
    /// Current EMA value.
    value: f64,
    /// Whether a first sample has been seen.
    initialized: bool,
}

impl Ema {
    /// Create a new EMA with the given smoothing factor.
    ///
    /// # Panics
    /// Panics if `alpha` is not in (0, 1].
    pub fn new(alpha: f64) -> Self {
        assert!(
            alpha > 0.0 && alpha <= 1.0,
            "EMA alpha must be in (0, 1], got {}",
            alpha
        );
        Self {
            alpha,
            value: 0.0,
            initialized: false,
        }
    }

    /// Update the EMA with a new sample and return the updated value.
    ///
    /// The first sample initializes the EMA to that value (no lag).
    #[inline]
    pub fn update(&mut self, sample: f64) -> f64 {
        if !self.initialized {
            self.value = sample;
            self.initialized = true;
        } else {
            self.value = self.alpha * sample + (1.0 - self.alpha) * self.value;
        }
        self.value
    }

    /// Get the current EMA value.
    #[inline]
    pub fn value(&self) -> f64 {
        self.value
    }

    /// Check if the EMA has been initialized with at least one sample.
    #[inline]
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Reset the EMA to uninitialized state.
    pub fn reset(&mut self) {
        self.value = 0.0;
        self.initialized = false;
    }
}

// ============================================================================
// Scale Action
// ============================================================================

/// Decision produced by the hill climber for thread scaling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleAction {
    /// Unpark (activate) worker thread(s).
    Unpark,
    /// Park (deactivate) worker thread(s).
    Park,
    /// No change — objective is stable or within dead zone.
    Hold,
}

/// Scaling decision with the number of workers to park/unpark.
///
/// Returned by `HillClimber::step()`. The `count` field indicates how many
/// workers to park or unpark (0 for `Hold`). With geometric stepping, the
/// count doubles on consecutive improvements in the same direction and
/// resets to 1 on direction reversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScaleDecision {
    /// The scaling action (Unpark, Park, or Hold).
    pub action: ScaleAction,
    /// Number of workers to park/unpark (0 for Hold).
    pub count: usize,
}

// ============================================================================
// Hill Climber
// ============================================================================

/// Geometric-step hill climber for adaptive thread pool sizing.
///
/// The climber explores by adjusting thread count in the current direction.
/// If improvement exceeds the threshold, the climber continues and doubles
/// the step size (geometric acceleration). On worsening, it reverses
/// direction and resets step size to 1.
///
/// A cooldown period prevents oscillation after scaling changes, allowing the
/// system to settle before the next perturbation.
///
/// The climber minimizes the objective function: lower values = better.
///
/// ## Geometric Step Size
///
/// Instead of ±1 per action, the step size doubles on consecutive improvements
/// in the same direction, capped at `max_threads / 4`. On direction reversal,
/// step size resets to 1. This enables rapid ramp-up:
///
/// ```text
/// 1 + 2 + 4 + 8 + 16 = 31 workers in 5 actions × cooldown
/// ```
#[derive(Debug, Clone)]
pub struct HillClimber {
    /// Previous objective value for comparison.
    prev_objective: f64,
    /// Current exploration direction: +1 (unpark) or -1 (park).
    direction: i32,
    /// Cooldown counter (number of ticks to wait after a scale action).
    cooldown_remaining: u32,
    /// Cooldown period (ticks to wait after each scale action).
    cooldown_period: u32,
    /// Minimum improvement required to accept a perturbation.
    improvement_threshold: f64,
    /// Current number of active threads (for boundary clamping).
    current_active: usize,
    /// Minimum threads (never park below this).
    min_threads: usize,
    /// Maximum threads (never unpark above this).
    max_threads: usize,
    /// Whether the climber has been initialized with a first objective.
    initialized: bool,
    /// Geometric step size: doubles on consecutive improvements, resets on reversal.
    /// Capped at `max_threads / 4` to prevent overshooting.
    step_size: usize,
}

impl HillClimber {
    /// Create a new hill climber.
    ///
    /// # Arguments
    /// * `cooldown_period` - Ticks to wait after each scale action
    /// * `improvement_threshold` - Minimum objective improvement to accept a perturbation
    /// * `min_threads` - Minimum thread count (never park below)
    /// * `max_threads` - Maximum thread count (never unpark above)
    /// * `initial_active` - Starting number of active threads
    pub fn new(
        cooldown_period: u32,
        improvement_threshold: f64,
        min_threads: usize,
        max_threads: usize,
        initial_active: usize,
    ) -> Self {
        Self {
            prev_objective: 0.0,
            direction: 1, // Start by trying to add a thread
            cooldown_remaining: 0,
            cooldown_period,
            improvement_threshold,
            current_active: initial_active,
            min_threads,
            max_threads,
            initialized: false,
            step_size: 1,
        }
    }

    /// Feed a new objective value and get the recommended scaling decision.
    ///
    /// The objective should be minimized (lower = better). Four-term formula:
    /// ```text
    /// J(N) = -w_tp * ema_tp + w_qd * ema_qd + w_mp * M(N) + w_rss * R(N)
    /// ```
    /// Weight dominance verified in `formal/rocq/work_pool_stability/theories/WeightDominance.v`.
    ///
    /// Returns `ScaleDecision` with action and count. The count uses geometric
    /// stepping: doubles on consecutive improvements, resets to 1 on reversal.
    pub fn step(&mut self, objective: f64) -> ScaleDecision {
        let hold = ScaleDecision {
            action: ScaleAction::Hold,
            count: 0,
        };

        // First call: initialize baseline and hold
        if !self.initialized {
            self.prev_objective = objective;
            self.initialized = true;
            return hold;
        }

        // During cooldown: hold and decrement.
        // Do NOT update prev_objective here — keep it frozen at the value when
        // the last action was taken. This way, when cooldown expires, the
        // comparison measures the *actual* effect of the action over the full
        // cooldown period, rather than tracking EMA decay (which erases the signal).
        if self.cooldown_remaining > 0 {
            self.cooldown_remaining -= 1;
            return hold;
        }

        // Compare current objective to previous
        let improvement = self.prev_objective - objective; // positive = improvement
        self.prev_objective = objective;

        if improvement >= self.improvement_threshold {
            // Improvement: continue in the same direction, accelerate
            self.step_size = self.accelerated_step_size();
            self.apply_direction()
        } else if improvement <= -self.improvement_threshold {
            // Worsening: reverse direction, reset step size
            self.direction = -self.direction;
            self.step_size = 1;
            self.apply_direction()
        } else {
            // Within dead zone: hold (step_size unchanged)
            hold
        }
    }

    /// Compute the next step size with geometric acceleration.
    ///
    /// Doubles the current step size, capped at `max_threads / 4` (minimum 1).
    /// On a 72-thread pool: max step = 18. On a 4-thread pool: max step = 1.
    fn accelerated_step_size(&self) -> usize {
        let cap = (self.max_threads / 4).max(1);
        (self.step_size * 2).min(cap)
    }

    /// Apply the current direction with geometric step size, respecting boundaries.
    fn apply_direction(&mut self) -> ScaleDecision {
        if self.direction > 0 {
            let new = (self.current_active + self.step_size).min(self.max_threads);
            if new == self.current_active {
                return ScaleDecision {
                    action: ScaleAction::Hold,
                    count: 0,
                }; // At ceiling
            }
            let count = new - self.current_active;
            self.current_active = new;
            self.cooldown_remaining = self.cooldown_period;
            ScaleDecision {
                action: ScaleAction::Unpark,
                count,
            }
        } else {
            let new = self
                .current_active
                .saturating_sub(self.step_size)
                .max(self.min_threads);
            if new == self.current_active {
                return ScaleDecision {
                    action: ScaleAction::Hold,
                    count: 0,
                }; // At floor
            }
            let count = self.current_active - new;
            self.current_active = new;
            self.cooldown_remaining = self.cooldown_period;
            ScaleDecision {
                action: ScaleAction::Park,
                count,
            }
        }
    }

    /// Get the current active thread count as tracked by the climber.
    #[inline]
    pub fn current_active(&self) -> usize {
        self.current_active
    }

    /// Get the current exploration direction: +1 (unpark) or -1 (park).
    #[inline]
    pub fn direction(&self) -> i32 {
        self.direction
    }

    /// Get the remaining cooldown ticks (0 = ready to act).
    #[inline]
    pub fn cooldown_remaining(&self) -> u32 {
        self.cooldown_remaining
    }

    /// Get the previous objective value (baseline for comparison).
    #[inline]
    pub fn prev_objective(&self) -> f64 {
        self.prev_objective
    }

    /// Get the minimum improvement threshold for accepting a perturbation.
    #[inline]
    pub fn improvement_threshold(&self) -> f64 {
        self.improvement_threshold
    }

    /// Get the current geometric step size.
    #[inline]
    pub fn step_size(&self) -> usize {
        self.step_size
    }

    /// Externally update the active count (e.g., after forced scaling).
    pub fn set_current_active(&mut self, count: usize) {
        self.current_active = count.clamp(self.min_threads, self.max_threads);
    }
}

// ============================================================================
// WorkerPark — Per-Worker Parking Primitive
// ============================================================================

/// Per-worker parking primitive for adaptive thread pool workers.
///
/// Each worker has its own `WorkerPark`. The scaling monitor can park or
/// unpark individual workers by toggling the `parked` flag and signaling
/// the condvar.
///
/// Workers check `should_park()` between tasks. If parked, the worker
/// blocks on the condvar until unparked.
pub struct WorkerPark {
    /// Whether this worker is parked.
    parked: Mutex<bool>,
    /// Condvar signaled when the worker should wake up.
    condvar: Condvar,
}

impl WorkerPark {
    /// Create a new WorkerPark in the given initial state.
    pub fn new(initially_parked: bool) -> Self {
        Self {
            parked: Mutex::new(initially_parked),
            condvar: Condvar::new(),
        }
    }

    /// Park the worker (called by the scaling monitor).
    ///
    /// Sets the parked flag. The worker will notice on its next check.
    pub fn park(&self) {
        let _ = self.try_park();
    }

    /// Park the worker only if it was not already parked.
    ///
    /// Returns `true` exactly when this call changes the worker state from
    /// active to parked. Pool active-count accounting must be driven by this
    /// transition result, not by a separate pre-check.
    pub fn try_park(&self) -> bool {
        let mut parked = self.parked.lock();
        if *parked {
            return false;
        }
        *parked = true;
        true
    }

    /// Unpark the worker (called by the scaling monitor).
    ///
    /// Clears the parked flag and notifies the worker.
    pub fn unpark(&self) {
        let _ = self.try_unpark();
    }

    /// Unpark the worker only if it was parked.
    ///
    /// Returns `true` exactly when this call changes the worker state from
    /// parked to active. Pool active-count accounting must be driven by this
    /// transition result, not by a separate pre-check.
    pub fn try_unpark(&self) -> bool {
        let mut parked = self.parked.lock();
        if !*parked {
            return false;
        }
        *parked = false;
        self.condvar.notify_one();
        true
    }

    /// Check if the worker should park, and if so, block until unparked.
    ///
    /// Called by the worker thread between task executions.
    /// Returns immediately if not parked.
    pub fn wait_if_parked(&self) {
        let mut parked = self.parked.lock();
        while *parked {
            self.condvar.wait(&mut parked);
        }
    }

    /// Non-blocking check if this worker is currently parked.
    pub fn is_parked(&self) -> bool {
        *self.parked.lock()
    }

    /// Wait if parked, but with a timeout.
    ///
    /// Returns `true` if unparked, `false` if timed out while still parked.
    pub fn wait_if_parked_timeout(&self, timeout: Duration) -> bool {
        let mut parked = self.parked.lock();
        if !*parked {
            return true;
        }
        // Wait with timeout
        let result = self.condvar.wait_for(&mut parked, timeout);
        if result.timed_out() {
            return !*parked; // Return true if unparked during timeout
        }
        !*parked
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;

    // ========================================================================
    // EMA Tests
    // ========================================================================

    #[test]
    fn test_ema_converges_to_steady_state() {
        let mut ema = Ema::new(0.15);

        // Feed a constant value — EMA should converge to it
        for _ in 0..100 {
            ema.update(42.0);
        }

        assert!(
            (ema.value() - 42.0).abs() < 0.001,
            "EMA should converge to 42.0, got {}",
            ema.value()
        );
    }

    #[test]
    fn test_ema_first_sample_initializes() {
        let mut ema = Ema::new(0.5);
        assert!(!ema.is_initialized());

        let val = ema.update(100.0);
        assert_eq!(val, 100.0, "First sample should set EMA directly");
        assert!(ema.is_initialized());
    }

    #[test]
    fn test_ema_alpha_responsiveness() {
        // High alpha: more responsive
        let mut high = Ema::new(0.9);
        high.update(0.0);
        high.update(100.0);
        let high_val = high.value();

        // Low alpha: less responsive
        let mut low = Ema::new(0.1);
        low.update(0.0);
        low.update(100.0);
        let low_val = low.value();

        assert!(
            high_val > low_val,
            "High alpha EMA ({}) should be closer to 100 than low alpha ({})",
            high_val,
            low_val
        );
    }

    #[test]
    fn test_ema_reset() {
        let mut ema = Ema::new(0.5);
        ema.update(100.0);
        assert!(ema.is_initialized());

        ema.reset();
        assert!(!ema.is_initialized());
        assert_eq!(ema.value(), 0.0);
    }

    #[test]
    #[should_panic(expected = "EMA alpha must be in (0, 1]")]
    fn test_ema_rejects_zero_alpha() {
        Ema::new(0.0);
    }

    #[test]
    #[should_panic(expected = "EMA alpha must be in (0, 1]")]
    fn test_ema_rejects_negative_alpha() {
        Ema::new(-0.5);
    }

    // ========================================================================
    // HillClimber Tests
    // ========================================================================

    #[test]
    fn test_hill_climber_first_step_holds() {
        let mut climber = HillClimber::new(3, 0.05, 1, 8, 4);
        let decision = climber.step(10.0);
        assert_eq!(
            decision.action,
            ScaleAction::Hold,
            "First step should always hold"
        );
        assert_eq!(decision.count, 0);
    }

    #[test]
    fn test_hill_climber_improvement_continues_direction() {
        let mut climber = HillClimber::new(0, 0.05, 1, 8, 4);

        // Initialize
        climber.step(10.0);

        // Feed improving objective (lower = better)
        let decision = climber.step(9.0); // improvement = 10 - 9 = 1.0 > 0.05
        assert_eq!(
            decision.action,
            ScaleAction::Unpark,
            "Improvement should continue in default direction (unpark)"
        );
        // First improvement: step_size doubles from 1 → 2, but capped at max/4 = 8/4 = 2
        // apply_direction uses step_size=2, but current_active=4, new=min(4+2,8)=6, count=2
        assert!(decision.count >= 1, "Should unpark at least 1 worker");
    }

    #[test]
    fn test_hill_climber_worsening_reverses_direction() {
        let mut climber = HillClimber::new(0, 0.05, 1, 8, 4);

        // Initialize
        climber.step(10.0);

        // Feed worsening objective (higher = worse)
        let decision = climber.step(11.0); // improvement = 10 - 11 = -1.0 < -0.05
        assert_eq!(
            decision.action,
            ScaleAction::Park,
            "Worsening should reverse direction to park"
        );
        // Worsening resets step_size to 1
        assert_eq!(decision.count, 1, "Worsening should reset step to 1");
    }

    #[test]
    fn test_hill_climber_plateau_holds() {
        let mut climber = HillClimber::new(0, 0.05, 1, 8, 4);

        // Initialize
        climber.step(10.0);

        // Feed same objective (within dead zone)
        let decision = climber.step(10.01); // improvement = 10 - 10.01 = -0.01, abs < 0.05
        assert_eq!(
            decision.action,
            ScaleAction::Hold,
            "Small delta should hold (dead zone)"
        );
        assert_eq!(decision.count, 0);
    }

    #[test]
    fn test_hill_climber_cooldown_behavior() {
        let mut climber = HillClimber::new(3, 0.05, 1, 8, 4);

        // Initialize
        climber.step(10.0);

        // Trigger action (improvement)
        let decision = climber.step(5.0);
        assert_eq!(decision.action, ScaleAction::Unpark);

        // Next 3 steps should hold (cooldown = 3)
        assert_eq!(
            climber.step(4.0).action,
            ScaleAction::Hold,
            "Cooldown tick 1"
        );
        assert_eq!(
            climber.step(3.0).action,
            ScaleAction::Hold,
            "Cooldown tick 2"
        );
        assert_eq!(
            climber.step(2.0).action,
            ScaleAction::Hold,
            "Cooldown tick 3"
        );

        // Cooldown expired — prev_objective is frozen at 5.0 (Fix 3).
        // improvement = 5.0 - 1.0 = 4.0 > 0.05 → Unpark
        let decision = climber.step(1.0);
        assert_eq!(
            decision.action,
            ScaleAction::Unpark,
            "After cooldown, improvement should trigger action"
        );
    }

    #[test]
    fn test_hill_climber_max_boundary() {
        let mut climber = HillClimber::new(0, 0.05, 1, 4, 4); // At max already

        // Initialize
        climber.step(10.0);

        // Improvement in unpark direction, but already at max
        let decision = climber.step(5.0);
        assert_eq!(
            decision.action,
            ScaleAction::Hold,
            "At max threads, unpark should hold"
        );
    }

    #[test]
    fn test_hill_climber_min_boundary() {
        let mut climber = HillClimber::new(0, 0.05, 4, 8, 4); // At min already

        // Initialize
        climber.step(10.0);

        // Worsening should try to park, but we're at min
        let decision = climber.step(15.0);
        assert_eq!(
            decision.action,
            ScaleAction::Hold,
            "At min threads, park should hold"
        );
    }

    #[test]
    fn test_hill_climber_geometric_acceleration() {
        // Test that step size doubles on consecutive improvements
        let mut climber = HillClimber::new(0, 0.05, 1, 64, 1);

        // Initialize
        climber.step(100.0);

        // First improvement: step_size = min(1*2, 64/4=16) = 2 → unpark 2
        let d1 = climber.step(90.0);
        assert_eq!(d1.action, ScaleAction::Unpark);
        assert_eq!(d1.count, 2, "First improvement: step_size should be 2");
        assert_eq!(climber.current_active(), 3);

        // Second improvement: step_size = min(2*2, 16) = 4 → unpark 4
        let d2 = climber.step(80.0);
        assert_eq!(d2.action, ScaleAction::Unpark);
        assert_eq!(d2.count, 4, "Second improvement: step_size should be 4");
        assert_eq!(climber.current_active(), 7);

        // Third improvement: step_size = min(4*2, 16) = 8 → unpark 8
        let d3 = climber.step(70.0);
        assert_eq!(d3.action, ScaleAction::Unpark);
        assert_eq!(d3.count, 8, "Third improvement: step_size should be 8");
        assert_eq!(climber.current_active(), 15);

        // Fourth improvement: step_size = min(8*2, 16) = 16 → unpark 16
        let d4 = climber.step(60.0);
        assert_eq!(d4.action, ScaleAction::Unpark);
        assert_eq!(d4.count, 16, "Fourth improvement: capped at max/4=16");
        assert_eq!(climber.current_active(), 31);

        // Worsening: reverses direction, resets step_size to 1
        let d5 = climber.step(200.0);
        assert_eq!(d5.action, ScaleAction::Park);
        assert_eq!(d5.count, 1, "Worsening should reset step to 1");
        assert_eq!(climber.current_active(), 30);
    }

    // ========================================================================
    // WorkerPark Tests
    // ========================================================================

    #[test]
    fn test_worker_park_initially_unparked() {
        let wp = WorkerPark::new(false);
        assert!(!wp.is_parked());
    }

    #[test]
    fn test_worker_park_initially_parked() {
        let wp = WorkerPark::new(true);
        assert!(wp.is_parked());
    }

    #[test]
    fn test_worker_park_and_unpark() {
        let wp = WorkerPark::new(false);

        wp.park();
        assert!(wp.is_parked());

        wp.unpark();
        assert!(!wp.is_parked());
    }

    #[test]
    fn test_worker_park_transition_results_are_idempotent() {
        let wp = WorkerPark::new(false);

        assert!(wp.try_park(), "active -> parked should report a transition");
        assert!(
            !wp.try_park(),
            "parked -> parked must not report a transition"
        );
        assert!(wp.is_parked());

        assert!(
            wp.try_unpark(),
            "parked -> active should report a transition"
        );
        assert!(
            !wp.try_unpark(),
            "active -> active must not report a transition"
        );
        assert!(!wp.is_parked());
    }

    #[test]
    fn test_worker_park_blocks_thread() {
        let wp = Arc::new(WorkerPark::new(true));
        let wp_clone = Arc::clone(&wp);

        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = Arc::clone(&counter);

        let handle = thread::spawn(move || {
            // This should block because the worker starts parked
            wp_clone.wait_if_parked();
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        });

        // Give the thread time to reach the park wait
        thread::sleep(Duration::from_millis(50));
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "Thread should be blocked while parked"
        );

        // Unpark — thread should proceed
        wp.unpark();
        handle.join().expect("Thread panicked");

        assert_eq!(
            counter.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "Thread should have executed after unpark"
        );
    }

    #[test]
    fn test_worker_park_not_parked_returns_immediately() {
        let wp = WorkerPark::new(false);
        // Should not block
        wp.wait_if_parked();
    }

    #[test]
    fn test_worker_park_timeout() {
        let wp = WorkerPark::new(true);

        let start = std::time::Instant::now();
        let result = wp.wait_if_parked_timeout(Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(!result, "Should return false (still parked after timeout)");
        assert!(
            elapsed >= Duration::from_millis(40),
            "Should wait approximately the timeout, elapsed: {:?}",
            elapsed,
        );
    }
}
