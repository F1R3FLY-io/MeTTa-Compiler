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

use std::collections::VecDeque;
use std::fmt::Debug;

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
pub type GenericEvalResult<V, E = MettaEnvironment> = (Vec<V>, E);

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
    /// Evaluate a value and send result to continuation
    Eval {
        value: V,
        env: E,
        depth: usize,
        cont_id: usize,
        /// If true, this is a tail call - don't increment depth
        is_tail_call: bool,
    },
    /// Resume a continuation with a result
    Resume {
        cont_id: usize,
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
        remaining: VecDeque<V>,
        collected: Vec<GenericEvalResult<V, E>>,
        original_env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing rule match results with generic bindings.
    ProcessRuleMatches {
        remaining_matches: VecDeque<(V, GenericBindings<V>)>,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing TCO grounded operation.
    ProcessGroundedOp {
        state: GenericGroundedState<V>,
        env: E,
        parent_cont: usize,
        depth: usize,
    },

    /// Processing lazy Cartesian product combinations (generic version).
    ProcessCombinations {
        combinations: GenericCartesianProductIter<V>,
        results: Vec<V>,
        pending_rule_matches: VecDeque<(V, GenericBindings<V>)>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing let binding
    ProcessLet {
        pending_values: Option<VecDeque<V>>,
        pattern: V,
        body: V,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Collecting grounded arg evaluation results
    CollectGroundedArg {
        items: Vec<V>,
        grounded_indices: Vec<usize>,
        current_idx: usize,
        evaluated_results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing map-atom iteration
    ProcessMapAtom {
        remaining_elements: VecDeque<V>,
        var_name: String,
        template: V,
        collected_results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing filter-atom iteration
    ProcessFilterAtom {
        current_element: Option<V>,
        remaining_elements: VecDeque<V>,
        var_name: String,
        predicate: V,
        filtered_results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing foldl-atom iteration
    ProcessFoldlAtom {
        remaining_elements: VecDeque<V>,
        acc_var_name: String,
        item_var_name: String,
        operation: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing if condition
    ProcessIfCondition {
        then_branch: V,
        else_branch: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing case atom
    ProcessCaseAtom {
        cases: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing (eval expr)
    ProcessEvalEval {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing (return value)
    ProcessReturn {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing chain expression
    ProcessChainExpr {
        var: V,
        body: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing chain body evaluations
    ProcessChainBody {
        remaining_values: VecDeque<V>,
        var: V,
        body: V,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing function loop
    ProcessFunction {
        iteration_count: usize,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing is-error
    ProcessIsError {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing catch
    ProcessCatch {
        default: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing conjunction
    ProcessConjunction {
        remaining_goals: VecDeque<V>,
        accumulated_results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing unify pattern1
    ProcessUnifyPattern1 {
        pattern2: V,
        success_body: V,
        failure_body: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing unify pattern1 iteration
    ProcessUnifyPattern1Iter {
        remaining_pattern1_results: VecDeque<V>,
        pattern2: V,
        success_body: V,
        failure_body: V,
        all_results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing unify pattern2
    ProcessUnifyPattern2 {
        val1: V,
        pattern2: V,
        success_body: V,
        failure_body: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing unify bodies
    ProcessUnifyBodies {
        remaining_bodies: VecDeque<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing collapse
    ProcessCollapse {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing collapse-bind
    ProcessCollapseBind {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing amb
    ProcessAmb {
        remaining_alts: VecDeque<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing guard
    ProcessGuard {
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing get-atoms
    ProcessGetAtoms {
        space_ref: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing memo table
    ProcessMemoTable {
        memo_ref: V,
        expr: V,
        first_only: bool,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing memo expression
    ProcessMemoExpr {
        memo_handle: MemoHandle,
        expr: V,
        first_only: bool,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing new-memo name
    ProcessNewMemoName {
        name_arg: V,
        size_arg: Option<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing new-memo size
    ProcessNewMemoSize {
        name: String,
        size_arg: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing memo operation
    ProcessMemoOp {
        memo_ref: V,
        is_clear: bool,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing match space
    ProcessMatchSpace {
        space_arg: V,
        pattern: V,
        template: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing match templates
    ProcessMatchTemplates {
        remaining_templates: VecDeque<V>,
        results: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing add-atom space
    ProcessAddAtomSpace {
        space_ref: V,
        atom: V,
        env: E,
        depth: usize,
        parent_cont: usize,
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
        parent_cont: usize,
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
        parent_cont: usize,
    },

    /// Processing get-state
    ProcessGetState {
        state_ref: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing change-state reference
    ProcessChangeStateRef {
        state_ref: V,
        new_value: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing change-state value
    ProcessChangeStateValue {
        state_value: V,
        new_value: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing repr
    ProcessRepr {
        atom: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing format-args string
    ProcessFormatArgsString {
        format_arg: V,
        args_arg: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing format-args args
    ProcessFormatArgsArgs {
        format_str: String,
        args_arg: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing println
    ProcessPrintln {
        atom: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing trace message
    ProcessTraceMessage {
        message: V,
        value_expr: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing trace value
    ProcessTraceValue {
        message_str: String,
        value_expr: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing get-metatype
    ProcessGetMetatype {
        atom: V,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing bind
    ProcessBind {
        token: String,
        env: E,
        depth: usize,
        parent_cont: usize,
    },

    /// Processing case multi-results
    ProcessCaseMultiResults {
        remaining_atoms: VecDeque<V>,
        cases: V,
        collected: Vec<V>,
        env: E,
        depth: usize,
        parent_cont: usize,
    },
}

