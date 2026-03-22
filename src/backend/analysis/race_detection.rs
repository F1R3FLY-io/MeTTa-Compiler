//! Race Detection for Stateful Operations (Phase 5.7)
//!
//! Detects potential data races when parallel branches perform conflicting
//! operations on shared state (atomspaces, state cells). Uses purity analysis
//! from Phase 3.3 and reachability from Phase 4 to identify expressions that
//! contain Write-Write or Read-Write conflicts on the same target.

use std::collections::{HashMap, HashSet};

use super::derived::DerivedAnalysis;
use super::fixpoint::AnalysisResult;

// ============================================================================
// Abstract Operations
// ============================================================================

/// An abstract operation on shared state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AbstractOp {
    /// Read from a space or state.
    Read { target: AbstractTarget },
    /// Write to a space or state.
    Write { target: AbstractTarget },
}

/// Target of a state operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AbstractTarget {
    /// A named space (e.g., "&self", "&kb").
    Space(Option<&'static str>),
    /// A state cell (identified by hash).
    State(u64),
    /// Any unknown target.
    Unknown,
}

// ============================================================================
// Potential Race
// ============================================================================

/// A potential data race between two operations.
#[derive(Debug, Clone)]
pub struct PotentialRace {
    /// First operation.
    pub op1: AbstractOp,
    /// Expression hash where op1 occurs.
    pub op1_expr: u64,
    /// Second operation.
    pub op2: AbstractOp,
    /// Expression hash where op2 occurs.
    pub op2_expr: u64,
}

// ============================================================================
// Race Detection Result
// ============================================================================

/// Result of race detection analysis.
#[derive(Debug)]
pub struct RaceDetectionResult {
    /// All abstract operations found.
    pub operations: Vec<(u64, AbstractOp)>,
    /// Potential races detected.
    pub potential_races: Vec<PotentialRace>,
    /// Expression hashes that are safe for parallel execution (no conflicts).
    pub safe_parallel: HashSet<u64>,
    /// Expression hashes that must be serialized (have conflicts).
    pub must_serialize: HashSet<u64>,
}

impl RaceDetectionResult {
    /// Check if an expression is safe for parallel execution.
    pub fn is_safe(&self, expr_hash: u64) -> bool {
        self.safe_parallel.contains(&expr_hash) || !self.must_serialize.contains(&expr_hash)
    }

    /// Number of potential races found.
    pub fn race_count(&self) -> usize {
        self.potential_races.len()
    }
}

// ============================================================================
// Detection
// ============================================================================

/// Run race detection on AAM analysis results.
///
/// Identifies expression pairs that may execute in parallel and perform
/// conflicting operations on the same shared state target.
pub fn detect_races(
    analysis: &AnalysisResult,
    derived: &DerivedAnalysis,
) -> RaceDetectionResult {
    let mut operations: Vec<(u64, AbstractOp)> = Vec::new();
    let mut potential_races: Vec<PotentialRace> = Vec::new();

    // Classify expressions by their operations
    // Pure expressions (from derived.pure_expressions) have no state operations
    // Impure expressions may have reads or writes
    let impure_exprs: HashSet<u64> = analysis.expr_facts.keys()
        .filter(|hash| !derived.pure_expressions.contains(hash))
        .copied()
        .collect();

    // For impure expressions, conservatively mark as Write (any state op)
    for &hash in &impure_exprs {
        operations.push((hash, AbstractOp::Write { target: AbstractTarget::Unknown }));
    }

    // Detect Write-Write conflicts between impure expressions
    let impure_list: Vec<u64> = impure_exprs.iter().copied().collect();
    for i in 0..impure_list.len() {
        for j in (i+1)..impure_list.len() {
            potential_races.push(PotentialRace {
                op1: AbstractOp::Write { target: AbstractTarget::Unknown },
                op1_expr: impure_list[i],
                op2: AbstractOp::Write { target: AbstractTarget::Unknown },
                op2_expr: impure_list[j],
            });
        }
    }

    let safe_parallel = derived.pure_expressions.clone();
    let must_serialize = impure_exprs;

    RaceDetectionResult {
        operations,
        potential_races,
        safe_parallel,
        must_serialize,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::backend::analysis::abstract_domain::AbstractStore;
    use crate::backend::analysis::fixpoint::{ExprFact, AnalysisResult};

    fn make_result(expr_hashes: Vec<u64>) -> AnalysisResult {
        let expr_facts: HashMap<u64, ExprFact> = expr_hashes.into_iter()
            .map(|h| (h, ExprFact::default()))
            .collect();
        AnalysisResult {
            reachable_states: HashSet::new(),
            store: AbstractStore::new(),
            expr_facts,
            iterations: 1,
            converged: true,
            analysis_time_ms: 0,
        }
    }

    fn make_derived(pure: Vec<u64>) -> DerivedAnalysis {
        DerivedAnalysis {
            dead_rules: HashSet::new(),
            deterministic_dispatch: HashMap::new(),
            pure_expressions: pure.into_iter().collect(),
            type_specializations: HashMap::new(),
            ground_expressions: HashSet::new(),
            memo_candidates: HashSet::new(),
            parallel_candidates: HashSet::new(),
        }
    }

    #[test]
    fn test_all_pure_no_races() {
        let result = make_result(vec![1, 2, 3]);
        let derived = make_derived(vec![1, 2, 3]);
        let races = detect_races(&result, &derived);

        assert_eq!(races.race_count(), 0);
        assert!(races.is_safe(1));
        assert!(races.is_safe(2));
    }

    #[test]
    fn test_impure_races_detected() {
        let result = make_result(vec![1, 2, 3]);
        let derived = make_derived(vec![1]); // Only 1 is pure; 2,3 are impure

        let races = detect_races(&result, &derived);
        assert_eq!(races.race_count(), 1); // 2-3 conflict
        assert!(races.is_safe(1));
        assert!(!races.is_safe(2));
        assert!(!races.is_safe(3));
    }

    #[test]
    fn test_empty_program() {
        let result = make_result(vec![]);
        let derived = make_derived(vec![]);
        let races = detect_races(&result, &derived);
        assert_eq!(races.race_count(), 0);
    }
}
