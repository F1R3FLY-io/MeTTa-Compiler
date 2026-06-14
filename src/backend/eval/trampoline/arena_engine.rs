//! Arena-based Trampoline Engine
//!
//! This module provides zero-conversion arena evaluation using the global
//! slab allocator via `GcFactory`.
//!
//! ## Global Slab Allocator Model
//!
//! All allocations go through the process-wide `SlabAllocator` via `GcFactory`.
//! There is no dual-arena split — the slab's snapshot-based mark-sweep GC
//! handles reclamation of unreachable values.
//!
//! ## Zero-Conversion Pipeline
//!
//! 1. `compile()` compiles directly to `MettaState` with `MettaValue`
//! 2. `eval_trampoline()` evaluates `MettaValue` using `SessionContext`
//! 3. Results are `Vec<MettaValue>` — no conversion needed

// Production code threads the GC-migration seam `active_factory()` /
// `ActiveFactory`; the `#[cfg(test)]` module below imports `global_factory`
// directly for its concrete test fixtures.
use crate::backend::models::{active_factory, ActiveFactory, MettaState, MettaValue};

#[cfg(feature = "trace")]
use std::sync::Arc;

use super::context::MettaEnvironment;
use super::session_context::SessionContext;
use super::types::EvalResult;

/// Zero-conversion arena evaluation using the global slab allocator.
///
/// Evaluates `MettaValue` using the unified trampoline engine with
/// `SessionContext` for GC-backed allocation.
#[inline]
pub fn eval_trampoline(value: MettaValue, env: MettaEnvironment, state: &MettaState) -> EvalResult {
    let ctx = SessionContext::new(state);
    super::eval_loop::eval_trampoline(value, env, &ctx)
}

/// Zero-conversion arena evaluation with optional trace collector.
#[cfg(feature = "trace")]
#[inline]
pub fn eval_trampoline_with_trace(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
    collector: &Arc<crate::backend::trace::TraceCollector>,
) -> EvalResult {
    let ctx = SessionContext::new(state).with_trace_collector(Arc::clone(collector));
    super::eval_loop::eval_trampoline(value, env, &ctx)
}

/// Check if arena mode is available.
///
/// Returns `true` — arena evaluation is always available.
#[inline]
pub fn is_arena_mode_available() -> bool {
    true
}

/// Get the global factory for creating `MettaValue`.
///
/// Returns the active factory (GC-migration seam, currently the `GcFactory`
/// backed by the process-wide `SlabAllocator`).
#[inline]
pub fn get_static_factory() -> ActiveFactory {
    active_factory()
}

/// Create a new `MettaEnvironment` for session-based evaluation.
///
/// The environment uses the global `GcFactory` for all allocations.
/// Z.A.6a (2026-05-12): the new environment is pre-populated with
/// HE-equivalent math constants `PI` and `EXP` registered as tokens
/// that resolve to `Float(std::f64::consts::{PI, E})` respectively.
/// HE registers these in `lib/src/metta/runner/stdlib/math.rs`.
///
/// Plan Phase F (2026-05-20): MeTTaTron's corelib is now entirely native
/// Rust — no MeTTa source file is involved. Built-in helpers
/// (`if-decons-expr`, `if-error`, `return-on-error`, `assertIncludes`,
/// `noreduce-eq`) dispatch at the `'special_forms` arm in
/// `eval/step/sexpr.rs` before rule lookup. Built-in type declarations
/// (ErrorDescription, BadType, BadArgType, IncorrectNumberOfArguments)
/// are registered via `MettaEnvironment::register_corelib_types()` here.
/// All paths use the iterative trampoline; no direct Rust recursion
/// per [[feedback-stack-safety-mandate]].
#[inline]
pub fn new_env() -> MettaEnvironment {
    // Plan Phase F (2026-05-20): MeTTaTron's corelib has no MeTTa source
    // file — all built-in type metadata is registered natively here, and
    // built-in helper rules (if-decons-expr, if-error, return-on-error,
    // assertIncludes, noreduce-eq) are dispatched at the
    // `'special_forms` arm in `eval/step/sexpr.rs` before rule lookup.
    // Per [[feedback-stack-safety-mandate]], the helper desugars use the
    // existing iterative trampoline; no direct Rust recursion.
    let mut env = MettaEnvironment::new(active_factory());
    env.register_corelib_types();
    let f = active_factory();
    env.register_token(
        "PI",
        crate::backend::models::MettaValueFactory::float(&f, std::f64::consts::PI),
    );
    env.register_token(
        "EXP",
        crate::backend::models::MettaValueFactory::float(&f, std::f64::consts::E),
    );
    // Plan Phase J.2 (2026-05-20): `&rng` global RandomGenerator handle.
    // HE's stdlib pre-binds `&rng` to a process-wide seeded generator.
    // Deterministic seed (0) so bisim fixtures are reproducible.
    env.register_token(
        "&rng",
        crate::backend::grounded::random::create_seeded_generator(0, &f),
    );
    // Phase I (2026-05-20): `&shared` pre-bound named space for the
    // T08 multi-runner fixtures. In single-runner conformance mode the
    // space is local; under the harness's multi_runner mode (future
    // work), the harness binds it across runners. Pre-creating it as
    // a named space ensures add-atom / collapse-match operate on a
    // valid space handle even when only one runner is active.
    let shared_id = env.create_named_space("shared");
    env.register_token(
        "&shared",
        crate::backend::models::MettaValueFactory::space(
            &f,
            crate::backend::models::SpaceHandle::new(shared_id, "shared".to_string()),
        ),
    );
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValueFactory};
    use crate::ir::{Position, Span};

    #[test]
    fn test_arena_mode_available() {
        assert!(is_arena_mode_available());
    }

    #[test]
    fn test_get_static_factory() {
        let factory = get_static_factory();
        let value = factory.atom("test");
        assert!(value.is_atom());
        assert_eq!(value.as_atom(), Some("test"));
    }

    #[test]
    fn test_eval_simple_atom() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();
        let value = factory.atom("hello");

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_atom());
        assert_eq!(results[0].0.as_atom(), Some("hello"));
    }

    #[test]
    fn test_eval_simple_number() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();
        let value = factory.long(42);

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_long());
        assert_eq!(results[0].0.as_long(), Some(42));
    }

    #[test]
    fn test_eval_simple_arithmetic() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Create (+ 1 2)
        let value = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);

        let (results, _env) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        assert!(results[0].0.is_long());
        assert_eq!(results[0].0.as_long(), Some(3));
    }

    // ================================================================
    // Phase 4: End-to-end span threading through trampoline
    // ================================================================

    #[test]
    fn test_eval_trampoline_span_preserved_on_ground_type() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 2,
                byte_offset: 2,
            },
        };
        let value = factory.spanned(factory.long(42), span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // Self-evaluating: result carries the original span
        assert!(results[0].0.is_spanned());
        assert_eq!(results[0].0.as_long(), Some(42));
        let result_span = results[0].0.span().expect("should have span");
        assert_eq!(result_span.start.byte_offset, 0);
        assert_eq!(result_span.end.byte_offset, 2);
    }

    #[test]
    fn test_eval_trampoline_span_on_arithmetic() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Spanned (+ 1 2) — the outer expression has a span
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 7,
                byte_offset: 7,
            },
        };
        let sexpr = factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]);
        let value = factory.spanned(sexpr, span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // The computed result 3 should carry the source expression's span
        assert_eq!(results[0].0.as_long(), Some(3));
        // Note: grounded ops go through trampoline, so the outer span from
        // eval_step_generic wraps the Done result from the (quote ...) path,
        // but grounded ops return via StartGroundedOp → Resume continuation.
        // The span may or may not be present depending on trampoline path.
        // This tests the current behavior.
    }

    #[test]
    fn test_eval_trampoline_span_on_quote() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // Spanned (quote hello) — returns Done directly from eval_sexpr_step
        let span = Span {
            start: Position {
                row: 0,
                column: 0,
                byte_offset: 0,
            },
            end: Position {
                row: 0,
                column: 13,
                byte_offset: 13,
            },
        };
        let sexpr = factory.sexpr(vec![factory.atom("quote"), factory.atom("hello")]);
        let value = factory.spanned(sexpr, span);

        let (results, _) = eval_trampoline(value, env, &state);
        assert_eq!(results.len(), 1);
        // (quote hello) returns Done → outer span is attached
        assert!(results[0].0.is_spanned());
        assert!(results[0].0.is_quoted());
        let result_span = results[0].0.span().expect("should have span");
        assert_eq!(result_span.end.byte_offset, 13);
    }

    #[test]
    fn test_eval_trampoline_span_on_if_true_branch() {
        let state = MettaState::new();
        let factory = global_factory();
        let env = new_env();

        // (if True 42 0) — the then-branch 42 has its own span
        let then_span = Span {
            start: Position {
                row: 0,
                column: 9,
                byte_offset: 9,
            },
            end: Position {
                row: 0,
                column: 11,
                byte_offset: 11,
            },
        };
        let sexpr = factory.sexpr(vec![
            factory.atom("if"),
            factory.bool(true),
            factory.spanned(factory.long(42), then_span),
            factory.long(0),
        ]);

        let (results, _) = eval_trampoline(sexpr, env, &state);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_long(), Some(42));
        // The then-branch carries its own span (from compilation)
        // After evaluation, the result preserves the branch's span
        assert!(results[0].0.is_spanned());
        let result_span = results[0].0.span().expect("should have span");
        assert_eq!(result_span.start.byte_offset, 9);
        assert_eq!(result_span.end.byte_offset, 11);
    }

    // ================================================================
    // add-atom / remove-atom MeTTa HE semantic alignment tests
    // ================================================================

    /// Helper: compile and evaluate all expressions sequentially, returning
    /// only the results of `!`-prefixed (forced) evaluation expressions.
    fn eval_metta(source: &str) -> Vec<MettaValue> {
        let state = crate::compile(source).expect("compile failed");
        let mut env = new_env();
        let mut all_results = Vec::new();

        let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
        for expr in exprs {
            let (results, new_env) = eval_trampoline(expr, env, &state);
            env = (*new_env).clone();
            all_results.extend(results.into_iter().map(|(v, _)| v));
        }
        all_results
    }

    /// CESK A4.3 — machine-equivalence oracle CI invariant.
    ///
    /// Drives a recursive, rule-heavy, nondeterministic eval that crosses ≥1 GC
    /// safepoint with the discovered-root gap sources populated: the recursion +
    /// rule dispatch fill the EVAL_MEMO / MATCH_RESULT / subgoal / thunk caches,
    /// and `amb` forks the search (exercising the deferred-env path). Under
    /// `--features index-gc` + `debug_assertions` the in-safepoint oracle
    /// (eval_loop.rs) asserts `collect_machine_roots` ⊇ the discovered root set on
    /// EVERY safepoint hit; reaching the end without panicking == the structural
    /// reader stayed a superset across the whole run. This keeps the reader honest
    /// in CI after A5 deletes the discovery apparatus.
    ///
    /// `gc_mode_is_index()` is true by construction under `feature = "index-gc"`.
    #[test]
    #[cfg(all(debug_assertions, feature = "index-gc"))]
    fn a4_3_oracle_holds_across_safepoint() {
        // (cnt 5000 0) is tail-recursive (bounded K depth) but runs > 4096
        // trampoline iterations ⇒ ≥1 safepoint; `amb` adds a nondeterministic fork.
        let src = r#"
            (= (cnt $n $acc) (if (== $n 0) $acc (cnt (- $n 1) (+ $acc 1))))
            (= (amb $x) $x)
            (= (amb $x) (+ $x 1))
            !(cnt 5000 0)
            !(amb 7)
        "#;
        // The oracle asserts inside `eval_trampoline`; this must not panic.
        let results = eval_metta(src);
        assert!(
            !results.is_empty(),
            "the driving program must produce results (and cross a safepoint)"
        );
    }

    /// CESK A4.4 — QUIESCENCE machine-equivalence oracle CI invariant.
    ///
    /// Drives several directives through `eval()` (the quiescence collection site,
    /// eval/mod.rs) with the committed-bytes watermark forced to 0 so the index
    /// collector fires at EVERY quiescence. Under `--features index-gc` +
    /// `debug_assertions` the A4.4 quiescence oracle (`assert_quiescence_superset`)
    /// fires each time; reaching the end without panicking == `OLD ⊆ NEW∪KEPT` held
    /// at every quiescence collection. (The authoritative gate is the debug index
    /// conformance subset on real programs; this pins it in `cargo nextest` CI.)
    ///
    /// `MIN_BYTES` is parsed once — set it BEFORE any eval; nextest isolates each
    /// test in its own process, so this does not leak to other tests.
    #[test]
    #[cfg(all(debug_assertions, feature = "index-gc"))]
    fn a4_4_quiescence_oracle_holds() {
        std::env::set_var("METTATRON_INDEX_GC_MIN_BYTES", "0");
        let src = "!(+ 1 2)\n!(* 3 4)\n!(if (== 1 1) (+ 5 6) 0)";
        let state = crate::compile(src).expect("compile");
        let mut env = new_env();
        for expr in state.source_snapshot() {
            // eval() runs the quiescence collector (+ the A4.4 oracle) at each return.
            let r = crate::backend::eval::eval(expr, env, &state);
            env = r.1;
        }
    }

    #[test]
    fn test_add_atom_rule_becomes_reducible() {
        // Bug A+B fix: add-atom should not evaluate its atom arg AND should
        // update the rule table so the rule becomes usable for reduction.
        let results = eval_metta(
            r#"
            !(add-atom &self (= (foo) 42))
            !(foo)
        "#,
        );
        // add-atom returns Unit
        assert!(
            results[0].is_unit(),
            "add-atom should return Unit, got: {:?}",
            results[0]
        );
        // foo should now reduce to 42
        assert_eq!(
            results[1].as_long(),
            Some(42),
            "foo should reduce to 42 via add-atom rule"
        );
    }

    #[test]
    fn test_add_atom_rule_with_variables() {
        // Rules with variables should also work
        let results = eval_metta(
            r#"
            !(add-atom &self (= (double $x) (* 2 $x)))
            !(double 5)
        "#,
        );
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(results[1].as_long(), Some(10), "double 5 should be 10");
    }

    #[test]
    fn test_add_atom_non_rule() {
        // Non-rule atoms should be added to space without error
        let results = eval_metta(
            r#"
            !(add-atom &self (parent Alice Bob))
        "#,
        );
        assert!(
            results[0].is_unit(),
            "add-atom should return Unit for non-rule atoms"
        );
    }

    #[test]
    fn test_add_atom_does_not_evaluate_atom() {
        // Verify add-atom does NOT evaluate its atom argument.
        // If it did evaluate (= (foo) 42), the = special form handler would
        // add the rule as a side effect but return empty, causing an error.
        let results = eval_metta(
            r#"
            !(add-atom &self (= (bar) 99))
        "#,
        );
        assert!(
            results[0].is_unit(),
            "add-atom should NOT evaluate its atom arg (no error), got: {:?}",
            results[0]
        );
    }

    #[test]
    fn test_remove_atom_multiplicity_tracking() {
        // Test multiplicity in separate steps to avoid index confusion from
        // multiplicity expansion (a rule with multiplicity 2 returns 2 results).

        // Step 1: Add rule, verify it works
        let results = eval_metta(
            r#"
            !(add-atom &self (= (baz) 77))
            !(baz)
        "#,
        );
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(results[1].as_long(), Some(77), "baz should reduce to 77");

        // Step 2: Add same rule again (multiplicity 2), then remove once — should still work
        let results = eval_metta(
            r#"
            !(add-atom &self (= (baz2) 88))
            !(add-atom &self (= (baz2) 88))
            !(remove-atom &self (= (baz2) 88))
            !(baz2)
        "#,
        );
        assert!(results[0].is_unit(), "first add-atom should return Unit");
        assert!(results[1].is_unit(), "second add-atom should return Unit");
        assert!(results[2].is_unit(), "remove-atom should return Unit");
        // After removing one copy, the rule still works (multiplicity was 2, now 1)
        assert_eq!(
            results[3].as_long(),
            Some(88),
            "baz2 should still reduce to 88 after removing one of two copies"
        );

        // Step 3: Remove the last copy — rule should stop working
        let results = eval_metta(
            r#"
            !(add-atom &self (= (baz3) 99))
            !(remove-atom &self (= (baz3) 99))
            !(baz3)
        "#,
        );
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert!(results[1].is_unit(), "remove-atom should return Unit");
        // After removing the only copy, baz3 should be unreduced
        let last = &results[2];
        assert!(
            last.as_atom().is_some() || last.as_sexpr().is_some(),
            "baz3 should be unreduced after rule removed, got: {:?}",
            last
        );
    }

    #[test]
    fn test_remove_atom_non_existent() {
        // Removing a non-existent atom should return Unit (no error)
        let results = eval_metta(
            r#"
            !(remove-atom &self (= (nonexistent) 0))
        "#,
        );
        assert!(
            results[0].is_unit(),
            "remove-atom for non-existent should return Unit"
        );
    }

    #[test]
    fn test_add_atom_type_assertion() {
        // add-atom with type assertion should register in the type system
        let results = eval_metta(
            r#"
            !(add-atom &self (: myvar Int))
            !(get-type myvar)
        "#,
        );
        assert!(results[0].is_unit(), "add-atom should return Unit");
        // get-type should return Int
        assert_eq!(
            results[1].as_atom(),
            Some("Int"),
            "get-type myvar should return Int"
        );
    }

    #[test]
    fn test_add_atom_then_match() {
        // Rules added via add-atom should be queryable via match &self
        let results = eval_metta(
            r#"
            !(add-atom &self (parent Alice Bob))
            !(match &self (parent $x Bob) $x)
        "#,
        );
        assert!(results[0].is_unit(), "add-atom should return Unit");
        assert_eq!(
            results[1].as_atom(),
            Some("Alice"),
            "match should find atom added via add-atom"
        );
    }
}
