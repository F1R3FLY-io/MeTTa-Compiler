//! Generic Trampoline Types for Iterative Evaluation
//!
//! These types are parameterized over the value type V and environment type E,
//! enabling the same evaluation logic to work with both heap and arena allocation
//! strategies using their respective environment types.
//!
//! ## Design Notes
//!
//! - `WorkItem<V, E>` and `Continuation<V, E>` are generic over both types
//! - Default type parameter `E = Environment` for backward compatibility
//! - Bindings remain heap-allocated (converted at boundaries if needed)
//! - Some continuation variants (ProcessCombinations) use concrete types
//!   due to complex dependencies
//!
//! ## Type Parameters
//!
//! - `V: MettaValueTrait` - The value type (MettaValue or MettaValue)
//! - `E: Clone` - The environment type (Environment or GenericEnvironment<V>)

use std::fmt::Debug;

use smallvec::SmallVec;

use crate::backend::environment::MettaEnvironment;
use crate::backend::grounded::GenericGroundedState;
use crate::backend::models::{GenericBindings, MemoHandle, MettaValueTrait};
// SpaceHandle was previously used by ProcessAddAtomAtom and ProcessRemoveAtomAtom,
// which are now disabled (see comments on those variants below).

// Import generic Cartesian product iterator
use super::super::processing::GenericCartesianProductIter;

/// Generic evaluation result: (results, environment)
///
/// Parameterized over value type V and environment type E.
/// Default E = Environment for backward compatibility.
/// Uses SmallVec<[V; 2]> to inline up to 2 elements, avoiding heap allocation
/// for the common single-result case (93%+ of evaluations produce 1 result).
pub type GenericEvalResult<V, E = MettaEnvironment> = (SmallVec<[V; 2]>, E);

/// Generic work item representing pending evaluation work.
///
/// Parameterized over value type V and environment type E, enabling
/// the same evaluation logic to work with both heap and arena allocation.
///
/// # Type Parameters
///
/// - `V: MettaValueTrait` - The value type (MettaValue or MettaValue)
/// - `E: Clone` - The environment type (defaults to Environment for backward compatibility)
#[derive(Debug)]
pub enum GenericWorkItem<V: MettaValueTrait, E: Clone = MettaEnvironment> {
    /// Evaluate a value and send result to continuation at stack top
    Eval {
        value: V,
        env: E,
        depth: usize,
        /// If true, this is a tail call - don't increment depth
        is_tail_call: bool,
        /// Phase 8.7: Expected return type for branch pruning.
        /// When set, rules whose `rhs_type` is incompatible with this type
        /// are pruned from the match set before evaluation.
        expected_type: Option<V>,
    },
    /// Evaluate a template with deferred bindings (lazy binding).
    ///
    /// Instead of calling `apply_bindings_generic` upfront to materialize
    /// a fully-substituted expression tree, this carries `(template, bindings)`
    /// and resolves variables lazily:
    /// - Variables: look up in bindings, push result
    /// - Ground (no variables): push as Eval directly
    /// - Special forms (let, if, chain): resolve only immediate args, forward
    ///   remaining bindings to child evaluations via binding composition
    /// - Other S-expressions: fall back to `apply_bindings_generic` + Eval
    ///
    /// This avoids O(tree_depth) recursive allocation for nested `let*` chains,
    /// where each level would otherwise materialize the entire remaining body.
    EvalWithBindings {
        template: V,
        bindings: GenericBindings<V>,
        env: E,
        depth: usize,
        is_tail_call: bool,
        expected_type: Option<V>,
    },
    /// Resume the continuation at stack top with a result
    Resume {
        result: GenericEvalResult<V, E>,
    },
}

/// Generic continuation representing what to do with an evaluation result.
///
/// Parameterized over value type V and environment type E, enabling
/// the same evaluation logic to work with both heap and arena allocation.
///
/// # Type Parameters
///
/// - `V: MettaValueTrait` - The value type (MettaValue or MettaValue)
/// - `E: Clone` - The environment type (defaults to Environment for backward compatibility)
///
/// # Note on env/depth fields
///
/// Some variants store `env` and `depth` fields that are not read directly.
/// These fields are preserved for context but the actual environment from the
/// evaluation result is used instead. This is intentional - continuations track
/// the original environment for debugging/reference.
#[derive(Debug)]
pub enum GenericContinuation<V: MettaValueTrait, E: Clone = MettaEnvironment> {
    /// Final result - return from eval()
    Done,

    /// Collecting S-expression sub-results before processing
    CollectSExpr {
        remaining: std::vec::IntoIter<V>,
        collected: Vec<GenericEvalResult<V, E>>,
        original_env: E,
        depth: usize,
    },

    /// Processing rule match results with generic bindings.
    ProcessRuleMatches {
        remaining_matches: std::vec::IntoIter<(V, GenericBindings<V>)>,
        results: Vec<V>,
        env: E,
        depth: usize,
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
        coroutine: crate::backend::eval::cesk::coroutine::BranchCoroutine<V>,
        /// Results accumulated so far.
        results: Vec<V>,
        /// Environment for evaluation.
        env: E,
        /// Evaluation depth.
        depth: usize,
    },

    /// Processing TCO grounded operation.
    ProcessGroundedOp {
        state: GenericGroundedState<V>,
        /// The arg index whose evaluation result is pending.
        pending_arg_idx: usize,
        env: E,
        depth: usize,
    },

    /// Processing lazy Cartesian product combinations (generic version).
    ProcessCombinations {
        combinations: GenericCartesianProductIter<V>,
        results: Vec<V>,
        pending_rule_matches: Vec<(V, GenericBindings<V>)>,
        env: E,
        depth: usize,
    },

    /// Processing let binding
    ProcessLet {
        pending_values: Option<Vec<V>>,
        pattern: V,
        body: V,
        /// Outer bindings from an `EvalWithBindings` dispatch. When `Some`,
        /// these are composed with pattern-match bindings and the body is
        /// evaluated via `EvalWithBindings` instead of `apply_bindings_generic`.
        /// This enables O(N) instead of O(N^2) work for nested `let*` chains.
        outer_bindings: Option<GenericBindings<V>>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Collecting grounded arg evaluation results.
    ///
    /// `evaluated_results` stores ALL results per arg (Vec<Vec<V>>) to
    /// preserve nondeterminism. After all grounded args are evaluated,
    /// the Cartesian product is computed and each combination is evaluated.
    CollectGroundedArg {
        items: Vec<V>,
        grounded_indices: Vec<usize>,
        current_idx: usize,
        evaluated_results: Vec<Vec<V>>,
        env: E,
        depth: usize,
    },

    /// Collecting results from applicative evaluation of Cartesian product
    /// combinations produced by nondeterministic grounded arg evaluation.
    CollectApplicativeResults {
        remaining: std::vec::IntoIter<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing map-atom iteration
    ProcessMapAtom {
        remaining_elements: std::vec::IntoIter<V>,
        var_name: String,
        template: V,
        collected_results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing filter-atom iteration
    ProcessFilterAtom {
        current_element: Option<V>,
        remaining_elements: std::vec::IntoIter<V>,
        var_name: String,
        predicate: V,
        filtered_results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing foldl-atom iteration
    ProcessFoldlAtom {
        remaining_elements: std::vec::IntoIter<V>,
        acc_var_name: String,
        item_var_name: String,
        operation: V,
        env: E,
        depth: usize,
    },

    /// Processing if condition
    ProcessIfCondition {
        then_branch: V,
        else_branch: V,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, the taken branch is evaluated via EvalWithBindings
        /// instead of Eval, avoiding materialization of the untaken branch.
        outer_bindings: Option<GenericBindings<V>>,
        env: E,
        depth: usize,
    },

    /// Processing case atom
    ProcessCaseAtom {
        cases: V,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, case templates are evaluated via EvalWithBindings
        /// instead of Eval, deferring binding application to the matched arm.
        outer_bindings: Option<GenericBindings<V>>,
        env: E,
        depth: usize,
    },

    /// Processing (eval expr)
    ProcessEvalEval {
        env: E,
        depth: usize,
    },

    /// Processing (return value)
    ProcessReturn {
        env: E,
        depth: usize,
    },

    /// Processing chain expression
    ProcessChainExpr {
        var: V,
        body: V,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, chain body is evaluated via EvalWithBindings after
        /// composing the chain variable binding with outer_bindings.
        outer_bindings: Option<GenericBindings<V>>,
        env: E,
        depth: usize,
    },

    /// Processing chain body evaluations
    ProcessChainBody {
        remaining_values: std::vec::IntoIter<V>,
        var: V,
        body: V,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        outer_bindings: Option<GenericBindings<V>>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing function loop
    ProcessFunction {
        iteration_count: usize,
        env: E,
        depth: usize,
    },

    /// Processing is-error
    ProcessIsError {
        env: E,
        depth: usize,
    },

    /// Processing catch
    ProcessCatch {
        default: V,
        env: E,
        depth: usize,
    },

    /// Processing conjunction
    ProcessConjunction {
        remaining_goals: std::vec::IntoIter<V>,
        accumulated_results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing unify pattern1
    ProcessUnifyPattern1 {
        pattern2: V,
        success_body: V,
        failure_body: V,
        env: E,
        depth: usize,
    },

    /// Processing unify pattern1 iteration
    ProcessUnifyPattern1Iter {
        remaining_pattern1_results: std::vec::IntoIter<V>,
        pattern2: V,
        success_body: V,
        failure_body: V,
        all_results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing unify pattern2
    ProcessUnifyPattern2 {
        val1: V,
        pattern2: V,
        success_body: V,
        failure_body: V,
        env: E,
        depth: usize,
    },

    /// Processing unify bodies
    ProcessUnifyBodies {
        remaining_bodies: std::vec::IntoIter<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing collapse
    ProcessCollapse {
        env: E,
        depth: usize,
    },

    /// Processing collapse-bind
    ProcessCollapseBind {
        env: E,
        depth: usize,
    },

    /// Evaluating individual collapse results before assembling the tuple.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// before wrapping in an S-expression tuple. This mirrors HE's use of `metta`
    /// (the full recursive interpreter) inside `collapse`.
    ProcessCollapseEvalResults {
        /// Remaining unevaluated results to evaluate
        remaining_raw: std::vec::IntoIter<V>,
        /// Fully evaluated results collected so far
        evaluated: Vec<V>,
        /// Whether this is for collapse-bind (vs plain collapse)
        is_bind: bool,
        /// Environment
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing amb
    ProcessAmb {
        remaining_alts: std::vec::IntoIter<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing guard
    ProcessGuard {
        env: E,
        depth: usize,
    },

    /// Processing get-atoms
    ProcessGetAtoms {
        space_ref: V,
        env: E,
        depth: usize,
    },

    /// Processing memo table
    ProcessMemoTable {
        memo_ref: V,
        expr: V,
        first_only: bool,
        env: E,
        depth: usize,
    },

    /// Processing memo expression
    ProcessMemoExpr {
        memo_handle: MemoHandle,
        expr: V,
        first_only: bool,
        env: E,
        depth: usize,
    },

    /// Processing new-memo name
    ProcessNewMemoName {
        name_arg: V,
        size_arg: Option<V>,
        env: E,
        depth: usize,
    },

    /// Processing new-memo size
    ProcessNewMemoSize {
        name: String,
        size_arg: V,
        env: E,
        depth: usize,
    },

    /// Processing memo operation
    ProcessMemoOp {
        memo_ref: V,
        is_clear: bool,
        env: E,
        depth: usize,
    },

    /// Processing match space
    ProcessMatchSpace {
        space_arg: V,
        pattern: V,
        template: V,
        env: E,
        depth: usize,
    },

    /// Processing match templates
    ProcessMatchTemplates {
        remaining_templates: std::vec::IntoIter<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Processing add-atom space
    ProcessAddAtomSpace {
        space_ref: V,
        atom: V,
        env: E,
        depth: usize,
    },

    // Disabled: ProcessAddAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — add-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessAddAtomSpace.
    // ProcessAddAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: V,
    //     env: E,
    //     depth: usize,
    //     parent_cont: usize,
    // },

    /// Processing remove-atom space
    ProcessRemoveAtomSpace {
        space_ref: V,
        atom: V,
        env: E,
        depth: usize,
    },

    // Disabled: ProcessRemoveAtomAtom is no longer constructed. The atom evaluation
    // step has been eliminated — remove-atom now takes unevaluated atoms per MeTTa HE
    // semantics. Rule table and type system are updated directly in ProcessRemoveAtomSpace.
    // ProcessRemoveAtomAtom {
    //     space_handle: SpaceHandle,
    //     atom: V,
    //     env: E,
    //     depth: usize,
    //     parent_cont: usize,
    // },

    /// Processing new-state
    ProcessNewState {
        initial_value: V,
        env: E,
        depth: usize,
    },

    /// Processing get-state
    ProcessGetState {
        state_ref: V,
        env: E,
        depth: usize,
    },

    /// Processing change-state reference
    ProcessChangeStateRef {
        state_ref: V,
        new_value: V,
        env: E,
        depth: usize,
    },

    /// Processing change-state value
    ProcessChangeStateValue {
        state_value: V,
        new_value: V,
        env: E,
        depth: usize,
    },

    /// Processing repr
    ProcessRepr {
        atom: V,
        env: E,
        depth: usize,
    },

    /// Processing format-args string
    ProcessFormatArgsString {
        format_arg: V,
        args_arg: V,
        env: E,
        depth: usize,
    },

    /// Processing format-args args
    ProcessFormatArgsArgs {
        format_str: String,
        args_arg: V,
        env: E,
        depth: usize,
    },

    /// Processing println
    ProcessPrintln {
        atom: V,
        env: E,
        depth: usize,
    },

    /// Processing trace message
    ProcessTraceMessage {
        message: V,
        value_expr: V,
        env: E,
        depth: usize,
    },

    /// Processing trace value
    ProcessTraceValue {
        value_expr: V,
        env: E,
        depth: usize,
    },

    /// Processing get-metatype
    ProcessGetMetatype {
        atom: V,
        env: E,
        depth: usize,
    },

    /// Processing bind
    ProcessBind {
        token: String,
        env: E,
        depth: usize,
    },

    /// Processing if-reducible: expr has been evaluated, now compare to original.
    ProcessIfReducible {
        /// Original expression (before evaluation) for comparison
        original_expr: V,
        /// Branch to evaluate if expr reduced
        then_branch: V,
        /// Branch to evaluate if expr is irreducible
        else_branch: V,
        /// Environment
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing match-or space evaluation
    ProcessMatchOrSpace {
        /// Space reference being evaluated
        space_arg: V,
        /// Pattern to match
        pattern: V,
        /// Default if no matches
        default: V,
        /// Template to instantiate
        template: V,
        /// Environment
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing sort-tuple: insertion sort via trampoline comparator evaluation.
    /// Sorted elements accumulate in `sorted`, unsorted elements wait in `unsorted`.
    /// `current` is being inserted into `sorted` at position `insert_pos`.
    ProcessSortTuple {
        /// Already-sorted elements
        sorted: Vec<V>,
        /// Remaining elements to insert
        unsorted: Vec<V>,
        /// Element currently being inserted
        current: V,
        /// Current comparison position in sorted
        insert_pos: usize,
        /// Variable name for left operand
        var1_name: String,
        /// Variable name for right operand
        var2_name: String,
        /// Comparator expression template
        comparator: V,
        /// Environment
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing best-candidate: linear scan evaluating rank function.
    /// Tracks the best element and its rank, evaluating remaining elements.
    ProcessBestCandidate {
        /// Best element so far (None = first iteration)
        best: Option<V>,
        /// Rank of best element
        best_rank: Option<f64>,
        /// Elements still to evaluate
        remaining: std::vec::IntoIter<V>,
        /// Element whose rank we're currently evaluating
        current: V,
        /// Variable name for rank function binding
        var_name: String,
        /// Rank function expression template
        rank_fn: V,
        /// Environment
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Processing case multi-results
    ProcessCaseMultiResults {
        remaining_atoms: std::vec::IntoIter<V>,
        cases: V,
        collected: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Evaluating individual scrutinee results for case before pattern matching.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// from the scrutinee expression before matching against case patterns.
    /// This mirrors HE's `(let $c (collapse $atom) ...)` which invokes the full
    /// interpreter on the scrutinee, ensuring rule applications are completed.
    ProcessCaseEvalScrutineeResults {
        /// Remaining unevaluated scrutinee results to evaluate
        remaining_raw: std::vec::IntoIter<V>,
        /// Fully evaluated scrutinee results collected so far
        evaluated: Vec<V>,
        /// Case patterns to match against
        cases: V,
        /// Environment
        env: E,
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
        /// Environment (for result forwarding)
        env: E,
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
        current_pattern: V,
        /// Remaining (pattern, value_expr) pairs to process after the current one.
        remaining_pairs: Vec<(V, V)>,
        /// The body template — kept raw until all bindings are resolved.
        body: V,
        /// Accumulated bindings from resolved pattern matches + outer context.
        accumulated_bindings: GenericBindings<V>,
        /// Environment for evaluation.
        env: E,
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
        env: E,
        /// Evaluation depth.
        depth: usize,
    },

    /// I-6: Complete a thunk after evaluation finishes.
    /// Updates the ThunkTable with the cached results.
    CompleteThunk {
        /// Content hash of the (template, bindings) pair.
        thunk_hash: u64,
        /// Environment (for result forwarding).
        env: E,
        /// Evaluation depth.
        depth: usize,
    },
}

// ============================================================================
// GC Root Collection — collect_values() for Safepoint GC
// ============================================================================
//
// These methods extract all V values reachable from trampoline state (work items
// and continuations) so the GC can trace them as roots during intra-evaluation
// safepoints. Environment values (rules, bindings, space facts) are NOT collected
// here — they are already registered via ROOT_REGISTRY + RootProvider on
// GenericEnvironmentShared.
//
// The exhaustive match on each enum ensures compile-time safety: adding a new
// variant without updating collect_values() causes a compile error.

impl<V: MettaValueTrait + Clone, E: Clone> GenericWorkItem<V, E> {
    /// Collect all V values reachable from this work item into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard.
    pub fn collect_values(&self, out: &mut Vec<V>) {
        match self {
            Self::Eval { value, expected_type, .. } => {
                out.push(value.clone());
                if let Some(et) = expected_type {
                    out.push(et.clone());
                }
            }
            Self::EvalWithBindings { template, bindings, expected_type, .. } => {
                out.push(template.clone());
                collect_bindings_values(bindings, out);
                if let Some(et) = expected_type {
                    out.push(et.clone());
                }
            }
            Self::Resume { result: (values, _), .. } => {
                out.extend(values.iter().cloned());
            }
        }
    }
}

/// Helper: collect all V values from a GenericBindings into `out`.
fn collect_bindings_values<V: MettaValueTrait + Clone>(
    bindings: &GenericBindings<V>,
    out: &mut Vec<V>,
) {
    for (_name, val) in bindings.iter() {
        out.push(val.clone());
    }
}

/// Helper: collect all V values from a GenericGroundedState into `out`.
fn collect_grounded_state_values<V: MettaValueTrait + Clone>(
    state: &GenericGroundedState<V>,
    out: &mut Vec<V>,
) {
    out.extend(state.args.iter().cloned());
    for vals in state.evaluated_args.values() {
        out.extend(vals.iter().cloned());
    }
    for (v, bindings_opt) in &state.accumulated_results {
        out.push(v.clone());
        if let Some(bindings) = bindings_opt {
            collect_bindings_values(bindings, out);
        }
    }
}

/// Helper: collect all V values from a GenericCartesianProductIter into `out`.
fn collect_cartesian_values<V: MettaValueTrait + Clone>(
    iter: &GenericCartesianProductIter<V>,
    out: &mut Vec<V>,
) {
    for input_vec in iter.inputs() {
        out.extend(input_vec.iter().cloned());
    }
}

impl<V: MettaValueTrait + Clone, E: Clone> GenericContinuation<V, E> {
    /// Collect all V values reachable from this continuation into `out`.
    ///
    /// Used by the safepoint GC protocol to register trampoline state as
    /// temporary roots before dropping the EvalGuard. The exhaustive match
    /// ensures compile-time safety — any new variant causes a compile error
    /// until root collection is added.
    pub fn collect_values(&self, out: &mut Vec<V>) {
        match self {
            Self::Done => {}

            Self::CollectSExpr { remaining, collected, .. } => {
                out.extend(remaining.as_slice().iter().cloned());
                for (vals, _env) in collected {
                    out.extend(vals.iter().cloned());
                }
            }

            Self::ProcessRuleMatches { remaining_matches, results, .. } => {
                for (rhs, bindings) in remaining_matches.as_slice() {
                    out.push(rhs.clone());
                    collect_bindings_values(bindings, out);
                }
                out.extend(results.iter().cloned());
            }

            Self::ProcessGroundedOp { state, .. } => {
                collect_grounded_state_values(state, out);
            }

            Self::ProcessCombinations { combinations, results, pending_rule_matches, .. } => {
                collect_cartesian_values(combinations, out);
                out.extend(results.iter().cloned());
                for (rhs, bindings) in pending_rule_matches {
                    out.push(rhs.clone());
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessLet { pending_values, pattern, body, outer_bindings, results, .. } => {
                if let Some(pending) = pending_values {
                    out.extend(pending.iter().cloned());
                }
                out.push(pattern.clone());
                out.push(body.clone());
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                out.extend(results.iter().cloned());
            }

            Self::CollectGroundedArg { items, evaluated_results, .. } => {
                out.extend(items.iter().cloned());
                for result_vec in evaluated_results {
                    out.extend(result_vec.iter().cloned());
                }
            }

            Self::CollectApplicativeResults { remaining, results, .. } => {
                out.extend(remaining.as_slice().iter().cloned());
                out.extend(results.iter().cloned());
            }

            Self::ProcessMapAtom { remaining_elements, template, collected_results, .. } => {
                out.extend(remaining_elements.as_slice().iter().cloned());
                out.push(template.clone());
                out.extend(collected_results.iter().cloned());
            }

            Self::ProcessFilterAtom { current_element, remaining_elements, predicate, filtered_results, .. } => {
                if let Some(elem) = current_element {
                    out.push(elem.clone());
                }
                out.extend(remaining_elements.as_slice().iter().cloned());
                out.push(predicate.clone());
                out.extend(filtered_results.iter().cloned());
            }

            Self::ProcessFoldlAtom { remaining_elements, operation, .. } => {
                out.extend(remaining_elements.as_slice().iter().cloned());
                out.push(operation.clone());
            }

            Self::ProcessIfCondition { then_branch, else_branch, outer_bindings, .. } => {
                out.push(then_branch.clone());
                out.push(else_branch.clone());
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(v.clone());
                    }
                }
            }

            Self::ProcessCaseAtom { cases, outer_bindings, .. } => {
                out.push(cases.clone());
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(v.clone());
                    }
                }
            }

            Self::ProcessEvalEval { .. } => {}
            Self::ProcessReturn { .. } => {}

            Self::ProcessChainExpr { var, body, outer_bindings, .. } => {
                out.push(var.clone());
                out.push(body.clone());
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(v.clone());
                    }
                }
            }

            Self::ProcessChainBody { remaining_values, var, body, outer_bindings, results, .. } => {
                out.extend(remaining_values.as_slice().iter().cloned());
                out.push(var.clone());
                out.push(body.clone());
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(v.clone());
                    }
                }
                out.extend(results.iter().cloned());
            }

            Self::ProcessFunction { .. } => {}
            Self::ProcessIsError { .. } => {}

            Self::ProcessCatch { default, .. } => {
                out.push(default.clone());
            }

            Self::ProcessConjunction { remaining_goals, accumulated_results, .. } => {
                out.extend(remaining_goals.as_slice().iter().cloned());
                out.extend(accumulated_results.iter().cloned());
            }

            Self::ProcessUnifyPattern1 { pattern2, success_body, failure_body, .. } => {
                out.push(pattern2.clone());
                out.push(success_body.clone());
                out.push(failure_body.clone());
            }

            Self::ProcessUnifyPattern1Iter {
                remaining_pattern1_results, pattern2, success_body, failure_body, all_results, ..
            } => {
                out.extend(remaining_pattern1_results.as_slice().iter().cloned());
                out.push(pattern2.clone());
                out.push(success_body.clone());
                out.push(failure_body.clone());
                out.extend(all_results.iter().cloned());
            }

            Self::ProcessUnifyPattern2 { val1, pattern2, success_body, failure_body, .. } => {
                out.push(val1.clone());
                out.push(pattern2.clone());
                out.push(success_body.clone());
                out.push(failure_body.clone());
            }

            Self::ProcessUnifyBodies { remaining_bodies, results, .. } => {
                out.extend(remaining_bodies.as_slice().iter().cloned());
                out.extend(results.iter().cloned());
            }

            Self::ProcessCollapse { .. } => {}
            Self::ProcessCollapseBind { .. } => {}

            Self::ProcessCollapseEvalResults { remaining_raw, evaluated, .. } => {
                out.extend(remaining_raw.as_slice().iter().cloned());
                out.extend(evaluated.iter().cloned());
            }

            Self::ProcessAmb { remaining_alts, results, .. } => {
                out.extend(remaining_alts.as_slice().iter().cloned());
                out.extend(results.iter().cloned());
            }

            Self::ProcessGuard { .. } => {}

            Self::ProcessGetAtoms { space_ref, .. } => {
                out.push(space_ref.clone());
            }

            Self::ProcessMemoTable { memo_ref, expr, .. } => {
                out.push(memo_ref.clone());
                out.push(expr.clone());
            }

            Self::ProcessMemoExpr { expr, .. } => {
                out.push(expr.clone());
            }

            Self::ProcessNewMemoName { name_arg, size_arg, .. } => {
                out.push(name_arg.clone());
                if let Some(size) = size_arg {
                    out.push(size.clone());
                }
            }

            Self::ProcessNewMemoSize { size_arg, .. } => {
                out.push(size_arg.clone());
            }

            Self::ProcessMemoOp { memo_ref, .. } => {
                out.push(memo_ref.clone());
            }

            Self::ProcessMatchSpace { space_arg, pattern, template, .. } => {
                out.push(space_arg.clone());
                out.push(pattern.clone());
                out.push(template.clone());
            }

            Self::ProcessMatchTemplates { remaining_templates, results, .. } => {
                out.extend(remaining_templates.as_slice().iter().cloned());
                out.extend(results.iter().cloned());
            }

            Self::ProcessAddAtomSpace { space_ref, atom, .. } => {
                out.push(space_ref.clone());
                out.push(atom.clone());
            }

            Self::ProcessRemoveAtomSpace { space_ref, atom, .. } => {
                out.push(space_ref.clone());
                out.push(atom.clone());
            }

            Self::ProcessNewState { initial_value, .. } => {
                out.push(initial_value.clone());
            }

            Self::ProcessGetState { state_ref, .. } => {
                out.push(state_ref.clone());
            }

            Self::ProcessChangeStateRef { state_ref, new_value, .. } => {
                out.push(state_ref.clone());
                out.push(new_value.clone());
            }

            Self::ProcessChangeStateValue { state_value, new_value, .. } => {
                out.push(state_value.clone());
                out.push(new_value.clone());
            }

            Self::ProcessRepr { atom, .. } => {
                out.push(atom.clone());
            }

            Self::ProcessFormatArgsString { format_arg, args_arg, .. } => {
                out.push(format_arg.clone());
                out.push(args_arg.clone());
            }

            Self::ProcessFormatArgsArgs { args_arg, .. } => {
                out.push(args_arg.clone());
            }

            Self::ProcessPrintln { atom, .. } => {
                out.push(atom.clone());
            }

            Self::ProcessTraceMessage { message, value_expr, .. } => {
                out.push(message.clone());
                out.push(value_expr.clone());
            }

            Self::ProcessTraceValue { value_expr, .. } => {
                out.push(value_expr.clone());
            }

            Self::ProcessGetMetatype { atom, .. } => {
                out.push(atom.clone());
            }

            Self::ProcessBind { .. } => {}

            Self::ProcessIfReducible { original_expr, then_branch, else_branch, .. } => {
                out.push(original_expr.clone());
                out.push(then_branch.clone());
                out.push(else_branch.clone());
            }

            Self::ProcessMatchOrSpace { space_arg, pattern, default, template, .. } => {
                out.push(space_arg.clone());
                out.push(pattern.clone());
                out.push(default.clone());
                out.push(template.clone());
            }

            Self::ProcessSortTuple { sorted, unsorted, current, comparator, .. } => {
                out.extend(sorted.iter().cloned());
                out.extend(unsorted.iter().cloned());
                out.push(current.clone());
                out.push(comparator.clone());
            }

            Self::ProcessBestCandidate { best, remaining, current, rank_fn, .. } => {
                if let Some(b) = best {
                    out.push(b.clone());
                }
                out.extend(remaining.as_slice().iter().cloned());
                out.push(current.clone());
                out.push(rank_fn.clone());
            }

            Self::ProcessCaseMultiResults { remaining_atoms, cases, collected, .. } => {
                out.extend(remaining_atoms.as_slice().iter().cloned());
                out.push(cases.clone());
                out.extend(collected.iter().cloned());
            }

            Self::ProcessCaseEvalScrutineeResults { remaining_raw, evaluated, cases, .. } => {
                out.extend(remaining_raw.as_slice().iter().cloned());
                out.extend(evaluated.iter().cloned());
                out.push(cases.clone());
            }

            Self::MemoizeResult { .. } => {
                // No V values to collect — only stores a u64 hash key.
            }

            Self::ProcessLetStar { current_pattern, remaining_pairs, body, accumulated_bindings, .. } => {
                out.push(current_pattern.clone());
                for (pattern, value_expr) in remaining_pairs {
                    out.push(pattern.clone());
                    out.push(value_expr.clone());
                }
                out.push(body.clone());
                collect_bindings_values(accumulated_bindings, out);
            }

            Self::ProcessRuleMatchesLazy { coroutine: _, results, .. } => {
                out.extend(results.iter().cloned());
                // Coroutine remaining branches contain (V, GenericBindings<V>) pairs
                // The V values (rhs templates) must be rooted
                // Access is limited since BranchCoroutine fields are private;
                // results vec is the primary root source.
            }

            Self::CompleteSubgoal { .. } => {
                // No V values to collect — only stores a u64 hash key.
            }

            Self::CompleteThunk { .. } => {
                // No V values to collect — only stores a u64 hash key.
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
    use crate::backend::environment::MettaEnvironment;
    use crate::backend::models::{MettaValue, MettaValueFactory, global_factory};

    type TestWorkItem = GenericWorkItem<MettaValue, MettaEnvironment>;
    type TestContinuation = GenericContinuation<MettaValue, MettaEnvironment>;

    fn factory() -> crate::backend::models::GcFactory {
        global_factory()
    }

    fn env() -> MettaEnvironment {
        MettaEnvironment::new(factory())
    }

    #[test]
    fn test_work_item_eval_collects_value() {
        let f = factory();
        let item: TestWorkItem = GenericWorkItem::Eval {
            value: f.long(42),
            env: env(),
            depth: 0,
            is_tail_call: false,
            expected_type: None,
        };
        let mut roots = Vec::new();
        item.collect_values(&mut roots);
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].as_long(), Some(42));
    }

    #[test]
    fn test_work_item_resume_collects_results() {
        let f = factory();
        let item: TestWorkItem = GenericWorkItem::Resume {
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
        let cont: TestContinuation = GenericContinuation::Done;
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert!(roots.is_empty());
    }

    #[test]
    fn test_continuation_collect_sexpr_collects_all() {
        let f = factory();
        let cont: TestContinuation = GenericContinuation::CollectSExpr {
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
        let cont: TestContinuation = GenericContinuation::ProcessIfCondition {
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
        let cont: TestContinuation = GenericContinuation::ProcessLet {
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
        let cont: TestContinuation = GenericContinuation::ProcessBind {
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
        let cont: TestContinuation = GenericContinuation::ProcessMatchSpace {
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
        let cont: TestContinuation = GenericContinuation::ProcessCollapseEvalResults {
            remaining_raw: vec![f.long(1), f.long(2)].into_iter(),
            evaluated: vec![f.long(3)],
            is_bind: false,
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
        let cont: TestContinuation = GenericContinuation::ProcessUnifyPattern1Iter {
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

