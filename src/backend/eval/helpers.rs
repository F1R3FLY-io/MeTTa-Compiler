//! Helper functions for MeTTa evaluation.
//!
//! This module contains utility functions used throughout the evaluation process,
//! including grounded operation detection, special form dispatch, fuzzy suggestions,
//! binding application, and structural equality checking.
//!
//! Note: Functions that depend on `eval()` (like eval_conjunction, evaluate_grounded_args)
//! remain in mod.rs to avoid circular dependencies.

use std::borrow::Cow;

use phf::phf_set;
use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, MettaValueInner};

/// Grounded operations that should be evaluated eagerly (before pattern matching).
/// Uses compile-time perfect hash for O(1) lookup.
static GROUNDED_OPS: phf::Set<&'static str> = phf_set! {
    // Basic arithmetic
    "+", "-", "*", "/", "%",
    // Math functions (short names)
    "pow", "abs", "floor", "ceil", "round", "sqrt",
    // Math functions (full names from try_eval_builtin)
    "floor-div",
    "pow-math", "sqrt-math", "abs-math", "log-math", "trunc-math",
    "ceil-math", "floor-math", "round-math",
    // Trigonometric functions
    "sin-math", "asin-math", "cos-math", "acos-math",
    "tan-math", "atan-math",
    // Float classification
    "isnan-math", "isinf-math",
    // Comparison operations
    "<", "<=", ">", ">=", "==", "!=",
    // Boolean operations
    "not", "and", "or",
    // Type operations that return concrete values
    "get-type", "get-metatype",
    // Atom/expression manipulation operations (all return immediate values)
    "car-atom", "cdr-atom", "cons-atom", "decons-atom", "size-atom",
    "max-atom", "min-atom", "index-atom",
};

/// Set of operations that need re-dispatch through eval_sexpr_step after
/// Cartesian product argument evaluation. Uses compile-time perfect hash
/// for O(1) lookup.
///
/// These are special forms that:
/// 1. Have dedicated dispatch in eval_sexpr_step
/// 2. Are NOT already handled by try_eval_builtin (arithmetic ops)
/// 3. Need special argument handling (lazy args, iteration, etc.)
static SPECIAL_FORMS_REDISPATCH: phf::Set<&'static str> = phf_set! {
    // Higher-order list operations (iterate over elements)
    "map-atom", "filter-atom", "foldl-atom",
    // Control flow (lazy branch evaluation)
    "if", "if-equal", "case", "switch", "switch-minimal", "switch-internal",
    // Binding forms (special scoping)
    "let", "let*", "unify",
    // Sequencing/continuation forms
    "chain", "function", "return",
    // Pattern/substitution forms
    "sealed", "atom-subst", "match",
    // Error handling (special flow)
    "catch", "is-error",
    // Evaluation control
    "eval", "quote", "unquote",
    // Space operations that need special handling
    "collapse", "collapse-bind", "amb", "guard",
    // State operations
    "new-state", "get-state", "change-state!",
    // I/O operations
    "println!", "trace!",
    // Set operations
    "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
    // Alpha equivalence
    "=alpha",
    // Testing/assertion operations
    "assertEqual", "assertAlphaEqual",
    "assertEqualMsg", "assertAlphaEqualMsg",
    "assertEqualToResult", "assertAlphaEqualToResult",
    "assertEqualToResultMsg", "assertAlphaEqualToResultMsg",
};

/// Special forms that should be evaluated BEFORE being passed to user-defined rules.
/// These are special forms that produce values and need eager evaluation when used
/// as arguments to other expressions.
///
/// This is critical for MeTTa HE semantic alignment. In MeTTa HE, map-atom is a
/// regular rule that gets evaluated through normal rule application. In MeTTaTron,
/// it's a special form. To maintain semantic equivalence, we need to evaluate these
/// special forms eagerly when they appear as arguments.
///
/// Example: For `(get-expr-size (map-atom (a b) $v ($v x)))`:
/// - MeTTa HE: map-atom is a rule, evaluated as part of normal rule application
/// - MeTTaTron: Without eager evaluation, map-atom would be passed unevaluated
///   to get-expr-size, causing semantic mismatch
static EAGER_SPECIAL_FORMS: phf::Set<&'static str> = phf_set! {
    // Higher-order list operations (produce list values)
    "map-atom", "filter-atom", "foldl-atom",
    // Evaluation control that produces values
    "eval", "unquote",
    // Space operations that produce values
    "collapse", "collapse-bind", "superpose",
    // State operations that produce values
    "get-state",
    // Error handling that produces values
    "catch",
    // Other value-producing special forms
    "get-metatype",
    // String operations
    "repr", "format-args",
    // Set operations (produce list values)
    "unique-atom", "union-atom", "intersection-atom", "subtraction-atom",
    // Alpha equivalence (produces Bool value)
    "=alpha",
};

/// Check if an operation needs re-dispatch through eval_sexpr_step after
/// Cartesian product argument evaluation.
///
/// Inlined to eliminate function call overhead - compiles to just the phf hash lookup.
#[inline(always)]
pub fn needs_special_form_redispatch(op: &str) -> bool {
    SPECIAL_FORMS_REDISPATCH.contains(op)
}

/// Check if an operation is a special form that should be evaluated eagerly
/// when appearing as an argument to other expressions.
///
/// This ensures MeTTa HE semantic alignment: special forms that produce values
/// (like map-atom) are evaluated before being passed to user-defined rules.
#[inline(always)]
pub fn is_eager_special_form(op: &str) -> bool {
    EAGER_SPECIAL_FORMS.contains(op)
}

/// Check if an atom name is a grounded operation that should be eagerly evaluated.
///
/// Inlined to eliminate function call overhead - compiles to just the phf hash lookup.
#[inline(always)]
pub fn is_grounded_op(name: &str) -> bool {
    GROUNDED_OPS.contains(name)
}


/// Apply variable bindings to a value
///
/// This is made public to support optimized match operations in Environment
///
/// Uses Cow<'a, MettaValue> to avoid cloning when no substitution is needed.
/// Returns Cow::Borrowed(value) when the expression contains no variables bound in `bindings`.
/// Returns Cow::Owned(new_value) only when actual substitution occurred.
///
/// # Implementation Note
///
/// This function uses an explicit work stack instead of recursion to avoid
/// stack overflow on deeply nested S-expressions. This is critical for:
/// - Async evaluation (Tokio workers have smaller stacks ~2MB)
/// - Deeply nested data structures common in knowledge graphs
/// - Processing before trampoline's MAX_EVAL_DEPTH check runs
pub fn apply_bindings<'a>(value: &'a MettaValue, bindings: &Bindings) -> Cow<'a, MettaValue> {
    trace!(target: "mettatron::backend::eval::apply_bindings", ?value, ?bindings);

    // Fast path: empty bindings means no substitutions possible
    if bindings.is_empty() {
        return Cow::Borrowed(value);
    }

    // For simple cases without nesting, use fast path
    match value.inner() {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValueInner::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && *s != "&" =>
        {
            match bindings.iter().find(|(name, _)| name.as_str() == *s) {
                Some((_name, val)) => return Cow::Owned(val.clone()),
                None => return Cow::Borrowed(value),
            }
        }
        // Non-compound types don't need substitution
        MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::Bool(_)
        | MettaValueInner::String(_)
        | MettaValueInner::Unit
        | MettaValueInner::Space(_)
        | MettaValueInner::State(_)
        | MettaValueInner::Type(_)
        | MettaValueInner::Quoted(_)
        | MettaValueInner::Memo(_)
        | MettaValueInner::Empty => return Cow::Borrowed(value),
        // Regular atoms (not variables)
        MettaValueInner::Atom(_) => return Cow::Borrowed(value),
        // Compound types need iterative processing
        MettaValueInner::SExpr(_)
        | MettaValueInner::Conjunction(_)
        | MettaValueInner::Error(_, _) => {}
    }

    // Iterative implementation using explicit work stack
    apply_bindings_iterative(value, bindings)
}

/// Work item for iterative apply_bindings
#[derive(Clone)]
enum ApplyBindingsWork<'a> {
    /// Process a value - may push more work
    Process(&'a MettaValue),
    /// Build an SExpr from the last N results
    BuildSExpr(usize, &'a MettaValue),
    /// Build a Conjunction from the last N results
    BuildConjunction(usize, &'a MettaValue),
    /// Build an Error from the last result
    BuildError(String, &'a MettaValue),
}

/// Iterative implementation of apply_bindings using explicit work stack.
///
/// This avoids recursion to prevent stack overflow on deeply nested structures.
fn apply_bindings_iterative<'a>(value: &'a MettaValue, bindings: &Bindings) -> Cow<'a, MettaValue> {
    // Work stack: items to process
    let mut work_stack: Vec<ApplyBindingsWork<'a>> = Vec::with_capacity(32);
    // Result stack: processed results (MettaValue, was_modified)
    let mut result_stack: Vec<(MettaValue, bool)> = Vec::with_capacity(32);

    work_stack.push(ApplyBindingsWork::Process(value));

    while let Some(work) = work_stack.pop() {
        match work {
            ApplyBindingsWork::Process(val) => {
                match val.inner() {
                    // Variable substitution
                    MettaValueInner::Atom(s)
                        if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\''))
                            && *s != "&" =>
                    {
                        match bindings.iter().find(|(name, _)| name.as_str() == *s) {
                            Some((_name, bound_val)) => {
                                result_stack.push((bound_val.clone(), true));
                            }
                            None => {
                                result_stack.push((val.clone(), false));
                            }
                        }
                    }
                    // S-expression: push build marker, then push children in reverse order
                    MettaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push((val.clone(), false));
                        } else {
                            // Push build marker first (processed last)
                            work_stack.push(ApplyBindingsWork::BuildSExpr(items.len(), val));
                            // Push children in reverse order so first child is processed first
                            for item in items.iter().rev() {
                                work_stack.push(ApplyBindingsWork::Process(item));
                            }
                        }
                    }
                    // Conjunction: similar to SExpr
                    MettaValueInner::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push((val.clone(), false));
                        } else {
                            work_stack.push(ApplyBindingsWork::BuildConjunction(goals.len(), val));
                            for goal in goals.iter().rev() {
                                work_stack.push(ApplyBindingsWork::Process(goal));
                            }
                        }
                    }
                    // Error: push build marker, then push details
                    MettaValueInner::Error(msg, details) => {
                        work_stack.push(ApplyBindingsWork::BuildError(msg.to_string(), val));
                        work_stack.push(ApplyBindingsWork::Process(details));
                    }
                    // All other types: no substitution needed
                    _ => {
                        result_stack.push((val.clone(), false));
                    }
                }
            }
            ApplyBindingsWork::BuildSExpr(count, original) => {
                // Pop `count` results and build SExpr
                let start = result_stack.len() - count;
                let children: Vec<(MettaValue, bool)> = result_stack.drain(start..).collect();

                let any_modified = children.iter().any(|(_, modified)| *modified);
                if any_modified {
                    let new_items: Vec<MettaValue> = children.into_iter().map(|(v, _)| v).collect();
                    result_stack.push((MettaValue::SExpr(new_items), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
            ApplyBindingsWork::BuildConjunction(count, original) => {
                let start = result_stack.len() - count;
                let children: Vec<(MettaValue, bool)> = result_stack.drain(start..).collect();

                let any_modified = children.iter().any(|(_, modified)| *modified);
                if any_modified {
                    let new_goals: Vec<MettaValue> = children.into_iter().map(|(v, _)| v).collect();
                    result_stack.push((MettaValue::Conjunction(new_goals), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
            ApplyBindingsWork::BuildError(msg, original) => {
                // Pop the details result
                let (details, modified) = result_stack
                    .pop()
                    .expect("BuildError should have details on result stack");

                if modified {
                    result_stack.push((MettaValue::Error(msg, details), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
        }
    }

    // Final result should be on the stack
    debug_assert_eq!(
        result_stack.len(),
        1,
        "apply_bindings should produce exactly one result"
    );
    let (result, modified) = result_stack
        .pop()
        .expect("Result stack should not be empty");

    if modified {
        Cow::Owned(result)
    } else {
        Cow::Borrowed(value)
    }
}


