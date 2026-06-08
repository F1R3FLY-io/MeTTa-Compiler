//! Regression tests for Bug-Fix 2026-04: VM locals separation.
//!
//! Locks in the fix for 8 confirmed bugs in the interaction between
//! `compile_let` / `compile_let_star` / `compile_chain` and the
//! nondeterminism-sandboxing barriers `CollapseBegin/End` and
//! `CollapseBindBegin/End`. Before the fix, the chunk pre-allocated
//! `local_count` slots at the bottom of `value_stack`, so `compile_let`'s
//! trailing `Swap; Pop` cleanup moved the body result INTO the local slot
//! instead of above it. The `CollapseEnd` / `CollapseBindEnd` barrier check
//! `value_stack.len() > saved_height` then missed the body result, yielding
//! spurious `[()]` outputs instead of the correct nondeterministic pairs.
//!
//! Fix (commit series starting at Bug-Fix Phase 1): move locals into a
//! dedicated `locals: Vec<V>` vector on the VM, matching the JIT tier's
//! existing `CodegenContext::locals` design. `LoadLocal` / `StoreLocal`
//! (and `Wide` variants) read/write there, so the operand stack is no
//! longer aliased with local slots and barrier checks work correctly.
//!
//! Sanity tests guard against regressions at the top level and in nested
//! let forms (which exercise multiple local slots).

use mettatron::backend::models::MettaValueTrait;
use mettatron::{compile, eval, new_env};

fn eval_last(source: &str) -> Vec<String> {
    let state = compile(source).expect("compile failed");
    let mut env = new_env();
    let mut last: Vec<String> = Vec::new();
    let source_exprs = state.source_snapshot();
    let expr_count = source_exprs.len();
    for (idx, expr) in source_exprs.into_iter().enumerate() {
        let (results, env_after, ..) = eval(expr, env, &state);
        env = env_after;
        if idx == expr_count - 1 {
            last = results
                .iter()
                .map(|r| format!("{}", r.friendly_repr()))
                .collect();
        }
    }
    last
}

// ============================================================================
// Category 1: collapse + let/let*/chain scope barrier bugs
// ============================================================================

/// Bug #1 (Explore agent): `!(collapse (let $x a 42))` returned `[()]`.
/// After Phase 1 fix: `[(42)]` — body result preserved past barrier.
#[test]
fn collapse_let_pushes_body_result() {
    let output = eval_last("!(collapse (let $x a 42))");
    assert_eq!(output, vec!["(42)"]);
}

/// Bug #2: `!(collapse-bind (let $x a 42))` returned `[()]`.
/// After Bucket A commit 2356d93 (decomp accepts `{ }`) + 2ee7467
/// (collapse-bind empty `{ }`): the empty-bindings sidecar shape is now
/// `{ }` per HE-empirical alignment, not the older `(Bindings)`.
#[test]
fn collapse_bind_let_pushes_body_result() {
    let output = eval_last("!(collapse-bind (let $x a 42))");
    assert_eq!(output, vec!["((42 { }))"]);
}

/// Bug #3: `!(collapse (let* (($x a)) 42))` returned `[()]`.
#[test]
fn collapse_let_star_pushes_body_result() {
    let output = eval_last("!(collapse (let* (($x a)) 42))");
    assert_eq!(output, vec!["(42)"]);
}

/// Bug #4: `!(collapse-bind (let* (($x a)) 42))` returned `[()]`.
/// Empty-bindings shape is `{ }` per HE-empirical alignment
/// (Bucket A commits 2356d93 + 2ee7467).
#[test]
fn collapse_bind_let_star_pushes_body_result() {
    let output = eval_last("!(collapse-bind (let* (($x a)) 42))");
    assert_eq!(output, vec!["((42 { }))"]);
}

/// Bug #5: `!(collapse (chain a $x 42))` returned `[()]`.
#[test]
fn collapse_chain_pushes_body_result() {
    let output = eval_last("!(collapse (chain a $x 42))");
    assert_eq!(output, vec!["(42)"]);
}

/// Bug #6: `!(collapse-bind (chain a $x 42))` returned `[()]`.
/// Empty-bindings shape is `{ }` per HE-empirical alignment
/// (Bucket A commits 2356d93 + 2ee7467).
#[test]
fn collapse_bind_chain_pushes_body_result() {
    let output = eval_last("!(collapse-bind (chain a $x 42))");
    assert_eq!(output, vec!["((42 { }))"]);
}

/// Bug #7: `!(collapse (let $x a (superpose (1 2 3))))` returned `[()]`.
/// The superposed results were masked by the local-slot overlap.
/// After fix: all three alternatives emitted.
#[test]
fn collapse_let_superpose_body_pushes_all() {
    let output = eval_last("!(collapse (let $x a (superpose (1 2 3))))");
    assert_eq!(output, vec!["(1 2 3)"]);
}

/// Bug #8: `!(collapse (collapse (let $x a 42)))` returned `[()]`.
/// Both inner and outer barriers missed the result.
#[test]
fn nested_collapse_let_pushes_body_result() {
    let output = eval_last("!(collapse (collapse (let $x a 42)))");
    assert_eq!(output, vec!["((42))"]);
}

// ============================================================================
// Category 2: Sanity — do not regress top-level let or nested lets
// ============================================================================

/// Top-level `(let ...)` must continue producing the body result as always.
#[test]
fn let_at_top_level_still_works() {
    let output = eval_last("!(let $x 1 (+ $x 1))");
    assert_eq!(output, vec!["2"]);
}

/// Nested lets exercise multiple local slots; inner must see outer's binding.
#[test]
fn nested_let_inner_uses_outer() {
    let output = eval_last("!(let $x 1 (let $y 2 (+ $x $y)))");
    assert_eq!(output, vec!["3"]);
}

/// Case inside collapse: the scrutinee-barrier path doesn't use the
/// `len > saved_height` pattern, but we guard here against regressions
/// introduced while fixing neighbouring code. Using a literal-matching
/// arm with wildcard fallback exercises the common pattern used across
/// the test corpus.
#[test]
fn case_in_collapse_pushes_body_result() {
    let output = eval_last("!(collapse (case 1 ((1 42) (_ 0))))");
    assert_eq!(output, vec!["(42)"]);
}
