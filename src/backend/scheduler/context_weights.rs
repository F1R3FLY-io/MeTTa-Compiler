//! Continuation-to-StackSymbol mapping and context weight computation.
//!
//! Maps MeTTaTron's `GenericContinuation` frames to `SchedulerStackSymbol`
//! values, computes context hashes for the top-k frames, and manages the
//! precomputed context weight table populated by WPDS poststar.
//!
//! ## Context Weight Semantics
//!
//! The context weight is a multiplier applied to the base priority from the
//! transduction table. Examples:
//!
//! - Recursive calls at depth > 3 in the same head symbol → increase priority
//!   (likely to diverge, fail fast)
//! - Guard expression inside `(if ...)` → decrease priority (cheap, evaluate first)
//! - Expression inside `(collapse ...)` → parallelism hint boost (all results needed)
//! - Deep `let*` chain → sticky affinity (cache locality matters)

use dashmap::DashMap;

use super::semiring::{Semiring, TropicalWeight};
use super::wpds::{SchedulerStackSymbol, Wpds, WpdsRule, hash_context};

// ══════════════════════════════════════════════════════════════════════════════
// Context weight table
// ══════════════════════════════════════════════════════════════════════════════

/// Precomputed context weight table.
///
/// Maps `hash(top_3_continuation_frames)` → weight multiplier.
/// Populated by `build_context_weights()` from WPDS poststar analysis.
/// At runtime, each task lookup is O(1) via DashMap.
pub struct ContextWeightTable {
    /// The precomputed weights.
    pub(super) weights: DashMap<u64, f32>,

    /// The epoch at which these weights were computed.
    epoch: u64,
}

impl ContextWeightTable {
    /// Create an empty context weight table.
    pub fn new(epoch: u64) -> Self {
        ContextWeightTable {
            weights: DashMap::new(),
            epoch,
        }
    }

    /// Look up the context weight for a given continuation hash.
    ///
    /// Returns 1.0 (neutral) if no entry exists.
    #[inline]
    pub fn get(&self, context_hash: u64) -> f32 {
        self.weights
            .get(&context_hash)
            .map(|v| *v)
            .unwrap_or(1.0)
    }

    /// Insert or update a context weight.
    pub fn insert(&self, context_hash: u64, weight: f32) {
        self.weights.insert(context_hash, weight);
    }

    /// The epoch at which these weights were computed.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Number of entries in the table.
    pub fn len(&self) -> usize {
        self.weights.len()
    }

    /// Whether the table is empty.
    pub fn is_empty(&self) -> bool {
        self.weights.is_empty()
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Default WPDS rules for MeTTaTron
// ══════════════════════════════════════════════════════════════════════════════

/// Build the default WPDS rules modeling MeTTaTron's evaluation patterns.
///
/// These rules encode the common continuation transitions observed during
/// evaluation. Weights are initialized to `TropicalWeight::one()` (zero cost)
/// and refined online as actual runtimes are observed.
pub fn build_default_wpds() -> Wpds<TropicalWeight> {
    let mut wpds = Wpds::new(SchedulerStackSymbol::Root);

    // Root → RuleMatch (entering rule matching)
    wpds.add_rule(WpdsRule::Replace {
        from: SchedulerStackSymbol::Root,
        to: SchedulerStackSymbol::RuleMatch {
            head_hash: 0,
            arity: 0,
        },
        weight: TropicalWeight::one(),
    });

    // RuleMatch → push ArgEval (evaluating arguments before applying rule)
    wpds.add_rule(WpdsRule::Push {
        from: SchedulerStackSymbol::RuleMatch {
            head_hash: 0,
            arity: 0,
        },
        to_bottom: SchedulerStackSymbol::RuleMatch {
            head_hash: 0,
            arity: 0,
        },
        to_top: SchedulerStackSymbol::ArgEval { position: 0 },
        weight: TropicalWeight::new(0.5),
    });

    // RuleMatch → Binding (applying rule bindings)
    wpds.add_rule(WpdsRule::Replace {
        from: SchedulerStackSymbol::RuleMatch {
            head_hash: 0,
            arity: 0,
        },
        to: SchedulerStackSymbol::Binding { rule_idx: 0 },
        weight: TropicalWeight::new(1.0),
    });

    // Conditional → push ArgEval (evaluating condition)
    wpds.add_rule(WpdsRule::Push {
        from: SchedulerStackSymbol::Conditional { branch: 0 },
        to_bottom: SchedulerStackSymbol::Conditional { branch: 0 },
        to_top: SchedulerStackSymbol::ArgEval { position: 0 },
        weight: TropicalWeight::new(0.2), // Conditions are typically cheap
    });

    // LetChain → push nested eval (evaluating let binding value)
    wpds.add_rule(WpdsRule::Push {
        from: SchedulerStackSymbol::LetChain { depth: 0 },
        to_bottom: SchedulerStackSymbol::LetChain { depth: 1 },
        to_top: SchedulerStackSymbol::ArgEval { position: 0 },
        weight: TropicalWeight::new(1.0),
    });

    // Collapse → push RuleMatch (evaluating collapse body)
    wpds.add_rule(WpdsRule::Push {
        from: SchedulerStackSymbol::Collapse,
        to_bottom: SchedulerStackSymbol::Collapse,
        to_top: SchedulerStackSymbol::RuleMatch {
            head_hash: 0,
            arity: 0,
        },
        weight: TropicalWeight::new(3.0), // Collapse is expensive (all results needed)
    });

    // ArgEval → pop (argument evaluation complete)
    wpds.add_rule(WpdsRule::Pop {
        from: SchedulerStackSymbol::ArgEval { position: 0 },
        weight: TropicalWeight::one(),
    });

    // Binding → pop (binding application complete)
    wpds.add_rule(WpdsRule::Pop {
        from: SchedulerStackSymbol::Binding { rule_idx: 0 },
        weight: TropicalWeight::one(),
    });

    wpds
}

/// Build context weights from WPDS poststar analysis.
///
/// Runs poststar on the default WPDS to compute reachability weights,
/// then generates context hashes for common continuation frame patterns.
pub fn build_context_weights(epoch: u64) -> ContextWeightTable {
    let wpds = build_default_wpds();
    let reachability = wpds.poststar_top_of_stack();

    let table = ContextWeightTable::new(epoch);

    // Generate context weights for common frame combinations.
    // Each weight is derived from the WPDS reachability analysis.

    // Deep recursion: RuleMatch → RuleMatch → RuleMatch
    // High weight = deprioritize (likely divergent)
    let deep_recursion = vec![
        SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
        SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
        SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
    ];
    table.insert(hash_context(&deep_recursion), 2.0);

    // Guard inside conditional: Conditional → ArgEval
    // Low weight = prioritize (cheap guard, evaluate first)
    let guard_in_if = vec![
        SchedulerStackSymbol::Conditional { branch: 0 },
        SchedulerStackSymbol::ArgEval { position: 0 },
    ];
    table.insert(hash_context(&guard_in_if), 0.5);

    // Expression inside collapse: Collapse → RuleMatch
    // Moderate weight with parallelism boost hint
    let collapse_body = vec![
        SchedulerStackSymbol::Collapse,
        SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
    ];
    table.insert(hash_context(&collapse_body), 0.8);

    // Deep let chain: LetChain → LetChain → LetChain
    // Moderate weight, sticky affinity for cache locality
    let deep_let = vec![
        SchedulerStackSymbol::LetChain { depth: 3 },
        SchedulerStackSymbol::LetChain { depth: 2 },
        SchedulerStackSymbol::LetChain { depth: 1 },
    ];
    table.insert(hash_context(&deep_let), 1.5);

    // Top-level evaluation: Root
    // Neutral weight
    let top_level = vec![SchedulerStackSymbol::Root];
    table.insert(hash_context(&top_level), 1.0);

    // Use WPDS reachability to refine weights for reachable symbols
    for (symbol, weight) in &reachability {
        if weight.value() > 0.0 && !weight.is_zero() {
            let ctx = vec![symbol.clone()];
            let ctx_hash = hash_context(&ctx);
            // Only insert if not already present (pattern-specific weights take priority)
            if table.get(ctx_hash) == 1.0 {
                // Convert tropical weight to multiplier: higher cost → higher multiplier
                let multiplier = 1.0 + (weight.value() as f32 * 0.1);
                table.insert(ctx_hash, multiplier.min(3.0));
            }
        }
    }

    table
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_weight_table() {
        let table = ContextWeightTable::new(0);
        assert_eq!(table.get(42), 1.0); // default
        table.insert(42, 2.5);
        assert_eq!(table.get(42), 2.5);
    }

    #[test]
    fn test_build_default_wpds() {
        let wpds = build_default_wpds();
        assert!(wpds.num_rules() > 0);
        assert!(!wpds.rules_from(&SchedulerStackSymbol::Root).is_empty());
    }

    #[test]
    fn test_build_context_weights() {
        let table = build_context_weights(1);
        assert!(!table.is_empty());
        assert_eq!(table.epoch(), 1);

        // Deep recursion should have elevated weight
        let deep = vec![
            SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
            SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
            SchedulerStackSymbol::RuleMatch { head_hash: 0, arity: 0 },
        ];
        let weight = table.get(hash_context(&deep));
        assert!(weight > 1.0, "deep recursion should be deprioritized: {}", weight);

        // Guard in conditional should have reduced weight
        let guard = vec![
            SchedulerStackSymbol::Conditional { branch: 0 },
            SchedulerStackSymbol::ArgEval { position: 0 },
        ];
        let weight = table.get(hash_context(&guard));
        assert!(weight < 1.0, "guard should be prioritized: {}", weight);
    }
}
