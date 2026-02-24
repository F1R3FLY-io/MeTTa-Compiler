//! Generic Step Types for Evaluation
//!
//! These types represent the results of a single evaluation step, parameterized
//! over the value type V and environment type E. This enables the same evaluation
//! logic to work with both heap-allocated (`MettaValue`) and arena-allocated
//! (`MettaValue`) values, using their respective environment types.
//!
//! ## Design Notes
//!
//! - All value fields become generic `V`
//! - All environment fields become generic `E`
//! - Default type parameter `E = Environment` for backward compatibility
//! - Cartesian product iteration is handled via GenericCartesianProductIter in processing
//!
//! ## Type Parameters
//!
//! - `V: MettaValueTrait` - The value type (MettaValue or MettaValue)
//! - `E: Clone` - The environment type (Environment or GenericEnvironment<V>)

use crate::backend::environment::MettaEnvironment;
use crate::backend::grounded::GenericGroundedState;
use crate::backend::models::{GenericBindings, MettaValueTrait};

use super::super::trampoline::GenericEvalResult;
/// Type of memo operation for StartMemoOp
#[derive(Debug, Clone)]
pub enum MemoOpType {
    /// clear-memo! operation
    Clear,
    /// memo-stats operation
    Stats,
}

/// Generic result of a single evaluation step.
///
/// Parameterized over the value type V and environment type E, enabling
/// the same evaluation logic to work with both heap and arena allocation.
///
/// # Type Parameters
///
/// - `V: MettaValueTrait` - The value type (MettaValue or MettaValue)
/// - `E: Clone` - The environment type (defaults to Environment for backward compatibility)
#[derive(Debug)]
pub enum GenericEvalStep<V: MettaValueTrait, E: Clone = MettaEnvironment> {
    /// Evaluation complete, return this result
    Done(GenericEvalResult<V, E>),

    /// Need to evaluate S-expression items (iteratively)
    EvalSExpr {
        items: Vec<V>,
        env: E,
        depth: usize,
    },

    /// Start TCO grounded operation (e.g., +, -, and, or)
    /// This defers evaluation to the trampoline for proper tail call handling.
    ///
    /// ## Zero-Conversion Design
    ///
    /// Uses `GenericGroundedState<V>` to store arguments in their native type,
    /// eliminating conversions between heap and arena types during execution.
    StartGroundedOp {
        state: GenericGroundedState<V>,
        env: E,
        depth: usize,
    },

    /// Start let binding - first evaluates value expression, then pattern matches
    /// and evaluates body. This enables let body to participate in trampoline (TCO).
    StartLetBinding {
        /// Pattern to match against evaluated value
        pattern: V,
        /// Value expression to evaluate first
        value_expr: V,
        /// Body template to instantiate with bindings
        body: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },

    /// Evaluate if branch - condition has been evaluated, now evaluate selected branch.
    /// This enables if branches to participate in trampoline (TCO).
    EvalIfBranch {
        /// Branch expression to evaluate (then or else)
        branch: V,
        /// Environment after condition evaluation
        env: E,
        /// Evaluation depth (preserved for TCO)
        depth: usize,
    },

    /// Evaluate rule matches with UNEVALUATED arguments (lazy evaluation semantics).
    /// This is used when user-defined rules match before argument evaluation.
    /// MeTTa HE uses normal-order (lazy) evaluation for rule arguments.
    ///
    /// ## Zero-Conversion Design
    ///
    /// Matches now store generic values, converted once at rule retrieval.
    EvalRuleMatchesLazy {
        /// Matched rules: (RHS expression, bindings from pattern match)
        /// Both RHS and bindings are in generic type V (converted once at rule retrieval)
        matches: Vec<(V, GenericBindings<V>)>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate grounded arguments before rule matching.
    /// This defers grounded arg evaluation to the trampoline to prevent stack overflow.
    EvalGroundedArgs {
        /// The original S-expression items
        items: Vec<V>,
        /// Indices of arguments that need evaluation (grounded ops)
        grounded_indices: Vec<usize>,
        /// Environment
        env: E,
        /// Depth
        depth: usize,
    },

    /// Start map-atom operation - iterates lazily via trampoline.
    StartMapAtom {
        /// List elements to process
        elements: Vec<V>,
        /// Variable name for substitution (e.g., "$v")
        var_name: String,
        /// Template to evaluate for each element
        template: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start filter-atom operation - iterates lazily via trampoline.
    StartFilterAtom {
        /// List elements to process
        elements: Vec<V>,
        /// Variable name for substitution (e.g., "$v")
        var_name: String,
        /// Predicate to evaluate for each element
        predicate: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start foldl-atom operation - iterates lazily via trampoline.
    StartFoldlAtom {
        /// List elements to process
        elements: Vec<V>,
        /// Initial accumulator value
        init: V,
        /// Accumulator variable name (e.g., "$acc")
        acc_var_name: String,
        /// Item variable name (e.g., "$x")
        item_var_name: String,
        /// Operation template to evaluate for each element
        operation: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate if condition - defers condition evaluation to trampoline.
    EvalIfCondition {
        /// Condition expression to evaluate
        condition: V,
        /// Then branch (evaluated if condition is truthy)
        then_branch: V,
        /// Else branch (evaluated if condition is falsy)
        else_branch: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate case atom expression - defers atom evaluation to trampoline.
    /// For `case`: evaluates atom, then pattern matches.
    EvalCaseAtom {
        /// Atom expression to evaluate
        atom: V,
        /// Cases to match against (pattern-template pairs)
        cases: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Switch pattern matching - does NOT evaluate atom, matches directly.
    /// For `switch`: pattern matches atom as-is without evaluation.
    SwitchAtom {
        /// Atom to pattern match (not evaluated)
        atom: V,
        /// Cases to match against (pattern-template pairs)
        cases: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate (eval expr) - first evaluates argument, then evaluates the result.
    EvalEval {
        /// The argument expression to evaluate first
        arg: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate (return value) - evaluates argument, then wraps in return structure.
    EvalReturn {
        /// The value expression to evaluate
        value: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start (chain expr $var body) evaluation.
    StartChain {
        /// The expression to evaluate first
        expr: V,
        /// The variable pattern to bind each result
        var: V,
        /// The body template to instantiate and evaluate for each binding
        body: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start (function expr) evaluation.
    StartFunction {
        /// The expression to start evaluating
        expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate (is-error expr) - evaluates expression to check if it's an error.
    EvalIsError {
        /// Expression to evaluate and check
        expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start (catch expr default) evaluation.
    StartCatch {
        /// Expression to evaluate
        expr: V,
        /// Default expression to use if expr is error
        default: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start conjunction evaluation (, goal1 goal2 ...).
    StartConjunction {
        /// Goals to evaluate sequentially
        goals: Vec<V>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start unify evaluation (unify pattern1 pattern2 success failure).
    StartUnify {
        /// First pattern to evaluate
        pattern1: V,
        /// Second pattern (may or may not be evaluated depending on first result)
        pattern2: V,
        /// Success body to evaluate on match
        success_body: V,
        /// Failure body to evaluate on no match
        failure_body: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start collapse evaluation - evaluates expr, then collects results into a list.
    StartCollapse {
        /// Expression to evaluate
        expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start collapse-bind evaluation - evaluates expr, collects ALL results.
    StartCollapseBind {
        /// Expression to evaluate
        expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start amb evaluation - evaluates each alternative and collects all results.
    StartAmb {
        /// Alternatives to evaluate
        alternatives: Vec<V>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start guard evaluation - evaluates condition, passes if true, fails if false.
    StartGuard {
        /// Condition to evaluate
        condition: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start get-atoms evaluation - evaluates space ref, returns atoms as superposition.
    StartGetAtoms {
        /// Space reference to evaluate
        space_ref: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start memo evaluation - evaluates memo table, then checks cache and evaluates expr.
    StartMemo {
        /// Memo table reference to evaluate
        memo_ref: V,
        /// Expression to evaluate and cache
        expr: V,
        /// Whether to cache only first result (memo-first vs memo)
        first_only: bool,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start new-memo evaluation - evaluates name and optional size.
    StartNewMemo {
        /// Name argument to evaluate
        name_arg: V,
        /// Optional size argument
        size_arg: Option<V>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start clear-memo! or memo-stats - evaluates memo table reference.
    StartMemoOp {
        /// Memo table reference to evaluate
        memo_ref: V,
        /// Operation type: "clear" or "stats"
        op_type: MemoOpType,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start match evaluation (3-arg syntax: match space pattern template).
    StartMatch {
        /// Space argument to evaluate
        space_arg: V,
        /// Pattern to match against atoms in space
        pattern: V,
        /// Template to instantiate with bindings
        template: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start add-atom evaluation - evaluates space then atom.
    StartAddAtom {
        /// Space reference to evaluate
        space_ref: V,
        /// Atom to add (will be evaluated)
        atom: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start remove-atom evaluation - evaluates space then atom.
    StartRemoveAtom {
        /// Space reference to evaluate
        space_ref: V,
        /// Atom to remove (will be evaluated)
        atom: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start new-state evaluation - evaluates initial value.
    StartNewState {
        /// Initial value expression to evaluate
        initial_value: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start get-state evaluation - evaluates state reference.
    StartGetState {
        /// State reference to evaluate
        state_ref: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start change-state! evaluation - evaluates state ref then new value.
    StartChangeState {
        /// State reference to evaluate
        state_ref: V,
        /// New value to set
        new_value: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start repr evaluation - evaluates atom then converts to string representation.
    StartRepr {
        /// Atom expression to evaluate
        atom: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start format-args evaluation - evaluates format string then args.
    StartFormatArgs {
        /// Format string expression to evaluate
        format_arg: V,
        /// Args expression to evaluate
        args_arg: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start println! evaluation - evaluates atom then prints it.
    StartPrintln {
        /// Atom expression to evaluate
        atom: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start trace! evaluation - evaluates message then value.
    StartTrace {
        /// Message expression to evaluate
        message: V,
        /// Value expression to evaluate
        value_expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start get-metatype evaluation - evaluates atom then returns its meta-type.
    StartGetMetatype {
        /// Atom expression to evaluate
        atom: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start bind! evaluation - evaluates atom expression then registers token.
    StartBind {
        /// Token name to bind (e.g., "&kb")
        token: String,
        /// Atom expression to evaluate
        atom_expr: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Evaluate if-reducible: evaluates expr, checks if it reduced, then evaluates appropriate branch.
    /// `(if-reducible expr then-branch else-branch)` — if expr changes through evaluation,
    /// evaluate then-branch; if expr is irreducible (returns itself), evaluate else-branch.
    EvalIfReducible {
        /// Expression to evaluate and check for reducibility
        expr: V,
        /// Branch evaluated if expr reduces (changes from original)
        then_branch: V,
        /// Branch evaluated if expr is irreducible (unchanged)
        else_branch: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Start match-or evaluation (4-arg: match-or space pattern default template).
    /// Like `match` but returns `default` when no match is found instead of empty.
    StartMatchOr {
        /// Space argument to evaluate
        space_arg: V,
        /// Pattern to match against atoms in space
        pattern: V,
        /// Default expression to evaluate if no matches
        default: V,
        /// Template to instantiate with bindings from matches
        template: V,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },
}


