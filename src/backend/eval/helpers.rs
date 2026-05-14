//! Helper functions for MeTTa evaluation.
//!
//! This module contains utility functions used throughout the evaluation process,
//! including grounded operation detection, special form dispatch, fuzzy suggestions,
//! binding application, and structural equality checking.
//!
//! Note: Functions that depend on `eval()` (like eval_conjunction, evaluate_grounded_args)
//! remain in mod.rs to avoid circular dependencies.

use std::borrow::Cow;

use tracing::trace;

use crate::backend::models::{Bindings, MettaValue, MettaValueInner};

/// Check if an operation needs re-dispatch through eval_sexpr_step after
/// Cartesian product argument evaluation.
///
/// Uses `matches!()` for compiler-generated jump table — faster than PHF
/// runtime hash+probe on the hot evaluation path.
#[inline(always)]
pub fn needs_special_form_redispatch(op: &str) -> bool {
    matches!(
        op,
        // Higher-order list operations (iterate over elements)
        "map-atom" | "filter-atom" | "foldl-atom"
        // Higher-order tuple operations (iterate via trampoline)
        | "sort-tuple" | "best-candidate"
        // Control flow (lazy branch evaluation)
        | "if" | "if-equal" | "if-reducible" | "case" | "switch" | "switch-minimal" | "switch-internal"
        // Binding forms (special scoping)
        | "let" | "let*" | "unify"
        // Sequencing/continuation forms
        | "chain" | "function" | "return"
        // Pattern/substitution forms
        | "sealed" | "atom-subst" | "match" | "match-or"
        // Error handling (special flow)
        | "catch" | "is-error"
        // Evaluation control
        | "eval" | "capture" | "quote" | "unquote"
        // Space operations that need special handling
        | "collapse" | "collapse-bind" | "amb" | "guard" | "ground-with-bindings" | "freeze-tuple"
        // State operations
        | "new-state" | "get-state" | "change-state!"
        // I/O operations
        | "println!" | "print-alternatives!" | "trace!"
        // Set operations
        | "unique-atom" | "alpha-unique-atom" | "struct-unique-atom" | "union-atom" | "intersection-atom" | "subtraction-atom"
        // Alpha equivalence
        | "=alpha"
        // Type matching (HE stdlib parity)
        | "match-types"
        // Testing/assertion operations
        | "test"
        | "assertEqual" | "assertAlphaEqual"
        | "assertEqualMsg" | "assertAlphaEqualMsg"
        | "assertEqualToResult" | "assertAlphaEqualToResult"
        | "assertEqualToResultMsg" | "assertAlphaEqualToResultMsg"
    )
}

/// Check if an operation is a special form that should be evaluated eagerly
/// when appearing as an argument to other expressions.
///
/// This ensures MeTTa HE semantic alignment: special forms that produce values
/// (like map-atom) are evaluated before being passed to user-defined rules.
#[inline(always)]
pub fn is_eager_special_form(op: &str) -> bool {
    matches!(
        op,
        // Higher-order list operations (produce list values)
        "map-atom" | "filter-atom" | "foldl-atom"
        // Higher-order tuple operations (produce values)
        | "sort-tuple" | "best-candidate"
        // Evaluation control that produces values
        | "eval" | "capture" | "reduce" | "unquote"
        // Space operations that produce values
        | "collapse" | "collapse-bind" | "superpose"
        // State operations that produce values
        | "get-state"
        // Error handling that produces values
        | "catch"
        // Other value-producing special forms
        | "get-metatype" | "validate-atom" | "get-type-space"
        // String operations
        | "repr" | "format-args"
        // Set operations (produce list values)
        | "unique-atom" | "alpha-unique-atom" | "struct-unique-atom" | "union-atom" | "intersection-atom" | "subtraction-atom"
        // Alpha equivalence (produces Bool value)
        | "=alpha"
    )
}

/// Check if an atom name is a grounded operation that should be eagerly evaluated.
#[inline(always)]
pub fn is_grounded_op(name: &str) -> bool {
    matches!(
        name,
        // Basic arithmetic. Plan 1 audit (2026-05-06): `mod` is the bytecode-
        // compiler synonym for `%` (`bytecode/compiler/core.rs:391`); `negate`
        // is the JIT synonym for unary `-` (`bytecode/jit/runtime/call_support.rs:128`).
        // Both must be visible to the tree-walker so cross-tier dispatch
        // is consistent.
        "+" | "-" | "*" | "/" | "%" | "mod" | "negate" | "min" | "max"
        // Math functions (short names)
        | "pow" | "abs" | "floor" | "ceil" | "round" | "sqrt"
        // Math functions (full names from try_eval_builtin)
        | "floor-div"
        | "pow-math" | "sqrt-math" | "abs-math" | "log-math" | "trunc-math"
        | "ceil-math" | "floor-math" | "round-math"
        // Trigonometric functions
        | "sin-math" | "asin-math" | "cos-math" | "acos-math"
        | "tan-math" | "atan-math"
        // Float classification
        | "isnan-math" | "isinf-math"
        // Comparison operations
        | "<" | "<=" | ">" | ">=" | "==" | "!="
        // Boolean operations
        | "not" | "and" | "or" | "xor"
        // Type operations that return concrete values
        | "get-type" | "get-metatype" | "validate-atom" | "get-type-space"
        // Atom/expression manipulation operations (all return immediate values)
        | "car-atom" | "cdr-atom" | "cons-atom" | "decons-atom" | "size-atom"
        | "max-atom" | "min-atom" | "index-atom"
        // Tuple operations (all return immediate values)
        | "tuple-concat" | "tuple-count" | "without" | "element-of"
        | "range" | "reverse-atom" | "flatten-atom" | "zip-atom" | "take-atom" | "drop-atom"
        // PeTTa-compatible tuple helpers (B5 + B9): same semantics as the
        // MeTTaTron names above. Must be listed here so that nested calls
        // (e.g. `(msort (append $a $b))` in lib_pln.metta line 384) trigger
        // pre-evaluation of the inner expression — otherwise the outer call
        // receives an unreduced S-expression and fails to operate.
        | "is-member" | "append" | "length" | "exclude-item" | "msort" | "cut"
        // PeTTa-compatible `progn` and `reduce` are dispatched as special
        // forms in step/sexpr.rs (desugaring to `let` and `eval`), but they
        // also need to appear here so that nested usage in expressions like
        // `(some-fn (progn ...))` triggers pre-evaluation of the progn.
        | "progn" | "reduce"
        // Higher-order tuple operations (iterate via trampoline)
        | "sort-tuple" | "best-candidate"
        // Safe arithmetic utilities
        | "/safe" | "clamp"
        // Set operations — `unique-atom` and `alpha-unique-atom` both use
        // alpha-equivalence (matching MeTTa HE); `struct-unique-atom` uses
        // structural equality (matching PeTTa).
        | "unique-atom" | "alpha-unique-atom" | "struct-unique-atom" | "union-atom"
        | "intersection-atom" | "subtraction-atom"
        // String operations (Workstream X.5a)
        | "stringToChars"
    )
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
    match value.inner_ref() {
        // Apply bindings to variables (atoms starting with $, &, or ')
        // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
        MettaValueInner::Atom(s)
            if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && *s != "&" =>
        {
            match bindings.iter().find(|(name, _)| name == s) {
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
        | MettaValueInner::Empty
        | MettaValueInner::NotReducible => return Cow::Borrowed(value),
        // Regular atoms (not variables)
        MettaValueInner::Atom(_) => return Cow::Borrowed(value),
        // Compound types need iterative processing
        MettaValueInner::SExpr(_)
        | MettaValueInner::Conjunction(_)
        | MettaValueInner::Error(_, _) => {}

        // Spanned: apply bindings to inner value, re-wrap with same span
        MettaValueInner::Spanned(v, span) => {
            let result = apply_bindings(v, bindings);
            return match result {
                Cow::Borrowed(_) => Cow::Borrowed(value),
                Cow::Owned(new_val) => Cow::Owned(MettaValue::Spanned(new_val, **span)),
            };
        }
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
    /// Build an Error from the last 2 results (offending, detail).
    /// HE-bisimilar: slot 1 = offending expression, slot 2 = detail value.
    BuildError(&'a MettaValue),
    /// Re-wrap the last result in Spanned with the given span
    BuildSpanned(&'static crate::ir::Span, &'a MettaValue),
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
                match val.inner_ref() {
                    // Variable substitution
                    MettaValueInner::Atom(s)
                        if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\''))
                            && *s != "&" =>
                    {
                        match bindings.iter().find(|(name, _)| name == s) {
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
                    // Error: HE-bisimilar `(offending, detail)`. Push the
                    // build marker, then process both children — Process is
                    // LIFO, so push detail FIRST (popped second) and offending
                    // LAST (popped first) to keep `offending` on top of the
                    // result stack for BuildError's pop order.
                    MettaValueInner::Error(offending, detail) => {
                        work_stack.push(ApplyBindingsWork::BuildError(val));
                        work_stack.push(ApplyBindingsWork::Process(detail));
                        work_stack.push(ApplyBindingsWork::Process(offending));
                    }
                    // Spanned: process inner value and re-wrap with same span
                    MettaValueInner::Spanned(v, span) => {
                        work_stack.push(ApplyBindingsWork::BuildSpanned(*span, val));
                        work_stack.push(ApplyBindingsWork::Process(v));
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
            ApplyBindingsWork::BuildError(original) => {
                // Process order: offending pushed first onto work_stack
                // (popped FIRST since work_stack is LIFO with offending on top),
                // detail pushed second (popped second). So offending_result
                // lands on result_stack first; detail_result is on top. Pop
                // detail first to recover (offending, detail).
                let (detail, detail_mod) = result_stack
                    .pop()
                    .expect("BuildError should have detail on result stack");
                let (offending, offending_mod) = result_stack
                    .pop()
                    .expect("BuildError should have offending on result stack");

                if offending_mod || detail_mod {
                    result_stack.push((MettaValue::Error(offending, detail), true));
                } else {
                    result_stack.push((original.clone(), false));
                }
            }
            ApplyBindingsWork::BuildSpanned(span, original) => {
                // Pop the inner result and re-wrap in Spanned
                let (inner, modified) = result_stack
                    .pop()
                    .expect("BuildSpanned should have inner result on result stack");

                if modified {
                    result_stack.push((MettaValue::Spanned(inner, *span), true));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::Bindings;
    use crate::ir::{Position, Span};

    fn test_span(start_byte: usize, end_byte: usize) -> Span {
        Span::new(
            Position::new(0, start_byte, start_byte),
            Position::new(0, end_byte, end_byte),
        )
    }

    fn single_binding(name: &'static str, val: MettaValue) -> Bindings {
        let mut b = Bindings::new();
        b.insert(name, val);
        b
    }

    #[test]
    fn test_apply_bindings_preserves_span_on_substitution() {
        // Template: Spanned($x, template_span)
        // Binding: $x → Long(42)
        // Result should be: Spanned(Long(42), template_span)
        let template_span = test_span(5, 7);
        let template = MettaValue::Spanned(MettaValue::Atom("$x".to_string()), template_span);
        let bindings = single_binding("$x", MettaValue::Long(42));

        let result = apply_bindings(&template, &bindings);
        let result = result.into_owned();

        // Result should be Spanned(Long(42), template_span)
        assert!(result.is_spanned(), "result should be Spanned");
        assert_eq!(
            result.span().expect("should have span").start.byte_offset,
            5
        );
        assert_eq!(result.span().expect("should have span").end.byte_offset, 7);
        assert!(result.is_long(), "inner should be Long");
        assert_eq!(result.as_long(), Some(42));
    }

    #[test]
    fn test_apply_bindings_preserves_span_on_sexpr() {
        // Template: Spanned(SExpr([Atom("+"), Atom("$x"), Long(1)]), sexpr_span)
        // Binding: $x → Long(42)
        // Result: Spanned(SExpr([Atom("+"), Long(42), Long(1)]), sexpr_span)
        let sexpr_span = test_span(0, 10);
        let template = MettaValue::Spanned(
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
            sexpr_span,
        );
        let bindings = single_binding("$x", MettaValue::Long(42));

        let result = apply_bindings(&template, &bindings);
        let result = result.into_owned();

        // Outer span should be preserved
        assert!(result.is_spanned());
        assert_eq!(
            result.span().expect("should have span").start.byte_offset,
            0
        );
        assert_eq!(result.span().expect("should have span").end.byte_offset, 10);

        // Inner should be SExpr with substituted value
        if let MettaValueInner::SExpr(items) = result.inner() {
            assert_eq!(items.len(), 3);
            assert_eq!(items[1], MettaValue::Long(42));
        } else {
            panic!("Expected SExpr, got {:?}", result.inner());
        }
    }

    #[test]
    fn test_apply_bindings_no_span_loss_on_no_change() {
        // Template: Spanned(Atom("foo"), span) — no variables, no change
        // Result: Cow::Borrowed (the original Spanned value)
        let span = test_span(0, 3);
        let template = MettaValue::Spanned(MettaValue::Atom("foo".to_string()), span);
        let bindings = Bindings::new();

        let result = apply_bindings(&template, &bindings);
        // With empty bindings, should return Borrowed
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn test_apply_bindings_nested_spanned_children() {
        // Template: Spanned(SExpr([Spanned(Atom("$x"), s1), Spanned(Long(1), s2)]), outer_span)
        // Binding: $x → Long(42)
        // Result: Spanned(SExpr([Long(42), Spanned(Long(1), s2)]), outer_span)
        // The unchanged child Spanned(Long(1), s2) keeps its span via clone.
        let outer_span = test_span(0, 10);
        let s1 = test_span(1, 3);
        let s2 = test_span(4, 5);
        let template = MettaValue::Spanned(
            MettaValue::SExpr(vec![
                MettaValue::Spanned(MettaValue::Atom("$x".to_string()), s1),
                MettaValue::Spanned(MettaValue::Long(1), s2),
            ]),
            outer_span,
        );
        let bindings = single_binding("$x", MettaValue::Long(42));

        let result = apply_bindings(&template, &bindings);
        let result = result.into_owned();

        // Outer span preserved
        assert!(result.is_spanned());
        assert_eq!(result.span().expect("span").start.byte_offset, 0);

        // Inner is SExpr
        if let MettaValueInner::SExpr(items) = result.inner() {
            // First item: $x was substituted → Long(42)
            assert_eq!(items[0].as_long(), Some(42));

            // Second item: unchanged → Spanned(Long(1), s2) preserved
            assert!(items[1].is_spanned(), "unchanged child should keep span");
            assert_eq!(items[1].span().expect("s2").start.byte_offset, 4);
            assert_eq!(items[1].as_long(), Some(1));
        } else {
            panic!("Expected SExpr");
        }
    }
}
