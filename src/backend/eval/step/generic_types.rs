//! Generic Step Types for Evaluation
//!
//! These types represent the results of a single evaluation step, parameterized
//! over the value type V and environment type E. This enables the same evaluation
//! logic to work with both heap-allocated (`MettaValue`) and arena-allocated
//! (`ArenaValue`) values, using their respective environment types.
//!
//! ## Design Notes
//!
//! - All value fields become generic `V`
//! - All environment fields become generic `E`
//! - Default type parameter `E = Environment` for backward compatibility
//! - `CartesianProductIter` stays concrete (complex iterator with internal state)
//!
//! ## Type Parameters
//!
//! - `V: MettaValueTrait` - The value type (MettaValue or ArenaValue)
//! - `E: Clone` - The environment type (Environment or GenericEnvironment<V>)

use crate::backend::environment::Environment;
use crate::backend::grounded::GenericGroundedState;
use crate::backend::models::{Bindings, GenericBindings, MettaValue, MettaValueTrait};

use super::super::trampoline::GenericEvalResult;
use super::super::CartesianProductIter;
use super::MemoOpType;

/// Generic result of a single evaluation step.
///
/// Parameterized over the value type V and environment type E, enabling
/// the same evaluation logic to work with both heap and arena allocation.
///
/// # Type Parameters
///
/// - `V: MettaValueTrait` - The value type (MettaValue or ArenaValue)
/// - `E: Clone` - The environment type (defaults to Environment for backward compatibility)
#[derive(Debug)]
pub enum GenericEvalStep<V: MettaValueTrait, E: Clone = Environment> {
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

    /// Evaluate switch case result - defers template evaluation to trampoline.
    #[allow(dead_code)]
    EvalSwitchResult {
        /// Template expression to evaluate (already instantiated with bindings)
        template: V,
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
}

/// Generic result of processing collected S-expression results.
///
/// Parameterized over the value type V and environment type E, enabling
/// the same evaluation logic to work with both heap and arena allocation.
///
/// # Type Parameters
///
/// - `V: MettaValueTrait` - The value type (MettaValue or ArenaValue)
/// - `E: Clone` - The environment type (defaults to Environment for backward compatibility)
#[derive(Debug)]
#[allow(dead_code)]
pub enum GenericProcessedSExpr<V: MettaValueTrait, E: Clone = Environment> {
    /// Processing complete, return this result
    Done(GenericEvalResult<V, E>),

    /// Need to evaluate rule matches with generic bindings.
    ///
    /// ## Zero-Conversion Design
    ///
    /// Matches store generic values, converted once at rule retrieval.
    EvalRuleMatches {
        /// Matched rules: (RHS expression, bindings)
        /// Both RHS and bindings are in generic type V
        matches: Vec<(V, GenericBindings<V>)>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
        /// Base results to include (evaluated S-expr that didn't match rules)
        base_results: Vec<V>,
    },

    /// Need to lazily process Cartesian product combinations
    /// Note: Uses concrete CartesianProductIter (complex iterator)
    EvalCombinations {
        /// Iterator over combinations
        combinations: CartesianProductIter,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },

    /// Need to re-dispatch through eval_sexpr_step for special form handling.
    RedispatchSExpr {
        /// S-expression items to re-evaluate
        items: Vec<V>,
        /// Environment for evaluation
        env: E,
        /// Evaluation depth
        depth: usize,
    },
}

// ============================================================================
// Type aliases for backward compatibility
// ============================================================================

/// EvalStep specialized for heap-allocated MettaValue with standard Environment
#[allow(dead_code)]
pub type HeapEvalStep = GenericEvalStep<MettaValue, Environment>;

/// ProcessedSExpr specialized for heap-allocated MettaValue with standard Environment
#[allow(dead_code)]
pub type HeapProcessedSExpr = GenericProcessedSExpr<MettaValue, Environment>;

// ============================================================================
// Conversion utilities
// ============================================================================

/// Convert concrete Bindings (with MettaValue) to GenericBindings<MettaValue>.
///
/// This is used when converting from concrete EvalStep/ProcessedSExpr to generic
/// types. Since both store MettaValue, this is essentially a structural copy.
#[allow(dead_code)]
fn bindings_to_generic(bindings: Bindings) -> GenericBindings<MettaValue> {
    let mut result = GenericBindings::new();
    for (name, value) in bindings.iter() {
        result.insert(name.clone(), value.clone());
    }
    result
}

/// Convert a Vec of (MettaValue, Bindings) to Vec of (MettaValue, GenericBindings<MettaValue>).
#[allow(dead_code)]
fn matches_to_generic(
    matches: Vec<(MettaValue, Bindings)>,
) -> Vec<(MettaValue, GenericBindings<MettaValue>)> {
    matches
        .into_iter()
        .map(|(rhs, bindings)| (rhs, bindings_to_generic(bindings)))
        .collect()
}

impl GenericEvalStep<MettaValue> {
    /// Convert from concrete EvalStep to generic HeapEvalStep.
    /// This is a no-op conversion since they share the same underlying type.
    #[inline]
    #[allow(dead_code)]
    pub fn from_concrete(step: super::EvalStep) -> Self {
        match step {
            super::EvalStep::Done(result) => GenericEvalStep::Done(result),
            super::EvalStep::EvalSExpr { items, env, depth } => {
                GenericEvalStep::EvalSExpr { items, env, depth }
            }
            super::EvalStep::StartGroundedOp { state, env, depth } => {
                // Convert GroundedState to GenericGroundedState<MettaValue>
                let generic_state = GenericGroundedState::from_arc(
                    state.op_name.clone(),
                    state.args.clone(),
                );
                GenericEvalStep::StartGroundedOp { state: generic_state, env, depth }
            }
            super::EvalStep::StartLetBinding {
                pattern,
                value_expr,
                body,
                env,
                depth,
            } => GenericEvalStep::StartLetBinding {
                pattern,
                value_expr,
                body,
                env,
                depth,
            },
            super::EvalStep::EvalIfBranch { branch, env, depth } => {
                GenericEvalStep::EvalIfBranch { branch, env, depth }
            }
            super::EvalStep::EvalRuleMatchesLazy { matches, env, depth } => {
                GenericEvalStep::EvalRuleMatchesLazy {
                    matches: matches_to_generic(matches),
                    env,
                    depth,
                }
            }
            super::EvalStep::EvalGroundedArgs {
                items,
                grounded_indices,
                env,
                depth,
            } => GenericEvalStep::EvalGroundedArgs {
                items,
                grounded_indices,
                env,
                depth,
            },
            super::EvalStep::StartMapAtom {
                elements,
                var_name,
                template,
                env,
                depth,
            } => GenericEvalStep::StartMapAtom {
                elements,
                var_name,
                template,
                env,
                depth,
            },
            super::EvalStep::StartFilterAtom {
                elements,
                var_name,
                predicate,
                env,
                depth,
            } => GenericEvalStep::StartFilterAtom {
                elements,
                var_name,
                predicate,
                env,
                depth,
            },
            super::EvalStep::StartFoldlAtom {
                elements,
                init,
                acc_var_name,
                item_var_name,
                operation,
                env,
                depth,
            } => GenericEvalStep::StartFoldlAtom {
                elements,
                init,
                acc_var_name,
                item_var_name,
                operation,
                env,
                depth,
            },
            super::EvalStep::EvalIfCondition {
                condition,
                then_branch,
                else_branch,
                env,
                depth,
            } => GenericEvalStep::EvalIfCondition {
                condition,
                then_branch,
                else_branch,
                env,
                depth,
            },
            super::EvalStep::EvalCaseAtom {
                atom,
                cases,
                env,
                depth,
            } => GenericEvalStep::EvalCaseAtom {
                atom,
                cases,
                env,
                depth,
            },
            super::EvalStep::EvalSwitchResult { template, env, depth } => {
                GenericEvalStep::EvalSwitchResult { template, env, depth }
            }
            super::EvalStep::EvalEval { arg, env, depth } => {
                GenericEvalStep::EvalEval { arg, env, depth }
            }
            super::EvalStep::EvalReturn { value, env, depth } => {
                GenericEvalStep::EvalReturn { value, env, depth }
            }
            super::EvalStep::StartChain {
                expr,
                var,
                body,
                env,
                depth,
            } => GenericEvalStep::StartChain {
                expr,
                var,
                body,
                env,
                depth,
            },
            super::EvalStep::StartFunction { expr, env, depth } => {
                GenericEvalStep::StartFunction { expr, env, depth }
            }
            super::EvalStep::EvalIsError { expr, env, depth } => {
                GenericEvalStep::EvalIsError { expr, env, depth }
            }
            super::EvalStep::StartCatch {
                expr,
                default,
                env,
                depth,
            } => GenericEvalStep::StartCatch {
                expr,
                default,
                env,
                depth,
            },
            super::EvalStep::StartConjunction { goals, env, depth } => {
                GenericEvalStep::StartConjunction { goals, env, depth }
            }
            super::EvalStep::StartUnify {
                pattern1,
                pattern2,
                success_body,
                failure_body,
                env,
                depth,
            } => GenericEvalStep::StartUnify {
                pattern1,
                pattern2,
                success_body,
                failure_body,
                env,
                depth,
            },
            super::EvalStep::StartCollapse { expr, env, depth } => {
                GenericEvalStep::StartCollapse { expr, env, depth }
            }
            super::EvalStep::StartCollapseBind { expr, env, depth } => {
                GenericEvalStep::StartCollapseBind { expr, env, depth }
            }
            super::EvalStep::StartAmb {
                alternatives,
                env,
                depth,
            } => GenericEvalStep::StartAmb {
                alternatives,
                env,
                depth,
            },
            super::EvalStep::StartGuard {
                condition,
                env,
                depth,
            } => GenericEvalStep::StartGuard {
                condition,
                env,
                depth,
            },
            super::EvalStep::StartGetAtoms {
                space_ref,
                env,
                depth,
            } => GenericEvalStep::StartGetAtoms {
                space_ref,
                env,
                depth,
            },
            super::EvalStep::StartMemo {
                memo_ref,
                expr,
                first_only,
                env,
                depth,
            } => GenericEvalStep::StartMemo {
                memo_ref,
                expr,
                first_only,
                env,
                depth,
            },
            super::EvalStep::StartNewMemo {
                name_arg,
                size_arg,
                env,
                depth,
            } => GenericEvalStep::StartNewMemo {
                name_arg,
                size_arg,
                env,
                depth,
            },
            super::EvalStep::StartMemoOp {
                memo_ref,
                op_type,
                env,
                depth,
            } => GenericEvalStep::StartMemoOp {
                memo_ref,
                op_type,
                env,
                depth,
            },
            super::EvalStep::StartMatch {
                space_arg,
                pattern,
                template,
                env,
                depth,
            } => GenericEvalStep::StartMatch {
                space_arg,
                pattern,
                template,
                env,
                depth,
            },
            super::EvalStep::StartAddAtom {
                space_ref,
                atom,
                env,
                depth,
            } => GenericEvalStep::StartAddAtom {
                space_ref,
                atom,
                env,
                depth,
            },
            super::EvalStep::StartRemoveAtom {
                space_ref,
                atom,
                env,
                depth,
            } => GenericEvalStep::StartRemoveAtom {
                space_ref,
                atom,
                env,
                depth,
            },
            super::EvalStep::StartNewState {
                initial_value,
                env,
                depth,
            } => GenericEvalStep::StartNewState {
                initial_value,
                env,
                depth,
            },
            super::EvalStep::StartGetState {
                state_ref,
                env,
                depth,
            } => GenericEvalStep::StartGetState {
                state_ref,
                env,
                depth,
            },
            super::EvalStep::StartChangeState {
                state_ref,
                new_value,
                env,
                depth,
            } => GenericEvalStep::StartChangeState {
                state_ref,
                new_value,
                env,
                depth,
            },
            super::EvalStep::StartRepr { atom, env, depth } => {
                GenericEvalStep::StartRepr { atom, env, depth }
            }
            super::EvalStep::StartFormatArgs {
                format_arg,
                args_arg,
                env,
                depth,
            } => GenericEvalStep::StartFormatArgs {
                format_arg,
                args_arg,
                env,
                depth,
            },
            super::EvalStep::StartPrintln { atom, env, depth } => {
                GenericEvalStep::StartPrintln { atom, env, depth }
            }
            super::EvalStep::StartTrace {
                message,
                value_expr,
                env,
                depth,
            } => GenericEvalStep::StartTrace {
                message,
                value_expr,
                env,
                depth,
            },
            super::EvalStep::StartGetMetatype { atom, env, depth } => {
                GenericEvalStep::StartGetMetatype { atom, env, depth }
            }
            super::EvalStep::StartBind {
                token,
                atom_expr,
                env,
                depth,
            } => GenericEvalStep::StartBind {
                token,
                atom_expr,
                env,
                depth,
            },
        }
    }
}

impl GenericProcessedSExpr<MettaValue> {
    /// Convert from concrete ProcessedSExpr to generic HeapProcessedSExpr.
    /// This is a no-op conversion since they share the same underlying type.
    #[inline]
    #[allow(dead_code)]
    pub fn from_concrete(processed: super::ProcessedSExpr) -> Self {
        match processed {
            super::ProcessedSExpr::Done(result) => GenericProcessedSExpr::Done(result),
            super::ProcessedSExpr::EvalRuleMatches {
                matches,
                env,
                depth,
                base_results,
            } => GenericProcessedSExpr::EvalRuleMatches {
                matches: matches_to_generic(matches),
                env,
                depth,
                base_results,
            },
            super::ProcessedSExpr::EvalCombinations {
                combinations,
                env,
                depth,
            } => GenericProcessedSExpr::EvalCombinations {
                combinations,
                env,
                depth,
            },
            super::ProcessedSExpr::RedispatchSExpr { items, env, depth } => {
                GenericProcessedSExpr::RedispatchSExpr { items, env, depth }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heap_eval_step_size() {
        let size = std::mem::size_of::<HeapEvalStep>();
        // EvalStep should be reasonably sized
        assert!(size < 512, "HeapEvalStep is unexpectedly large: {} bytes", size);
    }

    #[test]
    fn test_heap_processed_sexpr_size() {
        let size = std::mem::size_of::<HeapProcessedSExpr>();
        // ProcessedSExpr should be reasonably sized
        assert!(
            size < 512,
            "HeapProcessedSExpr is unexpectedly large: {} bytes",
            size
        );
    }
}
