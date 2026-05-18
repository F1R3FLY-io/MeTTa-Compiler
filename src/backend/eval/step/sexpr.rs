//! Generic S-Expression Step Evaluation
//!
//! This module handles the generic evaluation step for S-expressions, including
//! special forms dispatch and rule matching. It works with any value type
//! implementing `MettaValueTrait`.
//!
//! ## Design
//!
//! The generic sexpr step uses:
//! - `MettaValueTrait` for type checking and value inspection
//! - `MettaValueFactory` (via `EvalContext`) for value construction
//! - `GenericEnvironment<V, F>` directly for environment operations
//!
//! Special forms use generic implementations that work with `GenericEnvironment`,
//! achieving full genericization over the value type.

use smallvec::{smallvec, SmallVec};

use tracing::trace;

use super::grounded::{
    find_grounded_arg_indices_generic, find_typed_arg_indices_generic, is_declared_value_type,
    validate_grounded_arg_types,
};
use super::types::GenericEvalStep;

use crate::backend::eval::bindings::{eval_atom_subst_generic, eval_sealed_generic};
use crate::backend::eval::list_ops::helpers::suggest_variable_format;
use crate::backend::eval::list_ops::ops::{
    eval_append_generic, eval_car_atom_generic, eval_cdr_atom_generic, eval_cons_atom_generic,
    eval_cut_generic, eval_decons_atom_generic, eval_drop_atom_generic, eval_element_of_generic,
    eval_exclude_item_generic, eval_flatten_atom_generic, eval_index_atom_generic,
    eval_is_member_generic, eval_length_generic, eval_max_atom_generic, eval_min_atom_generic,
    eval_msort_generic, eval_range_generic, eval_reverse_atom_generic, eval_size_atom_generic,
    eval_take_atom_generic, eval_tuple_concat_generic, eval_tuple_count_generic,
    eval_without_generic, eval_zip_atom_generic,
};
// Generic module operations - used directly (no boundary conversion)
use crate::backend::eval::modules::{
    eval_get_modules_generic, eval_import_generic, eval_include_generic, eval_mod_space_generic,
    eval_print_mods_generic,
};
// Generic MORK operations - used directly (no boundary conversion)
use crate::backend::environment::dispatch_overrides::{overridable_op_id, OverridableOpId};
use crate::backend::eval::mork_forms::{
    eval_coalg_generic, eval_exec_generic, eval_lookup_generic, eval_rulify_generic,
};
use crate::backend::eval::trampoline::{EvalContext, MettaEnvironment};
use crate::backend::eval::types::{
    check_call_site_types, eval_check_type_generic, eval_get_type_generic, types_match_generic,
};
use crate::backend::grounded::{has_grounded_op, GroundedState};
use crate::backend::models::metta_value::{MettaValueInner, ValueView};
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait, SpaceHandle};

/// Generic S-expression step evaluation.
///
/// This is the generic version of `eval_sexpr_step` that works with any value type
/// implementing `MettaValueTrait`. It handles special forms dispatch and rule matching.
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticEvalContext`)
///
/// # Arguments
///
/// - `items`: The S-expression items to evaluate
/// - `env`: The evaluation environment (`MettaEnvironment`)
/// - `depth`: Current evaluation depth
/// - `ctx`: The evaluation context providing the factory
///
/// All environment operations use `GenericEnvironment` methods directly.
pub fn eval_sexpr_step_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    env: MettaEnvironment,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    eval_sexpr_step_generic_inner(items, None, env, depth, ctx)
}

/// Like `eval_sexpr_step_generic`, but accepts a pre-built S-expr value to avoid
/// redundant allocation. When the caller already has the expression (e.g., from
/// EvalWithBindings materialization), passing it here skips the `factory.sexpr(items.clone())`
/// allocation at the rule-matching catch-all arm.
pub fn eval_sexpr_step_with_original<C: EvalContext>(
    items: Vec<MettaValue>,
    original_sexpr: MettaValue,
    env: MettaEnvironment,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    eval_sexpr_step_generic_inner(items, Some(original_sexpr), env, depth, ctx)
}

/// Inner implementation accepting an optional pre-built S-expr to avoid redundant
/// allocation when the caller already has the expression (e.g., from EvalWithBindings
/// materialization). When `original_sexpr` is `Some`, it's used directly for rule
/// matching instead of re-wrapping items via `factory.sexpr(items.clone())`.
fn eval_sexpr_step_generic_inner<C: EvalContext>(
    items: Vec<MettaValue>,
    original_sexpr: Option<MettaValue>,
    env: MettaEnvironment,
    depth: usize,
    ctx: &C,
) -> GenericEvalStep<MettaValue, MettaEnvironment>
where
    MettaValue: Clone,
{
    trace!(target: "mettatron::backend::eval::eval_sexpr_step_generic", ?items, depth);

    // Preprocess to combine `& self` into `&self` for HE-compatible space references
    let orig_len = items.len();
    let items = preprocess_space_refs_generic(items, ctx);
    // If preprocessing changed items, the original_sexpr is no longer valid
    let original_sexpr = if items.len() != orig_len {
        None
    } else {
        original_sexpr
    };

    if items.is_empty() {
        // HE-compatible: empty SExpr () evaluates to itself, not Nil
        return GenericEvalStep::Done((smallvec![ctx.factory().sexpr(vec![])], env));
    }

    // Cached parent operator types: computed once in the catch-all arm (Phase 1),
    // reused by find_typed_arg_indices_generic (Step 2) and
    // is_declared_value_type (Step 2.5) to avoid redundant RwLock reads.
    let mut cached_parent_op_types: Option<Vec<MettaValue>> = None;

    // Check for special forms - these are handled directly
    if let Some(op) = items.first().and_then(|v| v.as_atom()) {
        // Trace: SpecialForm dispatch
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let is_special = matches!(
                    op,
                    "=" | "!"
                        | "quote"
                        | "unquote"
                        | "if"
                        | "if-reducible"
                        | "error"
                        | "Error"
                        | "is-error"
                        | "catch"
                        | "eval"
                        | "capture"
                        | "chain"
                        | "let"
                        | "let*"
                        | ":"
                        | ":<"
                        | "get-type"
                        | "check-type"
                        | "match"
                        | "match-or"
                        | "superpose"
                        | "amb"
                        | "collapse"
                        | "map-atom"
                        | "filter-atom"
                        | "foldl-atom"
                        | "add-atom"
                        | "add-reduct"
                        | "add-reducts"
                        | "add-atoms"
                        | "remove-atom"
                        | "get-atoms"
                        | "new-space"
                        | "new-state"
                        | "get-state"
                        | "change-state!"
                        | "pragma!"
                        | "println!"
                        | "print-alternatives!"
                        | "import!"
                        | "git-import!"
                        | "git-module!"
                        | "include"
                        | "register-module!"
                        | "mod-space!"
                        | "print-mods!"
                        | "test"
                        | "unique"
                        | "subtraction"
                        | "intersection"
                        | "union"
                        | "assertEqual"
                        | "assertEqualToResult"
                        | "is-function"
                        | "type-cast"
                        | "match-types"
                        | "match-type-or"
                        | "first-from-pair"
                        | "metta"
                );
                if is_special {
                    let input_tv = crate::backend::trace::trace_value_generic(
                        &ctx.factory().sexpr(items.clone()),
                    );
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        input_tv,
                        vec![],
                        None,
                        trace_format::TraceEventKind::SpecialForm {
                            form_name: op.to_string(),
                            phase: "dispatch".to_string(),
                        },
                    );
                }
            }
        }
        // The labeled block lets gated arms below `break 'special_forms` to
        // skip the built-in handling and fall through to Step 2 / Step 3
        // (rule matching). Used by the override mechanism for grounded
        // helpers that user rules may shadow — see
        // `crate::backend::environment::dispatch_overrides`.
        'special_forms: {
            match op {
                // Rule definition - native generic implementation (zero-conversion).
                //
                // S2 BANG-WORD / decl-atom dispatch (2026-05-13): per HE
                // §02.6 + fixtures T02/044, `(= lhs rhs)` is a DECLARATION
                // ATOM, not a special form. It registers as a rule ONLY in
                // ADD mode (bare top-level). When wrapped in `(! ...)` the
                // runner is in INTERPRET mode and the form must evaluate to
                // itself as unreduced data. The `interpret_mode` flag is set
                // by the `!` arm above; here it gates the register-side
                // effect so deep / interpret-mode evaluations return the
                // S-expression unchanged.
                "=" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "= requires exactly 2 arguments, got {}. Usage: (= pattern body)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Inside `(! ...)` body: return the form unreduced. The
                    // rule is NOT registered because the form is being
                    // evaluated (matches HE: `!(= a b)` yields `(= a b)` as
                    // data, no side effect). The bang_body flag is the
                    // strict signal (set only by the `!` arm), distinct from
                    // interpret_mode which is also set by the programmatic
                    // `eval()` entry point for return-value semantics.
                    if env.in_bang_body() {
                        let resolved =
                            original_sexpr.unwrap_or_else(|| ctx.factory().sexpr(items));
                        return GenericEvalStep::Done((smallvec![resolved], env));
                    }

                    // ADD mode (default top-level): register the rule.
                    // Zero-conversion storage; `add_rule` populates both the
                    // RuleIndex and the underlying PathMap.
                    let mut new_env = env.clone();
                    new_env.add_rule(items[1].clone(), items[2].clone());

                    // Rule definitions return empty list (no observable
                    // output line in ADD mode — see processing/ops.rs).
                    return GenericEvalStep::Done((smallvec![], new_env));
                }

                // Force evaluation operator - defer to trampoline for TCO
                "!" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "! requires exactly 1 argument, got {}. Usage: (! expr)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // S1 TOPLEVEL (2026-05-13): HE INTERPRET mode for (! expr).
                    // Flag the environment so downstream ADD-mode gates in
                    // process_single_combination_generic / _bound emit the
                    // reduction result instead of swallowing it.
                    //
                    // Iterative trampoline: no explicit restore needed since
                    // each top-level directive starts with a fresh env clone
                    // from the runner loop, and nested `(! ...)` is a no-op
                    // (already true). The flag is environment-scoped, not
                    // task-scoped.
                    //
                    // S2 BANG-WORD (2026-05-13): also set `bang_body` so
                    // decl-atom arms (`=`, `:`) inside the body return the
                    // form unreduced rather than registering as rules/types.
                    // `interpret_mode` alone is insufficient because the
                    // programmatic `eval()` API also sets it (for return-
                    // value semantics on bare top-level forms). `bang_body`
                    // is the stricter signal — only the `!` arm sets it.
                    let mut env = env;
                    env.set_interpret_mode(true);
                    env.set_bang_body(true);
                    // Defer evaluation to trampoline - this IS a tail call (TCO)
                    return GenericEvalStep::EvalIfBranch {
                        branch: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // Quote - wraps argument in Quoted variant (prevents evaluation)
                "quote" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "quote requires exactly 1 argument, got {}. Usage: (quote expr)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::Done((
                        smallvec![ctx.factory().quote(items[1].clone())],
                        env,
                    ));
                }

                // S2/S12 NOEVAL (2026-05-13): per HE §11.5 + fixture T02/048,
                // `(noeval X)` returns the bare argument `X` unevaluated.
                // Unlike `noreduce` (which preserves the wrapper), `noeval`
                // STRIPS itself, returning the inner form. HE behavior:
                // `!(noeval (+ 1 2))` returns `(+ 1 2)`, not `(noeval (+ 1 2))`
                // and not `3`. The argument is taken raw — no evaluation, no
                // sub-step dispatch.
                "noeval" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "noeval requires exactly 1 argument, got {}. Usage: (noeval expr)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::Done((smallvec![items[1].clone()], env));
                }

                // noreduce - returns the entire S-expression unevaluated
                // (Workstream X.5c — MTT-FN-NOREDUCE). Quote-like sentinel
                // that preserves the (noreduce X) wrapper. MeTTaTron-only
                // extension; HE has only the related noreduce-eq operator
                // (stdlib.metta:966-967).
                "noreduce" => {
                    return GenericEvalStep::Done((
                        smallvec![ctx.factory().sexpr(items)],
                        env,
                    ));
                }

                // Unquote - unwraps Quoted variant, returns inner value
                "unquote" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "unquote requires exactly 1 argument, got {}. Usage: (unquote expr)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // If argument is Quoted(inner), return inner; otherwise return as-is
                    if let Some(inner) = items[1].as_quoted() {
                        return GenericEvalStep::Done((smallvec![inner], env));
                    }
                    return GenericEvalStep::Done((smallvec![items[1].clone()], env));
                }

                // Conditional - defers condition evaluation to trampoline
                "if" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "if requires exactly 3 arguments, got {}. Usage: (if condition then else)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalIfCondition {
                        condition: items[1].clone(),
                        then_branch: items[2].clone(),
                        else_branch: items[3].clone(),
                        env,
                        depth,
                    };
                }

                // if-reducible - evaluates expr, checks if it reduced, branches accordingly
                "if-reducible" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "if-reducible requires exactly 3 arguments, got {}. Usage: (if-reducible expr then else)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalIfReducible {
                        expr: items[1].clone(),
                        then_branch: items[2].clone(),
                        else_branch: items[3].clone(),
                        env,
                        depth,
                    };
                }

                // Error construction (NO conversion needed).
                //
                // User-facing source form: `(error <message> <details>)`.
                // Internal HE-bisimilar shape: `Error(offending, detail)` where
                // we map `offending = <details>` (the atom-being-errored) and
                // `detail = <message>` (the human string). MTT's old slot
                // convention had message first; the new internal variant has
                // it last, so we conceptually swap them here per the migration
                // spec — preserving the legacy `(error MSG DETAILS)` source
                // syntax while emitting HE-shaped values.
                "error" => {
                    if items.len() < 2 {
                        return GenericEvalStep::Done((smallvec![], env));
                    }
                    let detail = items[1].clone();
                    let offending = if items.len() > 2 {
                        items[2].clone()
                    } else {
                        ctx.factory().unit()
                    };
                    return GenericEvalStep::Done((
                        smallvec![ctx.factory().error(offending, detail)],
                        env,
                    ));
                }

                // HE-compatible Error form (NO conversion needed).
                //
                // HE source convention is `(Error <details> <message>)` — see
                // `hyperon-experimental/lib/src/metta/runner/stdlib/atom.rs`'s
                // `ErrorAtomOp`. The first user arg is the offending atom; the
                // second is the message. Direct slot mapping for the internal
                // Error variant.
                "Error" => {
                    if items.len() < 2 {
                        return GenericEvalStep::Done((smallvec![], env));
                    }
                    let offending = items[1].clone();
                    let detail = if items.len() > 2 {
                        items[2].clone()
                    } else {
                        ctx.factory().unit()
                    };
                    return GenericEvalStep::Done((
                        smallvec![ctx.factory().error(offending, detail)],
                        env,
                    ));
                }

                // is-error - defers evaluation to trampoline
                "is-error" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "is-error requires exactly 1 argument, got {}. Usage: (is-error expr)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalIsError {
                        expr: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // catch - defers evaluation to trampoline
                "catch" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "catch requires exactly 2 arguments, got {}. Usage: (catch expr default)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartCatch {
                        expr: items[1].clone(),
                        default: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // if-error - HE-bisim: evaluate first arg; if Error → second arg, else third.
                //
                // HE source: stdlib.metta:300-317 —
                //   `(= (if-error $atom $then $else)
                //      (case $atom (((Error $a $c) $then) ($_ $else))))`.
                // We desugar to case at the IR level. Reuses case's collapse
                // semantics, and (post-T04/117 fix) Error-as-terminal re-eval
                // skip — so `(if-error (function) caught else)` works because
                // case no longer re-evaluates the Error.
                "if-error" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "if-error requires exactly 3 arguments, got {}. Usage: (if-error atom then else)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let f = ctx.factory();
                    // Pattern variables prefixed with `$__ie_` to minimize
                    // collision with user vars (pattern_match freshens anyway).
                    let cases = f.sexpr(vec![
                        f.sexpr(vec![
                            f.sexpr(vec![
                                f.atom("Error"),
                                f.atom("$__ie_a"),
                                f.atom("$__ie_c"),
                            ]),
                            items[2].clone(),
                        ]),
                        f.sexpr(vec![f.atom("$_"), items[3].clone()]),
                    ]);
                    return GenericEvalStep::EvalCaseAtom {
                        atom: items[1].clone(),
                        cases,
                        env,
                        depth,
                    };
                }

                // eval - Plan S4 (2026-05-14) HE-faithful ONE-STEP semantics.
                //
                // HE's `eval_impl` in
                // `hyperon-experimental/lib/src/metta/interpreter.rs:504` performs
                // a single rewrite step then `finished`. Variable-headed,
                // grounded-scalar-at-head, or no-match cases emit `NotReducible`.
                // The outer `metta`/`metta_call_return` wrapping converts
                // `NotReducible` back to the original atom for user-visible `!`
                // output (matching empirical HE behaviour: `!(eval 42)` →
                // `[(eval 42)]`).
                //
                // We route `eval` to a new `EvalEvalStep` variant that performs
                // classify-and-rewrite inline — NOT the legacy `EvalEval` which
                // is used by internal full-reduction paths (progn, metta).
                "eval" => {
                    if items.len() != 2 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().atom("IncorrectNumberOfArguments"),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalEvalStep {
                        arg: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // T04/050 (2026-05-17): evalc — explicit-space variant of eval.
                //
                // HE source: `hyperon-experimental/lib/src/metta/interpreter.rs::evalc`
                // at line 478. `evalc(stack, bindings)` extracts `(_op, to_eval, space)`
                // and delegates to `eval_impl(to_eval, space, ...)` — same semantics
                // as `eval` but uses the explicit `space` arg for rule lookup
                // instead of the interpreter's `context.space`.
                //
                // MeTTaTron desugars `(evalc atom space)` to `(match space (= atom $body) $body)`
                // followed by `(eval $body)` for each match. For the T04/050 test:
                //   `!(evalc (bar) &s)` where `&s` has `(= (bar) 42)`
                //   → `(match &s (= (bar) $body) $body)` → 42 (from the rule's RHS).
                //
                // Match returns the RHS values (already evaluated by the rule
                // mechanism), so a subsequent `eval` is unnecessary for the
                // common case. We use a direct desugar via the existing
                // StartMatch infrastructure (which resolves the space arg
                // and dispatches against the resolved SpaceHandle).
                "evalc" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "evalc requires exactly 2 arguments, got {}. Usage: (evalc atom space)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let f = ctx.factory();
                    let atom = items[1].clone();
                    let space_arg = items[2].clone();
                    let body_var = f.atom("$__ec_body");
                    // Build pattern (= <atom> $__ec_body) and template $__ec_body.
                    let pattern = f.sexpr(vec![f.atom("="), atom, body_var.clone()]);
                    return GenericEvalStep::StartMatch {
                        space_arg,
                        pattern,
                        template: body_var,
                        env,
                        depth,
                    };
                }

                // capture - HE-faithful full-reduction.
                //
                // HE source: `hyperon-experimental/lib/src/metta/runner/stdlib/core.rs:224-254`.
                // `CaptureOp::execute` calls `interpret(space, atom, settings)` —
                // it FULLY evaluates the argument in the current space with the
                // captured PragmaSettings, returning a Vec<Atom> of results.
                //
                // In MeTTaTron, PragmaSettings aren't a first-class value type
                // (they live on the env via `pragma!` and propagate implicitly).
                // We retain `EvalEval` (transitive full reduction via trampoline)
                // for `capture`, matching HE's `interpret`-loop semantics.
                "capture" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "capture requires exactly 1 argument, got {}. Usage: (capture expr)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalEval {
                        arg: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // PeTTa-compatible `reduce` built-in. HE has no `reduce` op;
                // PeTTa's `reduce/2` Prolog predicate (translator.pl:50) forces
                // evaluation to normal form. PLN (`lib_pln.metta`,
                // `examples/Direct.metta`) depends on `reduce` being full
                // reduction to enumerate all rule-derivation stv candidates.
                //
                // Decision (S4, 2026-05-14): keep `reduce` as full-reduction
                // (legacy `EvalEval`) to preserve PLN. Treating `reduce` as a
                // one-step alias of `eval` (as the S4 plan text suggested)
                // would break Direct/Smokes' multi-step `(stv ...)` derivations.
                // See `/home/dylon/Workspace/f1r3fly.io/PLN/examples/Direct.metta:42`
                // — `(collapse (reduce (eval $grounded)))` needs the outer
                // `reduce` to enumerate all rule applications.
                "reduce" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "reduce requires exactly 1 argument, got {}. Usage: (reduce expr)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalEval {
                        arg: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // PeTTa-compatible `progn` built-in. Desugars to nested `let`
                // bindings on a fresh anonymous variable, which gives the correct
                // sequential-evaluation semantics:
                //   (progn a b)         -> (let $_progn_unused a b)
                //   (progn a b c)       -> (let $_progn_unused a (let $_progn_unused b c))
                //   (progn a b c d)     -> (let $_progn_unused a (let $_progn_unused b (let $_progn_unused c d)))
                //
                // The let semantics evaluate the binding value first (for side
                // effects), then evaluate the body. The body is the next progn
                // step or the final expression. This works correctly because:
                // 1. let pre-evaluates its value_expr (so side-effecting earlier
                //    args fire correctly)
                // 2. The body is re-evaluated by the trampoline (so a recursive
                //    function call as the last arg is fully reduced)
                //
                // Mirrors PeTTa's `progn/N` from `<PeTTa>/src/metta.pl:298`.
                "progn" => {
                    let factory = ctx.factory();
                    if items.len() < 3 {
                        let arg_count = items.len() - 1;
                        let err = factory.error(
                            factory.sexpr(items),
                            factory.string(&format!(
                                "progn requires at least 2 arguments, got {}. \
                             Usage: (progn expr1 expr2 [...])",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Build nested let from the right: start with the last arg as
                    // the innermost body, then wrap each preceding arg in a let.
                    // Use a fresh variable name with a leading `_` to signal "unused".
                    let unused_var = factory.atom("$_progn_unused");
                    let last_idx = items.len() - 1;
                    let mut result = items[last_idx].clone();
                    // Process args[1..last_idx] in reverse order (skip head at idx 0).
                    for arg in items[1..last_idx].iter().rev() {
                        result = factory.sexpr(vec![
                            factory.atom("let"),
                            unused_var.clone(),
                            arg.clone(),
                            result,
                        ]);
                    }

                    // The 2-arg case (most common): (progn a b) -> (let $_ a b).
                    // Return StartLetBinding directly, skipping a redundant trampoline pass.
                    if items.len() == 3 {
                        return GenericEvalStep::StartLetBinding {
                            pattern: unused_var,
                            value_expr: items[1].clone(),
                            body: items[2].clone(),
                            env,
                            depth,
                        };
                    }

                    // Multi-arg case (3+ args): the constructed nested-let
                    // expression needs full re-evaluation. Use EvalEval to push it
                    // back to the trampoline.
                    return GenericEvalStep::EvalEval {
                        arg: result,
                        env,
                        depth,
                    };
                }

                // function - defers evaluation to trampoline
                "function" => {
                    if items.len() != 2 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().atom("IncorrectNumberOfArguments"),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // §06.10.4: function body must be Expression. T04/022.
                    // HE empirical: when body is not an Expression (e.g.
                    // `(function Bar)` where `Bar` is a bare atom), HE emits
                    //   (Error (function Bar) expected: (function (: <body> Expression)), found: (function Bar))
                    // The detail is rendered as a sequence of atoms (HE's
                    // string-format printer drops quotes in error position).
                    // The harness's parser treats `,` as a standalone atom
                    // (whitespace-delimited word tokenizer). So we build the
                    // error as a 7-element SExpr with head "Error", the comma
                    // as its own atom, and the rest as separate atoms /
                    // sub-sexprs — matching the harness's parsed shape.
                    if items[1].as_sexpr().is_none() {
                        let call_form = ctx.factory().sexpr(items.clone());
                        let body_shape_sexpr = ctx.factory().sexpr(vec![
                            ctx.factory().atom("function"),
                            ctx.factory().sexpr(vec![
                                ctx.factory().atom(":"),
                                ctx.factory().atom("<body>"),
                                ctx.factory().atom("Expression"),
                            ]),
                        ]);
                        let err = ctx.factory().sexpr(vec![
                            ctx.factory().atom("Error"),
                            call_form.clone(),
                            ctx.factory().atom("expected:"),
                            body_shape_sexpr,
                            ctx.factory().atom(","),
                            ctx.factory().atom("found:"),
                            call_form,
                        ]);
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartFunction {
                        expr: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // return - defers evaluation to trampoline
                "return" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "return requires exactly 1 argument, got {}. Usage: (return value)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalReturn {
                        value: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // chain - defers evaluation to trampoline
                "chain" => {
                    if items.len() != 4 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().atom("IncorrectNumberOfArguments"),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartChain {
                        expr: items[1].clone(),
                        var: items[2].clone(),
                        body: items[3].clone(),
                        env,
                        depth,
                    };
                }

                // match - defers space evaluation to trampoline
                // Supports: (match space pattern template) - 3 args
                //       or: (match & self pattern template) - 4 args (legacy, preprocessed to 3)
                "match" => {
                    if !(items.len() == 4 || items.len() == 5) {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // Handle both syntaxes
                    if items.len() == 4 {
                        // New syntax: (match space pattern template)
                        return GenericEvalStep::StartMatch {
                            space_arg: items[1].clone(),
                            pattern: items[2].clone(),
                            template: items[3].clone(),
                            env,
                            depth,
                        };
                    } else {
                        // Legacy syntax: (match & self pattern template)
                        // The & and self should have been preprocessed into &self
                        // If not, this is an error
                        let head_repr = if let Some(atom) = items[1].as_atom() {
                            atom.to_string()
                        } else {
                            "non-atom".to_string()
                        };
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "match requires & as first argument (legacy syntax), got: {}",
                                head_repr
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                }

                // match-or - like match but with a default fallback when no match found
                "match-or" => {
                    if items.len() != 5 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "match-or requires exactly 4 arguments, got {}. Usage: (match-or space pattern default template)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartMatchOr {
                        space_arg: items[1].clone(),
                        pattern: items[2].clone(),
                        default: items[3].clone(),
                        template: items[4].clone(),
                        env,
                        depth,
                    };
                }

                // case - defers atom evaluation to trampoline
                "case" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "case requires exactly 2 arguments, got {}. Usage: (case atom ((pattern template) ...))",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::EvalCaseAtom {
                        atom: items[1].clone(),
                        cases: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // switch - pattern matches atom WITHOUT evaluation (unlike case)
                "switch" | "switch-minimal" | "switch-internal" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "{} requires exactly 2 arguments, got {}. Usage: ({} atom cases)",
                                op, arg_count, op
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::SwitchAtom {
                        atom: items[1].clone(),
                        cases: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // let - defers value evaluation to trampoline
                "let" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartLetBinding {
                        pattern: items[1].clone(),
                        value_expr: items[2].clone(),
                        body: items[3].clone(),
                        env,
                        depth,
                    };
                }

                // let* - native generic implementation (zero conversion)
                // Sequential bindings - desugars to nested let
                "let*" => {
                    if items.len() < 3 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(
                                "let* requires at least 2 arguments. Usage: (let* ((pat val) ...) body)",
                            ),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    let bindings_expr = &items[1];
                    let body = &items[2];

                    // Extract bindings list
                    let bindings = match bindings_expr.as_sexpr() {
                        Some(items) => items,
                        None if bindings_expr.is_unit() => {
                            // Empty bindings - evaluate body via trampoline (tail call)
                            return GenericEvalStep::EvalIfBranch {
                                branch: body.clone(),
                                env,
                                depth,
                            };
                        }
                        None => {
                            let err = ctx.factory().error(
                                bindings_expr.clone(),
                                ctx.factory().string(
                                    "let* bindings must be a list. Usage: (let* ((pattern value) ...) body)",
                                ),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    };

                    if bindings.is_empty() {
                        // No bindings - evaluate body via trampoline (tail call)
                        return GenericEvalStep::EvalIfBranch {
                            branch: body.clone(),
                            env,
                            depth,
                        };
                    }

                    // Transform to nested let
                    // (let* ((a 1) (b 2) (c 3)) body) -> (let a 1 (let b 2 (let c 3 body)))
                    let mut result_body = body.clone();

                    // Process bindings in reverse order to build nested structure
                    for binding in bindings.iter().rev() {
                        if let Some(pair) = binding.as_sexpr() {
                            if pair.len() == 2 {
                                let pattern = &pair[0];
                                let value = &pair[1];

                                result_body = ctx.factory().sexpr(vec![
                                    ctx.factory().atom("let"),
                                    pattern.clone(),
                                    value.clone(),
                                    result_body,
                                ]);
                            } else {
                                let err = ctx.factory().error(
                                    binding.clone(),
                                    ctx.factory().string(
                                        "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                                    ),
                                );
                                return GenericEvalStep::Done((smallvec![err], env));
                            }
                        } else {
                            let err = ctx.factory().error(
                                binding.clone(),
                                ctx.factory().string(
                                    "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                                ),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    }

                    // Evaluate the nested let structure via trampoline (tail call)
                    return GenericEvalStep::EvalIfBranch {
                        branch: result_body,
                        env,
                        depth,
                    };
                }

                // unify - defers pattern evaluation to trampoline
                "unify" => {
                    if items.len() != 5 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().atom("IncorrectNumberOfArguments"),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartUnify {
                        pattern1: items[1].clone(),
                        pattern2: items[2].clone(),
                        success_body: items[3].clone(),
                        failure_body: items[4].clone(),
                        env,
                        depth,
                    };
                }

                // sealed - native generic implementation (zero conversion)
                "sealed" => {
                    let results = eval_sealed_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // atom-subst - native generic implementation (zero conversion)
                "atom-subst" => {
                    let results = eval_atom_subst_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // Subtype declaration - native generic implementation (zero-conversion)
                // (:< SubType SuperType) registers a subtype relation used by the type checker.
                // HE parity: this is a declaration form like (:), not evaluated as a rule.
                ":<" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            ":< requires exactly 2 arguments, got {}. Usage: (:< SubType SuperType)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Extract sub and super type names
                    let sub_name = match items[1].as_atom() {
                        Some(atom) => atom.to_string(),
                        None => {
                            let err = ctx.factory().error(
                                items[1].clone(),
                                ctx.factory().string(
                                    ":< requires atom arguments. Usage: (:< SubType SuperType)",
                                ),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    };
                    let super_name = match items[2].as_atom() {
                        Some(atom) => atom.to_string(),
                        None => {
                            let err = ctx.factory().error(
                                items[2].clone(),
                                ctx.factory().string(
                                    ":< requires atom arguments. Usage: (:< SubType SuperType)",
                                ),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    };

                    let mut new_env = env.clone();
                    new_env.add_subtype_generic(&sub_name, &super_name);

                    // Also add the (:< ...) atom to space so match/get-atoms can see it
                    let atom = ctx.factory().sexpr(items);
                    new_env.add_to_space(&atom);

                    // Subtype declarations return empty list (like type assertions)
                    return GenericEvalStep::Done((smallvec![], new_env));
                }

                // Type assertion - native generic implementation (zero-conversion).
                //
                // S2 BANG-WORD / decl-atom dispatch (2026-05-13): per HE
                // §02.5 + fixture T02/043, `(: name type)` is a DECLARATION
                // ATOM, not a special form. It registers as a type binding
                // ONLY in ADD mode (bare top-level). When wrapped in
                // `(! ...)` the runner is in INTERPRET mode and the form
                // evaluates to itself as unreduced data. Matches HE:
                // `!(: foo Bar)` yields `(: foo Bar)` as a single result
                // atom with no side effect.
                ":" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                ": requires exactly 2 arguments, got {}. Usage: (: expr type)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Inside `(! ...)` body: return the form unreduced. The
                    // type binding is NOT registered (HE semantics —
                    // declarations are observable data in INTERPRET-body,
                    // side effects only in ADD mode at the runner level).
                    // See the `=` arm above for the bang_body vs.
                    // interpret_mode distinction.
                    if env.in_bang_body() {
                        let resolved =
                            original_sexpr.unwrap_or_else(|| ctx.factory().sexpr(items));
                        return GenericEvalStep::Done((smallvec![resolved], env));
                    }

                    // ADD mode (default top-level): register the type
                    // binding. Extract name from expression (atom or first
                    // element of sexpr).
                    let name = match (items[1].as_atom(), items[1].as_sexpr()) {
                        (Some(atom), _) => atom.to_string(),
                        (_, Some(expr_items)) => match expr_items.first().and_then(|f| f.as_atom())
                        {
                            Some(atom) => atom.to_string(),
                            None => format!("{:?}", items[1]),
                        },
                        _ => format!("{:?}", items[1]),
                    };

                    // Add type directly (V is already the correct type)
                    let mut new_env = env.clone();
                    new_env.add_type_generic(&name, items[2].clone());

                    // Type assertions return empty list
                    return GenericEvalStep::Done((smallvec![], new_env));
                }

                // get-type - native generic implementation
                "get-type" => {
                    let results = eval_get_type_generic(&items, ctx.factory(), &env);
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // check-type - native generic implementation
                "check-type" => {
                    let results = eval_check_type_generic(&items, ctx.factory(), &env);
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // validate-atom - recursive well-typedness checking (Phase 4)
                "validate-atom" => {
                    let results = crate::backend::eval::types::eval_validate_atom_generic(
                        &items,
                        ctx.factory(),
                        &env,
                    );
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // get-type-space - query types in a specific space (Phase 5)
                "get-type-space" => {
                    let results = crate::backend::eval::types::eval_get_type_space_generic(
                        &items,
                        ctx.factory(),
                        &env,
                    );
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // is-function - check if a type is an arrow type (Phase G, HE parity)
                "is-function" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "is-function requires exactly 1 argument, got {}. Usage: (is-function type)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let typ = &items[1];
                    let is_fn = if let Some(type_items) = typ.as_sexpr() {
                        type_items.first().and_then(|v| v.as_atom()) == Some("->")
                    } else {
                        false
                    };
                    return GenericEvalStep::Done((smallvec![ctx.factory().bool(is_fn)], env));
                }

                // type-cast - validate atom against expected type (Phase H, HE parity)
                "type-cast" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "type-cast requires exactly 3 arguments, got {}. Usage: (type-cast atom type space)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let results = crate::backend::eval::types::eval_type_cast_generic(
                        &items,
                        ctx.factory(),
                        &env,
                    );
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // metta - interpreter operation (HE stdlib parity)
                // (metta atom type space) — evaluates atom with type constraint in space
                "metta" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "metta requires exactly 3 arguments, got {}. Usage: (metta atom type space)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    let atom = &items[1];
                    let typ = &items[2];
                    // items[3] is space — accepted but we use env's type system (same as type-cast)

                    // %Undefined% type constraint → evaluate without type checking
                    if let Some(name) = typ.as_atom() {
                        if name == "%Undefined%" || name == "Atom" {
                            // Evaluate the atom; no type constraint to enforce
                            return GenericEvalStep::EvalEval {
                                arg: atom.clone(),
                                env,
                                depth,
                            };
                        }
                    }

                    // Variables pass through unchanged (HE: variables are never evaluated)
                    if atom.as_atom().map_or(false, |s| s.starts_with('$')) {
                        return GenericEvalStep::Done((smallvec![atom.clone()], env));
                    }

                    // Check metatype match (Symbol/Variable/Expression/Grounded)
                    if let Some(type_name) = typ.as_atom() {
                        let meta_match = match type_name {
                            "Symbol" => atom.as_atom().map_or(false, |s| !s.starts_with('$')),
                            "Variable" => atom.as_atom().map_or(false, |s| s.starts_with('$')),
                            "Expression" => atom.as_sexpr().is_some() || atom.is_unit(),
                            "Grounded" => matches!(
                                atom.inner_raw(),
                                MettaValueInner::Bool(_)
                                    | MettaValueInner::Long(_)
                                    | MettaValueInner::Float(_)
                                    | MettaValueInner::String(_)
                            ),
                            _ => false,
                        };
                        if meta_match {
                            return GenericEvalStep::Done((smallvec![atom.clone()], env));
                        }
                    }

                    // For non-expression atoms (symbols, grounded): type-cast check only
                    if atom.as_sexpr().is_none() && !atom.is_unit() {
                        let results = crate::backend::eval::types::eval_type_cast_generic(
                            &items,
                            ctx.factory(),
                            &env,
                        );
                        return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                    }

                    // For expressions: evaluate first, then type-cast each result.
                    // Desugar to: (let $__metta_result (eval atom) (type-cast $__metta_result type space))
                    let fresh_var = ctx.factory().atom("$__metta_result");
                    let type_cast_expr = ctx.factory().sexpr(vec![
                        ctx.factory().atom("type-cast"),
                        fresh_var.clone(),
                        typ.clone(),
                        items[3].clone(),
                    ]);
                    return GenericEvalStep::StartLetBinding {
                        pattern: fresh_var,
                        value_expr: atom.clone(),
                        body: type_cast_expr,
                        env,
                        depth,
                    };
                }

                // match-types - structural type matching (HE stdlib parity)
                "match-types" => {
                    if items.len() != 5 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().atom("BadArity"),
                            ctx.factory().string(&format!(
                                "match-types requires 4 arguments, got {}. Usage: (match-types type1 type2 then else)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let type1 = &items[1];
                    let type2 = &items[2];
                    let then_branch = &items[3];
                    let else_branch = &items[4];

                    // %Undefined% and Atom match anything, per HE semantics
                    let undefined = ctx.factory().atom("%Undefined%");
                    let atom_type = ctx.factory().atom("Atom");

                    let matched = *type1 == undefined
                        || *type2 == undefined
                        || *type1 == atom_type
                        || *type2 == atom_type
                        || types_match_generic(type1, type2);

                    let branch = if matched { then_branch } else { else_branch };
                    return GenericEvalStep::EvalIfBranch {
                        branch: branch.clone(),
                        env,
                        depth,
                    };
                }

                // match-type-or - fold helper for type matching (HE stdlib parity)
                // (match-type-or $folded $next $type) = (or $folded (match-types $next $type True False))
                "match-type-or" => {
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().atom("BadArity"),
                            ctx.factory().string(&format!(
                                "match-type-or requires 3 arguments, got {}. Usage: (match-type-or folded next type)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let folded = &items[1];
                    let next = &items[2];
                    let target_type = &items[3];

                    // Check if next matches type
                    let undefined = ctx.factory().atom("%Undefined%");
                    let atom_type = ctx.factory().atom("Atom");
                    let matched = *next == undefined
                        || *target_type == undefined
                        || *next == atom_type
                        || *target_type == atom_type
                        || types_match_generic(next, target_type);

                    // or(folded, matched)
                    let folded_bool = folded
                        .as_bool()
                        .unwrap_or_else(|| folded.as_atom() == Some("True"));
                    let result = folded_bool || matched;
                    return GenericEvalStep::Done((smallvec![ctx.factory().bool(result)], env));
                }

                // first-from-pair - extract first element from a pair (HE stdlib parity)
                // (first-from-pair ($first $second)) = $first
                "first-from-pair" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "first-from-pair requires 1 argument, got {}. Usage: (first-from-pair pair)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let pair = &items[1];
                    if let Some(pair_items) = pair.as_sexpr() {
                        if pair_items.len() == 2 {
                            return GenericEvalStep::Done((smallvec![pair_items[0].clone()], env));
                        }
                    }
                    // Not a valid pair — return error per HE
                    let offending = ctx
                        .factory()
                        .sexpr(vec![ctx.factory().atom("first-from-pair"), pair.clone()]);
                    let err = ctx
                        .factory()
                        .error(offending, ctx.factory().string("incorrect pair format"));
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                // map-atom - defers iteration to trampoline.
                // Overridable: HE defines `map-atom` as a MeTTa rule in
                // `stdlib.metta`. User rules take precedence when present.
                "map-atom" => {
                    if env
                        .dispatch_overrides()
                        .is_overridden(OverridableOpId::MapAtom)
                    {
                        break 'special_forms;
                    }
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "map-atom requires exactly 3 arguments, got {}. Usage: (map-atom list $var template)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // MeTTa HE applicative order: the list argument must be
                    // reduced BEFORE we extract its elements. Without this,
                    // `(map-atom (collapse X) $v T)` would see `(collapse X)`
                    // as a 2-element tuple `(collapse X)` instead of the
                    // evaluated collapse result.
                    if is_reducible_sub_expr(&items[1], &env) {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }
                    // Extract list elements
                    let list_arg = &items[1];
                    let var_arg = &items[2];
                    let template = items[3].clone();

                    let var_name = match extract_var_name::<C>(
                        var_arg,
                        "map-atom",
                        "second argument",
                        &env,
                        ctx,
                    ) {
                        Ok(name) => name,
                        Err(step) => return step,
                    };

                    let elements = match extract_list_elements::<C>(list_arg, "map-atom", ctx, &env)
                    {
                        Ok(elems) => elems,
                        Err(step) => return step,
                    };

                    return GenericEvalStep::StartMapAtom {
                        elements,
                        var_name,
                        template,
                        env,
                        depth,
                    };
                }

                // filter-atom - defers iteration to trampoline.
                // Overridable: HE defines `filter-atom` as a MeTTa rule in
                // `stdlib.metta`. User rules take precedence when present.
                "filter-atom" => {
                    if env
                        .dispatch_overrides()
                        .is_overridden(OverridableOpId::FilterAtom)
                    {
                        break 'special_forms;
                    }
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "filter-atom requires exactly 3 arguments, got {}. Usage: (filter-atom list $var predicate)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // HE applicative order: reduce the list arg before
                    // extracting elements.
                    if is_reducible_sub_expr(&items[1], &env) {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }

                    let list_arg = &items[1];
                    let var_arg = &items[2];
                    let predicate = items[3].clone();

                    let var_name = match extract_var_name::<C>(
                        var_arg,
                        "filter-atom",
                        "second argument",
                        &env,
                        ctx,
                    ) {
                        Ok(name) => name,
                        Err(step) => return step,
                    };

                    let elements =
                        match extract_list_elements::<C>(list_arg, "filter-atom", ctx, &env) {
                            Ok(elems) => elems,
                            Err(step) => return step,
                        };

                    return GenericEvalStep::StartFilterAtom {
                        elements,
                        var_name,
                        predicate,
                        env,
                        depth,
                    };
                }

                // foldl-atom - supports two argument signatures:
                //
                //   1. MeTTaTron 5-arg form: `(foldl-atom list init $acc $x op)` — explicit
                //      accumulator/item variables, defers iteration to the trampoline.
                //
                //   2. PeTTa 3-arg form: `(foldl-atom list init f)` — implicit
                //      accumulator/item, builds a deferred (f (f (f init h0) h1) h2) chain.
                //      Mirrors PeTTa's `metta.pl:245-247` `'foldl-atom'/4` predicate.
                //
                // The arity of the call decides which form is used.
                //
                // Overridable: HE defines `foldl-atom` as a MeTTa rule in
                // `stdlib.metta`. User rules take precedence when present.
                "foldl-atom" => {
                    if env
                        .dispatch_overrides()
                        .is_overridden(OverridableOpId::FoldlAtom)
                    {
                        break 'special_forms;
                    }
                    // HE applicative order: the list argument must be reduced
                    // before we extract its elements. Applies to both the HE
                    // 5-arg form and the PeTTa 3-arg extension. Without this,
                    // `(foldl-atom (collapse X) init op)` would see the
                    // literal children of `(collapse X)` (i.e. `[collapse, X]`)
                    // instead of the evaluated collapse tuple.
                    if items.len() >= 2 && is_reducible_sub_expr(&items[1], &env) {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }
                    let arity = items.len() - 1; // exclude head
                    if arity == 5 {
                        // MeTTaTron 5-arg form: (foldl-atom list init $acc $x op)
                        let list_arg = &items[1];
                        let init = items[2].clone();
                        let acc_var = &items[3];
                        let item_var = &items[4];
                        let operation = items[5].clone();

                        let acc_var_name = match extract_var_name::<C>(
                            acc_var,
                            "foldl-atom",
                            "third argument",
                            &env,
                            ctx,
                        ) {
                            Ok(name) => name,
                            Err(step) => return step,
                        };

                        let item_var_name = match extract_var_name::<C>(
                            item_var,
                            "foldl-atom",
                            "fourth argument",
                            &env,
                            ctx,
                        ) {
                            Ok(name) => name,
                            Err(step) => return step,
                        };

                        let elements =
                            match extract_list_elements::<C>(list_arg, "foldl-atom", ctx, &env) {
                                Ok(elems) => elems,
                                Err(step) => return step,
                            };

                        return GenericEvalStep::StartFoldlAtom {
                            elements,
                            init,
                            acc_var_name,
                            item_var_name,
                            operation,
                            env,
                            depth,
                        };
                    } else if arity == 3 {
                        // PeTTa 3-arg form: (foldl-atom list init f)
                        //
                        // Route through the same `StartFoldlAtom` → `ProcessFoldlAtom`
                        // pipeline as the 5-arg form, using synthesized accumulator
                        // and item variable names. This ensures per-iteration binding
                        // propagation: when the function body unifies a variable
                        // against a rule, subsequent iterations see that binding
                        // via `acc_bindings`. The previous pre-expansion to a
                        // nested `(f (f init h0) h1)` chain evaluated each element
                        // independently, losing shared-variable constraints across
                        // iterations (e.g. `((father a $b) (father $b c))` needs
                        // `$b` consistent, but pre-expansion let each premise pick
                        // a local `$b` — producing spurious derivations for
                        // PLN implication rules that share variables across the
                        // antecedent list).
                        let list_arg = &items[1];
                        let init = items[2].clone();
                        let func = items[3].clone();

                        let elements =
                            match extract_list_elements::<C>(list_arg, "foldl-atom", ctx, &env) {
                                Ok(elems) => elems,
                                Err(step) => return step,
                            };

                        // H15 (2026-05-05): synthesized variable names use
                        // a per-call freshened epoch via `intern_fresh_name`.
                        // The `$__fr_<epoch>_*` prefix interlocks with the
                        // existing propagate_keys filter at eval_loop.rs:7428
                        // (`!name.starts_with("$__fr_")`), so per-iter accumulator
                        // names don't leak across nested fold scopes. The earlier
                        // literal `$__fa_acc` / `$__fa_item` strings caused
                        // cross-contamination in nested foldl-atom calls.
                        let epoch = crate::backend::eval::freshening::allocate_epoch();
                        let acc_var_static =
                            crate::backend::eval::freshening::intern_fresh_name(epoch, "fa_acc");
                        let item_var_static =
                            crate::backend::eval::freshening::intern_fresh_name(epoch, "fa_item");
                        let acc_var_name = acc_var_static.to_string();
                        let item_var_name = item_var_static.to_string();
                        let operation = ctx.factory().sexpr(vec![
                            func,
                            ctx.factory().atom(acc_var_static),
                            ctx.factory().atom(item_var_static),
                        ]);

                        return GenericEvalStep::StartFoldlAtom {
                            elements,
                            init,
                            acc_var_name,
                            item_var_name,
                            operation,
                            env,
                            depth,
                        };
                    } else {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "foldl-atom requires either 3 args (PeTTa form: (foldl-atom list init f)) \
                                 or 5 args (MeTTaTron form: (foldl-atom list init $acc $x op)), got {}",
                                arity
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                }

                // List operations - native generic implementations (zero conversion)
                //
                // MeTTa HE semantics: list/tuple operations evaluate their arguments
                // before operating on them (applicative order). Without this pre-evaluation,
                // reducible S-expression arguments (like `(collapse ...)`) would be treated
                // as structural tuples instead of being evaluated first. This is critical
                // for PLN's BestCandidate which passes `(collapse ...)` to car-atom/cdr-atom.
                //
                // Two arms below split this set into:
                //   - **Arm A**: TRUE HE primitives (`cons-atom`, `decons-atom`,
                //     `size-atom`, `max-atom`, `min-atom`, `index-atom`). HE
                //     dispatches these as `Atom::Grounded` or embedded ops, so
                //     they MUST NOT be overridable by user rules. No gate.
                //   - **Arm B**: helpers HE leaves at the MeTTa level, or that
                //     HE doesn't have at all. These ARE overridable: a single
                //     atomic load + bit test gates the grounded fast path; a
                //     set bit means "user has rules for this name" and we
                //     `break 'special_forms` to fall through to rule matching.
                //
                // See `crate::backend::environment::dispatch_overrides` for the
                // partition rationale and the `OverridableOpId` enum.

                // Arm A1: cons-atom / decons-atom — HE treats both args as
                // literal structure, NEVER pre-evaluating. T04/020 and
                // T04/061 verify this empirically:
                //   `(cons-atom 5 (+ 1 2))` → `(5 + 1 2)` (no reduction)
                //   `(decons-atom (cons-atom a (b c)))` →
                //     `(cons-atom (a (b c)))` (the inner cons-atom is
                //     preserved as expression structure).
                // Going through `EvalGroundedArgs` here would reduce the
                // chain/cons-atom/etc. arg, diverging from HE.
                "cons-atom" => {
                    let results = eval_cons_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }
                "decons-atom" => {
                    let results = eval_decons_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // Arm A2: size-atom — pre-eval the (Expression) arg where
                // type signature allows; meta-typed args (Atom, Variable)
                // are NOT pre-evaluated, matching HE embedded-op semantics.
                "size-atom" => {
                    let reducible_indices = list_op_reducible_arg_indices_typed(op, &items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = eval_size_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }
                "max-atom" => {
                    let reducible_indices = list_op_reducible_arg_indices_typed(op, &items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = eval_max_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }
                "min-atom" => {
                    let reducible_indices = list_op_reducible_arg_indices_typed(op, &items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = eval_min_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }
                "index-atom" => {
                    let reducible_indices = list_op_reducible_arg_indices_typed(op, &items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = eval_index_atom_generic(&items, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // Arm B-structural: car-atom and cdr-atom are structural destructuring ops.
                // They operate on the SYNTACTIC structure of their argument and must NOT
                // pre-evaluate user-defined function calls. Doing so causes nondeterminism
                // (multiple rule matches) that forks the evaluation into branches, each of
                // which receives a cloned environment from Arc::make_mut. Any add-atom &self
                // call inside a forked branch writes to a branch-local clone that does NOT
                // persist to the main environment — silently dropping the added rule.
                //
                // The only arguments that SHOULD be pre-evaluated are:
                //   - Eager special forms (collapse, superpose, eval, …) — explicitly list-producing
                //   - Grounded ops (+, ==, …) — always return a concrete value
                //   - Type-declared functions (should_pre_eval_by_type) — explicitly typed
                //
                // User-defined functions (identified by the bloom filter) must NOT be
                // pre-evaluated. This matches HE semantics where car-atom/cdr-atom operate
                // on the expression structure, not the evaluated value.
                //
                // Example: (car-atom (grandfather $a $x)) should return `grandfather`, NOT
                // evaluate `(grandfather $a $x)` via matching rules.
                "car-atom" | "cdr-atom" => {
                    let id =
                        overridable_op_id(op).expect("car-atom/cdr-atom are overridable list ops");
                    if env.dispatch_overrides().is_overridden(id) {
                        break 'special_forms;
                    }
                    let reducible_indices = structural_op_reducible_arg_indices(&items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = match op {
                        "car-atom" => eval_car_atom_generic(&items, ctx.factory()),
                        "cdr-atom" => eval_cdr_atom_generic(&items, ctx.factory()),
                        _ => unreachable!("Arm B-structural op dispatch mismatch"),
                    };
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // Arm B: overridable helpers (Class B in HE or not in HE at all).
                // Single relaxed atomic load + bit test (~1-2 ns) gates the
                // grounded fast path. When the bit is set, fall through past
                // the entire special-form match block and let standard rule
                // matching (Step 2 / Step 3 below) handle the call.
                "tuple-concat" | "tuple-count" | "without" | "element-of" | "range"
                | "reverse-atom" | "flatten-atom" | "zip-atom" | "take-atom" | "drop-atom"
                | "is-member" | "append" | "length" | "exclude-item" | "msort" | "cut" => {
                    let id = overridable_op_id(op).expect("Arm B covers all overridable list ops");
                    if env.dispatch_overrides().is_overridden(id) {
                        // User rules exist for this name — let rule matching
                        // handle the call instead of the grounded fast path.
                        break 'special_forms;
                    }
                    let reducible_indices = list_op_reducible_arg_indices(&items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    let results = match op {
                        "tuple-concat" => eval_tuple_concat_generic(&items, ctx.factory()),
                        "tuple-count" => eval_tuple_count_generic(&items, ctx.factory()),
                        "without" => eval_without_generic(&items, ctx.factory()),
                        "element-of" => eval_element_of_generic(&items, ctx.factory()),
                        "range" => eval_range_generic(&items, ctx.factory()),
                        "reverse-atom" => eval_reverse_atom_generic(&items, ctx.factory()),
                        "flatten-atom" => eval_flatten_atom_generic(&items, ctx.factory()),
                        "zip-atom" => eval_zip_atom_generic(&items, ctx.factory()),
                        "take-atom" => eval_take_atom_generic(&items, ctx.factory()),
                        "drop-atom" => eval_drop_atom_generic(&items, ctx.factory()),
                        "is-member" => eval_is_member_generic(&items, ctx.factory()),
                        "append" => eval_append_generic(&items, ctx.factory()),
                        "length" => eval_length_generic(&items, ctx.factory()),
                        "exclude-item" => eval_exclude_item_generic(&items, ctx.factory()),
                        "msort" => eval_msort_generic(&items, ctx.factory()),
                        "cut" => eval_cut_generic(&items, ctx.factory()),
                        _ => unreachable!("Arm B list op dispatch mismatch: {}", op),
                    };
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }

                // freeze-tuple: construct a tuple from arguments AS-IS (no
                // evaluation) and mark it as normal form so the trampoline's
                // fixpoint loop won't reduce it. General-purpose data constructor.
                // Callers that want args evaluated should use chain/let first.
                // (freeze-tuple arg1 arg2 ... argN) → (arg1 arg2 ... argN) [frozen]
                "freeze-tuple" => {
                    if items.len() < 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "freeze-tuple requires at least 1 argument, got {}. \
                             Usage: (freeze-tuple expr1 expr2 ... exprN)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let args: Vec<_> = items[1..]
                        .iter()
                        .map(|item| {
                            if let Some(inner) = item.as_quoted() {
                                inner
                            } else {
                                item.clone()
                            }
                        })
                        .collect();
                    let tuple = ctx.factory().sexpr(args);
                    crate::backend::eval::trampoline::dispatch_hints::memoize_normal_form(&tuple);
                    return GenericEvalStep::Done((smallvec![tuple], env));
                }

                // ground-with-bindings: apply serialized bindings to a template.
                // (ground-with-bindings template (Bindings ($var val) ...)) → grounded template.
                // Used by the ? macro to apply captured collapse-bind bindings.
                "ground-with-bindings" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "ground-with-bindings requires 2 arguments, got {}. Usage: (ground-with-bindings template (Bindings ...))",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let template = items[1].clone();
                    let bindings_sexpr = &items[2];
                    let bindings =
                        crate::backend::eval::trampoline::eval_loop::decode_bindings_from_sexpr(
                            bindings_sexpr,
                            ctx.factory(),
                        );
                    // Spec §06.6.3: chain dispatches one kernel step on its expr
                    // arg. Now that ground-with-bindings is in the kernel-op
                    // whitelist (dispatch_hints.rs is_embedded_kernel_op), chain
                    // invokes this handler and binds the substituted template AS
                    // DATA to $var — no further reduction unless the result's own
                    // head is itself a kernel op. Quote wrappers here would push
                    // a (quote ...) atom into $var, breaking PLN's
                    //     (chain (ground-with-bindings $term $binds) $grounded
                    //       (... (eval $grounded) ... (freeze-tuple $grounded ...)))
                    // pattern where $grounded must be the substituted template
                    // itself, not a quoted wrapper.
                    if bindings.is_empty() {
                        return GenericEvalStep::Done((smallvec![template], env));
                    }
                    let grounded = crate::backend::eval::trampoline::engine::apply_bindings(
                        &template,
                        &bindings,
                        ctx.factory(),
                    );
                    return GenericEvalStep::Done((smallvec![grounded], env));
                }

                // sort-tuple - defers iteration to trampoline.
                // Overridable: MeTTaTron-only helper, HE has nothing equivalent.
                // User rules take precedence when present.
                "sort-tuple" => {
                    if env
                        .dispatch_overrides()
                        .is_overridden(OverridableOpId::SortTuple)
                    {
                        break 'special_forms;
                    }
                    if items.len() != 5 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "sort-tuple requires exactly 4 arguments, got {}. Usage: (sort-tuple tuple $var1 $var2 comparator)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Applicative order: reduce the tuple arg before extracting elements.
                    if is_reducible_sub_expr(&items[1], &env) {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }

                    let list_arg = &items[1];
                    let var1_arg = &items[2];
                    let var2_arg = &items[3];
                    let comparator = items[4].clone();

                    let var1_name = match extract_var_name::<C>(
                        var1_arg,
                        "sort-tuple",
                        "second argument",
                        &env,
                        ctx,
                    ) {
                        Ok(name) => name,
                        Err(step) => return step,
                    };

                    let var2_name = match extract_var_name::<C>(
                        var2_arg,
                        "sort-tuple",
                        "third argument",
                        &env,
                        ctx,
                    ) {
                        Ok(name) => name,
                        Err(step) => return step,
                    };

                    let elements =
                        match extract_list_elements::<C>(list_arg, "sort-tuple", ctx, &env) {
                            Ok(elems) => elems,
                            Err(step) => return step,
                        };

                    return GenericEvalStep::StartSortTuple {
                        elements,
                        var1_name,
                        var2_name,
                        comparator,
                        env,
                        depth,
                    };
                }

                // best-candidate - defers iteration to trampoline.
                // Overridable: MeTTaTron-only helper, HE has nothing equivalent.
                // User rules take precedence when present.
                "best-candidate" => {
                    if env
                        .dispatch_overrides()
                        .is_overridden(OverridableOpId::BestCandidate)
                    {
                        break 'special_forms;
                    }
                    if items.len() != 4 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "best-candidate requires exactly 3 arguments, got {}. Usage: (best-candidate tuple $var rank-fn)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // Applicative order: reduce the tuple arg before extracting elements.
                    if is_reducible_sub_expr(&items[1], &env) {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }

                    let list_arg = &items[1];
                    let var_arg = &items[2];
                    let rank_fn = items[3].clone();

                    let var_name = match extract_var_name::<C>(
                        var_arg,
                        "best-candidate",
                        "second argument",
                        &env,
                        ctx,
                    ) {
                        Ok(name) => name,
                        Err(step) => return step,
                    };

                    let elements =
                        match extract_list_elements::<C>(list_arg, "best-candidate", ctx, &env) {
                            Ok(elems) => elems,
                            Err(step) => return step,
                        };

                    return GenericEvalStep::StartBestCandidate {
                        elements,
                        var_name,
                        rank_fn,
                        env,
                        depth,
                    };
                }

                // Space operations - native generic implementation (zero-conversion)
                "new-space" => {
                    // Get optional name, default to "unnamed"
                    let name = if items.len() <= 1 {
                        "unnamed".to_string()
                    } else {
                        match (items[1].as_string(), items[1].as_atom()) {
                            (Some(s), _) | (_, Some(s)) => s.to_string(),
                            _ => {
                                let err = ctx.factory().error(
                                    items[1].clone(),
                                    ctx.factory().string(&format!(
                                        "new-space: optional name must be a string, got {:?}. Usage: (new-space) or (new-space \"name\")",
                                        items[1]
                                    )),
                                );
                                return GenericEvalStep::Done((smallvec![err], env));
                            }
                        }
                    };

                    // Create named space via GenericEnvironment
                    let mut new_env = env.clone();
                    let space_id = new_env.create_named_space(&name);
                    crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    let handle = SpaceHandle::new(space_id, name);

                    // Return Space value using factory
                    let space_val = ctx.factory().space(handle);
                    return GenericEvalStep::Done((smallvec![space_val], new_env));
                }

                "add-atom" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "add-atom requires exactly 2 arguments, got {}. Usage: (add-atom space atom)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartAddAtom {
                        space_ref: items[1].clone(),
                        atom: items[2].clone(),
                        env,
                        depth,
                    };
                }

                "remove-atom" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "remove-atom requires exactly 2 arguments, got {}. Usage: (remove-atom space atom)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartRemoveAtom {
                        space_ref: items[1].clone(),
                        atom: items[2].clone(),
                        env,
                        depth,
                    };
                }

                "collapse" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "collapse requires exactly 1 argument, got {}. Usage: (collapse expr)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartCollapse {
                        expr: items[1].clone(),
                        env,
                        depth,
                    };
                }

                "collapse-bind" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "collapse-bind requires exactly 1 argument, got {}. Usage: (collapse-bind expr)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartCollapseBind {
                        expr: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // S5: superpose-bind — decompose a collapse-bind-shaped result
                // `((atom (Bindings ...)) (atom (Bindings ...)) ...)` into bare
                // nondet, merging each pair's saved bindings with caller's
                // carrying_bindings. Used by PLN's flow
                // `(chain (collapse-bind X) $bs (chain (superpose-bind $bs) ...))`.
                // The arg is already a concrete value because `chain` substitutes
                // the variable before dispatching the body.
                //
                // HE reference: hyperon-experimental/lib/src/metta/interpreter.rs:893-918
                "superpose-bind" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "superpose-bind requires exactly 1 argument, got {}. Usage: (superpose-bind collapsed)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // HE-bisim §06.12.3: superpose-bind takes a *collapsed*
                    // tuple of `(value bindings)` pairs — i.e. the result of
                    // `collapse-bind` — and fans it back out as nondet.
                    // T04/027 verifies the inverse property
                    // `superpose-bind ∘ collapse-bind ≅ id`.
                    //
                    // If the arg's head is a reducible op (e.g. literal
                    // `(collapse-bind …)` source form), we pre-evaluate it
                    // via the standard `EvalGroundedArgs` mechanism so the
                    // re-dispatched `superpose-bind` sees the evaluated
                    // tuple. Already-evaluated args (whose head is not an
                    // atom, or whose head is a non-reducible data atom)
                    // proceed directly to `StartSuperposeBind`.
                    let needs_preeval = items[1]
                        .as_sexpr()
                        .and_then(|s| s.first())
                        .and_then(|h| h.as_atom())
                        .map(|head| {
                            crate::backend::eval::trampoline::dispatch_hints::is_reducible_head(head)
                        })
                        .unwrap_or(false);
                    if needs_preeval {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: vec![1],
                            env,
                            depth,
                        };
                    }
                    return GenericEvalStep::StartSuperposeBind {
                        arg: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // superpose - HE-compatible: post-evaluate each element via StartAmb
                "superpose" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "superpose requires 1 argument, got {}. Usage: (superpose list)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let expr = &items[1];
                    // DON'T evaluate the argument - treat it as a data list (HE-compatible)
                    if let Some(elements) = expr.as_sexpr() {
                        if elements.is_empty() {
                            // Empty superpose returns empty (no results) - nondeterministic failure
                            return GenericEvalStep::Done((smallvec![], env));
                        }
                        // Post-evaluate each element via StartAmb (HE-compatible)
                        return GenericEvalStep::StartAmb {
                            alternatives: elements.to_vec(),
                            env,
                            depth,
                        };
                    }
                    if expr.is_unit() {
                        // Unit superposes to empty (no results)
                        return GenericEvalStep::Done((smallvec![], env));
                    }
                    // Single non-tuple arg: evaluate it
                    return GenericEvalStep::StartAmb {
                        alternatives: vec![expr.clone()],
                        env,
                        depth,
                    };
                }

                // Advanced nondeterminism
                "amb" => {
                    if items.len() < 2 {
                        // Empty amb returns empty
                        return GenericEvalStep::Done((smallvec![], env));
                    }
                    let alternatives: Vec<MettaValue> = items[1..].iter().cloned().collect();
                    return GenericEvalStep::StartAmb {
                        alternatives,
                        env,
                        depth,
                    };
                }

                "guard" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "guard requires exactly 1 argument, got {}. Usage: (guard condition)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartGuard {
                        condition: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // commit - native generic implementation (zero conversion)
                // In tree-walker evaluation, commit is a no-op - returns Unit
                "commit" => {
                    return GenericEvalStep::Done((smallvec![ctx.factory().unit()], env));
                }

                // backtrack - native generic implementation (zero conversion)
                // Force immediate backtracking - returns empty (nondeterministic failure)
                "backtrack" => {
                    return GenericEvalStep::Done((smallvec![], env));
                }

                "get-atoms" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "get-atoms requires exactly 1 argument, got {}. Usage: (get-atoms space)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartGetAtoms {
                        space_ref: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // State operations
                "new-state" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "new-state requires exactly 1 argument, got {}. Usage: (new-state initial-value)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartNewState {
                        initial_value: items[1].clone(),
                        env,
                        depth,
                    };
                }

                "get-state" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "get-state requires exactly 1 argument, got {}. Usage: (get-state state)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartGetState {
                        state_ref: items[1].clone(),
                        env,
                        depth,
                    };
                }

                "change-state!" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "change-state! requires exactly 2 arguments, got {}. Usage: (change-state! state new-value)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartChangeState {
                        state_ref: items[1].clone(),
                        new_value: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // Memoization operations
                "new-memo" => {
                    if items.len() < 2 || items.len() > 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "new-memo requires 1-2 arguments, got {}. Usage: (new-memo name [size])",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let size_arg = if items.len() == 3 {
                        Some(items[2].clone())
                    } else {
                        None
                    };
                    return GenericEvalStep::StartNewMemo {
                        name_arg: items[1].clone(),
                        size_arg,
                        env,
                        depth,
                    };
                }

                "memo" | "memo-first" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "{} requires exactly 2 arguments, got {}. Usage: ({} memo-table expr)",
                                op, arg_count, op
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let first_only = op == "memo-first";
                    return GenericEvalStep::StartMemo {
                        memo_ref: items[1].clone(),
                        expr: items[2].clone(),
                        first_only,
                        env,
                        depth,
                    };
                }

                "clear-memo!" | "memo-stats" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "{} requires exactly 1 argument, got {}. Usage: ({} memo-table)",
                                op, arg_count, op
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let op_type = if op == "clear-memo!" {
                        super::MemoOpType::Clear
                    } else {
                        super::MemoOpType::Stats
                    };
                    return GenericEvalStep::StartMemoOp {
                        memo_ref: items[1].clone(),
                        op_type,
                        env,
                        depth,
                    };
                }

                // Token binding
                "bind!" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "bind! requires exactly 2 arguments, got {}. Usage: (bind! token atom)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let token = match items[1].as_atom() {
                        Some(t) => t.to_string(),
                        None => {
                            let err = ctx.factory().error(
                                ctx.factory().sexpr(items),
                                ctx.factory().string(
                                    "bind! requires an atom as first argument",
                                ),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    };
                    return GenericEvalStep::StartBind {
                        token,
                        atom_expr: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // S9 PRAGMA-RET (2026-05-14): HE-bisimilar `pragma!`.
                //
                // HE source: hyperon-experimental/lib/src/metta/runner/stdlib/
                // core.rs:21-53 — `PragmaOp::execute`. Validates the key (atom),
                // validates the value for `max-stack-depth` (must be unsigned
                // int), then stores `(key, value)` in `settings` and returns
                // `unit_result()`.
                //
                // MeTTaTron currently has no first-class `PragmaSettings`;
                // pragmas are no-ops semantically. The HE-bisimilar return
                // value is Unit. Validation for `max-stack-depth` is preserved
                // because the conformance fixtures (T06-stdlib/102) assert the
                // exact `UnsignedIntegerIsExpected` Error wording.
                "pragma!" => {
                    if items.len() != 3 {
                        return GenericEvalStep::Done((
                            smallvec![ctx.factory().error(
                                ctx.factory().sexpr(items),
                                ctx.factory().string(
                                    "pragma! expects key and value as arguments",
                                ),
                            )],
                            env,
                        ));
                    }
                    let key = match items[1].as_atom() {
                        Some(k) => k,
                        None => {
                            return GenericEvalStep::Done((
                                smallvec![ctx.factory().error(
                                    ctx.factory().sexpr(items),
                                    ctx.factory().string(
                                        "pragma! expects symbol atom as a key",
                                    ),
                                )],
                                env,
                            ));
                        }
                    };
                    if key == "max-stack-depth" {
                        // HE: parse value as usize. Negative or non-integer -> Error.
                        let value_ok = match items[2].as_long() {
                            Some(n) if n >= 0 => true,
                            _ => false,
                        };
                        if !value_ok {
                            return GenericEvalStep::Done((
                                smallvec![ctx.factory().error(
                                    ctx.factory().sexpr(items),
                                    ctx.factory().atom("UnsignedIntegerIsExpected"),
                                )],
                                env,
                            ));
                        }
                    }
                    // S-step (2026-05-16): persist pragma settings on env.
                    // HE-bisim: store all key/value pairs; semantic effect
                    // only for keys MTT recognizes (`type-check` controls
                    // call-site checking via `check_call_site_types`).
                    use crate::backend::environment::core::{RuleFireMode, TypeCheckMode};
                    if key == "type-check" {
                        if let Some(mode_atom) = items[2].as_atom() {
                            match mode_atom {
                                "auto" => env.set_type_check_mode(TypeCheckMode::Auto),
                                "permissive" => {
                                    env.set_type_check_mode(TypeCheckMode::Permissive)
                                }
                                _ => env.set_pragma_other(key, mode_atom),
                            }
                        }
                    } else if key == "rule-fire-mode" {
                        // MTT SUPERSET (2026-05-17): multi-pattern rule dispatch.
                        // `specificity` engages the structural-depth-weighted
                        // filter so naive recursive patterns (e.g., overlapping
                        // base + variable Fibonacci rules) terminate cleanly.
                        // `nondet` (default) is HE-bisim — all matching rules
                        // fire nondeterministically. See PragmaSettings docs.
                        if let Some(mode_atom) = items[2].as_atom() {
                            match mode_atom {
                                "specificity" => {
                                    env.set_rule_fire_mode(RuleFireMode::Specificity)
                                }
                                "nondet" => env.set_rule_fire_mode(RuleFireMode::Nondet),
                                _ => env.set_pragma_other(key, mode_atom),
                            }
                        }
                    } else if key != "max-stack-depth" {
                        // Store unknown keys as stringified (key already validated above)
                        let value_str = if let Some(s) = items[2].as_string() {
                            s.to_string()
                        } else if let Some(a) = items[2].as_atom() {
                            a.to_string()
                        } else if let Some(n) = items[2].as_long() {
                            n.to_string()
                        } else {
                            format!("{:?}", items[2])
                        };
                        env.set_pragma_other(key, &value_str);
                    }
                    return GenericEvalStep::Done((smallvec![ctx.factory().unit()], env));
                }

                // S9 PRAGMA-RET (2026-05-14): HE-bisimilar `add-reduct`.
                //
                // HE source: hyperon-experimental/lib/src/metta/runner/stdlib/
                // stdlib.metta:567-568. Rule body: `(add-atom $dst $atom)`,
                // where `$atom` is reduced because it's a normal rule arg.
                //
                // Lower to `(chain $atom $r (add-atom $space $r))`: `chain`
                // evaluates the atom expression first and binds the reduced
                // result to a synthetic variable, then routes through the
                // existing `add-atom` special form (which evaluates the space
                // arg and performs the side effect). Returns Unit.
                "add-reduct" => {
                    if items.len() != 3 {
                        return GenericEvalStep::Done((
                            smallvec![ctx.factory().error(
                                ctx.factory().sexpr(items),
                                ctx.factory().atom("IncorrectNumberOfArguments"),
                            )],
                            env,
                        ));
                    }
                    let r_var = ctx.factory().atom("$__add_reduct_r__");
                    let add_atom_expr = ctx.factory().sexpr(vec![
                        ctx.factory().atom("add-atom"),
                        items[1].clone(),
                        r_var.clone(),
                    ]);
                    let chain_expr = ctx.factory().sexpr(vec![
                        ctx.factory().atom("chain"),
                        items[2].clone(),
                        r_var,
                        add_atom_expr,
                    ]);
                    return GenericEvalStep::EvalIfBranch {
                        branch: chain_expr,
                        env,
                        depth,
                    };
                }

                // S9 PRAGMA-RET (2026-05-14): HE-bisimilar `add-reducts`.
                //
                // HE source: hyperon-experimental/lib/src/metta/runner/stdlib/
                // stdlib.metta:671-673. Rule body:
                //   `(foldl-atom $tuple () $a $b (add-atom $space $b))`
                // The `add-reducts` type signature declares the second arg as
                // `%Undefined%`, which means each tuple element is reduced
                // before iteration. We model that by wrapping `$b` in a
                // `chain` so the element is evaluated before `add-atom`.
                //
                // The foldl result IS the return value: Unit on success
                // (init is `()` and each add-atom returns Unit), Error if
                // `$tuple` is not a list (foldl-atom errors).
                "add-reducts" => {
                    if items.len() != 3 {
                        return GenericEvalStep::Done((
                            smallvec![ctx.factory().error(
                                ctx.factory().sexpr(items),
                                ctx.factory().atom("IncorrectNumberOfArguments"),
                            )],
                            env,
                        ));
                    }
                    let space = items[1].clone();
                    let tuple = items[2].clone();
                    let acc_var = ctx.factory().atom("$__add_reducts_a__");
                    let item_var = ctx.factory().atom("$__add_reducts_b__");
                    let r_var = ctx.factory().atom("$__add_reducts_r__");
                    let inner = ctx.factory().sexpr(vec![
                        ctx.factory().atom("add-atom"),
                        space,
                        r_var.clone(),
                    ]);
                    let body = ctx.factory().sexpr(vec![
                        ctx.factory().atom("chain"),
                        item_var.clone(),
                        r_var,
                        inner,
                    ]);
                    let foldl_expr = ctx.factory().sexpr(vec![
                        ctx.factory().atom("foldl-atom"),
                        tuple,
                        ctx.factory().unit(),
                        acc_var,
                        item_var,
                        body,
                    ]);
                    return GenericEvalStep::EvalIfBranch {
                        branch: foldl_expr,
                        env,
                        depth,
                    };
                }

                // S9 PRAGMA-RET (2026-05-14): HE-bisimilar `add-atoms`.
                //
                // HE source: hyperon-experimental/lib/src/metta/runner/stdlib/
                // stdlib.metta:681-683. Same rule body as `add-reducts` but
                // the type signature declares the second arg as `Expression`,
                // preventing arg reduction. So each `$b` is added AS-IS without
                // evaluation. As with `add-reducts`, the foldl result is the
                // return value (Unit / Error).
                "add-atoms" => {
                    if items.len() != 3 {
                        return GenericEvalStep::Done((
                            smallvec![ctx.factory().error(
                                ctx.factory().sexpr(items),
                                ctx.factory().atom("IncorrectNumberOfArguments"),
                            )],
                            env,
                        ));
                    }
                    let space = items[1].clone();
                    let tuple = items[2].clone();
                    let acc_var = ctx.factory().atom("$__add_atoms_a__");
                    let item_var = ctx.factory().atom("$__add_atoms_b__");
                    let body = ctx.factory().sexpr(vec![
                        ctx.factory().atom("add-atom"),
                        space,
                        item_var.clone(),
                    ]);
                    let foldl_expr = ctx.factory().sexpr(vec![
                        ctx.factory().atom("foldl-atom"),
                        tuple,
                        ctx.factory().unit(),
                        acc_var,
                        item_var,
                        body,
                    ]);
                    return GenericEvalStep::EvalIfBranch {
                        branch: foldl_expr,
                        env,
                        depth,
                    };
                }

                // I/O operations.
                "println!" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                            "println! requires exactly 1 argument, got {}. Usage: (println! atom)",
                            arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartPrintln {
                        atom: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // Z.A.6.b (2026-05-12, revised): `print-alternatives!` is HE's
                // 2-arg debug primitive at `hyperon-experimental/lib/src/metta/
                // runner/stdlib/debug.rs:159`. Signature differs from `println!`:
                //   (print-alternatives! <msg-atom> <expr-list>)
                // Prints "N <msg>:" then "    <child>" for each child of the
                // expression list, returns Unit. Distinct from `println!`
                // (1-arg, just renders the atom).
                "print-alternatives!" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "print-alternatives! requires exactly 2 arguments, got {}. Usage: (print-alternatives! msg expr-list)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    let msg = match items[1].as_atom() {
                        Some(s) => s.to_string(),
                        None => match items[1].as_string() {
                            Some(s) => s.to_string(),
                            None => format!("{}", items[1]),
                        },
                    };
                    let children: Vec<MettaValue> = match items[2].as_sexpr() {
                        Some(items) => items.iter().cloned().collect(),
                        None => vec![items[2].clone()],
                    };
                    println!("{} {}:", children.len(), msg);
                    for child in &children {
                        println!("    {}", child);
                    }
                    return GenericEvalStep::Done((smallvec![ctx.factory().unit()], env));
                }

                "trace!" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "trace! requires exactly 2 arguments, got {}. Usage: (trace! message value)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartTrace {
                        message: items[1].clone(),
                        value_expr: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // nop - returns Unit (NO conversion needed)
                "nop" => {
                    return GenericEvalStep::Done((smallvec![ctx.factory().unit()], env));
                }

                // String operations
                "repr" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!(
                                "repr requires exactly 1 argument, got {}. Usage: (repr atom)",
                                arg_count
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartRepr {
                        atom: items[1].clone(),
                        env,
                        depth,
                    };
                }

                "format-args" => {
                    if items.len() != 3 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "format-args requires exactly 2 arguments, got {}. Usage: (format-args format-string args)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartFormatArgs {
                        format_arg: items[1].clone(),
                        args_arg: items[2].clone(),
                        env,
                        depth,
                    };
                }

                // empty - MeTTa HE semantics: zero results (branch annihilation)
                // In MeTTa HE, (empty) produces zero results, causing Cartesian product
                // collapse in parent grounded ops. This enables clean branch death when
                // e.g. `/safe` division guards hit zero divisors, where `/safe` is
                // defined as `(= (/safe $A $B) (if (> $B 0.0) (/ $A $B) (empty)))`
                "empty" => {
                    return GenericEvalStep::Done((smallvec![], env));
                }

                // S14e (2026-05-14): `(context-space)` returns the current evaluation
                // context's space, mirroring HE's `context_space` in
                // `hyperon-experimental/lib/src/metta/interpreter.rs:954`. Equivalent
                // to evaluating the `&self` atom: both yield `factory.space(env.self_space())`.
                // Arity > 0 raises the HE-shaped error per K T04-kernel/029.
                "context-space" => {
                    if items.len() != 1 {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items.clone()),
                            ctx.factory().string(&format!(
                                "expected: (context-space), found: {}",
                                ctx.factory().sexpr(items).friendly_repr()
                            )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::Done((
                        smallvec![ctx.factory().space(env.self_space())],
                        env,
                    ));
                }

                "get-metatype" => {
                    if items.len() != 2 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "get-metatype requires exactly 1 argument, got {}. Usage: (get-metatype atom)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    return GenericEvalStep::StartGetMetatype {
                        atom: items[1].clone(),
                        env,
                        depth,
                    };
                }

                // Module operations - use generic implementations directly (zero-conversion)
                // Passes full ctx (not just factory) so import/include can force-eval `!` expressions
                "include" => {
                    let (results, new_env) = eval_include_generic(items, env, ctx);
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                // Z.A.6.c (2026-05-12, revised): `register-module!` is HE's analog at
                // `hyperon-experimental/lib/src/metta/runner/stdlib/package.rs:35-51`.
                // Its `execute` calls `metta.load_module_at_path(path, None)`, which
                // **loads the file as a module** (separately namespaced) rather than
                // splicing contents into the current scope. The semantically correct
                // MeTTaTron analog is `import!` (which loads a file as a named module
                // and registers it in the module registry), NOT `include` (which is
                // inline evaluation).
                //
                // We dispatch `register-module!` through `eval_import_generic` —
                // import!'s 2-arg form `(import! <path>)` takes a single path argument
                // and matches HE's `register-module!` signature/semantics precisely.
                "import!" | "register-module!" => {
                    let (results, new_env) = eval_import_generic(items, env, ctx);
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                "git-import!" | "git-module!" => {
                    // PeTTa-compatible: clone a git repo and register its directory
                    // as a library search path. Pure side-effecting form: returns Unit
                    // on success or an error MettaValue on any failure (no panics).
                    //
                    // Z.A.6b (2026-05-12): `git-module!` is the HE-canonical name
                    // (`hyperon-experimental/lib/src/metta/runner/stdlib/package.rs:117`).
                    // MeTTaTron used `git-import!` historically; both names dispatch
                    // to the same handler so HE-sourced MeTTa modules using
                    // `git-module!` resolve correctly without rewrites.
                    let results = crate::backend::eval::git_import::eval_git_import_generic(
                        &items,
                        ctx.factory(),
                    );
                    return GenericEvalStep::Done((SmallVec::from_vec(results), env));
                }
                "mod-space!" => {
                    let (results, new_env) = eval_mod_space_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                "print-mods!" => {
                    let (results, new_env) = eval_print_mods_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                // Workstream X.5g — MTT-FN-GETMODULES.
                "get-modules" => {
                    let (results, new_env) =
                        eval_get_modules_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }

                // MORK special forms - use generic implementations directly (zero-conversion)
                "exec" => {
                    let (results, new_env) = eval_exec_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                "coalg" => {
                    let (results, new_env) = eval_coalg_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                "lookup" => {
                    let (results, new_env) = eval_lookup_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }
                "rulify" => {
                    let (results, new_env) = eval_rulify_generic(items, env, ctx.factory());
                    return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
                }

                // if-equal — alpha-equivalence with lazy branches (MeTTa HE compatible)
                "if-equal" => {
                    if items.len() != 5 {
                        let arg_count = items.len() - 1;
                        let err = ctx.factory().error(
                        ctx.factory().sexpr(items),
                        ctx.factory().string(&format!(
                            "if-equal requires exactly 4 arguments, got {}. Usage: (if-equal pred1 pred2 then else)",
                            arg_count
                        )),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                    // Alpha-equivalence comparison (matches MeTTa HE's atoms_are_equivalent)
                    if crate::backend::eval::alpha_equiv::atoms_are_alpha_equivalent(
                        &items[1], &items[2],
                    ) {
                        return GenericEvalStep::EvalIfBranch {
                            branch: items[3].clone(),
                            env,
                            depth,
                        };
                    } else {
                        return GenericEvalStep::EvalIfBranch {
                            branch: items[4].clone(),
                            env,
                            depth,
                        };
                    }
                }

                // Set operations — generic multiset semantics
                //
                // Like list operations, set operations must evaluate reducible
                // arguments before operating. E.g., `(unique-atom (collapse ...))`.
                "unique-atom" | "alpha-unique-atom" | "struct-unique-atom" | "union-atom"
                | "intersection-atom" | "subtraction-atom" => {
                    let reducible_indices = list_op_reducible_arg_indices(&items, &env);
                    if !reducible_indices.is_empty() {
                        return GenericEvalStep::EvalGroundedArgs {
                            items,
                            grounded_indices: reducible_indices,
                            env,
                            depth,
                        };
                    }
                    return crate::backend::eval::set_ops::eval_set_op_generic(items, env, ctx);
                }

                // Bare set-op aliases (Workstream X.5b — MTT-FN-SET-BARE; T06/108-111)
                //
                // HE-bisimilar desugar matching `hyperon-experimental/lib/src/metta/
                // runner/stdlib/stdlib.metta:629-663`. HE uses a let-chain so that
                // the inner `(op-atom (collapse arg)…)` is FIRST evaluated to a
                // tuple, then `(superpose tuple)` re-emits as multi-result:
                //
                //   (unique $a)
                //   = (let $c (collapse $a) (let $u (unique-atom $c) (superpose $u)))
                //
                // A direct nesting `(superpose (op-atom (collapse arg)…))` would
                // be WRONG: MeTTaTron's `superpose` treats its argument as data
                // (the literal tuple to fan out), not as an expression to evaluate
                // first. So we must materialize the inner result via `let` before
                // superposing.
                //
                // The same shape applies for the 2-arg ops (union / intersection /
                // subtraction) — each collapse + the outer let → superpose.
                //
                // 2026-05-17: gated by `dispatch_overrides` — user rules of
                // the form `(= (unique $x) ...)` etc. shadow the bare-setop
                // desugar. HE stdlib defines these as METTA-level rules
                // (stdlib.metta:629-663) so they are user-overridable.
                "unique" | "union" | "intersection" | "subtraction" => {
                    let id = overridable_op_id(op)
                        .expect("bare set-ops registered in OverridableOpId");
                    if env.dispatch_overrides().is_overridden(id) {
                        break 'special_forms;
                    }
                    let factory = ctx.factory();
                    let op_atom = factory.atom(&format!("{}-atom", op));
                    let collapse_sym = factory.atom("collapse");
                    let superpose_sym = factory.atom("superpose");
                    let let_sym = factory.atom("let");
                    let u_var = factory.atom("$__set_u");

                    // Build `(op-atom (collapse arg1) (collapse arg2)…)`.
                    let mut inner_call: Vec<MettaValue> = Vec::with_capacity(items.len());
                    inner_call.push(op_atom);
                    for arg in items.into_iter().skip(1) {
                        inner_call.push(factory.sexpr(vec![collapse_sym.clone(), arg]));
                    }
                    let inner_sexpr = factory.sexpr(inner_call);

                    // Wrap: `(let $__set_u <inner> (superpose $__set_u))` — the
                    // `let` evaluates `<inner>` to a single tuple `$__set_u`, then
                    // `(superpose $__set_u)` fans out its elements.
                    let superpose_call =
                        factory.sexpr(vec![superpose_sym, u_var.clone()]);
                    let wrapped = factory.sexpr(vec![
                        let_sym,
                        u_var,
                        inner_sexpr,
                        superpose_call,
                    ]);
                    return GenericEvalStep::EvalIfBranch {
                        branch: wrapped,
                        env,
                        depth,
                    };
                }

                // Alpha equivalence — (=alpha expr1 expr2) → Bool
                "=alpha" => {
                    return crate::backend::eval::testing_ops::eval_testing_op_generic(
                        items, env, ctx,
                    );
                }

                // PeTTa-compatible test — (test actual expected) → Unit | Error
                // Both args are pre-evaluated (applicative order). Compares with
                // alpha-equivalence and prints a diagnostic line. Returns Unit on
                // match, error MettaValue on mismatch (NEVER halts/panics).
                //
                // T06/095 (2026-05-17): `test` is in the overridable set, so a
                // user-defined rule `(= (test $v) ...)` shadows the PeTTa
                // builtin. Without this gate, the builtin's 2-arg arity check
                // fires first and produces a spurious "test requires exactly
                // 2 arguments" error when called with 1 arg (e.g. inside
                // `(filter-atom $coll $v (test $v))`). Falling through to
                // rule matching via `break 'special_forms` lets the user
                // rule apply for ANY arity. When no user rule exists the
                // gate is false (single relaxed atomic load ~1 ns) and
                // dispatch proceeds to the PeTTa builtin as before.
                "test" => {
                    let id = overridable_op_id("test")
                        .expect("test is registered in OverridableOpId");
                    if env.dispatch_overrides().is_overridden(id) {
                        break 'special_forms;
                    }
                    return crate::backend::eval::testing_ops::eval_testing_op_generic(
                        items, env, ctx,
                    );
                }

                // Testing/assertion operations — multiset nondeterministic comparison
                "assertEqual"
                | "assertAlphaEqual"
                | "assertEqualMsg"
                | "assertAlphaEqualMsg"
                | "assertEqualToResult"
                | "assertAlphaEqualToResult"
                | "assertEqualToResultMsg"
                | "assertAlphaEqualToResultMsg" => {
                    return crate::backend::eval::testing_ops::eval_testing_op_generic(
                        items, env, ctx,
                    );
                }

                // Step 1: Try grounded operations with RAW (unevaluated) arguments
                _ => {
                    // Phase 9.1: Variable-head guard.
                    // Variable-headed S-expressions (e.g. ($f x y)) cannot match any
                    // rule or grounded op. Send directly to tuple path (evaluate
                    // sub-elements independently). This is HE-equivalent behavior
                    // (interpreter.rs:604–611).
                    if op.starts_with('$') {
                        return GenericEvalStep::EvalSExpr { items, env, depth };
                    }

                    // Phase 9.6 + Phase 1 cache: Compute parent op types ONCE.
                    // This result is reused by Phase 9.6 (all-error-types check),
                    // find_typed_arg_indices_generic (Step 2), and
                    // is_declared_value_type (Step 2.5) — avoids 3x redundant
                    // RwLock reads + bloom hash + supertype closure.
                    let op_types = if env.may_have_type(op) {
                        env.get_types_generic(op)
                    } else {
                        Vec::new()
                    };

                    // Phase 9.6: All-error-types early exit.
                    // If ALL declared types for this operator are error types,
                    // skip rule matching and return an error immediately.
                    if !op_types.is_empty()
                        && op_types.iter().all(|t| {
                            t.as_sexpr().map_or(false, |type_items| {
                                type_items.first().and_then(|v| v.as_atom()) == Some("Error")
                            })
                        })
                    {
                        let err = ctx.factory().error(
                            ctx.factory().sexpr(items),
                            ctx.factory().string(&format!("All types for '{}' are errors", op)),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }

                    // 2026-05-17: dispatch override gate for overridable
                    // built-ins (id, test, car-atom, map-atom, …). When a
                    // user rule exists for this name, the grounded fast
                    // path is skipped and rule matching at the bottom of
                    // this function handles dispatch. Class A (HE-reserved
                    // kernel ops like +, ==, eval) have NO OverridableOpId
                    // and pass through unconditionally.
                    let overridden = overridable_op_id(op).map_or(false, |id| {
                        env.dispatch_overrides().is_overridden(id)
                    });

                    // Try generic grounded operation (zero-conversion path)
                    // Uses static dispatch - works with any V: MettaValueTrait
                    if has_grounded_op(op) && !overridden {
                        let args: Vec<MettaValue> = items[1..].to_vec();
                        // X.4 — HE Empty annihilation (MTT-EMPTY-ANNIHILATION).
                        // If any argument is the Empty sentinel OR the literal
                        // `Empty` symbol, short-circuit to Empty per HE's
                        // `interpret_tuple` return_on_error (lib/src/metta/
                        // interpreter.rs:1406). Catches the literal-Empty
                        // case in initial args; arg-evaluation-produces-Empty
                        // is handled by per-op is_empty() checks inside
                        // GroundedOperationTCO::execute_step.
                        if args.iter().any(|a| {
                            a.is_empty()
                                || matches!(a.view(), ValueView::Atom(s) if s == "Empty")
                        }) {
                            return GenericEvalStep::Done((
                                smallvec![ctx.factory().empty()],
                                env,
                            ));
                        }
                        // Phase 8.8: Pre-validate ground-type args against arrow signature.
                        // Returns clear type error instead of NoReduce → unreduced expression.
                        if let Some(type_error) =
                            validate_grounded_arg_types(op, &args, ctx.factory())
                        {
                            return GenericEvalStep::Done((smallvec![type_error], env));
                        }
                        // Use GroundedState with native value type - NO conversion needed
                        let state = GroundedState::new(op.to_string(), args);
                        return GenericEvalStep::StartGroundedOp { state, env, depth };
                    }

                    // Store cached types for Steps 2 & 2.5 (outside this match arm)
                    cached_parent_op_types = Some(op_types);
                }
            }
        } // 'special_forms
    }

    // Step 2: Applicative pre-evaluation of S-expression arguments.
    //
    // Two sources of pre-eval indices:
    // (a) Type-driven: operator has an arrow type `(-> T1 T2 ... Tret)`,
    //     meta-typed args are passed unevaluated, value-typed S-expr args
    //     are pre-evaluated (MeTTa HE's `interpret_function` path).
    //     If the type system was consulted (returns Some), we use ONLY its
    //     result — do NOT fall through to bloom filter even if the index
    //     list is empty (all args are meta-typed → no pre-eval needed).
    // (b) Bloom filter: operator has NO type; S-expr args whose head has
    //     rules are pre-evaluated (call-by-value). Fixpoint detection in
    //     `CollectGroundedArg` prevents infinite loops on false positives.
    //
    // This MUST fire BEFORE rule matching (Step 3). Otherwise, rules match
    // with unevaluated args (e.g., `(g (f))` matches `(g $x)` binding
    // `$x = (f)` instead of pre-evaluating `(f)` → {1,2,3} first).
    match find_typed_arg_indices_generic(&items, &env, cached_parent_op_types.as_deref()) {
        Some(typed_indices) => {
            // Type system was consulted. Use only its result.
            if !typed_indices.is_empty() {
                // Trace: ApplicativePreEval (type-driven)
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let operator = items
                            .first()
                            .and_then(|v| v.as_atom())
                            .unwrap_or("?")
                            .to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(
                                &ctx.factory().sexpr(items.clone()),
                            ),
                            vec![],
                            None,
                            trace_format::TraceEventKind::ApplicativePreEval {
                                operator,
                                arg_indices: typed_indices.iter().map(|&i| i as u16).collect(),
                                source: "type-driven".to_string(),
                            },
                        );
                    }
                }
                return GenericEvalStep::EvalGroundedArgs {
                    items,
                    grounded_indices: typed_indices,
                    env,
                    depth,
                };
            }
            // All args are meta-typed — skip pre-eval, fall through to rule matching.
        }
        None => {
            // No type info — use bloom filter fallback.
            let bloom_indices = find_grounded_arg_indices_generic(&items, &env);
            if !bloom_indices.is_empty() {
                // Trace: ApplicativePreEval (bloom-filter)
                #[cfg(feature = "trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let operator = items
                            .first()
                            .and_then(|v| v.as_atom())
                            .unwrap_or("?")
                            .to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(
                                &ctx.factory().sexpr(items.clone()),
                            ),
                            vec![],
                            None,
                            trace_format::TraceEventKind::ApplicativePreEval {
                                operator,
                                arg_indices: bloom_indices.iter().map(|&i| i as u16).collect(),
                                source: "bloom-filter".to_string(),
                            },
                        );
                    }
                }
                return GenericEvalStep::EvalGroundedArgs {
                    items,
                    grounded_indices: bloom_indices,
                    env,
                    depth,
                };
            }
        }
    }

    // Step 2.5: Data constructor shortcut — if operator has ONLY value types
    // (no arrow types), it can't have rules. Skip to tuple path directly.
    // This avoids unnecessary rule matching for known data constructors.
    if let Some(op) = items.first().and_then(|v| v.as_atom()) {
        if is_declared_value_type(op, &env, cached_parent_op_types.as_deref()) {
            return GenericEvalStep::EvalSExpr { items, env, depth };
        }
    }

    // Step 3: Rule matching with unevaluated arguments (lazy evaluation).
    // Only reached when Step 2 found no args to pre-evaluate.
    // Use original_sexpr if available (avoids redundant factory.sexpr allocation).
    let resolved_sexpr = original_sexpr.unwrap_or_else(|| ctx.factory().sexpr(items.clone()));
    let all_matches = crate::backend::eval::trampoline::try_match_all_rules(
        &resolved_sexpr,
        &env,
        *ctx.factory(),
    );

    if !all_matches.is_empty() {
        // P1 trace event: native path produced matches; no unify ran.
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let (call_head, call_arity) = match resolved_sexpr.as_sexpr() {
                    Some(items) => {
                        let head = items
                            .first()
                            .and_then(|v| v.as_atom())
                            .unwrap_or("")
                            .to_string();
                        (head, items.len().saturating_sub(1) as u32)
                    }
                    None => (resolved_sexpr.as_atom().unwrap_or("").to_string(), 0u32),
                };
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    crate::backend::trace::trace_value_generic(&resolved_sexpr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RuleMatchDispatchPath {
                        call_head,
                        call_arity,
                        path: "native".to_string(),
                        native_count: all_matches.len() as u32,
                        unify_count: 0,
                        expr_has_variables: resolved_sexpr.has_variables_fast(),
                    },
                );
            }
        }
        // Trace: RuleMatchSet
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let match_count = all_matches.len() as u32;
                let matches_tv: Vec<(trace_format::TraceValue, Option<trace_format::TraceSpan>)> =
                    all_matches
                        .iter()
                        .map(|(rhs, _bindings, _rhs_type)| {
                            (crate::backend::trace::trace_value_generic(rhs), None)
                        })
                        .collect();
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    crate::backend::trace::trace_value_generic(&resolved_sexpr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RuleMatchSet {
                        match_count,
                        matches: matches_tv,
                    },
                );
            }
        }
        // User rules matched — evaluate RHS with bindings from pattern match
        // Phase 8.7: rhs_type (3rd element) preserved for branch pruning at trampoline level
        return GenericEvalStep::EvalRuleMatchesLazy {
            matches: all_matches,
            env,
            depth,
        };
    }

    // Step 3.5: HE-conformant free-variable rule enumeration.
    //
    // Step 3 above uses StructuralMatcher which does literal atom comparison
    // — it fails when a query position has a free variable that should
    // unify with the rule's atom at the same position. PeTTa / MeTTa HE
    // handle this case via Prolog-style unification: e.g. `(father a $b)`
    // against rule `(father a b)` should bind `$b → b` and produce the
    // rule's RHS as a result. Without this branch, expressions with free
    // variables silently self-evaluate, breaking PLN's `?` macro and any
    // free-variable query.
    //
    // Gating:
    //   - The query must contain free variables (`has_variables_fast()`).
    //     The ground case is fully handled by Step 3 above.
    //   - Step 3 must have returned no matches (otherwise the structural
    //     fast path already produced the right answer).
    //   - The head atom must be present and not a special form
    //     (special forms like `if`, `match`, `let` are dispatched in
    //     Step 1 / 2 and never reach here).
    //
    // The matches are returned via the existing `EvalRuleMatchesLazy`
    // dispatch, so all downstream cut handling, trace events, fork
    // accounting and post-processing are inherited from the ground path.
    if resolved_sexpr.has_variables_fast() {
        let unified_matches =
            crate::backend::eval::trampoline::engine::enumerate_rules_via_unification(
                &resolved_sexpr,
                &env,
                ctx.factory(),
            );
        #[cfg(feature = "trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let (call_head, call_arity) = match resolved_sexpr.as_sexpr() {
                    Some(items) => {
                        let head = items
                            .first()
                            .and_then(|v| v.as_atom())
                            .unwrap_or("")
                            .to_string();
                        (head, items.len().saturating_sub(1) as u32)
                    }
                    None => (resolved_sexpr.as_atom().unwrap_or("").to_string(), 0u32),
                };
                let path = if unified_matches.is_empty() {
                    "neither"
                } else {
                    "unify"
                };
                tc.emit_converted(
                    trace_format::TraceTier::TreeWalker,
                    depth as u32,
                    crate::backend::trace::trace_value_generic(&resolved_sexpr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RuleMatchDispatchPath {
                        call_head,
                        call_arity,
                        path: path.to_string(),
                        native_count: 0,
                        unify_count: unified_matches.len() as u32,
                        expr_has_variables: true,
                    },
                );
            }
        }
        if !unified_matches.is_empty() {
            #[cfg(feature = "trace")]
            {
                if let Some(tc) = ctx.trace_collector() {
                    let match_count = unified_matches.len() as u32;
                    let matches_tv: Vec<(
                        trace_format::TraceValue,
                        Option<trace_format::TraceSpan>,
                    )> = unified_matches
                        .iter()
                        .map(|(rhs, _bindings, _rhs_type)| {
                            (crate::backend::trace::trace_value_generic(rhs), None)
                        })
                        .collect();
                    tc.emit_converted(
                        trace_format::TraceTier::TreeWalker,
                        depth as u32,
                        crate::backend::trace::trace_value_generic(&resolved_sexpr),
                        vec![],
                        None,
                        trace_format::TraceEventKind::RuleMatchSet {
                            match_count,
                            matches: matches_tv,
                        },
                    );
                }
            }
            return GenericEvalStep::EvalRuleMatchesLazy {
                matches: unified_matches,
                env,
                depth,
            };
        }
    }

    // Step 3.6 (S-step 2026-05-16): Call-site type checking.
    //
    // After rule matching fails (Step 3 didn't take an early return), check
    // whether the call site is ill-typed against the head's declared arrow
    // type. Permissive mode (default) only fires when both head has a
    // concrete `(-> ...)` declaration and arg types are determinable.
    // `(pragma! type-check auto)` switches to strict mode that also fires
    // on `%Undefined%` arg types.
    //
    // HE parity: `hyperon-experimental/lib/src/metta/types.rs::check_type`
    // returns BadArgType / IncorrectNumberOfArguments errors with 1-indexed
    // arg position; same shape emitted here.
    if let Some(err) = check_call_site_types(&items, ctx.factory(), &env) {
        return GenericEvalStep::Done((smallvec![err], env));
    }

    // Step 4: No rules matched. Fall through to tuple path (HE's
    // `interpret_tuple`). Data constructors (no rules) evaluate as tuples;
    // function calls with no matching rule return the call unreduced.
    //
    // NOTE: HE's strict "failed function app → Empty" semantics is NOT
    // applied here globally — it would regress tests that rely on data
    // atoms being stored in the space with the same head as rules (e.g.,
    // `(number 42)` as space atoms alongside `(number ...)` rules).
    // PLN's fail-fast behavior is achieved per-call via `ProcessFoldlAtom`'s
    // Empty-value check (eval_loop.rs Continuation::ProcessFoldlAtom).
    GenericEvalStep::EvalSExpr { items, env, depth }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Identify arguments to a list/tuple operation that are reducible S-expressions
/// needing pre-evaluation before the operation can proceed.
///
/// This ensures MeTTa HE semantic parity: list operations like `car-atom`,
/// `cdr-atom`, `size-atom`, etc. evaluate their arguments before operating on
/// them. Without this, reducible S-expression arguments (e.g., `(collapse ...)`,
/// `(superpose ...)`, or user-defined functions that produce lists) would be
/// treated as structural tuples instead of being evaluated first.
///
/// An argument is considered reducible if it is an S-expression whose head:
/// - Starts with `$` (variable — may resolve to a function)
/// - Is a grounded operation (e.g., `+`, `==`)
/// - Is an eager special form (e.g., `collapse`, `superpose`, `map-atom`)
/// - Has a `(-> ...)` type signature (declared function)
///
/// ## Why Not Use the Bloom Filter
///
/// Unlike `find_grounded_arg_indices_generic` (Step 2 in eval_sexpr_step_inner),
/// this function is called from Step 1 (special form dispatch). The fixpoint
/// detection in `CollectGroundedArg` handles bloom filter false positives by
/// falling through to Steps 3-4 (rule matching → data constructor). However,
/// Steps 3-4 would incorrectly return `(car-atom (a b c))` as a data
/// constructor when `car-atom` should still execute. Since the parent operator
/// is a special form (not a user rule), the fixpoint fallback is wrong.
///
/// By restricting to deterministic checks (grounded ops, eager special forms,
/// type-declared functions), we guarantee that any argument we mark for
/// pre-evaluation WILL change after evaluation, avoiding both infinite loops
/// and incorrect data constructor fallback.
///
/// Returns a vector of 1-based indices into `items` for arguments needing
/// pre-evaluation. Returns empty if all arguments are already in normal form.
fn list_op_reducible_arg_indices(items: &[MettaValue], env: &MettaEnvironment) -> Vec<usize> {
    let mut indices = Vec::new();
    // Skip index 0 (the operator itself), check all arguments
    for (i, item) in items.iter().enumerate().skip(1) {
        if is_reducible_sub_expr(item, env) {
            indices.push(i);
        }
    }
    indices
}

/// Structural-op variant of `list_op_reducible_arg_indices` for `car-atom`/`cdr-atom`.
///
/// These operations perform SYNTACTIC destructuring — they return the head/tail of
/// the expression structure without evaluating it. User-defined function calls must
/// NOT be pre-evaluated because:
///
/// 1. If the call is nondeterministic (multiple matching rules), pre-evaluation forks
///    the trampoline into multiple branches. Each branch receives a cloned Arc<Env>
///    (refcount > 1 → Arc::make_mut clones). Any `add-atom &self` call inside a
///    forked branch writes to a branch-local clone that is DISCARDED after the branch
///    completes — the rule is never visible to subsequent evaluations.
///
/// 2. HE semantics: `car-atom: (-> Atom Atom)` — the argument type `Atom` is a
///    meta-type in HE. Meta-typed arguments are passed unevaluated.
///
/// Arguments that SHOULD be pre-evaluated:
///   - Eager special forms (collapse, superpose, eval, …) — always list-producing
///   - Grounded ops (+, ==, …) — always return a concrete value
///   - Type-declared functions (should_pre_eval_by_type) — explicitly typed with (-> …)
///
/// Arguments that must NOT be pre-evaluated:
///   - User-defined function calls (bloom filter) — may produce nondeterminism
///
fn structural_op_reducible_arg_indices(items: &[MettaValue], env: &MettaEnvironment) -> Vec<usize> {
    let mut indices = Vec::new();
    for (i, item) in items.iter().enumerate().skip(1) {
        if is_reducible_structural_arg(item, env) {
            indices.push(i);
        }
    }
    indices
}

/// Returns true if `value` is a reducible argument for a structural op (`car-atom`,
/// `cdr-atom`). Unlike `is_reducible_sub_expr`, this does NOT include the bloom filter
/// fallback for user-defined function calls.
fn is_reducible_structural_arg(value: &MettaValue, env: &MettaEnvironment) -> bool {
    use crate::backend::eval::helpers::{is_eager_special_form, is_grounded_op};
    use crate::backend::eval::step::grounded::should_pre_eval_by_type;

    let sub_items = match value.as_sexpr() {
        Some(s) => s,
        None => return false,
    };
    let head = match sub_items.first().and_then(|v| v.as_atom()) {
        Some(h) => h,
        None => return false,
    };
    // Variable heads may resolve to functions — always evaluate
    head.starts_with('$')
        // Grounded ops (+, ==, etc.) always produce a concrete result
        || is_grounded_op(head)
        // Eager special forms (collapse, superpose, eval, …) are explicitly
        // designed to produce list values for structural operations
        || is_eager_special_form(head)
        // Type-declared functions with (-> …) signatures are explicitly typed
        // and should be evaluated to their return values
        || should_pre_eval_by_type(head, env)
    // NOTE: intentionally does NOT include:
    //   - bloom filter (env.may_have_rules_for) — user-defined rules must not be pre-evaluated
    //   - inferred types — may misidentify data constructors as functions
}

/// Type-aware variant of `list_op_reducible_arg_indices` for Arm A (TRUE HE
/// primitives). Consults the builtin type signature to skip args with meta
/// types (`Atom`, `Variable`, `Pattern`) — HE does NOT pre-evaluate those.
///
/// This matters for `cons-atom` whose type is `(-> Atom Expression Expression)`:
/// the first arg (type `Atom`) must NOT be pre-evaluated, matching HE's
/// embedded-op semantics where `cons-atom` constructs the result directly
/// from the raw atoms.
fn list_op_reducible_arg_indices_typed(
    op: &str,
    items: &[MettaValue],
    env: &MettaEnvironment,
) -> Vec<usize> {
    use crate::backend::builtin_signatures::{
        get_expected_type_at_position, get_signature, TypeExpr,
    };
    let sig = get_signature(op);
    let mut indices = Vec::new();
    for (i, item) in items.iter().enumerate().skip(1) {
        if let Some(ref s) = sig {
            if let Some(te) = get_expected_type_at_position(s, i - 1) {
                if matches!(te, TypeExpr::Atom | TypeExpr::Variable | TypeExpr::Pattern) {
                    continue;
                }
            }
        }
        if is_reducible_sub_expr(item, env) {
            indices.push(i);
        }
    }
    indices
}

/// Returns true if `value` is a reducible S-expression under the same
/// criteria used by `list_op_reducible_arg_indices`. Used by
/// higher-order list ops (`map-atom`, `filter-atom`, `foldl-atom`,
/// `sort-tuple`, `best-candidate`) to decide whether to pre-evaluate a
/// list argument before extracting its children.
///
/// MeTTa HE applicative-order reduces arguments before function dispatch,
/// so `(map-atom (collapse X) $v T)` first reduces `(collapse X)` to a
/// tuple, then `map-atom` operates on that tuple. MeTTaTron dispatches
/// special forms before normal applicative pre-eval, so these arms must
/// explicitly trigger pre-eval on their list arg.
fn is_reducible_sub_expr(value: &MettaValue, env: &MettaEnvironment) -> bool {
    use crate::backend::eval::helpers::{is_eager_special_form, is_grounded_op};
    use crate::backend::eval::step::grounded::should_pre_eval_by_type;

    let sub_items = match value.as_sexpr() {
        Some(s) => s,
        None => return false,
    };
    let head = match sub_items.first().and_then(|v| v.as_atom()) {
        Some(h) => h,
        None => return false,
    };
    // Variables as heads need evaluation (the var may resolve to a function)
    head.starts_with('$')
        // Grounded ops (e.g. +, ==) always produce a result different
        // from the input S-expression.
        || is_grounded_op(head)
        // Eager special forms (collapse, superpose, map-atom, etc.)
        // always produce a result different from the input.
        || is_eager_special_form(head)
        // Type-driven: operator has (-> ...) type signature, indicating
        // it's a declared function that should evaluate.
        || should_pre_eval_by_type(head, env)
        // Phase 10 inferred-type fallback: operators with inferred
        // arrow types from deep type inference also need pre-eval.
        || (env.has_inferred_type(head)
            && env
                .get_inferred_fn_types(head)
                .iter()
                .any(|t| crate::backend::eval::step::grounded::is_arrow_type(t)))
        // Bloom filter fallback for untyped user-defined operators with
        // rules. This is the same Tier 3 check used by
        // `find_grounded_arg_indices_generic` in `step/grounded.rs`.
        // Without it, calls like `(exclude-item x (kb))` fail to
        // pre-evaluate `(kb)` (a user-defined function call), leaving
        // exclude-item with an unreduced S-expression.
        || env.may_have_rules_for(head, sub_items.len() - 1)
}

/// Extract a variable name (atom starting with `$`) from a value, with a
/// helpful error if the value is not a valid variable.
///
/// Used by `map-atom`, `filter-atom`, and `foldl-atom` variable arguments.
fn extract_var_name<C: EvalContext>(
    var_arg: &MettaValue,
    op_name: &str,
    arg_position: &str,
    env: &MettaEnvironment,
    ctx: &C,
) -> Result<String, GenericEvalStep<MettaValue, MettaEnvironment>>
where
    MettaValue: Clone,
{
    match var_arg.as_atom() {
        Some(name) if name.starts_with('$') => Ok(name.to_string()),
        Some(name) => {
            let msg = match suggest_variable_format(name) {
                Some(suggestion) => format!(
                    "{}: {} must be a variable (starting with $). {}",
                    op_name, arg_position, suggestion
                ),
                None => format!(
                    "{}: {} must be a variable (starting with $)",
                    op_name, arg_position
                ),
            };
            Err(GenericEvalStep::Done((
                smallvec![ctx
                    .factory()
                    .error(var_arg.clone(), ctx.factory().string(&msg))],
                env.clone(),
            )))
        }
        None => {
            let msg = format!(
                "{}: {} must be a variable (starting with $)",
                op_name, arg_position
            );
            Err(GenericEvalStep::Done((
                smallvec![ctx
                    .factory()
                    .error(var_arg.clone(), ctx.factory().string(&msg))],
                env.clone(),
            )))
        }
    }
}

/// Extract list elements from a value that should be a list (S-expression) or unit (empty list).
///
/// Used by `map-atom`, `filter-atom`, and `foldl-atom` list arguments.
fn extract_list_elements<C: EvalContext>(
    list_arg: &MettaValue,
    op_name: &str,
    ctx: &C,
    env: &MettaEnvironment,
) -> Result<Vec<MettaValue>, GenericEvalStep<MettaValue, MettaEnvironment>>
where
    MettaValue: Clone,
{
    match (list_arg.is_unit(), list_arg.as_sexpr()) {
        (true, _) => Ok(vec![]),
        (_, Some(elems)) => Ok(elems.iter().cloned().collect()),
        _ => {
            let err = ctx.factory().error(
                list_arg.clone(),
                ctx.factory()
                    .string(&format!("{} requires a list as first argument", op_name)),
            );
            Err(GenericEvalStep::Done((smallvec![err], env.clone())))
        }
    }
}

/// Preprocess space references: combine `& self` into `&self`.
fn preprocess_space_refs_generic<C: EvalContext>(items: Vec<MettaValue>, ctx: &C) -> Vec<MettaValue>
where
    MettaValue: Clone,
{
    // Look for pattern: [... , "&", "self", ...]
    // and combine into [... , "&self", ...]
    let mut result = Vec::with_capacity(items.len());
    let mut i = 0;

    while i < items.len() {
        if i + 1 < items.len() {
            if let (Some("&"), Some("self")) = (items[i].as_atom(), items[i + 1].as_atom()) {
                result.push(ctx.factory().atom("&self"));
                i += 2;
                continue;
            }
        }
        result.push(items[i].clone());
        i += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::trampoline::StaticEvalContext;
    use crate::backend::models::MettaValueFactory;

    #[test]
    fn test_eval_sexpr_step_generic_empty() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();

        match eval_sexpr_step_generic(vec![], env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                // After BumpVec→slice migration, empty sexpr normalizes to Unit
                assert!(results[0].is_unit());
            }
            _ => panic!("Expected Done with unit"),
        }
    }

    #[test]
    fn test_eval_sexpr_step_generic_quote() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let items = vec![factory.atom("quote"), factory.atom("foo")];

        match eval_sexpr_step_generic(items, env, 0, &ctx) {
            GenericEvalStep::Done((results, _)) => {
                assert_eq!(results.len(), 1);
                // quote now wraps in Quoted variant
                assert!(results[0].is_quoted());
                let inner = results[0].as_quoted().expect("Expected Quoted variant");
                assert_eq!(inner.as_atom(), Some("foo"));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_eval_sexpr_step_generic_if_returns_condition_step() {
        let ctx = StaticEvalContext::get();
        let env = StaticEvalContext::new_env();
        let factory = ctx.factory();

        let items = vec![
            factory.atom("if"),
            factory.bool(true),
            factory.long(1),
            factory.long(2),
        ];

        match eval_sexpr_step_generic(items, env, 0, &ctx) {
            GenericEvalStep::EvalIfCondition {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                assert_eq!(condition.as_bool(), Some(true));
                assert_eq!(then_branch.as_long(), Some(1));
                assert_eq!(else_branch.as_long(), Some(2));
            }
            _ => panic!("Expected EvalIfCondition"),
        }
    }

    #[test]
    fn test_preprocess_space_refs_generic() {
        let ctx = StaticEvalContext::get();
        let factory = ctx.factory();

        let items = vec![
            factory.atom("match"),
            factory.atom("&"),
            factory.atom("self"),
            factory.atom("foo"),
        ];

        let result = preprocess_space_refs_generic(items, &ctx);
        assert_eq!(result.len(), 3);
        assert_eq!(result[1].as_atom(), Some("&self"));
    }
}
