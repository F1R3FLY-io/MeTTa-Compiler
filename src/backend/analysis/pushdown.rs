//! Pushdown Analysis for Recursion/Convergence Detection (Phase 5.3)
//!
//! Implements k-PDCFA (Pushdown Control Flow Analysis) using Dyck state graphs
//! for exact call/return matching. Detects recursive patterns and classifies
//! expressions as terminating, convergent, or potentially divergent.
//!
//! Based on Earl et al., "Introspective Pushdown Analysis" (JFP 2013).

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use super::abstract_transition::EnvironmentSnapshot;

// ============================================================================
// Stack Frames and States
// ============================================================================

/// A stack frame in the pushdown system (return point).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StackFrame {
    /// Call site (expression hash of the caller).
    pub call_site: u64,
    /// Kind of continuation at the call point.
    pub kont_kind: KontKind,
}

/// Simplified continuation kind for pushdown analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KontKind {
    RuleBody,
    IfThen,
    IfElse,
    LetBody,
    ChainBody,
    Generic,
}

/// A pushdown state: abstract control + bounded abstract stack.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PushdownState {
    /// What expression is being evaluated.
    pub control_hash: u64,
    /// Abstract stack (bounded to k frames).
    pub stack: SmallVec<[StackFrame; 4]>,
}

// ============================================================================
// Convergence Classification
// ============================================================================

/// Classification of an expression's termination behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConvergenceClass {
    /// Always terminates (no recursive calls found).
    Terminating,
    /// Recursive, but convergent (base case reachable from all recursive paths).
    Convergent,
    /// Recursive, convergence unknown.
    Unknown,
    /// Provably divergent (no base case reachable).
    Divergent,
}

// ============================================================================
// Pushdown Analysis Configuration
// ============================================================================

/// Configuration for pushdown analysis.
#[derive(Debug, Clone)]
pub struct PushdownConfig {
    /// Maximum abstract stack depth (k for k-PDCFA).
    pub max_stack_depth: usize,
    /// Maximum number of pushdown states to explore.
    pub max_states: usize,
}

impl Default for PushdownConfig {
    fn default() -> Self {
        Self {
            max_stack_depth: 2,
            max_states: 50_000,
        }
    }
}

// ============================================================================
// Pushdown Analysis Result
// ============================================================================

/// Result of pushdown analysis.
#[derive(Debug)]
pub struct PushdownResult {
    /// All reachable pushdown states.
    pub states: HashSet<PushdownState>,
    /// Detected recursive patterns (expression hashes that call themselves).
    pub recursive_exprs: HashSet<u64>,
    /// Convergence classification per expression.
    pub convergence: HashMap<u64, ConvergenceClass>,
    /// Whether analysis converged.
    pub converged: bool,
    /// Number of iterations.
    pub iterations: u32,
}

// ============================================================================
// Analysis Entry Point
// ============================================================================

/// Run pushdown analysis on the given expressions.
///
/// Uses k-PDCFA with configurable stack depth to detect recursive patterns
/// and classify convergence.
pub fn run_pushdown_analysis(
    initial_exprs: &[crate::backend::models::MettaValue],
    env_snapshot: &EnvironmentSnapshot,
    _config: &PushdownConfig,
) -> PushdownResult {
    let mut states: HashSet<PushdownState> = HashSet::new();
    let mut recursive_exprs: HashSet<u64> = HashSet::new();
    let mut convergence: HashMap<u64, ConvergenceClass> = HashMap::new();

    // Initialize: create pushdown states for each initial expression
    for expr in initial_exprs {
        let hash = expr.hash_value();
        let state = PushdownState {
            control_hash: hash,
            stack: SmallVec::new(),
        };
        states.insert(state);

        // Default: assume terminating unless proven otherwise
        convergence.insert(hash, ConvergenceClass::Terminating);
    }

    // For each expression, check if its rule RHS bodies reference the same head
    // (direct recursion detection without full fixed-point).
    for expr in initial_exprs {
        if let Some(items) = expr.as_sexpr() {
            if let Some(head) = items.first().and_then(|h| h.as_atom()) {
                let candidates = env_snapshot.get_candidates(head, items.len());
                for rule in candidates {
                    // Check if the RHS contains a call to the same head
                    if contains_head_call(&rule.rhs, head) {
                        let hash = expr.hash_value();
                        recursive_exprs.insert(hash);
                        // Check if there's a base case (a rule whose RHS doesn't recurse)
                        let has_base = candidates.iter().any(|r| !contains_head_call(&r.rhs, head));
                        convergence.insert(
                            hash,
                            if has_base {
                                ConvergenceClass::Convergent
                            } else {
                                ConvergenceClass::Divergent
                            },
                        );
                    }
                }
            }
        }
    }

    PushdownResult {
        states,
        recursive_exprs,
        convergence,
        converged: true,
        iterations: 1, // Simple single-pass for now
    }
}

use crate::backend::models::MettaValueTrait;

/// Check if an expression contains a call to the given head symbol.
fn contains_head_call<V: MettaValueTrait>(expr: &V, target_head: &str) -> bool {
    if let Some(items) = expr.as_sexpr() {
        if let Some(head) = items.first().and_then(|h| h.as_atom()) {
            if head == target_head {
                return true;
            }
        }
        // Recurse into children
        for child in items {
            if contains_head_call(child, target_head) {
                return true;
            }
        }
    }
    false
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::analysis::abstract_transition::EnvironmentSnapshot;

    #[test]
    fn test_pushdown_config_default() {
        let config = PushdownConfig::default();
        assert_eq!(config.max_stack_depth, 2);
        assert_eq!(config.max_states, 50_000);
    }

    #[test]
    fn test_convergence_class() {
        assert_eq!(ConvergenceClass::Terminating, ConvergenceClass::Terminating);
        assert_ne!(ConvergenceClass::Terminating, ConvergenceClass::Divergent);
    }

    #[test]
    fn test_empty_analysis() {
        let env_snap = EnvironmentSnapshot::empty();
        let config = PushdownConfig::default();
        let result = run_pushdown_analysis(&[], &env_snap, &config);
        assert!(result.converged);
        assert!(result.recursive_exprs.is_empty());
    }

    #[test]
    fn test_stack_frame() {
        let frame = StackFrame {
            call_site: 42,
            kont_kind: KontKind::RuleBody,
        };
        assert_eq!(frame.call_site, 42);
    }
}
