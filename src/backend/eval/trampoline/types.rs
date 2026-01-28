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
    /// Collecting grounded arg evaluation results.
    /// This enables grounded arg evaluation to use the trampoline instead of
    /// nested recursive calls, preventing stack overflow.
    CollectGroundedArg {
        /// Original S-expression items
        items: Vec<MettaValue>,
        /// All indices needing evaluation (indices into items)
        grounded_indices: Vec<usize>,
        /// Current position in grounded_indices being evaluated
        current_idx: usize,
        /// Evaluated results so far (corresponds to grounded_indices[0..current_idx])
        evaluated_results: Vec<MettaValue>,
        /// Environment
        env: Environment,
        /// Depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing map-atom iteration - collects results from element evaluations.
    /// This continuation enables map-atom to use the trampoline, preventing
    /// stack overflow for nested map operations (e.g., map inside map).
    ProcessMapAtom {
        /// Remaining elements to process (VecDeque for O(1) pop_front)
        remaining_elements: VecDeque<MettaValue>,
        /// Variable name for substitution
        var_name: String,
        /// Template to evaluate for each element
        template: MettaValue,
        /// Results collected so far
        collected_results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after all elements processed
        parent_cont: usize,
    },
    /// Processing filter-atom iteration - keeps elements that satisfy predicate.
    /// This continuation enables filter-atom to use the trampoline, preventing
    /// stack overflow for nested filter operations.
    ProcessFilterAtom {
        /// Current element being tested (Some if awaiting predicate result)
        current_element: Option<MettaValue>,
        /// Remaining elements to process (VecDeque for O(1) pop_front)
        remaining_elements: VecDeque<MettaValue>,
        /// Variable name for substitution
        var_name: String,
        /// Predicate to evaluate for each element
        predicate: MettaValue,
        /// Elements that passed the filter so far
        filtered_results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after all elements processed
        parent_cont: usize,
    },
    /// Processing foldl-atom iteration - reduces list to single value.
    /// This continuation enables foldl-atom to use the trampoline, preventing
    /// stack overflow for nested fold operations.
    ProcessFoldlAtom {
        /// Remaining elements to process (VecDeque for O(1) pop_front)
        remaining_elements: VecDeque<MettaValue>,
        /// Accumulator variable name
        acc_var_name: String,
        /// Item variable name
        item_var_name: String,
        /// Operation template to evaluate for each element
        operation: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after all elements processed
        parent_cont: usize,
    },
    /// Processing if condition - awaits condition result to select branch.
    /// This continuation enables if condition evaluation to use the trampoline,
    /// preventing stack overflow for deeply nested conditions.
    ProcessIfCondition {
        /// Then branch (evaluated if condition is truthy)
        then_branch: MettaValue,
        /// Else branch (evaluated if condition is falsy)
        else_branch: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after branch evaluation
        parent_cont: usize,
    },
    /// Processing case atom - awaits atom evaluation to match against cases.
    /// This continuation enables case atom evaluation to use the trampoline,
    /// preventing stack overflow for deeply nested atom expressions.
    ProcessCaseAtom {
        /// Cases to match against (pattern-template pairs)
        cases: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation to resume after case evaluation
        parent_cont: usize,
    },
    /// Processing (eval expr) - awaits argument evaluation, then evaluates result.
    ProcessEvalEval {
        /// Environment for second evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing (return value) - awaits value evaluation, then wraps in return.
    ProcessReturn {
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing (chain expr $var body) - awaits expr evaluation.
    /// Then evaluates body for each result with pattern bindings.
    ProcessChainExpr {
        /// Variable pattern to bind each result
        var: MettaValue,
        /// Body template to instantiate and evaluate
        body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing chain body evaluations - awaits body evaluation for current result.
    /// Accumulates results and processes remaining values.
    ProcessChainBody {
        /// Remaining values to process
        remaining_values: VecDeque<MettaValue>,
        /// Variable pattern for bindings
        var: MettaValue,
        /// Body template
        body: MettaValue,
        /// Accumulated results so far
        results: Vec<MettaValue>,
        /// Environment (updated after each body evaluation)
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing (function expr) - awaits expr evaluation.
    /// Loops until encountering a return value.
    ProcessFunction {
        /// Iteration count
        iteration_count: usize,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing (is-error expr) - awaits expression evaluation to check for error.
    ProcessIsError {
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing (catch expr default) - awaits expression evaluation.
    /// If all results are errors, evaluates default.
    ProcessCatch {
        /// Default expression to evaluate if expr is error
        default: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing conjunction goals - awaits current goal evaluation.
    /// Accumulates results and continues to next goal.
    ProcessConjunction {
        /// Remaining goals to evaluate (VecDeque for O(1) pop_front)
        remaining_goals: VecDeque<MettaValue>,
        /// Results accumulated so far (from previous goals)
        accumulated_results: Vec<MettaValue>,
        /// Environment for evaluation (updated after each goal)
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing unify pattern1 evaluation - awaits pattern1 results.
    /// Then computes matches and queues body evaluations.
    ProcessUnifyPattern1 {
        /// Second pattern (used for non-space unification)
        pattern2: MettaValue,
        /// Success body template
        success_body: MettaValue,
        /// Failure body template
        failure_body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing unify pattern2 evaluation (for non-space case).
    /// Receives pattern2 results, performs unification, evaluates bodies.
    ProcessUnifyPattern2 {
        /// Evaluated pattern1 value being unified
        val1: MettaValue,
        /// Remaining pattern1 results to process
        remaining_pattern1_results: VecDeque<MettaValue>,
        /// Success body template
        success_body: MettaValue,
        /// Failure body template
        failure_body: MettaValue,
        /// Accumulated results so far
        all_results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing unify body evaluations - iterates through bodies.
    /// Accumulates results from success/failure body evaluations.
    ProcessUnifyBodies {
        /// Remaining bodies to evaluate (VecDeque for O(1) pop_front)
        /// Arc-wrapped for O(1) cloning during multiplicity expansion
        remaining_bodies: VecDeque<Arc<MettaValue>>,
        /// Remaining pattern1 results to process after bodies done
        remaining_pattern1_results: VecDeque<MettaValue>,
        /// Pattern2 for non-space unification
        pattern2: MettaValue,
        /// Success body template
        success_body: MettaValue,
        /// Failure body template
        failure_body: MettaValue,
        /// Accumulated results so far
        all_results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing collapse expression result - collects into list.
    ProcessCollapse {
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing collapse-bind expression result - collects ALL into list.
    ProcessCollapseBind {
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing amb alternatives - evaluates each and collects results.
    ProcessAmb {
        /// Remaining alternatives to evaluate
        remaining_alts: VecDeque<MettaValue>,
        /// Accumulated results so far
        results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing guard condition result.
    ProcessGuard {
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing get-atoms space reference result.
    ProcessGetAtoms {
        /// Original space reference for error messages
        space_ref: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing memo table reference result - phase 1.
    ProcessMemoTable {
        /// Original memo reference for error messages
        memo_ref: MettaValue,
        /// Expression to evaluate and cache
        expr: MettaValue,
        /// Whether to cache only first result
        first_only: bool,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing memo expression result - phase 2 (after cache miss).
    ProcessMemoExpr {
        /// Memo handle to store result
        memo_handle: crate::backend::models::MemoHandle,
        /// Original expression for cache key
        expr: MettaValue,
        /// Whether to cache only first result
        first_only: bool,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing new-memo name result - phase 1.
    ProcessNewMemoName {
        /// Original name arg for error messages
        name_arg: MettaValue,
        /// Optional size argument to evaluate
        size_arg: Option<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing new-memo size result - phase 2.
    ProcessNewMemoSize {
        /// Evaluated name
        name: String,
        /// Original size arg for error messages
        size_arg: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing clear-memo! or memo-stats result.
    ProcessMemoOp {
        /// Original memo reference for error messages
        memo_ref: MettaValue,
        /// Operation type: "clear" or "stats"
        is_clear: bool,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing match space evaluation result - phase 1.
    /// Handles space result, does pattern matching, queues template evaluations.
    ProcessMatchSpace {
        /// Original space argument for error messages
        space_arg: MettaValue,
        /// Pattern to match against atoms
        pattern: MettaValue,
        /// Template to instantiate with bindings
        template: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing match template evaluations - phase 2.
    /// Collects results from evaluating instantiated templates.
    ProcessMatchTemplates {
        /// Remaining instantiated templates to evaluate
        remaining_templates: VecDeque<MettaValue>,
        /// Accumulated results so far
        results: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing add-atom space evaluation - phase 1.
    ProcessAddAtomSpace {
        /// Original space reference for error messages
        space_ref: MettaValue,
        /// Atom to add (will be evaluated next)
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing add-atom atom evaluation - phase 2.
    ProcessAddAtomAtom {
        /// Evaluated space handle
        space_handle: crate::backend::models::SpaceHandle,
        /// Original atom for error messages
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing remove-atom space evaluation - phase 1.
    ProcessRemoveAtomSpace {
        /// Original space reference for error messages
        space_ref: MettaValue,
        /// Atom to remove (will be evaluated next)
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing remove-atom atom evaluation - phase 2.
    ProcessRemoveAtomAtom {
        /// Evaluated space handle
        space_handle: crate::backend::models::SpaceHandle,
        /// Original atom for error messages
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing new-state initial value result.
    ProcessNewState {
        /// Original initial value for error messages
        initial_value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing get-state state reference result.
    ProcessGetState {
        /// Original state reference for error messages
        state_ref: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing change-state! state reference result (phase 1).
    ProcessChangeStateRef {
        /// Original state reference for error messages
        state_ref: MettaValue,
        /// New value to set (will be evaluated next)
        new_value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing change-state! new value result (phase 2).
    ProcessChangeStateValue {
        /// Evaluated state value
        state_value: MettaValue,
        /// Original new value for error messages
        new_value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing repr atom evaluation result.
    ProcessRepr {
        /// Original atom for error messages
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing format-args format string result (phase 1).
    ProcessFormatArgsString {
        /// Original format arg for error messages
        format_arg: MettaValue,
        /// Args expression to evaluate next
        args_arg: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing format-args args result (phase 2).
    ProcessFormatArgsArgs {
        /// Evaluated format string
        format_str: String,
        /// Original args arg for error messages
        args_arg: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing println! atom evaluation result.
    ProcessPrintln {
        /// Original atom for error messages
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing trace! message result (phase 1).
    ProcessTraceMessage {
        /// Original message for error messages
        message: MettaValue,
        /// Value expression to evaluate next
        value_expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing trace! value result (phase 2).
    ProcessTraceValue {
        /// Evaluated message string for printing
        message_str: String,
        /// Original value expr for error messages
        value_expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing get-metatype atom evaluation result.
    ProcessGetMetatype {
        /// Original atom for error messages
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
    /// Processing bind! atom expression result.
    ProcessBind {
        /// Token name to bind (e.g., "&kb")
        token: String,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
        /// Parent continuation
        parent_cont: usize,
    },
}
