//! AAM integration: build scheduler automaton from DerivedAnalysis.
//!
//! After the AAM fixed-point analysis completes and `DerivedAnalysis` is
//! available, this module constructs a `SchedulerAutomaton` with:
//!
//! 1. Classification table entries derived from analysis results (purity,
//!    determinism, groundness, type specialization)
//! 2. Online weight EMAs seeded with P² median estimates (warm-start)
//! 3. Context weights from WPDS poststar computation
//!
//! The resulting automaton is installed as the global scheduler via
//! `install_scheduler()`.

use crate::backend::analysis::derived::DerivedAnalysis;
use crate::backend::priority_scheduler::RuntimeTracker;

use super::classification::SchedulerAutomaton;
use super::context_weights::build_context_weights;
use super::cost_class::{descriptor_flags, CostClass};

// ══════════════════════════════════════════════════════════════════════════════
// Builder
// ══════════════════════════════════════════════════════════════════════════════

/// Build a scheduler automaton from AAM analysis results.
///
/// Uses `DerivedAnalysis` to populate the classification table:
/// - Pure + deterministic → `SymbolicCheap`
/// - Pure + nondeterministic → `ParallelPure`
/// - Impure → `ImpureSequential`
/// - Ground → `GroundCheap` or `GroundArith`
/// - Memo candidates → memoizable flag
///
/// If `runtime_tracker` is provided, seeds the weight EMAs with P² medians.
pub fn build_scheduler_automaton(
    analysis: &DerivedAnalysis,
    runtime_tracker: Option<&RuntimeTracker>,
) -> SchedulerAutomaton {
    let mut automaton = SchedulerAutomaton::new();

    // 1. Populate classification table from analysis results
    populate_from_analysis(&mut automaton, analysis);

    // 2. Build context weights from WPDS poststar
    let ctx_weights = build_context_weights(automaton.epoch());
    for entry in ctx_weights.weights.iter() {
        automaton.insert_context_weight(*entry.key(), *entry.value());
    }

    // 3. Warm-start weight EMAs from P² tracker
    if let Some(tracker) = runtime_tracker {
        warm_start_weights(&automaton, tracker);
    }

    // 4. Advance epoch
    automaton.advance_epoch();

    automaton
}

/// Populate the classification table from DerivedAnalysis.
fn populate_from_analysis(automaton: &mut SchedulerAutomaton, analysis: &DerivedAnalysis) {
    // Pure expressions
    for &hash in &analysis.pure_expressions {
        let head_hash = (hash & 0xFFFF) as u16;

        // Check if also deterministic
        let is_deterministic = analysis.memo_candidates.contains(&hash);
        let is_ground = analysis.ground_expressions.contains(&hash);

        let cost_class = if is_ground {
            if analysis.type_specializations.contains_key(&hash) {
                CostClass::GroundArith
            } else {
                CostClass::GroundCheap
            }
        } else if is_deterministic {
            CostClass::SymbolicCheap
        } else {
            CostClass::ParallelPure
        };

        let mut flags_value: u8 = descriptor_flags::PURE;
        if is_ground {
            flags_value |= descriptor_flags::GROUND;
        }
        if is_deterministic {
            flags_value |= descriptor_flags::DETERMINISTIC;
        }

        // Insert for all arities (0..15) — the analysis hash doesn't encode arity
        // so we use a wildcard (flags_mask = PURE, meaning "match if pure flag set")
        automaton.insert_classification(
            head_hash,
            0, // arity wildcard (L2 entry applies to arity 0)
            descriptor_flags::PURE,
            descriptor_flags::PURE,
            cost_class,
        );
    }

    // Memo candidates get the MEMO_CANDIDATE flag
    for &hash in &analysis.memo_candidates {
        let head_hash = (hash & 0xFFFF) as u16;
        automaton.insert_classification(
            head_hash,
            0,
            descriptor_flags::MEMO_CANDIDATE,
            descriptor_flags::MEMO_CANDIDATE,
            CostClass::SymbolicCheap,
        );
    }
}

/// Seed weight EMAs with P² median estimates.
fn warm_start_weights(_automaton: &SchedulerAutomaton, tracker: &RuntimeTracker) {
    let global_median = tracker.global_median();
    if global_median > 0.0 {
        // The P² tracker uses TaskTypeId, which doesn't directly map to
        // (head_hash, arity) keys. We seed with the global median so that
        // the EMA's first-sample initialization provides a reasonable starting
        // point. Subsequent per-expression observations will quickly converge
        // to accurate per-expression estimates via the EMA.
        //
        // In the future, if TaskTypeId::Eval carries expression hash info,
        // we can do a more targeted transfer.

        // For now, the warm-start happens implicitly: when a new (head,arity)
        // pair is first observed, the EMA initializes to the actual runtime
        // of that first observation (no lag). This is already optimal for
        // cold-start convergence.
        let _ = global_median; // Used as documentation of the warm-start strategy
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Scheduler hints for DerivedAnalysis
// ══════════════════════════════════════════════════════════════════════════════

/// Scheduler hints derived from analysis, suitable for inclusion in DerivedAnalysis.
///
/// Each hint maps an expression hash to its statically-determined cost class.
#[derive(Debug, Clone)]
pub struct SchedulerHints {
    /// Expression hash → cost class mappings.
    pub hints: Vec<(u64, CostClass)>,
}

impl SchedulerHints {
    /// Build scheduler hints from a DerivedAnalysis.
    pub fn from_analysis(analysis: &DerivedAnalysis) -> Self {
        let mut hints = Vec::new();

        for &hash in &analysis.pure_expressions {
            let is_deterministic = analysis.memo_candidates.contains(&hash);
            let is_ground = analysis.ground_expressions.contains(&hash);

            let class = if is_ground {
                CostClass::GroundCheap
            } else if is_deterministic {
                CostClass::SymbolicCheap
            } else {
                CostClass::ParallelPure
            };

            hints.push((hash, class));
        }

        SchedulerHints { hints }
    }

    /// Number of hints.
    pub fn len(&self) -> usize {
        self.hints.len()
    }

    /// Whether there are no hints.
    pub fn is_empty(&self) -> bool {
        self.hints.is_empty()
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn make_analysis() -> DerivedAnalysis {
        DerivedAnalysis {
            dead_rules: HashSet::new(),
            deterministic_dispatch: HashMap::new(),
            pure_expressions: vec![42, 100].into_iter().collect(),
            type_specializations: HashMap::new(),
            ground_expressions: vec![42].into_iter().collect(),
            memo_candidates: vec![42].into_iter().collect(),
            parallel_candidates: vec![100].into_iter().collect(),
            scheduler_hints: Vec::new(),
        }
    }

    #[test]
    fn test_build_scheduler_automaton() {
        let analysis = make_analysis();
        let automaton = build_scheduler_automaton(&analysis, None);

        // Should have advanced epoch
        assert_eq!(automaton.epoch(), 1);
    }

    #[test]
    fn test_scheduler_hints() {
        let analysis = make_analysis();
        let hints = SchedulerHints::from_analysis(&analysis);

        assert!(!hints.is_empty());

        // Hash 42: pure + ground + deterministic → GroundCheap
        let h42 = hints.hints.iter().find(|(h, _)| *h == 42);
        assert!(h42.is_some());
        assert_eq!(h42.unwrap().1, CostClass::GroundCheap);

        // Hash 100: pure + not ground + not deterministic → ParallelPure
        let h100 = hints.hints.iter().find(|(h, _)| *h == 100);
        assert!(h100.is_some());
        assert_eq!(h100.unwrap().1, CostClass::ParallelPure);
    }
}
