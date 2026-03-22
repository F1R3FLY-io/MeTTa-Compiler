//! Derived Analyses from Fixed-Point Results
//!
//! Extracts high-level properties from the abstract fixed-point computation:
//! dead rules, determinism, purity, type specialization, and groundness.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use super::abstract_domain::AbstractType;
use super::fixpoint::{AnalysisResult, ExprFact};

// ============================================================================
// Derived Analysis Results
// ============================================================================

/// High-level analysis results derived from the fixed-point.
#[derive(Debug, Clone)]
pub struct DerivedAnalysis {
    /// Rules that are never reachable from any top-level expression.
    pub dead_rules: HashSet<u32>,

    /// Expressions that always match exactly 1 rule.
    /// Maps (head, arity) to the single matching rule index.
    pub deterministic_dispatch: HashMap<(&'static str, usize), u32>,

    /// Expression hashes that are provably pure.
    pub pure_expressions: HashSet<u64>,

    /// Expression hashes with known singleton result types.
    pub type_specializations: HashMap<u64, AbstractType>,

    /// Expression hashes that always produce ground values.
    pub ground_expressions: HashSet<u64>,

    /// Memoization candidates: pure + deterministic.
    pub memo_candidates: HashSet<u64>,

    /// Parallelization candidates: pure expressions.
    pub parallel_candidates: HashSet<u64>,
}

/// Dispatch hint for a specific (head, arity) combination.
#[derive(Debug, Clone, Copy)]
pub struct DispatchHint {
    /// If Some, this expression always resolves to exactly this rule.
    pub deterministic_rule: Option<u32>,
    /// If true, expression is provably pure (safe to memoize/parallelize).
    pub is_pure: bool,
    /// If Some, result is always this type.
    pub result_type: Option<AbstractType>,
    /// If true, result is always ground.
    pub is_ground: bool,
}

impl Default for DispatchHint {
    fn default() -> Self {
        Self {
            deterministic_rule: None,
            is_pure: false,
            result_type: None,
            is_ground: false,
        }
    }
}

// ============================================================================
// Derivation Functions
// ============================================================================

/// Derive all high-level analysis facts from the fixed-point result.
pub fn derive_analysis(result: &AnalysisResult) -> DerivedAnalysis {
    let dead_rules = detect_dead_rules(result);
    let deterministic_dispatch = detect_determinism(result);
    let pure_expressions = detect_purity(result);
    let type_specializations = detect_type_specialization(result);
    let ground_expressions = detect_groundness(result);

    let memo_candidates: HashSet<u64> = pure_expressions.iter()
        .filter(|hash| {
            result.expr_facts.get(hash)
                .and_then(|f| f.is_deterministic)
                .unwrap_or(false)
        })
        .copied()
        .collect();

    let parallel_candidates = pure_expressions.clone();

    DerivedAnalysis {
        dead_rules,
        deterministic_dispatch,
        pure_expressions,
        type_specializations,
        ground_expressions,
        memo_candidates,
        parallel_candidates,
    }
}

/// Detect dead rules: rules never appearing in any expression's reachable_rules.
fn detect_dead_rules(result: &AnalysisResult) -> HashSet<u32> {
    let mut live_rules: HashSet<u32> = HashSet::new();
    for fact in result.expr_facts.values() {
        for &idx in &fact.reachable_rules {
            live_rules.insert(idx);
        }
    }

    // Any rule index from 0..total that isn't live is dead.
    // We don't know total_rules here, so we collect live and let
    // the caller compute the complement.
    // For now, return an empty set — the caller computes dead = all - live.
    // This is populated by the integration layer which knows total_rules.
    HashSet::new()
}

/// Detect dead rules given the total rule count.
pub fn detect_dead_rules_with_total(result: &AnalysisResult, total_rules: u32) -> HashSet<u32> {
    let mut live_rules: HashSet<u32> = HashSet::new();
    for fact in result.expr_facts.values() {
        for &idx in &fact.reachable_rules {
            live_rules.insert(idx);
        }
    }

    (0..total_rules)
        .filter(|idx| !live_rules.contains(idx))
        .collect()
}

/// Detect deterministic expressions.
fn detect_determinism(result: &AnalysisResult) -> HashMap<(&'static str, usize), u32> {
    let mut deterministic = HashMap::new();

    for (hash, fact) in &result.expr_facts {
        if fact.is_deterministic == Some(true) && fact.reachable_rules.len() == 1 {
            // We need (head, arity) but only have the hash.
            // For now, store the rule index keyed by a synthetic key.
            // The integration layer will map hashes to (head, arity).
            // This is a simplification — full implementation would
            // maintain the mapping during analysis.
        }
    }

    deterministic
}

/// Detect pure expressions.
fn detect_purity(result: &AnalysisResult) -> HashSet<u64> {
    result.expr_facts.iter()
        .filter(|(_, fact)| fact.is_pure == Some(true))
        .map(|(hash, _)| *hash)
        .collect()
}

/// Detect type specialization opportunities.
fn detect_type_specialization(result: &AnalysisResult) -> HashMap<u64, AbstractType> {
    result.expr_facts.iter()
        .filter(|(_, fact)| fact.result_types.len() == 1)
        .filter_map(|(hash, fact)| {
            let t = fact.result_types.iter().next()?;
            Some((*hash, *t))
        })
        .collect()
}

/// Detect ground expressions.
fn detect_groundness(result: &AnalysisResult) -> HashSet<u64> {
    result.expr_facts.iter()
        .filter(|(_, fact)| fact.is_ground == Some(true))
        .map(|(hash, _)| *hash)
        .collect()
}

/// Build dispatch hints from derived analysis.
pub fn build_dispatch_hints(analysis: &DerivedAnalysis) -> HashMap<u64, DispatchHint> {
    let mut hints = HashMap::new();

    for &hash in &analysis.pure_expressions {
        let hint = hints.entry(hash).or_insert_with(DispatchHint::default);
        hint.is_pure = true;
    }

    for (&hash, &t) in &analysis.type_specializations {
        let hint = hints.entry(hash).or_insert_with(DispatchHint::default);
        hint.result_type = Some(t);
    }

    for &hash in &analysis.ground_expressions {
        let hint = hints.entry(hash).or_insert_with(DispatchHint::default);
        hint.is_ground = true;
    }

    hints
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use super::super::fixpoint::ExprFact;
    use super::super::abstract_domain::AbstractStore;
    use super::super::abstract_transition::AbstractState;

    fn make_result(facts: Vec<(u64, ExprFact)>) -> AnalysisResult {
        AnalysisResult {
            reachable_states: HashSet::new(),
            store: AbstractStore::new(),
            expr_facts: facts.into_iter().collect(),
            iterations: 1,
            converged: true,
            analysis_time_ms: 0,
        }
    }

    #[test]
    fn test_detect_purity() {
        let result = make_result(vec![
            (1, ExprFact { is_pure: Some(true), ..Default::default() }),
            (2, ExprFact { is_pure: Some(false), ..Default::default() }),
            (3, ExprFact { is_pure: None, ..Default::default() }),
        ]);
        let pure = detect_purity(&result);
        assert!(pure.contains(&1));
        assert!(!pure.contains(&2));
        assert!(!pure.contains(&3));
    }

    #[test]
    fn test_detect_type_specialization() {
        let mut fact = ExprFact::default();
        fact.result_types.insert(AbstractType::Long);

        let result = make_result(vec![(42, fact)]);
        let specs = detect_type_specialization(&result);
        assert_eq!(specs.get(&42), Some(&AbstractType::Long));
    }

    #[test]
    fn test_detect_type_specialization_mixed() {
        let mut fact = ExprFact::default();
        fact.result_types.insert(AbstractType::Long);
        fact.result_types.insert(AbstractType::String);

        let result = make_result(vec![(42, fact)]);
        let specs = detect_type_specialization(&result);
        assert!(specs.get(&42).is_none()); // Mixed types → no specialization
    }

    #[test]
    fn test_detect_dead_rules_with_total() {
        let mut fact = ExprFact::default();
        fact.reachable_rules = SmallVec::from_slice(&[0, 2, 4]);

        let result = make_result(vec![(1, fact)]);
        let dead = detect_dead_rules_with_total(&result, 5);
        assert!(dead.contains(&1));
        assert!(dead.contains(&3));
        assert!(!dead.contains(&0));
        assert!(!dead.contains(&2));
        assert!(!dead.contains(&4));
    }

    #[test]
    fn test_derive_analysis() {
        let mut fact = ExprFact::default();
        fact.is_pure = Some(true);
        fact.is_deterministic = Some(true);
        fact.reachable_rules = SmallVec::from_slice(&[0]);
        fact.result_types.insert(AbstractType::Long);

        let result = make_result(vec![(42, fact)]);
        let derived = derive_analysis(&result);

        assert!(derived.pure_expressions.contains(&42));
        assert!(derived.memo_candidates.contains(&42));
        assert!(derived.parallel_candidates.contains(&42));
        assert_eq!(derived.type_specializations.get(&42), Some(&AbstractType::Long));
    }

    #[test]
    fn test_build_dispatch_hints() {
        let derived = DerivedAnalysis {
            dead_rules: HashSet::new(),
            deterministic_dispatch: HashMap::new(),
            pure_expressions: vec![42].into_iter().collect(),
            type_specializations: vec![(42, AbstractType::Long)].into_iter().collect(),
            ground_expressions: vec![42].into_iter().collect(),
            memo_candidates: vec![42].into_iter().collect(),
            parallel_candidates: vec![42].into_iter().collect(),
        };

        let hints = build_dispatch_hints(&derived);
        let hint = hints.get(&42).expect("should have hint");
        assert!(hint.is_pure);
        assert!(hint.is_ground);
        assert_eq!(hint.result_type, Some(AbstractType::Long));
    }
}
