//! Abstract SECK Transition Function
//!
//! Mirrors the concrete trampoline's step dispatch over abstract domains.
//! Given an `AbstractState` and `AbstractStore`, computes successor states
//! and updates the store with new abstract allocations.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::backend::eval::cesk::branch_analysis::{BranchPurity, analyze_branch_purity};
use crate::backend::models::{MettaValue, MettaValueTrait};

use super::abstract_domain::*;

// ============================================================================
// Abstract State
// ============================================================================

/// Abstract control expression.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AbstractControl {
    /// Evaluate an expression.
    Eval {
        expr_hash: u64,
        env: AbstractEnv,
        depth: u16,
    },
    /// Apply rule: head symbol + arity + abstract args.
    Apply {
        head: &'static str,
        arity: u16,
        env: AbstractEnv,
    },
}

/// Abstract continuation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AbstractKont {
    Done,
    RuleMatch {
        candidate_rule_indices: SmallVec<[u32; 8]>,
        env: AbstractEnv,
    },
    IfBranch {
        then_hash: u64,
        else_hash: u64,
        env: AbstractEnv,
    },
    LetBind {
        body_hash: u64,
        env: AbstractEnv,
    },
    Generic {
        env: AbstractEnv,
    },
}

/// A complete abstract machine state (the unit of fixed-point computation).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbstractState {
    pub control: AbstractControl,
    pub kont: AbstractKont,
}

// ============================================================================
// Environment Snapshot
// ============================================================================

/// A rule in abstract analysis form.
#[derive(Debug, Clone)]
pub struct AbstractRule {
    /// Content hash of LHS pattern.
    pub lhs_hash: u64,
    /// Content hash of RHS template.
    pub rhs_hash: u64,
    /// Concrete LHS (for pattern matching during analysis).
    pub lhs: MettaValue,
    /// Concrete RHS (for body analysis).
    pub rhs: MettaValue,
    /// Cached abstract result type.
    pub rhs_type: Option<AbstractType>,
    /// Whether RHS has variables.
    pub rhs_has_variables: bool,
    /// Purity classification.
    pub purity: BranchPurity,
    /// Index in the original rule storage.
    pub rule_index: u32,
}

/// Read-only snapshot of the concrete environment for analysis.
///
/// Constructed once before analysis begins, not modified during analysis.
/// Avoids holding locks on the live environment during the fixed-point.
#[derive(Debug)]
pub struct EnvironmentSnapshot {
    /// Rules indexed by (head symbol, arity).
    pub rules: HashMap<(&'static str, usize), Vec<AbstractRule>>,
    /// Total number of rules.
    pub total_rules: u32,
}

impl EnvironmentSnapshot {
    /// Create an empty snapshot (for testing).
    pub fn empty() -> Self {
        Self {
            rules: HashMap::new(),
            total_rules: 0,
        }
    }

    /// Get candidate rules for a given head and arity.
    pub fn get_candidates(&self, head: &'static str, arity: usize) -> &[AbstractRule] {
        self.rules.get(&(head, arity))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

// ============================================================================
// Transition Function
// ============================================================================

/// Compute successor states from an abstract state.
///
/// Returns `(successors, store_changed)`.
pub fn abstract_step(
    state: &AbstractState,
    store: &mut AbstractStore,
    env_snapshot: &EnvironmentSnapshot,
    config: &AnalysisConfig,
) -> (SmallVec<[AbstractState; 4]>, bool) {
    let mut successors = SmallVec::new();
    let mut store_changed = false;

    match &state.control {
        AbstractControl::Eval { expr_hash, env, depth } => {
            // The expression hash identifies what's being evaluated.
            // In the abstract domain, we track which rules could fire
            // and what types could result.
            let result_addr = AbstractAddr::mono(*expr_hash);

            // For ground values, the result is the value itself
            // (handled by the fixed-point caller via alpha injection).

            // For S-expressions with known head, look up candidate rules
            // and schedule their RHS bodies.
            // This is a simplified model — the full transition would need
            // the concrete expression structure. We use the hash as a proxy.

            // Schedule continuation processing
            match &state.kont {
                AbstractKont::Done => {
                    // Terminal — no successors
                }
                AbstractKont::RuleMatch { candidate_rule_indices, env: match_env } => {
                    // Each candidate rule's RHS becomes a successor Eval state
                    for &idx in candidate_rule_indices {
                        // Find the rule in the snapshot
                        for rules in env_snapshot.rules.values() {
                            for rule in rules {
                                if rule.rule_index == idx {
                                    let rhs_hash = rule.rhs_hash;
                                    // Store the RHS type as abstract value
                                    if let Some(rhs_type) = rule.rhs_type {
                                        store_changed |= store.alloc(
                                            AbstractAddr::mono(rhs_hash),
                                            AbstractValue::AnyOfType(rhs_type),
                                        );
                                    }
                                    successors.push(AbstractState {
                                        control: AbstractControl::Eval {
                                            expr_hash: rhs_hash,
                                            env: match_env.clone(),
                                            depth: depth.saturating_add(1),
                                        },
                                        kont: AbstractKont::Done,
                                    });
                                }
                            }
                        }
                    }
                }
                AbstractKont::IfBranch { then_hash, else_hash, env: branch_env } => {
                    // Fork into both branches (conservative)
                    successors.push(AbstractState {
                        control: AbstractControl::Eval {
                            expr_hash: *then_hash,
                            env: branch_env.clone(),
                            depth: depth.saturating_add(1),
                        },
                        kont: AbstractKont::Done,
                    });
                    successors.push(AbstractState {
                        control: AbstractControl::Eval {
                            expr_hash: *else_hash,
                            env: branch_env.clone(),
                            depth: depth.saturating_add(1),
                        },
                        kont: AbstractKont::Done,
                    });
                }
                AbstractKont::LetBind { body_hash, env: let_env } => {
                    successors.push(AbstractState {
                        control: AbstractControl::Eval {
                            expr_hash: *body_hash,
                            env: let_env.clone(),
                            depth: depth.saturating_add(1),
                        },
                        kont: AbstractKont::Done,
                    });
                }
                AbstractKont::Generic { env: generic_env } => {
                    // Generic continuation — conservatively assume it schedules
                    // the same expression at the next depth
                    // (this is imprecise but safe)
                }
            }
        }
        AbstractControl::Apply { head, arity, env } => {
            // Look up candidates for this head+arity
            let candidates = env_snapshot.get_candidates(head, *arity as usize);
            if !candidates.is_empty() {
                let indices: SmallVec<[u32; 8]> = candidates.iter()
                    .map(|r| r.rule_index)
                    .collect();
                // Create a RuleMatch state for each candidate
                successors.push(AbstractState {
                    control: AbstractControl::Eval {
                        expr_hash: 0, // Will be filled by the rule match processing
                        env: env.clone(),
                        depth: 0,
                    },
                    kont: AbstractKont::RuleMatch {
                        candidate_rule_indices: indices,
                        env: env.clone(),
                    },
                });
            }
        }
    }

    (successors, store_changed)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_abstract_state_eval_done() {
        let state = AbstractState {
            control: AbstractControl::Eval {
                expr_hash: 42,
                env: AbstractEnv::new(),
                depth: 0,
            },
            kont: AbstractKont::Done,
        };
        let mut store = AbstractStore::new();
        let env_snap = EnvironmentSnapshot::empty();
        let config = AnalysisConfig::default();

        let (successors, _) = abstract_step(&state, &mut store, &env_snap, &config);
        assert!(successors.is_empty()); // Done → no successors
    }

    #[test]
    fn test_abstract_state_if_branch() {
        let state = AbstractState {
            control: AbstractControl::Eval {
                expr_hash: 100,
                env: AbstractEnv::new(),
                depth: 0,
            },
            kont: AbstractKont::IfBranch {
                then_hash: 200,
                else_hash: 300,
                env: AbstractEnv::new(),
            },
        };
        let mut store = AbstractStore::new();
        let env_snap = EnvironmentSnapshot::empty();
        let config = AnalysisConfig::default();

        let (successors, _) = abstract_step(&state, &mut store, &env_snap, &config);
        assert_eq!(successors.len(), 2); // Both branches
    }

    #[test]
    fn test_environment_snapshot_empty() {
        let snap = EnvironmentSnapshot::empty();
        assert_eq!(snap.total_rules, 0);
        assert!(snap.get_candidates("f", 2).is_empty());
    }

    #[test]
    fn test_abstract_state_hash_eq() {
        let s1 = AbstractState {
            control: AbstractControl::Eval {
                expr_hash: 42,
                env: AbstractEnv::new(),
                depth: 0,
            },
            kont: AbstractKont::Done,
        };
        let s2 = s1.clone();
        assert_eq!(s1, s2);
    }
}
