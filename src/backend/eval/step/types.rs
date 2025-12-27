//! Step Types for Evaluation
//!
//! These types represent the results of a single evaluation step in the
//! trampoline-based evaluator.

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::grounded::GroundedState;
use crate::backend::models::{Bindings, EvalResult, MettaValue};

// Access CartesianProductIter via parent module re-export
use super::super::CartesianProductIter;

/// Result of a single evaluation step
#[derive(Debug)]
pub enum EvalStep {
    /// Evaluation complete, return this result
    Done(EvalResult),
    /// Need to evaluate S-expression items (iteratively)
    EvalSExpr {
        items: Vec<MettaValue>,
        env: Environment,
        depth: usize,
    },
    /// Start TCO grounded operation (e.g., +, -, and, or)
    /// This defers evaluation to the trampoline for proper tail call handling
    StartGroundedOp {
        state: GroundedState,
        env: Environment,
        depth: usize,
    },
    /// Start let binding - first evaluates value expression, then pattern matches
    /// and evaluates body. This enables let body to participate in trampoline (TCO).
    StartLetBinding {
        /// Pattern to match against evaluated value
        pattern: MettaValue,
        /// Value expression to evaluate first
        value_expr: MettaValue,
        /// Body template to instantiate with bindings
        body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },
    /// Evaluate if branch - condition has been evaluated, now evaluate selected branch.
    /// This enables if branches to participate in trampoline (TCO).
    EvalIfBranch {
        /// Branch expression to evaluate (then or else)
        branch: MettaValue,
        /// Environment after condition evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },
    /// Evaluate rule matches with UNEVALUATED arguments (lazy evaluation semantics).
    /// This is used when user-defined rules match before argument evaluation.
    /// MeTTa HE uses normal-order (lazy) evaluation for rule arguments.
    EvalRuleMatchesLazy {
        /// Matched rules: (RHS expression, bindings from pattern match)
        /// RHS is Arc-wrapped for O(1) cloning
        matches: Vec<(Arc<MettaValue>, Bindings)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
}

/// Result of processing collected S-expression results
#[derive(Debug)]
pub enum ProcessedSExpr {
    /// Processing complete, return this result
    Done(EvalResult),
    /// Need to evaluate rule matches
    /// RHS is Arc-wrapped for O(1) cloning
    EvalRuleMatches {
        matches: Vec<(Arc<MettaValue>, Bindings)>,
        env: Environment,
        depth: usize,
        base_results: Vec<MettaValue>,
    },
    /// Need to lazily process Cartesian product combinations
    EvalCombinations {
        combinations: CartesianProductIter,
        env: Environment,
        depth: usize,
    },
}
