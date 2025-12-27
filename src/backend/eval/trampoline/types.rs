//! Trampoline Types for Iterative Evaluation
//!
//! These types enable iterative evaluation using an explicit work stack instead
//! of recursive function calls. This prevents stack overflow for large expressions.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::grounded::GroundedState;
use crate::backend::models::{Bindings, EvalResult, MettaValue};

// Access CartesianProductIter via parent module re-export
use super::super::CartesianProductIter;

/// Maximum evaluation depth to prevent stack overflow
/// This limits how deep the evaluation can recurse through nested expressions
/// Set to 1000 to allow legitimate deep nesting while still catching runaway recursion
pub const MAX_EVAL_DEPTH: usize = 1000;

/// Work item representing pending evaluation work
#[derive(Debug)]
pub enum WorkItem {
    /// Evaluate a value and send result to continuation
    Eval {
        value: MettaValue,
        env: Environment,
        depth: usize,
        cont_id: usize,
        /// If true, this is a tail call - don't increment depth
        /// Tail calls include: rule RHS, if branches, let* final body, match templates
        is_tail_call: bool,
    },
    /// Resume a continuation with a result
    Resume { cont_id: usize, result: EvalResult },
}

/// Continuation representing what to do with an evaluation result
#[derive(Debug)]
pub enum Continuation {
    /// Final result - return from eval()
    Done,
    /// Collecting S-expression sub-results before processing
    CollectSExpr {
        /// Items still to evaluate (VecDeque for O(1) pop_front)
        remaining: VecDeque<MettaValue>,
        /// Results collected so far: (results_vec, env)
        collected: Vec<EvalResult>,
        /// Original environment for the S-expression
        original_env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after processing
        parent_cont: usize,
    },
    /// Processing rule match results
    ProcessRuleMatches {
        /// Remaining (rhs, bindings) pairs to evaluate (VecDeque for O(1) pop_front)
        /// RHS is Arc-wrapped for O(1) cloning
        remaining_matches: VecDeque<(Arc<MettaValue>, Bindings)>,
        /// Results accumulated so far
        results: Vec<MettaValue>,
        /// Environment
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing TCO grounded operation (e.g., +, -, and, or)
    /// This continuation tracks state across multiple argument evaluations
    ProcessGroundedOp {
        /// State of the grounded operation (tracks which args have been evaluated)
        state: GroundedState,
        /// Environment for evaluating arguments
        env: Environment,
        /// Parent continuation to resume after operation completes
        parent_cont: usize,
        /// Evaluation depth
        depth: usize,
    },
    /// Processing lazy Cartesian product combinations one at a time
    /// This continuation enables memory-efficient nondeterministic evaluation
    ProcessCombinations {
        /// Iterator over remaining combinations (lazy evaluation)
        combinations: CartesianProductIter,
        /// Results accumulated so far from processing combinations
        results: Vec<MettaValue>,
        /// Pending rule matches for the current combination (VecDeque for O(1) pop_front)
        /// RHS is Arc-wrapped for O(1) cloning
        pending_rule_matches: VecDeque<(Arc<MettaValue>, Bindings)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after all combinations processed
        parent_cont: usize,
    },
    /// Processing let binding - tracks state across value and body evaluations
    /// This enables let body evaluation to participate in the trampoline (TCO)
    ProcessLet {
        /// Value results to process (None if awaiting value evaluation)
        pending_values: Option<VecDeque<MettaValue>>,
        /// Pattern to match against values
        pattern: MettaValue,
        /// Body template to instantiate with bindings
        body: MettaValue,
        /// Collected body evaluation results
        results: Vec<MettaValue>,
        /// Environment for body evaluation
        env: Environment,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
        /// Parent continuation to resume after all values processed
        parent_cont: usize,
    },
}
