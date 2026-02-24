//! Testing and Assert Operations for MeTTa (Generic)
//!
//! Implements MeTTa HE-compatible testing operations with full nondeterministic
//! semantics. All assertion operations:
//!
//! 1. **Evaluate** their expression arguments (collecting all nondeterministic results)
//! 2. **Compare** result multisets (unordered, with multiplicity tracking)
//! 3. **Report** differences as "missed" (expected but absent) and "excessive"
//!    (present but unexpected)
//!
//! ## Operations
//!
//! | Operation | Evaluates | Comparison |
//! |-----------|-----------|------------|
//! | `=alpha` | Neither arg | Alpha-equivalence |
//! | `assertEqual` | Both args | Structural equality multiset |
//! | `assertAlphaEqual` | Both args | Alpha-equiv multiset |
//! | `assertEqualToResult` | First arg only | Structural multiset vs literal |
//! | `assertAlphaEqualToResult` | First arg only | Alpha multiset vs literal |
//! | `assert*Msg` variants | Same as above | Same + custom error message |
//!
//! ## Reference
//!
//! See: `hyperon-experimental/lib/src/metta/runner/stdlib/debug.rs`

use crate::backend::eval::alpha_equiv::atoms_are_alpha_equivalent;
use crate::backend::eval::frame_chain::{maybe_push_frame, FrameLabel};
use crate::backend::eval::trampoline::{
    eval_trampoline_generic, ContextEnv, EvalContext,
};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

use super::step::GenericEvalStep;

/// Dispatch a testing operation by name.
///
/// Called from `generic_sexpr.rs` when a testing/assert operation is encountered.
/// Extracts the operation name from `items[0]` to avoid borrow conflicts.
pub fn eval_testing_op_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    let op = items[0].as_atom().expect("testing_ops dispatch: head must be atom");
    match op {
        "=alpha" => eval_alpha_eq_generic(items, env, ctx),
        "assertEqual" => eval_assert_equal_generic(items, env, ctx),
        "assertAlphaEqual" => eval_assert_alpha_equal_generic(items, env, ctx),
        "assertEqualMsg" => eval_assert_equal_msg_generic(items, env, ctx),
        "assertAlphaEqualMsg" => eval_assert_alpha_equal_msg_generic(items, env, ctx),
        "assertEqualToResult" => eval_assert_equal_to_result_generic(items, env, ctx),
        "assertAlphaEqualToResult" => eval_assert_alpha_equal_to_result_generic(items, env, ctx),
        "assertEqualToResultMsg" => eval_assert_equal_to_result_msg_generic(items, env, ctx),
        "assertAlphaEqualToResultMsg" => {
            eval_assert_alpha_equal_to_result_msg_generic(items, env, ctx)
        }
        _ => unreachable!("eval_testing_op_generic called with unknown op: {}", op),
    }
}

// ============================================================================
// Alpha Equality
// ============================================================================

/// `(=alpha expr1 expr2)` — Check alpha-equivalence, return Bool.
///
/// Does NOT evaluate arguments — compares them as-is (like MeTTa HE).
fn eval_alpha_eq_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 3 {
        let err = ctx.factory().error(
            &format!(
                "=alpha requires exactly 2 arguments, got {}. Usage: (=alpha expr1 expr2)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let result = atoms_are_alpha_equivalent(&items[1], &items[2]);
    GenericEvalStep::Done((vec![ctx.factory().bool(result)], env))
}

// ============================================================================
// assertEqual / assertAlphaEqual
// ============================================================================

/// `(assertEqual actual expected)` — Evaluate both, compare as multisets.
fn eval_assert_equal_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 3 {
        let err = ctx.factory().error(
            &format!(
                "assertEqual requires exactly 2 arguments, got {}. Usage: (assertEqual actual expected)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    // Push frame protecting items across nested trampoline calls.
    // SAFETY: `items` outlives `_frame_guard`.
    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertEqual, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    let (expected_results, env) = eval_trampoline_generic(items[2].clone(), env, ctx);

    drop(_frame_guard);

    match compare_results_multiset(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(diff) => {
            let err = ctx.factory().error(
                &format!(
                    "assertEqual failed: {}",
                    diff
                ),
                ctx.factory().sexpr(items),
            );
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

/// `(assertAlphaEqual actual expected)` — Evaluate both, compare with alpha-equiv.
fn eval_assert_alpha_equal_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 3 {
        let err = ctx.factory().error(
            &format!(
                "assertAlphaEqual requires exactly 2 arguments, got {}. Usage: (assertAlphaEqual actual expected)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    // Push frame protecting items across nested trampoline calls.
    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertAlphaEqual, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    let (expected_results, env) = eval_trampoline_generic(items[2].clone(), env, ctx);

    drop(_frame_guard);

    match compare_results_multiset_alpha(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(diff) => {
            let err = ctx.factory().error(
                &format!(
                    "assertAlphaEqual failed: {}",
                    diff
                ),
                ctx.factory().sexpr(items),
            );
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

// ============================================================================
// assertEqualMsg / assertAlphaEqualMsg
// ============================================================================

/// `(assertEqualMsg actual expected msg)` — Like assertEqual with custom message.
fn eval_assert_equal_msg_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 4 {
        let err = ctx.factory().error(
            &format!(
                "assertEqualMsg requires exactly 3 arguments, got {}. Usage: (assertEqualMsg actual expected message)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertEqual, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    let (expected_results, env) = eval_trampoline_generic(items[2].clone(), env, ctx);

    drop(_frame_guard);

    match compare_results_multiset(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(_diff) => {
            let msg = extract_message(&items[3]);
            let err = ctx.factory().error(&msg, ctx.factory().sexpr(items));
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

/// `(assertAlphaEqualMsg actual expected msg)` — Like assertAlphaEqual with custom message.
fn eval_assert_alpha_equal_msg_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 4 {
        let err = ctx.factory().error(
            &format!(
                "assertAlphaEqualMsg requires exactly 3 arguments, got {}. Usage: (assertAlphaEqualMsg actual expected message)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertAlphaEqual, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    let (expected_results, env) = eval_trampoline_generic(items[2].clone(), env, ctx);

    drop(_frame_guard);

    match compare_results_multiset_alpha(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(_diff) => {
            let msg = extract_message(&items[3]);
            let err = ctx.factory().error(&msg, ctx.factory().sexpr(items));
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

// ============================================================================
// assertEqualToResult / assertAlphaEqualToResult
// ============================================================================

/// `(assertEqualToResult actual expected-results)` — Evaluate first arg only, compare with literal.
fn eval_assert_equal_to_result_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 3 {
        let err = ctx.factory().error(
            &format!(
                "assertEqualToResult requires exactly 2 arguments, got {}. Usage: (assertEqualToResult actual expected-results)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertEqualToResult, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    // expected-results is a list literal — extract its children as expected results
    let expected_results: Vec<C::Value> = match items[2].as_sexpr() {
        Some(children) => children.to_vec(),
        None => vec![items[2].clone()],
    };

    drop(_frame_guard);

    match compare_results_multiset(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(diff) => {
            let err = ctx.factory().error(
                &format!("assertEqualToResult failed: {}", diff),
                ctx.factory().sexpr(items),
            );
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

/// `(assertAlphaEqualToResult actual expected-results)` — Evaluate first, alpha-compare with literal.
fn eval_assert_alpha_equal_to_result_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 3 {
        let err = ctx.factory().error(
            &format!(
                "assertAlphaEqualToResult requires exactly 2 arguments, got {}. Usage: (assertAlphaEqualToResult actual expected-results)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertAlphaEqualToResult, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    // expected-results is a list literal — extract its children as expected results
    let expected_results: Vec<C::Value> = match items[2].as_sexpr() {
        Some(children) => children.to_vec(),
        None => vec![items[2].clone()],
    };

    drop(_frame_guard);

    match compare_results_multiset_alpha(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(diff) => {
            let err = ctx.factory().error(
                &format!("assertAlphaEqualToResult failed: {}", diff),
                ctx.factory().sexpr(items),
            );
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

// ============================================================================
// assertEqualToResultMsg / assertAlphaEqualToResultMsg
// ============================================================================

/// `(assertEqualToResultMsg actual expected-results msg)` — With custom message.
fn eval_assert_equal_to_result_msg_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 4 {
        let err = ctx.factory().error(
            &format!(
                "assertEqualToResultMsg requires exactly 3 arguments, got {}. Usage: (assertEqualToResultMsg actual expected-results message)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertEqualToResult, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    // expected-results is a list literal — extract its children as expected results
    let expected_results: Vec<C::Value> = match items[2].as_sexpr() {
        Some(children) => children.to_vec(),
        None => vec![items[2].clone()],
    };

    drop(_frame_guard);

    match compare_results_multiset(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(_diff) => {
            let msg = extract_message(&items[3]);
            let err = ctx.factory().error(&msg, ctx.factory().sexpr(items));
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

/// `(assertAlphaEqualToResultMsg actual expected-results msg)` — With custom message.
fn eval_assert_alpha_equal_to_result_msg_generic<C: EvalContext>(
    items: Vec<C::Value>,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalStep<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    if items.len() != 4 {
        let err = ctx.factory().error(
            &format!(
                "assertAlphaEqualToResultMsg requires exactly 3 arguments, got {}. Usage: (assertAlphaEqualToResultMsg actual expected-results message)",
                items.len() - 1
            ),
            ctx.factory().sexpr(items),
        );
        return GenericEvalStep::Done((vec![err], env));
    }

    let _frame_guard = unsafe {
        maybe_push_frame::<C>(FrameLabel::AssertAlphaEqualToResult, &items)
    };

    let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
    // expected-results is a list literal — extract its children as expected results
    let expected_results: Vec<C::Value> = match items[2].as_sexpr() {
        Some(children) => children.to_vec(),
        None => vec![items[2].clone()],
    };

    drop(_frame_guard);

    match compare_results_multiset_alpha(&actual_results, &expected_results) {
        None => GenericEvalStep::Done((vec![ctx.factory().unit()], env)),
        Some(_diff) => {
            let msg = extract_message(&items[3]);
            let err = ctx.factory().error(&msg, ctx.factory().sexpr(items));
            GenericEvalStep::Done((vec![err], env))
        }
    }
}

// ============================================================================
// Multiset Comparison Helpers
// ============================================================================

/// Compare two result sets as unordered multisets using structural equality.
///
/// Returns `None` if equal (same elements with same multiplicities),
/// `Some(diff_message)` if different.
///
/// Algorithm: Build count maps for both sides, then report differences.
fn compare_results_multiset<V: MettaValueTrait>(
    actual: &[V],
    expected: &[V],
) -> Option<String> {
    // Quick length check
    if actual.len() != expected.len() {
        return Some(format!(
            "result count mismatch: expected {} results, got {}.\nExpected: [{}]\nGot: [{}]",
            expected.len(),
            actual.len(),
            format_values(expected),
            format_values(actual),
        ));
    }

    // Build count map for expected results
    // Use index-based tracking: for each expected element, track how many remain unmatched
    let mut expected_unmatched: Vec<bool> = vec![true; expected.len()];

    // For each actual result, find a matching unmatched expected result
    let mut actual_unmatched: Vec<bool> = vec![true; actual.len()];

    for (ai, a) in actual.iter().enumerate() {
        for (ei, e) in expected.iter().enumerate() {
            if expected_unmatched[ei] && a == e {
                expected_unmatched[ei] = false;
                actual_unmatched[ai] = false;
                break;
            }
        }
    }

    let missed: Vec<&V> = expected
        .iter()
        .zip(expected_unmatched.iter())
        .filter(|(_, unmatched)| **unmatched)
        .map(|(v, _)| v)
        .collect();

    let excessive: Vec<&V> = actual
        .iter()
        .zip(actual_unmatched.iter())
        .filter(|(_, unmatched)| **unmatched)
        .map(|(v, _)| v)
        .collect();

    if missed.is_empty() && excessive.is_empty() {
        None
    } else {
        let mut msg = format!(
            "Expected: [{}]\nGot: [{}]",
            format_values(expected),
            format_values(actual),
        );
        if !missed.is_empty() {
            msg.push_str(&format!(
                "\nMissed: [{}]",
                missed
                    .iter()
                    .map(|v| v.friendly_repr())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !excessive.is_empty() {
            msg.push_str(&format!(
                "\nExcessive: [{}]",
                excessive
                    .iter()
                    .map(|v| v.friendly_repr())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some(msg)
    }
}

/// Compare two result sets as unordered multisets using alpha-equivalence.
///
/// Same algorithm as `compare_results_multiset` but uses `atoms_are_alpha_equivalent`
/// for element comparison.
fn compare_results_multiset_alpha<V: MettaValueTrait>(
    actual: &[V],
    expected: &[V],
) -> Option<String> {
    if actual.len() != expected.len() {
        return Some(format!(
            "result count mismatch: expected {} results, got {}.\nExpected: [{}]\nGot: [{}]",
            expected.len(),
            actual.len(),
            format_values(expected),
            format_values(actual),
        ));
    }

    let mut expected_unmatched: Vec<bool> = vec![true; expected.len()];
    let mut actual_unmatched: Vec<bool> = vec![true; actual.len()];

    for (ai, a) in actual.iter().enumerate() {
        for (ei, e) in expected.iter().enumerate() {
            if expected_unmatched[ei] && atoms_are_alpha_equivalent(a, e) {
                expected_unmatched[ei] = false;
                actual_unmatched[ai] = false;
                break;
            }
        }
    }

    let missed: Vec<&V> = expected
        .iter()
        .zip(expected_unmatched.iter())
        .filter(|(_, unmatched)| **unmatched)
        .map(|(v, _)| v)
        .collect();

    let excessive: Vec<&V> = actual
        .iter()
        .zip(actual_unmatched.iter())
        .filter(|(_, unmatched)| **unmatched)
        .map(|(v, _)| v)
        .collect();

    if missed.is_empty() && excessive.is_empty() {
        None
    } else {
        let mut msg = format!(
            "Expected: [{}]\nGot: [{}]",
            format_values(expected),
            format_values(actual),
        );
        if !missed.is_empty() {
            msg.push_str(&format!(
                "\nMissed: [{}]",
                missed
                    .iter()
                    .map(|v| v.friendly_repr())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !excessive.is_empty() {
            msg.push_str(&format!(
                "\nExcessive: [{}]",
                excessive
                    .iter()
                    .map(|v| v.friendly_repr())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some(msg)
    }
}

/// Format a slice of values for display in error messages.
fn format_values<V: MettaValueTrait>(values: &[V]) -> String {
    values
        .iter()
        .map(|v| v.friendly_repr())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Extract a message string from a value (atom or string content).
fn extract_message<V: MettaValueTrait>(v: &V) -> String {
    if let Some(s) = v.as_string() {
        s.to_string()
    } else if let Some(s) = v.as_atom() {
        s.to_string()
    } else {
        v.friendly_repr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::gc_allocator::global_factory;
    use crate::backend::models::MettaValueFactory;

    // ======================================================================
    // =alpha tests
    // ======================================================================

    #[test]
    fn test_alpha_eq_simple() {
        let f = global_factory();
        let items = vec![
            f.atom("=alpha"),
            f.sexpr(vec![f.atom("$x"), f.atom("$y")]),
            f.sexpr(vec![f.atom("$a"), f.atom("$b")]),
        ];

        let ctx = crate::backend::eval::trampoline::StaticEvalContext::get();
        let env = crate::backend::eval::trampoline::ContextEnv::<
            crate::backend::eval::trampoline::StaticEvalContext,
        >::default();

        let step = eval_alpha_eq_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].as_bool(), Some(true));
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_alpha_eq_false() {
        let f = global_factory();
        // ($x $x) vs ($a $b) — inconsistent mapping
        let items = vec![
            f.atom("=alpha"),
            f.sexpr(vec![f.atom("$x"), f.atom("$x")]),
            f.sexpr(vec![f.atom("$a"), f.atom("$b")]),
        ];

        let ctx = crate::backend::eval::trampoline::StaticEvalContext::get();
        let env = crate::backend::eval::trampoline::ContextEnv::<
            crate::backend::eval::trampoline::StaticEvalContext,
        >::default();

        let step = eval_alpha_eq_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            assert_eq!(results[0].as_bool(), Some(false));
        } else {
            panic!("expected Done");
        }
    }

    // ======================================================================
    // Multiset comparison tests
    // ======================================================================

    #[test]
    fn test_multiset_equal_same_order() {
        let f = global_factory();
        let a = vec![f.long(1), f.long(2), f.long(3)];
        let b = vec![f.long(1), f.long(2), f.long(3)];
        assert!(compare_results_multiset(&a, &b).is_none());
    }

    #[test]
    fn test_multiset_equal_different_order() {
        let f = global_factory();
        let a = vec![f.long(3), f.long(1), f.long(2)];
        let b = vec![f.long(1), f.long(2), f.long(3)];
        assert!(compare_results_multiset(&a, &b).is_none());
    }

    #[test]
    fn test_multiset_different_elements() {
        let f = global_factory();
        let a = vec![f.long(1), f.long(2)];
        let b = vec![f.long(1), f.long(3)];
        assert!(compare_results_multiset(&a, &b).is_some());
    }

    #[test]
    fn test_multiset_different_length() {
        let f = global_factory();
        let a = vec![f.long(1)];
        let b = vec![f.long(1), f.long(2)];
        assert!(compare_results_multiset(&a, &b).is_some());
    }

    #[test]
    fn test_multiset_with_duplicates() {
        let f = global_factory();
        let a = vec![f.long(1), f.long(1), f.long(2)];
        let b = vec![f.long(2), f.long(1), f.long(1)];
        assert!(compare_results_multiset(&a, &b).is_none());
    }

    #[test]
    fn test_multiset_alpha_with_variables() {
        let f = global_factory();
        let a = vec![f.sexpr(vec![f.atom("$x"), f.long(1)])];
        let b = vec![f.sexpr(vec![f.atom("$y"), f.long(1)])];
        assert!(compare_results_multiset_alpha(&a, &b).is_none());
    }

    #[test]
    fn test_multiset_alpha_different() {
        let f = global_factory();
        let a = vec![f.sexpr(vec![f.atom("$x"), f.atom("$x")])];
        let b = vec![f.sexpr(vec![f.atom("$a"), f.atom("$b")])];
        assert!(compare_results_multiset_alpha(&a, &b).is_some());
    }

    // ======================================================================
    // assertEqualToResult tests (list extraction)
    // ======================================================================

    #[test]
    fn test_assert_equal_to_result_extracts_list_children() {
        // Verify that (assertEqualToResult expr (expected1 expected2)) extracts
        // the children of the second arg, not wrapping the whole thing
        let f = global_factory();
        // Build: (assertEqualToResult 3 (3))
        // The second arg is (3), whose children are [3].
        // The actual result of evaluating literal 3 is [3].
        // So this should pass.
        let items = vec![
            f.atom("assertEqualToResult"),
            f.long(3),
            f.sexpr(vec![f.long(3)]),
        ];

        let ctx = crate::backend::eval::trampoline::StaticEvalContext::get();
        let env = crate::backend::eval::trampoline::ContextEnv::<
            crate::backend::eval::trampoline::StaticEvalContext,
        >::default();

        let step = eval_assert_equal_to_result_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            // Should succeed (unit result)
            assert!(results[0].is_unit(), "expected success (unit), got: {:?}", results[0]);
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_assert_equal_to_result_multiple_expected() {
        let f = global_factory();
        // (assertEqualToResult 1 (1 2)) should fail — evaluating literal 1 gives [1],
        // but expected is [1, 2] (two expected results)
        let items = vec![
            f.atom("assertEqualToResult"),
            f.long(1),
            f.sexpr(vec![f.long(1), f.long(2)]),
        ];

        let ctx = crate::backend::eval::trampoline::StaticEvalContext::get();
        let env = crate::backend::eval::trampoline::ContextEnv::<
            crate::backend::eval::trampoline::StaticEvalContext,
        >::default();

        let step = eval_assert_equal_to_result_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            // Should fail — count mismatch (1 actual vs 2 expected)
            assert!(results[0].is_error(), "expected error due to count mismatch");
        } else {
            panic!("expected Done");
        }
    }

    #[test]
    fn test_assert_alpha_equal_to_result_with_variables() {
        let f = global_factory();
        // (assertAlphaEqualToResult ($x $y) (($a $b)))
        // Evaluating ($x $y) returns [($x $y)] (variables are irreducible)
        // Expected: children of (($a $b)) = [($a $b)]
        // ($x $y) and ($a $b) are alpha-equivalent → should pass
        let items = vec![
            f.atom("assertAlphaEqualToResult"),
            f.sexpr(vec![f.atom("$x"), f.atom("$y")]),
            f.sexpr(vec![f.sexpr(vec![f.atom("$a"), f.atom("$b")])]),
        ];

        let ctx = crate::backend::eval::trampoline::StaticEvalContext::get();
        let env = crate::backend::eval::trampoline::ContextEnv::<
            crate::backend::eval::trampoline::StaticEvalContext,
        >::default();

        let step = eval_assert_alpha_equal_to_result_generic(items, env, &ctx);
        if let GenericEvalStep::Done((results, _)) = step {
            assert!(
                results[0].is_unit(),
                "expected success (unit), got: {}",
                results[0]
            );
        } else {
            panic!("expected Done");
        }
    }
}
