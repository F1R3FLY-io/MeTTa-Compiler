//! Trampoline Types for Iterative Evaluation
//!
//! Monomorphized types for the trampoline-based evaluator. These types use
//! concrete `MettaValue` and `MettaEnvironment` types directly, eliminating
//! generic type parameter overhead and enabling Copy semantics for MettaValue
//! (8-byte tagged pointer).
//!
//! ## Design Notes
//!
//! - `WorkItem`, `Continuation`, `EvalResult` are concrete
//! - MettaValue is Copy — all `.clone()` calls are zero-cost 8-byte memcpy
//! - Bindings use `GenericBindings<MettaValue>` (heap-allocated binding map)
//! - Names retain the `Generic` prefix for now; renaming is a separate step

use smallvec::SmallVec;

use crate::backend::environment::MettaEnvironment;
use crate::backend::grounded::GroundedState;
use crate::backend::models::{GenericBindings, MemoHandle, MettaValue};
// SpaceHandle was previously used by ProcessAddAtomAtom and ProcessRemoveAtomAtom,
// which are now disabled (see comments on those variants below).

// Import Cartesian product iterator
use super::super::processing::GenericCartesianProductIter;

use super::context::SharedEnv;

/// Evaluation result: (results, environment)
///
/// Uses SmallVec<[MettaValue; 2]> to inline up to 2 elements, avoiding heap allocation
/// for the common single-result case (93%+ of evaluations produce 1 result).
/// The environment is Arc-wrapped for O(1) sharing across continuations and work items,
/// eliminating the 8.7% CPU overhead from per-step clone/drop of MettaEnvironment.
pub type EvalResult = (SmallVec<[MettaValue; 2]>, SharedEnv);

/// Work item representing pending evaluation work.
///
/// Each variant represents a different kind of evaluation task that the
/// trampoline loop can process. MettaValue is Copy (8-byte tagged pointer),
/// so all value passing is zero-cost.
#[derive(Debug)]
pub enum WorkItem {
    /// Evaluate a value and send result to continuation at stack top
    Eval {
        value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// If true, this is a tail call - don't increment depth
        is_tail_call: bool,
        /// Phase 8.7: Expected return type for branch pruning.
        /// When set, rules whose `rhs_type` is incompatible with this type
        /// are pruned from the match set before evaluation.
        expected_type: Option<MettaValue>,
        /// Demand from consumer context for branch pruning.
        /// When `Some(Demand::AtLeast(1))`, nondeterministic dispatch uses lazy
        /// BranchCoroutine evaluation — stops after first result, eliminating
        /// 97% of wasted branch exploration in patterns like `match-atom`.
        demand: Option<crate::backend::eval::cesk::coroutine::Demand>,
    },
    /// Evaluate a template with deferred bindings (lazy binding).
    ///
    /// Instead of calling `apply_bindings` upfront to materialize
    /// a fully-substituted expression tree, this carries `(template, bindings)`
    /// and resolves variables lazily:
    /// - Variables: look up in bindings, push result
    /// - Ground (no variables): push as Eval directly
    /// - Special forms (let, if, chain): resolve only immediate args, forward
    ///   remaining bindings to child evaluations via binding composition
    /// - Other S-expressions: fall back to `apply_bindings` + Eval
    ///
    /// This avoids O(tree_depth) recursive allocation for nested `let*` chains,
    /// where each level would otherwise materialize the entire remaining body.
    EvalWithBindings {
        template: MettaValue,
        bindings: Box<GenericBindings<MettaValue>>,
        env: SharedEnv,
        depth: usize,
        is_tail_call: bool,
        expected_type: Option<MettaValue>,
    },
    /// Resume the continuation at stack top with a result
    Resume {
        result: EvalResult,
    },
}

/// Continuation representing what to do with an evaluation result.
///
/// Each variant captures the state needed to continue processing after
/// a sub-evaluation completes. MettaValue is Copy (8-byte tagged pointer),
/// so all value fields are zero-cost to store and retrieve.
///
/// # Note on env/depth fields
///
/// Some variants store `env` and `depth` fields that are not read directly.
/// These fields are preserved for context but the actual environment from the
/// evaluation result is used instead. This is intentional - continuations track
/// the original environment for debugging/reference.
#[derive(Debug)]
pub enum Continuation {
    /// Final result - return from eval()
    Done,

    /// Collecting S-expression sub-results before processing
    CollectSExpr {
        remaining: std::vec::IntoIter<MettaValue>,
        collected: Vec<EvalResult>,
        original_env: SharedEnv,
        depth: usize,
    },

    /// Processing rule match results with bindings.
    ProcessRuleMatches {
        remaining_matches: std::vec::IntoIter<(MettaValue, GenericBindings<MettaValue>)>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
        /// Pre-fork mutation epoch for cache isolation between sequential branches.
        /// Restored before evaluating each subsequent branch so that side effects
        /// from branch N don't invalidate caches for branch N+1.
        pre_fork_epoch: u64,
        /// Pre-fork scope generation for generation-based cache isolation.
        /// Used with `enter_fork_scope` / `next_branch_scope` / `leave_fork_scope`
        /// to isolate cache entries between nondeterministic branches without
        /// clearing caches.
        pre_fork_gen: u64,
        /// Fork depth for Prolog-style cut semantics. When `(cut)` is evaluated
        /// inside a branch's RHS, the cut signal is targeted at this depth.
        fork_depth: u32,
        /// Span correlation ID for the current branch (format v2).
        #[cfg(feature = "eval-trace")]
        branch_span_id: u64,
        /// Start timestamp of the current branch (format v2).
        #[cfg(feature = "eval-trace")]
        branch_start_ns: u64,
        /// Index of the current branch (0-based).
        #[cfg(feature = "eval-trace")]
        branch_index: u32,
        /// Total number of nondeterministic branches.
        #[cfg(feature = "eval-trace")]
        total_branches: u32,
    },

    /// I-15: Lazy nondeterministic branch evaluation via coroutine.
    /// Evaluates branches one-at-a-time until demand is satisfied.
    ProcessRuleMatchesLazy {
        /// The coroutine managing unevaluated branches.
        coroutine: Box<crate::backend::eval::cesk::coroutine::BranchCoroutine<MettaValue>>,
        /// Results accumulated so far.
        results: Vec<MettaValue>,
        /// Environment for evaluation.
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
    },

    /// Processing TCO grounded operation.
    ProcessGroundedOp {
        state: Box<GroundedState<MettaValue>>,
        /// The arg index whose evaluation result is pending.
        pending_arg_idx: usize,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing lazy Cartesian product combinations.
    ProcessCombinations {
        combinations: Box<GenericCartesianProductIter<MettaValue>>,
        results: Vec<MettaValue>,
        pending_rule_matches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing let binding
    ProcessLet {
        pending_values: Option<Vec<MettaValue>>,
        pattern: MettaValue,
        body: MettaValue,
        /// Outer bindings from an `EvalWithBindings` dispatch. When `Some`,
        /// these are composed with pattern-match bindings and the body is
        /// evaluated via `EvalWithBindings` instead of `apply_bindings`.
        /// This enables O(N) instead of O(N^2) work for nested `let*` chains.
        outer_bindings: Option<Box<GenericBindings<MettaValue>>>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Collecting grounded arg evaluation results.
    ///
    /// `evaluated_results` stores ALL results per arg (Vec<Vec<MettaValue>>) to
    /// preserve nondeterminism. After all grounded args are evaluated,
    /// the Cartesian product is computed and each combination is evaluated.
    CollectGroundedArg {
        items: Vec<MettaValue>,
        grounded_indices: Vec<usize>,
        current_idx: usize,
        evaluated_results: Vec<Vec<MettaValue>>,
        env: SharedEnv,
        depth: usize,
    },

    /// Collecting results from applicative evaluation of Cartesian product
    /// combinations produced by nondeterministic grounded arg evaluation.
    CollectApplicativeResults {
        remaining: std::vec::IntoIter<MettaValue>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing map-atom iteration
    ProcessMapAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        template: MettaValue,
        collected_results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing filter-atom iteration
    ProcessFilterAtom {
        current_element: Option<MettaValue>,
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        predicate: MettaValue,
        filtered_results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing foldl-atom iteration
    ProcessFoldlAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        acc_var_name: String,
        item_var_name: String,
        operation: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing if condition
    ProcessIfCondition {
        then_branch: MettaValue,
        else_branch: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, the taken branch is evaluated via EvalWithBindings
        /// instead of Eval, avoiding materialization of the untaken branch.
        outer_bindings: Option<Box<GenericBindings<MettaValue>>>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing case atom
    ProcessCaseAtom {
        cases: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, case templates are evaluated via EvalWithBindings
        /// instead of Eval, deferring binding application to the matched arm.
        outer_bindings: Option<Box<GenericBindings<MettaValue>>>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing (eval expr)
    ProcessEvalEval {
        env: SharedEnv,
        depth: usize,
    },

    /// Processing (return value)
    ProcessReturn {
        env: SharedEnv,
        depth: usize,
    },

    /// Processing chain expression
    ProcessChainExpr {
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, chain body is evaluated via EvalWithBindings after
        /// composing the chain variable binding with outer_bindings.
        outer_bindings: Option<Box<GenericBindings<MettaValue>>>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing chain body evaluations
    ProcessChainBody {
        remaining_values: std::vec::IntoIter<MettaValue>,
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        outer_bindings: Option<Box<GenericBindings<MettaValue>>>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing function loop
    ProcessFunction {
        iteration_count: usize,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing is-error
    ProcessIsError {
        env: SharedEnv,
        depth: usize,
    },

    /// Processing catch
    ProcessCatch {
        default: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing conjunction
    ProcessConjunction {
        remaining_goals: std::vec::IntoIter<MettaValue>,
        accumulated_results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing unify pattern1
    ProcessUnifyPattern1 {
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing unify pattern1 iteration
    ProcessUnifyPattern1Iter {
        remaining_pattern1_results: std::vec::IntoIter<MettaValue>,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        all_results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing unify pattern2
    ProcessUnifyPattern2 {
        val1: MettaValue,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing unify bodies
    ProcessUnifyBodies {
        remaining_bodies: std::vec::IntoIter<MettaValue>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing collapse
    ProcessCollapse {
        env: SharedEnv,
        depth: usize,
    },

    /// Processing collapse-bind
    ProcessCollapseBind {
        env: SharedEnv,
        depth: usize,
    },

    /// Evaluating individual collapse results before assembling the tuple.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// before wrapping in an S-expression tuple. This mirrors HE's use of `metta`
    /// (the full recursive interpreter) inside `collapse`.
    ProcessCollapseEvalResults {
        /// Remaining unevaluated results to evaluate
        remaining_raw: std::vec::IntoIter<MettaValue>,
        /// Fully evaluated results collected so far
        evaluated: Vec<MettaValue>,
        /// Whether this is for collapse-bind (vs plain collapse)
        is_bind: bool,
        /// Per-result binding snapshots from collapse-bind (None for plain collapse).
        /// When `is_bind` is true, each result is paired with its corresponding
        /// bindings encoded as `(Bindings ($var val) ...)`. Index i corresponds
        /// to the i-th raw result from the inner expression evaluation.
        per_result_bindings: Option<Vec<crate::backend::models::GenericBindings<MettaValue>>>,
        /// Index into per_result_bindings for the next result to process.
        bindings_index: usize,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing amb
    ProcessAmb {
        remaining_alts: std::vec::IntoIter<MettaValue>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing guard
    ProcessGuard {
        env: SharedEnv,
        depth: usize,
    },

    /// Processing get-atoms
    ProcessGetAtoms {
        space_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing memo table
    ProcessMemoTable {
        memo_ref: MettaValue,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing memo expression
    ProcessMemoExpr {
        memo_handle: MemoHandle,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing new-memo name
    ProcessNewMemoName {
        name_arg: MettaValue,
        size_arg: Option<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing new-memo size
    ProcessNewMemoSize {
        name: String,
        size_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing memo operation
    ProcessMemoOp {
        memo_ref: MettaValue,
        is_clear: bool,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing match space
    ProcessMatchSpace {
        space_arg: MettaValue,
        pattern: MettaValue,
        template: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing match templates
    ProcessMatchTemplates {
        remaining_templates: std::vec::IntoIter<MettaValue>,
        results: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing add-atom space
    ProcessAddAtomSpace {
        space_ref: MettaValue,
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    // Disabled: ProcessAddAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — add-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessAddAtomSpace.
    // ProcessAddAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: MettaValue,
    //     env: SharedEnv,
    //     depth: usize,
    //     parent_cont: usize,
    // },

    /// Processing remove-atom space
    ProcessRemoveAtomSpace {
        space_ref: MettaValue,
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    // Disabled: ProcessRemoveAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — remove-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessRemoveAtomSpace.
    // ProcessRemoveAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: MettaValue,
    //     env: SharedEnv,
    //     depth: usize,
    //     parent_cont: usize,
    // },

    /// Processing new-state
    ProcessNewState {
        initial_value: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing get-state
    ProcessGetState {
        state_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing change-state reference
    ProcessChangeStateRef {
        state_ref: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing change-state value
    ProcessChangeStateValue {
        state_value: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing repr
    ProcessRepr {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing format-args string
    ProcessFormatArgsString {
        format_arg: MettaValue,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing format-args args
    ProcessFormatArgsArgs {
        format_str: String,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing println
    ProcessPrintln {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing trace message
    ProcessTraceMessage {
        message: MettaValue,
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing trace value
    ProcessTraceValue {
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing get-metatype
    ProcessGetMetatype {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing bind
    ProcessBind {
        token: String,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing if-reducible: expr has been evaluated, now compare to original.
    ProcessIfReducible {
        /// Original expression (before evaluation) for comparison
        original_expr: MettaValue,
        /// Branch to evaluate if expr reduced
        then_branch: MettaValue,
        /// Branch to evaluate if expr is irreducible
        else_branch: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing match-or space evaluation
    ProcessMatchOrSpace {
        /// Space reference being evaluated
        space_arg: MettaValue,
        /// Pattern to match
        pattern: MettaValue,
        /// Default if no matches
        default: MettaValue,
        /// Template to instantiate
        template: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing sort-tuple: insertion sort via trampoline comparator evaluation.
    /// Sorted elements accumulate in `sorted`, unsorted elements wait in `unsorted`.
    /// `current` is being inserted into `sorted` at position `insert_pos`.
    ProcessSortTuple {
        /// Already-sorted elements
        sorted: Vec<MettaValue>,
        /// Remaining elements to insert
        unsorted: Vec<MettaValue>,
        /// Element currently being inserted
        current: MettaValue,
        /// Current comparison position in sorted
        insert_pos: usize,
        /// Variable name for left operand
        var1_name: String,
        /// Variable name for right operand
        var2_name: String,
        /// Comparator expression template
        comparator: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing best-candidate: linear scan evaluating rank function.
    /// Tracks the best element and its rank, evaluating remaining elements.
    ProcessBestCandidate {
        /// Best element so far (None = first iteration)
        best: Option<MettaValue>,
        /// Rank of best element
        best_rank: Option<f64>,
        /// Elements still to evaluate
        remaining: std::vec::IntoIter<MettaValue>,
        /// Element whose rank we're currently evaluating
        current: MettaValue,
        /// Variable name for rank function binding
        var_name: String,
        /// Rank function expression template
        rank_fn: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing case multi-results
    ProcessCaseMultiResults {
        remaining_atoms: std::vec::IntoIter<MettaValue>,
        cases: MettaValue,
        collected: Vec<MettaValue>,
        env: SharedEnv,
        depth: usize,
    },

    /// Evaluating individual scrutinee results for case before pattern matching.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// from the scrutinee expression before matching against case patterns.
    /// This mirrors HE's `(let $c (collapse $atom) ...)` which invokes the full
    /// interpreter on the scrutinee, ensuring rule applications are completed.
    ProcessCaseEvalScrutineeResults {
        /// Remaining unevaluated scrutinee results to evaluate
        remaining_raw: std::vec::IntoIter<MettaValue>,
        /// Fully evaluated scrutinee results collected so far
        evaluated: Vec<MettaValue>,
        /// Case patterns to match against
        cases: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Memoize evaluation results for a pure expression.
    ///
    /// When evaluation completes, the results are stored in the thread-local
    /// EVAL_MEMO cache keyed by the expression's content hash. Subsequent
    /// evaluations of structurally identical expressions skip evaluation entirely.
    MemoizeResult {
        /// Content hash of the expression (via `hash_value()`)
        expr_hash: u64,
        /// Mutation epoch at the time this continuation was pushed.
        /// If the epoch has advanced by the time evaluation completes,
        /// the result must not be cached (a side effect occurred transitively).
        mutation_epoch: u64,
        /// Environment (for result forwarding)
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing `let*` sequential bindings as a tight loop.
    ///
    /// Instead of desugaring `(let* ((p1 v1) (p2 v2) ...) body)` to nested
    /// `let` forms (which creates N S-expr allocations + 3N trampoline iterations),
    /// this continuation evaluates value expressions one at a time, accumulating
    /// bindings. When all bindings are resolved, the body is evaluated with the
    /// composed bindings via `EvalWithBindings`.
    ///
    /// **Savings**: For N bindings, reduces from 3N+2 trampoline iterations to
    /// N+2 iterations (eval each value + body), and eliminates N nested `let`
    /// S-expr allocations.
    ///
    /// **Nondeterminism**: If a value expression produces zero results, the
    /// entire `let*` produces zero results (pattern match fails). If it produces
    /// multiple results, we branch (materialize and use ProcessLet fallback).
    ProcessLetStar {
        /// The pattern for the CURRENT binding whose value is being evaluated.
        current_pattern: MettaValue,
        /// Remaining (pattern, value_expr) pairs to process after the current one.
        remaining_pairs: Vec<(MettaValue, MettaValue)>,
        /// The body template — kept raw until all bindings are resolved.
        body: MettaValue,
        /// Accumulated bindings from resolved pattern matches + outer context.
        accumulated_bindings: Box<GenericBindings<MettaValue>>,
        /// Environment for evaluation.
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Whether this is a tail call.
        is_tail_call: bool,
        /// I-5: Region ID for region-based allocation scoping.
        region_id: u32,
    },

    /// I-4: Complete a tabled subgoal after evaluation finishes.
    /// Stores the results in the SubgoalTable for future cache hits.
    CompleteSubgoal {
        /// Content hash of the expression being tabled.
        expr_hash: u64,
        /// Environment (for result forwarding).
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Mutation epoch when evaluation started (before any side effects).
        /// If the epoch changed during evaluation, the result is impure and
        /// must NOT be cached (it would suppress re-execution of side effects).
        start_epoch: u64,
    },

    /// I-6: Complete a thunk after evaluation finishes.
    /// Updates the ThunkTable with the cached results.
    CompleteThunk {
        /// Content hash of the (template, bindings) pair.
        thunk_hash: u64,
        /// Environment (for result forwarding).
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Mutation epoch when evaluation started.
        start_epoch: u64,
    },
}

// ============================================================================
// GC Root Collection — collect_values() for Safepoint GC
// ============================================================================
//
// These methods extract all MettaValue values reachable from trampoline state
// (work items and continuations) so the GC can trace them as roots during
// intra-evaluation safepoints. Environment values (rules, bindings, space facts)
// are NOT collected here — they are already registered via ROOT_REGISTRY +
// RootProvider on GenericEnvironmentShared.
//
// The exhaustive match on each enum ensures compile-time safety: adding a new
// variant without updating collect_values() causes a compile error.

impl WorkItem {
    /// Collect all MettaValue values reachable from this work item into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard.
    pub fn collect_values(&self, out: &mut Vec<MettaValue>) {
        match self {
            Self::Eval { value, expected_type, .. } => {
                out.push(*value);
                if let Some(et) = expected_type {
                    out.push(*et);
                }
            }
            Self::EvalWithBindings { template, bindings, expected_type, .. } => {
                out.push(*template);
                collect_bindings_values(bindings, out);
                if let Some(et) = expected_type {
                    out.push(*et);
                }
            }
            Self::Resume { result: (values, _), .. } => {
                out.extend(values.iter().copied());
            }
        }
    }
}

/// Helper: collect all MettaValue values from a GenericBindings into `out`.
fn collect_bindings_values(
    bindings: &GenericBindings<MettaValue>,
    out: &mut Vec<MettaValue>,
) {
    for (_name, val) in bindings.iter() {
        out.push(*val);
    }
}

/// Helper: collect all MettaValue values from a GroundedState into `out`.
fn collect_grounded_state_values(
    state: &GroundedState<MettaValue>,
    out: &mut Vec<MettaValue>,
) {
    out.extend(state.args.iter().copied());
    for vals in state.evaluated_args.values() {
        out.extend(vals.iter().copied());
    }
    for (v, bindings_opt) in &state.accumulated_results {
        out.push(*v);
        if let Some(bindings) = bindings_opt {
            collect_bindings_values(bindings, out);
        }
    }
}

/// Helper: collect all MettaValue values from a GenericCartesianProductIter into `out`.
fn collect_cartesian_values(
    iter: &GenericCartesianProductIter<MettaValue>,
    out: &mut Vec<MettaValue>,
) {
    for input_vec in iter.inputs() {
        out.extend(input_vec.iter().copied());
    }
}

impl Continuation {
    /// Collect all MettaValue values reachable from this continuation into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard. The exhaustive match
    /// ensures compile-time safety — any new variant causes a compile error
    /// until root collection is added.
    pub fn collect_values(&self, out: &mut Vec<MettaValue>) {
        match self {
            Self::Done => {}

            Self::CollectSExpr { remaining, collected, .. } => {
                out.extend(remaining.as_slice().iter().copied());
                for (vals, _env) in collected {
                    out.extend(vals.iter().copied());
                }
            }

            Self::ProcessRuleMatches { remaining_matches, results, .. } => {
                for (rhs, bindings) in remaining_matches.as_slice() {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
                out.extend(results.iter().copied());
            }

            Self::ProcessGroundedOp { state, .. } => {
                collect_grounded_state_values(state, out);
            }

            Self::ProcessCombinations { combinations, results, pending_rule_matches, .. } => {
                collect_cartesian_values(combinations, out);
                out.extend(results.iter().copied());
                for (rhs, bindings) in pending_rule_matches {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessLet { pending_values, pattern, body, outer_bindings, results, .. } => {
                if let Some(pending) = pending_values {
                    out.extend(pending.iter().copied());
                }
                out.push(*pattern);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                out.extend(results.iter().copied());
            }

            Self::CollectGroundedArg { items, evaluated_results, .. } => {
                out.extend(items.iter().copied());
                for result_vec in evaluated_results {
                    out.extend(result_vec.iter().copied());
                }
            }

            Self::CollectApplicativeResults { remaining, results, .. } => {
                out.extend(remaining.as_slice().iter().copied());
                out.extend(results.iter().copied());
            }

            Self::ProcessMapAtom { remaining_elements, template, collected_results, .. } => {
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*template);
                out.extend(collected_results.iter().copied());
            }

            Self::ProcessFilterAtom { current_element, remaining_elements, predicate, filtered_results, .. } => {
                if let Some(elem) = current_element {
                    out.push(*elem);
                }
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*predicate);
                out.extend(filtered_results.iter().copied());
            }

            Self::ProcessFoldlAtom { remaining_elements, operation, .. } => {
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*operation);
            }

            Self::ProcessIfCondition { then_branch, else_branch, outer_bindings, .. } => {
                out.push(*then_branch);
                out.push(*else_branch);
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(*v);
                    }
                }
            }

            Self::ProcessCaseAtom { cases, outer_bindings, .. } => {
                out.push(*cases);
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(*v);
                    }
                }
            }

            Self::ProcessEvalEval { .. } => {}
            Self::ProcessReturn { .. } => {}

            Self::ProcessChainExpr { var, body, outer_bindings, .. } => {
                out.push(*var);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(*v);
                    }
                }
            }

            Self::ProcessChainBody { remaining_values, var, body, outer_bindings, results, .. } => {
                out.extend(remaining_values.as_slice().iter().copied());
                out.push(*var);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(*v);
                    }
                }
                out.extend(results.iter().copied());
            }

            Self::ProcessFunction { .. } => {}
            Self::ProcessIsError { .. } => {}

            Self::ProcessCatch { default, .. } => {
                out.push(*default);
            }

            Self::ProcessConjunction { remaining_goals, accumulated_results, .. } => {
                out.extend(remaining_goals.as_slice().iter().copied());
                out.extend(accumulated_results.iter().copied());
            }

            Self::ProcessUnifyPattern1 { pattern2, success_body, failure_body, .. } => {
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
            }

            Self::ProcessUnifyPattern1Iter {
                remaining_pattern1_results, pattern2, success_body, failure_body, all_results, ..
            } => {
                out.extend(remaining_pattern1_results.as_slice().iter().copied());
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
                out.extend(all_results.iter().copied());
            }

            Self::ProcessUnifyPattern2 { val1, pattern2, success_body, failure_body, .. } => {
                out.push(*val1);
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
            }

            Self::ProcessUnifyBodies { remaining_bodies, results, .. } => {
                out.extend(remaining_bodies.as_slice().iter().copied());
                out.extend(results.iter().copied());
            }

            Self::ProcessCollapse { .. } => {}
            Self::ProcessCollapseBind { .. } => {}

            Self::ProcessCollapseEvalResults { remaining_raw, evaluated, per_result_bindings, .. } => {
                out.extend(remaining_raw.as_slice().iter().copied());
                out.extend(evaluated.iter().copied());
                if let Some(ref per_result) = per_result_bindings {
                    for bindings in per_result {
                        for (_, v) in bindings.iter() {
                            out.push(v.clone());
                        }
                    }
                }
            }

            Self::ProcessAmb { remaining_alts, results, .. } => {
                out.extend(remaining_alts.as_slice().iter().copied());
                out.extend(results.iter().copied());
            }

            Self::ProcessGuard { .. } => {}

            Self::ProcessGetAtoms { space_ref, .. } => {
                out.push(*space_ref);
            }

            Self::ProcessMemoTable { memo_ref, expr, .. } => {
                out.push(*memo_ref);
                out.push(*expr);
            }

            Self::ProcessMemoExpr { expr, .. } => {
                out.push(*expr);
            }

            Self::ProcessNewMemoName { name_arg, size_arg, .. } => {
                out.push(*name_arg);
                if let Some(size) = size_arg {
                    out.push(*size);
                }
            }

            Self::ProcessNewMemoSize { size_arg, .. } => {
                out.push(*size_arg);
            }

            Self::ProcessMemoOp { memo_ref, .. } => {
                out.push(*memo_ref);
            }

            Self::ProcessMatchSpace { space_arg, pattern, template, .. } => {
                out.push(*space_arg);
                out.push(*pattern);
                out.push(*template);
            }

            Self::ProcessMatchTemplates { remaining_templates, results, .. } => {
                out.extend(remaining_templates.as_slice().iter().copied());
                out.extend(results.iter().copied());
            }

            Self::ProcessAddAtomSpace { space_ref, atom, .. } => {
                out.push(*space_ref);
                out.push(*atom);
            }

            Self::ProcessRemoveAtomSpace { space_ref, atom, .. } => {
                out.push(*space_ref);
                out.push(*atom);
            }

            Self::ProcessNewState { initial_value, .. } => {
                out.push(*initial_value);
            }

            Self::ProcessGetState { state_ref, .. } => {
                out.push(*state_ref);
            }

            Self::ProcessChangeStateRef { state_ref, new_value, .. } => {
                out.push(*state_ref);
                out.push(*new_value);
            }

            Self::ProcessChangeStateValue { state_value, new_value, .. } => {
                out.push(*state_value);
                out.push(*new_value);
            }

            Self::ProcessRepr { atom, .. } => {
                out.push(*atom);
            }

            Self::ProcessFormatArgsString { format_arg, args_arg, .. } => {
                out.push(*format_arg);
                out.push(*args_arg);
            }

            Self::ProcessFormatArgsArgs { args_arg, .. } => {
                out.push(*args_arg);
            }

            Self::ProcessPrintln { atom, .. } => {
                out.push(*atom);
            }

            Self::ProcessTraceMessage { message, value_expr, .. } => {
                out.push(*message);
                out.push(*value_expr);
            }

            Self::ProcessTraceValue { value_expr, .. } => {
                out.push(*value_expr);
            }

            Self::ProcessGetMetatype { atom, .. } => {
                out.push(*atom);
            }

            Self::ProcessBind { .. } => {}

            Self::ProcessIfReducible { original_expr, then_branch, else_branch, .. } => {
                out.push(*original_expr);
                out.push(*then_branch);
                out.push(*else_branch);
            }

            Self::ProcessMatchOrSpace { space_arg, pattern, default, template, .. } => {
                out.push(*space_arg);
                out.push(*pattern);
                out.push(*default);
                out.push(*template);
            }

            Self::ProcessSortTuple { sorted, unsorted, current, comparator, .. } => {
                out.extend(sorted.iter().copied());
                out.extend(unsorted.iter().copied());
                out.push(*current);
                out.push(*comparator);
            }

            Self::ProcessBestCandidate { best, remaining, current, rank_fn, .. } => {
                if let Some(b) = best {
                    out.push(*b);
                }
                out.extend(remaining.as_slice().iter().copied());
                out.push(*current);
                out.push(*rank_fn);
            }

            Self::ProcessCaseMultiResults { remaining_atoms, cases, collected, .. } => {
                out.extend(remaining_atoms.as_slice().iter().copied());
                out.push(*cases);
                out.extend(collected.iter().copied());
            }

            Self::ProcessCaseEvalScrutineeResults { remaining_raw, evaluated, cases, .. } => {
                out.extend(remaining_raw.as_slice().iter().copied());
                out.extend(evaluated.iter().copied());
                out.push(*cases);
            }

            Self::MemoizeResult { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::ProcessLetStar { current_pattern, remaining_pairs, body, accumulated_bindings, .. } => {
                out.push(*current_pattern);
                for (pattern, value_expr) in remaining_pairs {
                    out.push(*pattern);
                    out.push(*value_expr);
                }
                out.push(*body);
                collect_bindings_values(accumulated_bindings, out);
            }

            Self::ProcessRuleMatchesLazy { coroutine, results, .. } => {
                coroutine.collect_values(out);
                out.extend(results.iter().copied());
            }

            Self::CompleteSubgoal { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::CompleteThunk { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }
        }
    }

    /// Return the depth hint from whichever variant is active.
    ///
    /// Used by the cooperative yield logic to record the evaluation depth
    /// at the suspension point for priority scheduling. All variants except
    /// `Done` carry a `depth` field.
    pub fn depth_hint(&self) -> usize {
        match self {
            Self::Done => 0,
            Self::CollectSExpr { depth, .. }
            | Self::ProcessRuleMatches { depth, .. }
            | Self::ProcessRuleMatchesLazy { depth, .. }
            | Self::ProcessGroundedOp { depth, .. }
            | Self::ProcessCombinations { depth, .. }
            | Self::ProcessLet { depth, .. }
            | Self::CollectGroundedArg { depth, .. }
            | Self::CollectApplicativeResults { depth, .. }
            | Self::ProcessMapAtom { depth, .. }
            | Self::ProcessFilterAtom { depth, .. }
            | Self::ProcessFoldlAtom { depth, .. }
            | Self::ProcessIfCondition { depth, .. }
            | Self::ProcessCaseAtom { depth, .. }
            | Self::ProcessEvalEval { depth, .. }
            | Self::ProcessReturn { depth, .. }
            | Self::ProcessChainExpr { depth, .. }
            | Self::ProcessChainBody { depth, .. }
            | Self::ProcessFunction { depth, .. }
            | Self::ProcessIsError { depth, .. }
            | Self::ProcessCatch { depth, .. }
            | Self::ProcessConjunction { depth, .. }
            | Self::ProcessUnifyPattern1 { depth, .. }
            | Self::ProcessUnifyPattern1Iter { depth, .. }
            | Self::ProcessUnifyPattern2 { depth, .. }
            | Self::ProcessUnifyBodies { depth, .. }
            | Self::ProcessCollapse { depth, .. }
            | Self::ProcessCollapseBind { depth, .. }
            | Self::ProcessCollapseEvalResults { depth, .. }
            | Self::ProcessAmb { depth, .. }
            | Self::ProcessGuard { depth, .. }
            | Self::ProcessGetAtoms { depth, .. }
            | Self::ProcessMemoTable { depth, .. }
            | Self::ProcessMemoExpr { depth, .. }
            | Self::ProcessNewMemoName { depth, .. }
            | Self::ProcessNewMemoSize { depth, .. }
            | Self::ProcessMemoOp { depth, .. }
            | Self::ProcessBind { depth, .. }
            | Self::ProcessMatchSpace { depth, .. }
            | Self::ProcessMatchTemplates { depth, .. }
            | Self::ProcessAddAtomSpace { depth, .. }
            | Self::ProcessRemoveAtomSpace { depth, .. }
            | Self::ProcessNewState { depth, .. }
            | Self::ProcessGetState { depth, .. }
            | Self::ProcessChangeStateRef { depth, .. }
            | Self::ProcessChangeStateValue { depth, .. }
            | Self::ProcessRepr { depth, .. }
            | Self::ProcessFormatArgsString { depth, .. }
            | Self::ProcessFormatArgsArgs { depth, .. }
            | Self::ProcessPrintln { depth, .. }
            | Self::ProcessTraceMessage { depth, .. }
            | Self::ProcessTraceValue { depth, .. }
            | Self::ProcessGetMetatype { depth, .. }
            | Self::ProcessIfReducible { depth, .. }
            | Self::ProcessMatchOrSpace { depth, .. }
            | Self::ProcessSortTuple { depth, .. }
            | Self::ProcessBestCandidate { depth, .. }
            | Self::ProcessCaseMultiResults { depth, .. }
            | Self::ProcessCaseEvalScrutineeResults { depth, .. }
            | Self::MemoizeResult { depth, .. }
            | Self::ProcessLetStar { depth, .. }
            | Self::CompleteSubgoal { depth, .. }
            | Self::CompleteThunk { depth, .. } => *depth,
        }
    }
}

// ============================================================================
// Tests for GC Root Collection
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use smallvec::smallvec;
    use crate::backend::models::{MettaValueFactory, global_factory};

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    fn env() -> SharedEnv {
        std::sync::Arc::new(MettaEnvironment::new(factory()))
    }

    #[test]
    fn test_work_item_eval_collects_value() {
        let f = factory();
        let item = WorkItem::Eval {
            value: f.long(42),
            env: env(),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
            demand: None,
        };
        let mut roots = Vec::new();
        item.collect_values(&mut roots);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].as_long(), Some(42));
    }

    #[test]
    fn test_work_item_resume_collects_results() {
        let f = factory();
        let item = WorkItem::Resume {
            result: (smallvec![f.long(1), f.long(2), f.long(3)], env()),
        };
        let mut roots = Vec::new();
        item.collect_values(&mut roots);
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0].as_long(), Some(1));
        assert_eq!(roots[1].as_long(), Some(2));
        assert_eq!(roots[2].as_long(), Some(3));
    }

    #[test]
    fn test_continuation_done_collects_nothing() {
        let cont = Continuation::Done;
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_continuation_collect_sexpr_collects_all() {
        let f = factory();
        let cont = Continuation::CollectSExpr {
            remaining: vec![f.long(10), f.long(20)].into_iter(),
            collected: vec![
                (smallvec![f.long(30)], env()),
                (smallvec![f.long(40), f.long(50)], env()),
            ],
            original_env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 remaining + 1 + 2 collected = 5
        assert_eq!(roots.len(), 5);
    }

    #[test]
    fn test_continuation_if_condition_collects_branches() {
        let f = factory();
        let cont = Continuation::ProcessIfCondition {
            then_branch: f.long(100),
            else_branch: f.long(200),
            outer_bindings: None,
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].as_long(), Some(100));
        assert_eq!(roots[1].as_long(), Some(200));
    }

    #[test]
    fn test_continuation_let_collects_pattern_body_results() {
        let f = factory();
        let cont = Continuation::ProcessLet {
            pending_values: Some(vec![f.atom("a"), f.atom("b")].into()),
            pattern: f.atom("$x"),
            body: f.atom("body"),
            outer_bindings: None,
            results: vec![f.long(1)],
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 pending + 1 pattern + 1 body + 1 result = 5
        assert_eq!(roots.len(), 5);
    }

    #[test]
    fn test_continuation_process_bind_collects_nothing() {
        let cont = Continuation::ProcessBind {
            token: "var".to_string(),
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_continuation_match_space_collects_three_values() {
        let f = factory();
        let cont = Continuation::ProcessMatchSpace {
            space_arg: f.atom("&self"),
            pattern: f.atom("$p"),
            template: f.atom("$t"),
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_continuation_collapse_eval_results() {
        let f = factory();
        let cont = Continuation::ProcessCollapseEvalResults {
            remaining_raw: vec![f.long(1), f.long(2)].into_iter(),
            evaluated: vec![f.long(3)],
            is_bind: false,
            per_result_bindings: None,
            bindings_index: 0,
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 2 remaining + 1 evaluated = 3
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_continuation_unify_pattern1_iter_collects_all() {
        let f = factory();
        let cont = Continuation::ProcessUnifyPattern1Iter {
            remaining_pattern1_results: vec![f.long(1)].into_iter(),
            pattern2: f.atom("p2"),
            success_body: f.atom("ok"),
            failure_body: f.atom("fail"),
            all_results: vec![f.long(99)],
            env: env(),
            depth: 0,
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 1 remaining + 1 pattern2 + 1 success + 1 failure + 1 result = 5
        assert_eq!(roots.len(), 5);
    }
}
