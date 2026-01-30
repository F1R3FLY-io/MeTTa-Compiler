//! Step Types for Evaluation
//!
//! These types represent the results of a single evaluation step in the
//! trampoline-based evaluator.

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
        /// MettaValue clone is O(1) since it uses Arc internally
        matches: Vec<(MettaValue, Bindings)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate grounded arguments before rule matching.
    /// This defers grounded arg evaluation to the trampoline to prevent stack overflow.
    EvalGroundedArgs {
        /// The original S-expression items
        items: Vec<MettaValue>,
        /// Indices of arguments that need evaluation (grounded ops)
        grounded_indices: Vec<usize>,
        /// Environment
        env: Environment,
        /// Depth
        depth: usize,
    },
    /// Start map-atom operation - iterates lazily via trampoline.
    /// This defers recursive evaluation to the trampoline, preventing stack overflow
    /// for nested map operations (e.g., map inside map).
    StartMapAtom {
        /// List elements to process
        elements: Vec<MettaValue>,
        /// Variable name for substitution (e.g., "$v")
        var_name: String,
        /// Template to evaluate for each element
        template: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start filter-atom operation - iterates lazily via trampoline.
    /// This defers recursive evaluation to the trampoline, preventing stack overflow
    /// for nested filter operations.
    StartFilterAtom {
        /// List elements to process
        elements: Vec<MettaValue>,
        /// Variable name for substitution (e.g., "$v")
        var_name: String,
        /// Predicate to evaluate for each element
        predicate: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start foldl-atom operation - iterates lazily via trampoline.
    /// This defers recursive evaluation to the trampoline, preventing stack overflow
    /// for nested fold operations.
    StartFoldlAtom {
        /// List elements to process
        elements: Vec<MettaValue>,
        /// Initial accumulator value
        init: MettaValue,
        /// Accumulator variable name (e.g., "$acc")
        acc_var_name: String,
        /// Item variable name (e.g., "$x")
        item_var_name: String,
        /// Operation template to evaluate for each element
        operation: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate if condition - defers condition evaluation to trampoline.
    /// This prevents stack overflow when conditions contain deeply nested expressions.
    EvalIfCondition {
        /// Condition expression to evaluate
        condition: MettaValue,
        /// Then branch (evaluated if condition is truthy)
        then_branch: MettaValue,
        /// Else branch (evaluated if condition is falsy)
        else_branch: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate case atom expression - defers atom evaluation to trampoline.
    /// This prevents stack overflow when the atom expression is deeply nested.
    EvalCaseAtom {
        /// Atom expression to evaluate
        atom: MettaValue,
        /// Cases to match against (pattern-template pairs)
        cases: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate switch case result - defers template evaluation to trampoline.
    /// This prevents stack overflow when switch templates are deeply nested.
    EvalSwitchResult {
        /// Template expression to evaluate (already instantiated with bindings)
        template: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate (eval expr) - first evaluates argument, then evaluates the result.
    /// Defers both evaluations to trampoline.
    EvalEval {
        /// The argument expression to evaluate first
        arg: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate (return value) - evaluates argument, then wraps in return structure.
    /// Defers argument evaluation to trampoline.
    EvalReturn {
        /// The value expression to evaluate
        value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start (chain expr $var body) evaluation.
    /// Evaluates expr first, then for each result, evaluates body with bindings.
    StartChain {
        /// The expression to evaluate first
        expr: MettaValue,
        /// The variable pattern to bind each result
        var: MettaValue,
        /// The body template to instantiate and evaluate for each binding
        body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start (function expr) evaluation.
    /// Creates a loop that evaluates until encountering a return value.
    StartFunction {
        /// The expression to start evaluating
        expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate (is-error expr) - evaluates expression to check if it's an error.
    EvalIsError {
        /// Expression to evaluate and check
        expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start (catch expr default) evaluation.
    /// Evaluates expr first, then default if expr returns error.
    StartCatch {
        /// Expression to evaluate
        expr: MettaValue,
        /// Default expression to use if expr is error
        default: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start conjunction evaluation (, goal1 goal2 ...).
    /// Evaluates goals left-to-right with binding threading.
    StartConjunction {
        /// Goals to evaluate sequentially
        goals: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start unify evaluation (unify pattern1 pattern2 success failure).
    /// First evaluates pattern1, then processes results for unification.
    StartUnify {
        /// First pattern to evaluate
        pattern1: MettaValue,
        /// Second pattern (may or may not be evaluated depending on first result)
        pattern2: MettaValue,
        /// Success body to evaluate on match
        success_body: MettaValue,
        /// Failure body to evaluate on no match
        failure_body: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start collapse evaluation - evaluates expr, then collects results into a list.
    StartCollapse {
        /// Expression to evaluate
        expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start collapse-bind evaluation - evaluates expr, collects ALL results (no filtering).
    StartCollapseBind {
        /// Expression to evaluate
        expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start amb evaluation - evaluates each alternative and collects all results.
    StartAmb {
        /// Alternatives to evaluate
        alternatives: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start guard evaluation - evaluates condition, passes if true, fails if false.
    StartGuard {
        /// Condition to evaluate
        condition: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start get-atoms evaluation - evaluates space ref, returns atoms as superposition.
    StartGetAtoms {
        /// Space reference to evaluate
        space_ref: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start memo evaluation - evaluates memo table, then checks cache and evaluates expr.
    StartMemo {
        /// Memo table reference to evaluate
        memo_ref: MettaValue,
        /// Expression to evaluate and cache
        expr: MettaValue,
        /// Whether to cache only first result (memo-first vs memo)
        first_only: bool,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start new-memo evaluation - evaluates name and optional size.
    StartNewMemo {
        /// Name argument to evaluate
        name_arg: MettaValue,
        /// Optional size argument
        size_arg: Option<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start clear-memo! or memo-stats - evaluates memo table reference.
    StartMemoOp {
        /// Memo table reference to evaluate
        memo_ref: MettaValue,
        /// Operation type: "clear" or "stats"
        op_type: MemoOpType,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start match evaluation (3-arg syntax: match space pattern template).
    /// First evaluates space argument, then does pattern matching.
    StartMatch {
        /// Space argument to evaluate
        space_arg: MettaValue,
        /// Pattern to match against atoms in space
        pattern: MettaValue,
        /// Template to instantiate with bindings
        template: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start add-atom evaluation - evaluates space then atom.
    StartAddAtom {
        /// Space reference to evaluate
        space_ref: MettaValue,
        /// Atom to add (will be evaluated)
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start remove-atom evaluation - evaluates space then atom.
    StartRemoveAtom {
        /// Space reference to evaluate
        space_ref: MettaValue,
        /// Atom to remove (will be evaluated)
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start new-state evaluation - evaluates initial value.
    StartNewState {
        /// Initial value expression to evaluate
        initial_value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start get-state evaluation - evaluates state reference.
    StartGetState {
        /// State reference to evaluate
        state_ref: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start change-state! evaluation - evaluates state ref then new value.
    StartChangeState {
        /// State reference to evaluate
        state_ref: MettaValue,
        /// New value to set
        new_value: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start repr evaluation - evaluates atom then converts to string representation.
    StartRepr {
        /// Atom expression to evaluate
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start format-args evaluation - evaluates format string then args.
    StartFormatArgs {
        /// Format string expression to evaluate
        format_arg: MettaValue,
        /// Args expression to evaluate
        args_arg: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start println! evaluation - evaluates atom then prints it.
    StartPrintln {
        /// Atom expression to evaluate
        atom: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start trace! evaluation - evaluates message then value.
    StartTrace {
        /// Message expression to evaluate
        message: MettaValue,
        /// Value expression to evaluate
        value_expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start bind! evaluation - evaluates atom expression then registers token.
    StartBind {
        /// Token name to bind (e.g., "&kb")
        token: String,
        /// Atom expression to evaluate
        atom_expr: MettaValue,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start include evaluation - loads and evaluates a MeTTa file.
    /// Expressions are evaluated iteratively via the trampoline to prevent stack overflow.
    StartInclude {
        /// Expressions to evaluate (after parsing and rule extraction)
        expressions: Vec<MettaValue>,
        /// Previous module path to restore after include completes
        prev_module_path: Option<std::path::PathBuf>,
        /// Content hash for cycle detection cleanup
        content_hash: u64,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start import! evaluation - loads a module and optionally imports specific items.
    /// This defers to eval_include_step for the module loading, then handles selective imports.
    StartImport {
        /// Module path argument to include
        module_arg: MettaValue,
        /// Destination (e.g., "&self" or an alias)
        dest: MettaValue,
        /// Optional selective import: (item_name, optional_alias)
        selective_import: Option<(String, Option<String>)>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Start lookup evaluation (MORK form) - conditional execution based on pattern lookup.
    /// Evaluates the success or failure branch based on whether pattern exists in space.
    StartLookup {
        /// Pattern to search for in space
        pattern: MettaValue,
        /// Success goals (conjunction) to evaluate if pattern found
        success_goals: Vec<MettaValue>,
        /// Failure goals (conjunction) to evaluate if pattern not found
        failure_goals: Vec<MettaValue>,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
    /// Evaluate a list operation argument that needs evaluation first.
    /// Used by car-atom, cdr-atom, size-atom, etc. when their argument is a grounded op.
    /// Example: (car-atom (map-atom ...)) - evaluates map-atom first, then takes car.
    EvalListOpArg {
        /// The operation name (e.g., "car-atom")
        op_name: String,
        /// The full operation items (e.g., [car-atom, (map-atom ...)])
        items: Vec<MettaValue>,
        /// Index of the argument to evaluate (usually 1)
        arg_index: usize,
        /// Environment for evaluation
        env: Environment,
        /// Evaluation depth
        depth: usize,
    },
}

/// Type of memo operation for StartMemoOp
#[derive(Debug, Clone)]
pub enum MemoOpType {
    /// clear-memo! operation
    Clear,
    /// memo-stats operation
    Stats,
}

/// Result of processing collected S-expression results
#[derive(Debug)]
pub enum ProcessedSExpr {
    /// Processing complete, return this result
    Done(EvalResult),
    /// Need to evaluate rule matches
    /// MettaValue clone is O(1) since it uses Arc internally
    EvalRuleMatches {
        matches: Vec<(MettaValue, Bindings)>,
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
