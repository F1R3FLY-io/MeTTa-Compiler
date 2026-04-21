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

use std::sync::Arc;

use smallvec::SmallVec;

use crate::backend::environment::MettaEnvironment;
use crate::backend::grounded::GroundedState;
use crate::backend::models::{GenericBindings, MemoHandle, MettaValue};
// SpaceHandle was previously used by ProcessAddAtomAtom and ProcessRemoveAtomAtom,
// which are now disabled (see comments on those variants below).

// Import Cartesian product iterator
use super::super::processing::GenericCartesianProductIter;

use super::context::SharedEnv;

/// Per-result bound value: a produced `MettaValue` paired with the
/// variable bindings that were active when it was produced. Mirrors MeTTa
/// HE's `InterpretedAtom = (Stack, Bindings)` — bindings travel with each
/// nondeterministic alternative through the evaluation pipeline.
///
/// For the vast majority of evaluations (all code outside an active
/// `collapse-bind` scope), the bindings are `GenericBindings::Empty` — a
/// cheap inline representation with no heap allocation. Within a
/// `collapse-bind` scope, the bindings reflect the tracked-variable
/// groundings established during that specific branch's rule unification.
pub type BoundValue = (MettaValue, GenericBindings<MettaValue>);

/// Layer C: `SharedBindings = Arc<GenericBindings<MettaValue>>`.
///
/// Carrying-bindings are shared across parallel fork branches and copied
/// into each spawned continuation. Under `Box`, each clone allocates a new
/// `GenericBindings` (O(N) in key count); across mmverify's ~200-deep
/// nested dispatches this creates O(N²) heap churn — the root cause of the
/// 41 GB memory spike and 90× slowdown previously attributed to Stage
/// 1d-revised.
///
/// `Arc` clones are O(1) reference-count bumps. Mutations of the
/// carrying-bindings (fewer than a dozen sites — rotating to a sibling
/// branch's bindings) become `shared = Arc::new(new_bindings)` rather than
/// in-place `*ptr = ...`. Reads/derefs (`&*shared`, `shared.iter()`) are
/// unchanged since `Arc<T>: Deref<Target=T>`.
pub type SharedBindings = Arc<GenericBindings<MettaValue>>;

/// Layer C: cached empty-bindings singleton. Every trampoline continuation
/// and work item that does not carry per-branch bindings clones this value,
/// turning a frequent allocation into a refcount bump. The constant is
/// wrapped in `OnceLock` so initialization is lazy and safe across threads.
#[inline]
pub fn empty_shared_bindings() -> SharedBindings {
    use std::sync::OnceLock;
    static EMPTY: OnceLock<SharedBindings> = OnceLock::new();
    EMPTY
        .get_or_init(|| Arc::new(GenericBindings::new()))
        .clone()
}

/// Evaluation result: (bound_values, environment)
///
/// Each element of the `SmallVec` is a `(value, bindings)` pair: the value
/// produced by a nondeterministic branch and the bindings accumulated
/// along that branch's rule-matching chain. The parallel structure across
/// branches is what enables `collapse-bind` to emit correct per-branch
/// bindings in its output.
///
/// Uses SmallVec<[BoundValue; 2]> to inline up to 2 elements, avoiding
/// heap allocation for the common single-result case (93%+ of evaluations
/// produce 1 result). The environment is Arc-wrapped for O(1) sharing
/// across continuations and work items.
pub type EvalResult = (SmallVec<[BoundValue; 2]>, SharedEnv);

/// Helper: construct a `BoundValue` from a `MettaValue` with empty bindings.
/// Used by the common no-collapse-bind path where every result has trivial
/// (empty) bindings.
#[inline(always)]
pub fn bv(value: MettaValue) -> BoundValue {
    (value, GenericBindings::new())
}

/// Helper: construct a `BoundValue` with specific bindings.
#[inline(always)]
pub fn bv_with(value: MettaValue, bindings: GenericBindings<MettaValue>) -> BoundValue {
    (value, bindings)
}

/// Helper: wrap a `SmallVec<[MettaValue; 2]>` as a `SmallVec<[BoundValue; 2]>`
/// with empty bindings on each element. Convenience for call sites that
/// produce raw values without bindings tracking.
#[inline]
pub fn bvs_from_values(values: SmallVec<[MettaValue; 2]>) -> SmallVec<[BoundValue; 2]> {
    values.into_iter().map(bv).collect()
}

/// Helper: extract just the values (drop bindings) from a bound result set.
/// Used by code paths that don't need per-result bindings.
#[inline]
pub fn values_of(results: &SmallVec<[BoundValue; 2]>) -> SmallVec<[MettaValue; 2]> {
    results.iter().map(|(v, _)| v.clone()).collect()
}

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
        /// Stage 1d-revised: the ambient binding context inherited from the
        /// caller's alternative. Mirrors HE's InterpretedAtom(Stack, Bindings):
        /// each in-flight alternative carries its own bindings, propagated
        /// into sub-evaluations via this field. At Done→Resume, each leaf
        /// result's BoundValue.1 starts as this carrying (then merged with
        /// any step-produced bindings). Default = empty (no ambient).
        carrying_bindings: SharedBindings,
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
        bindings: SharedBindings,
        env: SharedEnv,
        depth: usize,
        is_tail_call: bool,
        expected_type: Option<MettaValue>,
        /// Stage 1d-revised: see `WorkItem::Eval.carrying_bindings`.
        carrying_bindings: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        /// Propagated to child Eval pushes so per-branch bindings flow correctly.
        outer_carrying: SharedBindings,
    },

    /// Processing rule match results with bindings.
    ProcessRuleMatches {
        remaining_matches: std::vec::IntoIter<(MettaValue, GenericBindings<MettaValue>)>,
        results: Vec<BoundValue>,
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
        /// Stage 1c: The match unification bindings for the branch whose RHS
        /// is CURRENTLY being evaluated, composed with the outer ambient
        /// `outer_carrying` (Stage 1d-revised). When the RHS result arrives,
        /// these are merged into every result's BoundValue.1 to establish
        /// per-branch binding provenance. Rotated to `compose(outer_carrying,
        /// next_match_bindings)` before advancing to the next branch.
        current_branch_bindings: SharedBindings,
        /// Stage 1d-revised: ambient bindings from the caller's context
        /// (e.g., CollectSExpr's merged child bindings). Retained so that
        /// when rotating to the next branch we can compose anew with that
        /// branch's match bindings. Empty when no ambient is passed.
        outer_carrying: SharedBindings,
        /// Stage 1c: The tracked-variable set of the innermost active
        /// `collapse-bind` (if any). Used to project composed bindings so
        /// only the caller-relevant variables flow through. `None` when no
        /// collapse-bind is active (99% of evaluations) — zero overhead.
        /// Shared via Arc so parallel workers can see the same projection
        /// set without reading the thread-local.
        tracked_vars_hint: Option<std::sync::Arc<SmallVec<[&'static str; 4]>>>,
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
        results: Vec<BoundValue>,
        /// Environment for evaluation.
        env: SharedEnv,
        /// Evaluation depth.
        depth: usize,
        /// Stage 1c: see `ProcessRuleMatches.current_branch_bindings`.
        current_branch_bindings: SharedBindings,
        /// Stage 1d-revised: see `ProcessRuleMatches.outer_carrying`.
        outer_carrying: SharedBindings,
        /// Stage 1c: see `ProcessRuleMatches.tracked_vars_hint`.
        tracked_vars_hint: Option<std::sync::Arc<SmallVec<[&'static str; 4]>>>,
    },

    /// Processing TCO grounded operation.
    ProcessGroundedOp {
        state: Box<GroundedState<MettaValue>>,
        /// The arg index whose evaluation result is pending.
        pending_arg_idx: usize,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d: accumulated bindings from all evaluated args MERGE'd
        /// together. Grounded ops produce a new ground value whose bindings
        /// = merge of all arg bindings (conflict → empty output result).
        arg_bindings: SharedBindings,
    },

    /// HE-faithful fan-out over nondeterministic arg-eval alternatives.
    ///
    /// Pushed by `ProcessGroundedOp` when an `EvalArg` returns N>1
    /// alternatives. Self-repushes once per alternative (processing one at
    /// a time with its own cloned `GroundedState` carrying a single value
    /// at `pending_arg_idx` + its own composed `arg_bindings`), and
    /// accumulates all branches' outputs into a single `EvalResult` handed
    /// to the outer `Resume`.
    ///
    /// This mirrors MeTTa HE's plan-vector fan-out at grounded-call sites
    /// (`eval_impl` in `interpreter.rs:504-549`): each `(value, bindings)`
    /// alternative is an independent plan item, the grounded op fires once
    /// per substituted single-valued alternative, and outputs from all
    /// alternatives are collected into a flat result list.
    ProcessGroundedOpFanout {
        /// Remaining arg-eval alternatives to process (value +
        /// branch-bindings from that alt's Eval return).
        remaining_alts: std::vec::IntoIter<BoundValue>,
        /// Accumulated outputs across all alternatives processed so far.
        results: Vec<BoundValue>,
        /// Template state BEFORE `pending_arg_idx` was installed. Cloned
        /// per alternative and the single value is installed on the clone.
        template_state: Box<GroundedState<MettaValue>>,
        /// Arg index whose alternatives are being iterated.
        pending_arg_idx: usize,
        /// Prior `arg_bindings` (accumulated from earlier args). Each alt
        /// composes this with its own `alt_bindings` to form that alt's
        /// full binding bag before the grounded op fires.
        prior_arg_bindings: SharedBindings,
        env: SharedEnv,
        depth: usize,
    },

    /// Processing lazy Cartesian product combinations.
    ProcessCombinations {
        combinations: Box<GenericCartesianProductIter<MettaValue>>,
        results: Vec<BoundValue>,
        pending_rule_matches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Phase 2.B: binding-preserving Cartesian-product iteration.
    ///
    /// Used when `CollectSExpr` detects multi-alternative children. Each
    /// yielded combination already carries its composed bindings (with
    /// conflicts pruned inside the iterator). `pending_combo_bindings` is
    /// the current combo's bindings while its rule matches are dispatched.
    ProcessCombinationsBound {
        combinations: Box<crate::backend::eval::processing::ops::GenericCartesianProductBoundIter>,
        results: Vec<BoundValue>,
        pending_rule_matches: Vec<(MettaValue, GenericBindings<MettaValue>)>,
        /// The current combo's composed bindings — threaded to
        /// `dispatch_rule_matches` as `outer_carrying` for RHS evaluation,
        /// and attached to no-match data results.
        pending_combo_bindings: GenericBindings<MettaValue>,
        env: SharedEnv,
        depth: usize,
        /// Ambient bindings from the caller's context. NOTE: these are NOT
        /// the same as `pending_combo_bindings` — they are the outer context
        /// INPUT to CollectSExpr. The iterator already composed them into
        /// each combo's bindings, so this field is retained only for
        /// symmetry with `ProcessCombinations` (unused at dispatch time;
        /// see `dispatch_rule_matches` call sites).
        outer_carrying: SharedBindings,
    },

    /// Processing let binding
    ProcessLet {
        pending_values: Option<Vec<BoundValue>>,
        pattern: MettaValue,
        body: MettaValue,
        /// Outer bindings from an `EvalWithBindings` dispatch. When `Some`,
        /// these are composed with pattern-match bindings and the body is
        /// evaluated via `EvalWithBindings` instead of `apply_bindings`.
        /// This enables O(N) instead of O(N^2) work for nested `let*` chains.
        outer_bindings: Option<SharedBindings>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        evaluated_results: Vec<Vec<BoundValue>>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Collecting results from applicative evaluation of Cartesian product
    /// combinations produced by nondeterministic grounded arg evaluation.
    CollectApplicativeResults {
        remaining: std::vec::IntoIter<MettaValue>,
        /// Stage 1d-revised: per-combination pre-computed arg-bindings,
        /// parallel to `remaining`. Combined with outer_carrying via
        /// compose_outer_inner_generic when dispatching each combo's Eval.
        remaining_bindings: Vec<GenericBindings<MettaValue>>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing map-atom iteration
    ProcessMapAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        template: MettaValue,
        collected_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing filter-atom iteration
    ProcessFilterAtom {
        current_element: Option<MettaValue>,
        remaining_elements: std::vec::IntoIter<MettaValue>,
        var_name: String,
        predicate: MettaValue,
        filtered_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing foldl-atom iteration
    ProcessFoldlAtom {
        remaining_elements: std::vec::IntoIter<MettaValue>,
        acc_var_name: String,
        item_var_name: String,
        operation: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d: accumulated bindings from all consumed iterations
        /// MERGE'd together. Each iteration's eval result carries its own
        /// bindings (from inner rule dispatches) which we merge here.
        /// On conflict, the fold branch emits zero results.
        acc_bindings: SharedBindings,
    },

    /// Processing if condition
    ProcessIfCondition {
        then_branch: MettaValue,
        else_branch: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, the taken branch is evaluated via EvalWithBindings
        /// instead of Eval, avoiding materialization of the untaken branch.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing case atom
    ProcessCaseAtom {
        cases: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, case templates are evaluated via EvalWithBindings
        /// instead of Eval, deferring binding application to the matched arm.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing (eval expr)
    ProcessEvalEval {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing (return value)
    ProcessReturn {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing chain expression
    ProcessChainExpr {
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        /// When present, chain body is evaluated via EvalWithBindings after
        /// composing the chain variable binding with outer_bindings.
        outer_bindings: Option<SharedBindings>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing chain body evaluations
    ProcessChainBody {
        /// Remaining `(value, per-branch bindings)` alternatives from the
        /// chain-expr evaluation. Each alt's bindings are merged into the
        /// body's `outer_bindings` AND `carrying_bindings` when dispatched
        /// so the chain-var substitution carries its originating branch's
        /// binding context through the body evaluation.
        remaining_values: std::vec::IntoIter<BoundValue>,
        var: MettaValue,
        body: MettaValue,
        /// Deferred outer bindings from EvalWithBindings (Phase C).
        outer_bindings: Option<SharedBindings>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing function loop
    ProcessFunction {
        iteration_count: usize,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing is-error
    ProcessIsError {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing catch
    ProcessCatch {
        default: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing conjunction
    ProcessConjunction {
        remaining_goals: std::vec::IntoIter<MettaValue>,
        accumulated_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify pattern1
    ProcessUnifyPattern1 {
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify pattern1 iteration.
    ///
    /// Phase 2 Part A fix (task #64): `remaining_pattern1_results` carries
    /// `BoundValue` so per-pattern1-result bindings (from the pattern1 eval)
    /// compose into both the pattern-2 eval and the success-body eval.
    /// Previously `IntoIter<MettaValue>` stripped per-result bindings,
    /// losing variable unifications established by pattern1.
    ProcessUnifyPattern1Iter {
        remaining_pattern1_results: std::vec::IntoIter<BoundValue>,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        all_results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify pattern2
    ProcessUnifyPattern2 {
        val1: MettaValue,
        pattern2: MettaValue,
        success_body: MettaValue,
        failure_body: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing unify bodies
    ProcessUnifyBodies {
        remaining_bodies: std::vec::IntoIter<MettaValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing collapse
    ProcessCollapse {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing collapse-bind
    ProcessCollapseBind {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        /// Note: collapse-bind opens a fresh binding scope for the inner expr,
        /// so this is mainly retained for downstream propagation symmetry.
        outer_carrying: SharedBindings,
    },

    /// Evaluating individual collapse results before assembling the tuple.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// before wrapping in an S-expression tuple. This mirrors HE's use of `metta`
    /// (the full recursive interpreter) inside `collapse`.
    ProcessCollapseEvalResults {
        /// Remaining unevaluated results (with per-branch bindings) to evaluate.
        /// Each pair is `(raw_value, carrying_bindings)` — the bindings were
        /// active for the nondeterministic branch that produced `raw_value`.
        /// For plain `collapse` (is_bind=false), the bindings are discarded
        /// after evaluation. For `collapse-bind` (is_bind=true), the bindings
        /// are encoded into the `(value (Bindings …))` pair output.
        remaining_raw: std::vec::IntoIter<BoundValue>,
        /// Fully evaluated results collected so far, paired with the bindings
        /// they carry. For collapse-bind, these bindings become the sidecar
        /// `(Bindings …)` in each output pair.
        evaluated: Vec<BoundValue>,
        /// Whether this is for collapse-bind (vs plain collapse)
        is_bind: bool,
        /// Stage 1e: the bindings carried by the raw value CURRENTLY being
        /// re-evaluated. On result arrival, these are merged into each
        /// received eval result's bindings so the (re-)evaluated value
        /// retains its source branch's binding provenance — essential for
        /// `collapse-bind` to emit correct `(Bindings …)` sidecars.
        current_raw_bindings: SharedBindings,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Layer A: tracked variable names from the enclosing `collapse-bind`
        /// frame, captured when `ProcessCollapseBind` pops the scope. Used at
        /// the sidecar encoding site to project each result's bindings to
        /// just the user-visible set (matching HE `Bindings::resolve()` at
        /// the observation point, not earlier during match composition).
        /// None for plain `collapse` or when no free vars were tracked.
        tracked_vars_hint: Option<std::sync::Arc<smallvec::SmallVec<[&'static str; 4]>>>,
    },

    /// Processing amb
    ProcessAmb {
        /// Remaining nondeterministic alternatives still to be dispatched.
        /// Each alternative carries its own per-branch bindings so that
        /// downstream evaluation inherits the alt's binding context via
        /// `carrying_bindings`, mirroring HE's per-plan-item `(atom, bindings)`
        /// dispatch model.
        remaining_alts: std::vec::IntoIter<BoundValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing guard
    ProcessGuard {
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-atoms
    ProcessGetAtoms {
        space_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing memo table
    ProcessMemoTable {
        memo_ref: MettaValue,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing memo expression
    ProcessMemoExpr {
        memo_handle: MemoHandle,
        expr: MettaValue,
        first_only: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing new-memo name
    ProcessNewMemoName {
        name_arg: MettaValue,
        size_arg: Option<MettaValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing new-memo size
    ProcessNewMemoSize {
        name: String,
        size_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing memo operation
    ProcessMemoOp {
        memo_ref: MettaValue,
        is_clear: bool,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing match space
    ProcessMatchSpace {
        space_arg: MettaValue,
        pattern: MettaValue,
        template: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing match templates
    ProcessMatchTemplates {
        remaining_templates: std::vec::IntoIter<MettaValue>,
        results: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing add-atom space
    ProcessAddAtomSpace {
        space_ref: MettaValue,
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-state
    ProcessGetState {
        state_ref: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing change-state reference
    ProcessChangeStateRef {
        state_ref: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing change-state value
    ProcessChangeStateValue {
        state_value: MettaValue,
        new_value: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing repr
    ProcessRepr {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing format-args string
    ProcessFormatArgsString {
        format_arg: MettaValue,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing format-args args
    ProcessFormatArgsArgs {
        format_str: String,
        args_arg: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing println
    ProcessPrintln {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing trace message
    ProcessTraceMessage {
        message: MettaValue,
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing trace value
    ProcessTraceValue {
        value_expr: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing get-metatype
    ProcessGetMetatype {
        atom: MettaValue,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing bind
    ProcessBind {
        token: String,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Processing case multi-results.
    ///
    /// Phase 2 Part A fix (task #63): `remaining_atoms` carries BoundValue
    /// so per-scrutinee-result bindings (from the scrutinee's evaluation)
    /// compose with the outer carrying and flow into the case body eval.
    /// Previously `IntoIter<MettaValue>` stripped per-result bindings,
    /// making variables bound in the scrutinee invisible to the case body.
    ProcessCaseMultiResults {
        remaining_atoms: std::vec::IntoIter<BoundValue>,
        cases: MettaValue,
        collected: Vec<BoundValue>,
        env: SharedEnv,
        depth: usize,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
    },

    /// Evaluating individual scrutinee results for case before pattern matching.
    /// MeTTa HE collapse semantics: fully evaluate each nondeterministic result
    /// from the scrutinee expression before matching against case patterns.
    /// This mirrors HE's `(let $c (collapse $atom) ...)` which invokes the full
    /// interpreter on the scrutinee, ensuring rule applications are completed.
    ProcessCaseEvalScrutineeResults {
        /// Remaining unevaluated scrutinee results to evaluate (paired with
        /// their carrying bindings from nondeterministic branching).
        remaining_raw: std::vec::IntoIter<BoundValue>,
        /// Fully evaluated scrutinee results collected so far.
        evaluated: Vec<BoundValue>,
        /// Case patterns to match against
        cases: MettaValue,
        /// Environment
        env: SharedEnv,
        /// Evaluation depth
        depth: usize,
        /// Per-raw bindings of the value currently being re-evaluated.
        /// Used as the carrying when re-pushing for the next raw value.
        current_raw_bindings: SharedBindings,
        /// Stage 1d-revised: ambient bindings from the caller's context.
        outer_carrying: SharedBindings,
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
        accumulated_bindings: SharedBindings,
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

    /// Collecting evaluated arguments for `freeze-tuple`. After all args
    /// are evaluated, constructs the tuple and marks it as normal form
    /// (via `memoize_normal_form`) instead of re-evaluating — preventing
    /// the trampoline's fixpoint loop from reducing a data tuple whose
    /// head happens to be a reducible expression.
    CollectFreezeArgs {
        /// The argument slots (some already evaluated in-place).
        args: Vec<MettaValue>,
        /// Indices within `args` that need evaluation.
        reducible_indices: Vec<usize>,
        /// Current position in `reducible_indices`.
        current_idx: usize,
        /// Per-reducible-arg evaluation results.
        evaluated_results: Vec<Vec<BoundValue>>,
        env: SharedEnv,
        depth: usize,
        outer_carrying: SharedBindings,
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
                for (v, bindings) in values.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
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
                    for (v, bindings) in vals.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
            }

            Self::ProcessRuleMatches { remaining_matches, results, .. } => {
                for (rhs, bindings) in remaining_matches.as_slice() {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessGroundedOp { state, arg_bindings, .. } => {
                collect_grounded_state_values(state, out);
                collect_bindings_values(arg_bindings, out);
            }

            Self::ProcessGroundedOpFanout {
                remaining_alts,
                results,
                template_state,
                prior_arg_bindings,
                ..
            } => {
                collect_grounded_state_values(template_state, out);
                collect_bindings_values(prior_arg_bindings, out);
                for (v, bindings) in remaining_alts.as_slice() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessCombinations { combinations, results, pending_rule_matches, .. } => {
                collect_cartesian_values(combinations, out);
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (rhs, bindings) in pending_rule_matches {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessCombinationsBound { combinations, results, pending_rule_matches, pending_combo_bindings, .. } => {
                // Iterator inputs: each alternative carries value + bindings
                // (both need GC tracking to survive mark-sweep).
                for input_vec in combinations.inputs() {
                    for (v, bindings) in input_vec.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                // Iterator's outer_carrying also needs tracking.
                collect_bindings_values(combinations.outer_carrying(), out);
                // Current combo's composed bindings (during rule-match dispatch).
                collect_bindings_values(pending_combo_bindings, out);
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (rhs, bindings) in pending_rule_matches {
                    out.push(*rhs);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessLet { pending_values, pattern, body, outer_bindings, results, .. } => {
                if let Some(pending) = pending_values {
                    for (v, bindings) in pending.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
                out.push(*pattern);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    collect_bindings_values(ob, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::CollectGroundedArg { items, evaluated_results, .. } => {
                out.extend(items.iter().copied());
                for result_vec in evaluated_results {
                    for (v, bindings) in result_vec.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
            }

            Self::CollectApplicativeResults { remaining, results, .. } => {
                out.extend(remaining.as_slice().iter().copied());
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessMapAtom { remaining_elements, template, collected_results, .. } => {
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*template);
                for (v, bindings) in collected_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessFilterAtom { current_element, remaining_elements, predicate, filtered_results, .. } => {
                if let Some(elem) = current_element {
                    out.push(*elem);
                }
                out.extend(remaining_elements.as_slice().iter().copied());
                out.push(*predicate);
                for (v, bindings) in filtered_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
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
                for (v, bindings) in remaining_values.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*var);
                out.push(*body);
                if let Some(ref ob) = outer_bindings {
                    for (_, v) in ob.iter() {
                        out.push(*v);
                    }
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessFunction { .. } => {}
            Self::ProcessIsError { .. } => {}

            Self::ProcessCatch { default, .. } => {
                out.push(*default);
            }

            Self::ProcessConjunction { remaining_goals, accumulated_results, .. } => {
                out.extend(remaining_goals.as_slice().iter().copied());
                for (v, bindings) in accumulated_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessUnifyPattern1 { pattern2, success_body, failure_body, .. } => {
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
            }

            Self::ProcessUnifyPattern1Iter {
                remaining_pattern1_results, pattern2, success_body, failure_body, all_results, ..
            } => {
                for (v, bindings) in remaining_pattern1_results.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
                for (v, bindings) in all_results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessUnifyPattern2 { val1, pattern2, success_body, failure_body, .. } => {
                out.push(*val1);
                out.push(*pattern2);
                out.push(*success_body);
                out.push(*failure_body);
            }

            Self::ProcessUnifyBodies { remaining_bodies, results, .. } => {
                out.extend(remaining_bodies.as_slice().iter().copied());
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessCollapse { .. } => {}
            Self::ProcessCollapseBind { .. } => {}

            Self::ProcessCollapseEvalResults { remaining_raw, evaluated, .. } => {
                for (v, bindings) in remaining_raw.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in evaluated.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessAmb { remaining_alts, results, .. } => {
                for (v, bindings) in remaining_alts.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
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
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
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
                for (v, bindings) in remaining_atoms.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                out.push(*cases);
                for (v, bindings) in collected.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::ProcessCaseEvalScrutineeResults { remaining_raw, evaluated, cases, .. } => {
                for (v, bindings) in remaining_raw.as_slice().iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
                for (v, bindings) in evaluated.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
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
                for (v, bindings) in results.iter() {
                    out.push(*v);
                    collect_bindings_values(bindings, out);
                }
            }

            Self::CompleteSubgoal { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::CompleteThunk { .. } => {
                // No MettaValue values to collect — only stores a u64 hash key.
            }

            Self::CollectFreezeArgs { args, evaluated_results, .. } => {
                for v in args.iter() {
                    out.push(*v);
                }
                for results in evaluated_results.iter() {
                    for (v, bindings) in results.iter() {
                        out.push(*v);
                        collect_bindings_values(bindings, out);
                    }
                }
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
            | Self::ProcessGroundedOpFanout { depth, .. }
            | Self::ProcessCombinations { depth, .. }
            | Self::ProcessCombinationsBound { depth, .. }
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
            | Self::CompleteThunk { depth, .. }
            | Self::CollectFreezeArgs { depth, .. } => *depth,
        }
    }

    /// Return a stable static name for this continuation variant.
    /// Used by eval-trace binding-flow instrumentation to label
    /// `ContinuationEnter` / `Emit` events.
    #[cfg(feature = "eval-trace")]
    pub fn discriminant_name(&self) -> &'static str {
        match self {
            Self::Done => "Done",
            Self::CollectSExpr { .. } => "CollectSExpr",
            Self::ProcessRuleMatches { .. } => "ProcessRuleMatches",
            Self::ProcessRuleMatchesLazy { .. } => "ProcessRuleMatchesLazy",
            Self::ProcessGroundedOp { .. } => "ProcessGroundedOp",
            Self::ProcessGroundedOpFanout { .. } => "ProcessGroundedOpFanout",
            Self::ProcessCombinations { .. } => "ProcessCombinations",
            Self::ProcessCombinationsBound { .. } => "ProcessCombinationsBound",
            Self::ProcessLet { .. } => "ProcessLet",
            Self::CollectGroundedArg { .. } => "CollectGroundedArg",
            Self::CollectApplicativeResults { .. } => "CollectApplicativeResults",
            Self::ProcessMapAtom { .. } => "ProcessMapAtom",
            Self::ProcessFilterAtom { .. } => "ProcessFilterAtom",
            Self::ProcessFoldlAtom { .. } => "ProcessFoldlAtom",
            Self::ProcessIfCondition { .. } => "ProcessIfCondition",
            Self::ProcessCaseAtom { .. } => "ProcessCaseAtom",
            Self::ProcessEvalEval { .. } => "ProcessEvalEval",
            Self::ProcessReturn { .. } => "ProcessReturn",
            Self::ProcessChainExpr { .. } => "ProcessChainExpr",
            Self::ProcessChainBody { .. } => "ProcessChainBody",
            Self::ProcessFunction { .. } => "ProcessFunction",
            Self::ProcessIsError { .. } => "ProcessIsError",
            Self::ProcessCatch { .. } => "ProcessCatch",
            Self::ProcessConjunction { .. } => "ProcessConjunction",
            Self::ProcessUnifyPattern1 { .. } => "ProcessUnifyPattern1",
            Self::ProcessUnifyPattern1Iter { .. } => "ProcessUnifyPattern1Iter",
            Self::ProcessUnifyPattern2 { .. } => "ProcessUnifyPattern2",
            Self::ProcessUnifyBodies { .. } => "ProcessUnifyBodies",
            Self::ProcessCollapse { .. } => "ProcessCollapse",
            Self::ProcessCollapseBind { .. } => "ProcessCollapseBind",
            Self::ProcessCollapseEvalResults { .. } => "ProcessCollapseEvalResults",
            Self::ProcessAmb { .. } => "ProcessAmb",
            Self::ProcessGuard { .. } => "ProcessGuard",
            Self::ProcessGetAtoms { .. } => "ProcessGetAtoms",
            Self::ProcessMemoTable { .. } => "ProcessMemoTable",
            Self::ProcessMemoExpr { .. } => "ProcessMemoExpr",
            Self::ProcessNewMemoName { .. } => "ProcessNewMemoName",
            Self::ProcessNewMemoSize { .. } => "ProcessNewMemoSize",
            Self::ProcessMemoOp { .. } => "ProcessMemoOp",
            Self::ProcessBind { .. } => "ProcessBind",
            Self::ProcessMatchSpace { .. } => "ProcessMatchSpace",
            Self::ProcessMatchTemplates { .. } => "ProcessMatchTemplates",
            Self::ProcessAddAtomSpace { .. } => "ProcessAddAtomSpace",
            Self::ProcessRemoveAtomSpace { .. } => "ProcessRemoveAtomSpace",
            Self::ProcessNewState { .. } => "ProcessNewState",
            Self::ProcessGetState { .. } => "ProcessGetState",
            Self::ProcessChangeStateRef { .. } => "ProcessChangeStateRef",
            Self::ProcessChangeStateValue { .. } => "ProcessChangeStateValue",
            Self::ProcessRepr { .. } => "ProcessRepr",
            Self::ProcessFormatArgsString { .. } => "ProcessFormatArgsString",
            Self::ProcessFormatArgsArgs { .. } => "ProcessFormatArgsArgs",
            Self::ProcessPrintln { .. } => "ProcessPrintln",
            Self::ProcessTraceMessage { .. } => "ProcessTraceMessage",
            Self::ProcessTraceValue { .. } => "ProcessTraceValue",
            Self::ProcessGetMetatype { .. } => "ProcessGetMetatype",
            Self::ProcessIfReducible { .. } => "ProcessIfReducible",
            Self::ProcessMatchOrSpace { .. } => "ProcessMatchOrSpace",
            Self::ProcessSortTuple { .. } => "ProcessSortTuple",
            Self::ProcessBestCandidate { .. } => "ProcessBestCandidate",
            Self::ProcessCaseMultiResults { .. } => "ProcessCaseMultiResults",
            Self::ProcessCaseEvalScrutineeResults { .. } => "ProcessCaseEvalScrutineeResults",
            Self::MemoizeResult { .. } => "MemoizeResult",
            Self::ProcessLetStar { .. } => "ProcessLetStar",
            Self::CompleteSubgoal { .. } => "CompleteSubgoal",
            Self::CompleteThunk { .. } => "CompleteThunk",
            Self::CollectFreezeArgs { .. } => "CollectFreezeArgs",
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
            carrying_bindings: empty_shared_bindings(),
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
            result: (
                smallvec![bv(f.long(1)), bv(f.long(2)), bv(f.long(3))],
                env(),
            ),
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
                (smallvec![bv(f.long(30))], env()),
                (smallvec![bv(f.long(40)), bv(f.long(50))], env()),
            ],
            original_env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
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
            outer_carrying: empty_shared_bindings(),
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
            pending_values: Some(vec![bv(f.atom("a")), bv(f.atom("b"))]),
            pattern: f.atom("$x"),
            body: f.atom("body"),
            outer_bindings: None,
            results: vec![bv(f.long(1))],
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
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
            outer_carrying: empty_shared_bindings(),
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
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        assert_eq!(roots.len(), 3);
    }

    #[test]
    fn test_continuation_collapse_eval_results() {
        let f = factory();
        let cont = Continuation::ProcessCollapseEvalResults {
            remaining_raw: vec![bv(f.long(1)), bv(f.long(2))].into_iter(),
            evaluated: vec![bv(f.long(3))],
            is_bind: false,
            current_raw_bindings: empty_shared_bindings(),
            env: env(),
            depth: 0,
            tracked_vars_hint: None,
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
            remaining_pattern1_results: vec![bv(f.long(1))].into_iter(),
            pattern2: f.atom("p2"),
            success_body: f.atom("ok"),
            failure_body: f.atom("fail"),
            all_results: vec![bv(f.long(99))],
            env: env(),
            depth: 0,
            outer_carrying: empty_shared_bindings(),
        };
        let mut roots = Vec::new();
        cont.collect_values(&mut roots);
        // 1 remaining + 1 pattern2 + 1 success + 1 failure + 1 result = 5
        assert_eq!(roots.len(), 5);
    }
}
