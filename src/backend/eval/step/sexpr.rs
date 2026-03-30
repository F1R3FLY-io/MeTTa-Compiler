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

use smallvec::{SmallVec, smallvec};

use tracing::trace;

use super::types::GenericEvalStep;
use super::grounded::{find_grounded_arg_indices_generic, find_typed_arg_indices_generic, is_declared_value_type, validate_grounded_arg_types};

use crate::backend::eval::bindings::{eval_atom_subst_generic, eval_sealed_generic};
use crate::backend::eval::list_ops::ops::{
    eval_car_atom_generic, eval_cdr_atom_generic, eval_cons_atom_generic,
    eval_decons_atom_generic, eval_drop_atom_generic, eval_element_of_generic,
    eval_flatten_atom_generic, eval_index_atom_generic, eval_max_atom_generic,
    eval_min_atom_generic, eval_range_generic, eval_reverse_atom_generic,
    eval_size_atom_generic, eval_take_atom_generic, eval_tuple_concat_generic,
    eval_tuple_count_generic, eval_without_generic, eval_zip_atom_generic,
};
use crate::backend::eval::list_ops::helpers::suggest_variable_format;
// Generic module operations - used directly (no boundary conversion)
use crate::backend::eval::modules::{
    eval_import_generic, eval_include_generic, eval_mod_space_generic, eval_print_mods_generic,
};
// Generic MORK operations - used directly (no boundary conversion)
use crate::backend::eval::mork_forms::{
    eval_coalg_generic, eval_exec_generic, eval_lookup_generic, eval_rulify_generic,
};
use crate::backend::eval::trampoline::{MettaEnvironment, EvalContext};
use crate::backend::eval::types::{eval_check_type_generic, eval_get_type_generic, types_match_generic};
use crate::backend::grounded::{has_grounded_op, GroundedState};
use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait, SpaceHandle};
use crate::backend::models::metta_value::MettaValueInner;

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
    let original_sexpr = if items.len() != orig_len { None } else { original_sexpr };

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
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let is_special = matches!(op,
                    "=" | "!" | "quote" | "unquote" | "if" | "if-reducible" | "error" | "Error"
                    | "is-error" | "catch" | "eval" | "chain" | "let" | "let*" | ":" | ":<"
                    | "get-type" | "check-type" | "match" | "match-or" | "superpose" | "amb" | "collapse"
                    | "map-atom" | "filter-atom" | "foldl-atom" | "add-atom" | "remove-atom"
                    | "get-atoms" | "new-space" | "new-state" | "get-state" | "change-state!"
                    | "pragma!" | "println!" | "import!" | "include" | "mod-space!"
                    | "print-mods!" | "unique" | "subtraction" | "intersection" | "union"
                    | "assertEqual" | "assertEqualToResult" | "is-function" | "type-cast"
                    | "match-types" | "match-type-or" | "first-from-pair" | "metta"
                );
                if is_special {
                    let input_tv = crate::backend::trace::trace_value_generic(&ctx.factory().sexpr(items.clone()));
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
        match op {
            // Rule definition - native generic implementation (zero-conversion)
            "=" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "= requires exactly 2 arguments, got {}. Usage: (= pattern body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                // Add rule directly - zero-conversion storage
                let mut new_env = env.clone();
                new_env.add_rule(items[1].clone(), items[2].clone());

                // Rule definitions return empty list
                return GenericEvalStep::Done((smallvec![], new_env));
            }

            // Force evaluation operator - defer to trampoline for TCO
            "!" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "! requires exactly 1 argument, got {}. Usage: (! expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
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
                    let err = ctx.factory().error(
                        &format!(
                            "quote requires exactly 1 argument, got {}. Usage: (quote expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                return GenericEvalStep::Done((smallvec![ctx.factory().quote(items[1].clone())], env));
            }

            // Unquote - unwraps Quoted variant, returns inner value
            "unquote" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "unquote requires exactly 1 argument, got {}. Usage: (unquote expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "if requires exactly 3 arguments, got {}. Usage: (if condition then else)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "if-reducible requires exactly 3 arguments, got {}. Usage: (if-reducible expr then else)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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

            // Error construction (NO conversion needed)
            "error" => {
                if items.len() < 2 {
                    return GenericEvalStep::Done((smallvec![], env));
                }
                // Extract message from atom or string
                let msg = items[1].as_string()
                    .or_else(|| items[1].as_atom())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| format!("{:?}", items[1]));
                let details = if items.len() > 2 {
                    items[2].clone()
                } else {
                    ctx.factory().unit()
                };
                return GenericEvalStep::Done((smallvec![ctx.factory().error(&msg, details)], env));
            }

            // HE-compatible Error form (NO conversion needed)
            "Error" => {
                if items.len() < 2 {
                    return GenericEvalStep::Done((smallvec![], env));
                }
                // HE format: (Error details msg)
                let (details, msg) = if items.len() == 2 {
                    (items[1].clone(), String::new())
                } else {
                    let details = items[1].clone();
                    let msg = items[2].as_string()
                        .or_else(|| items[2].as_atom())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| format!("{:?}", items[2]));
                    (details, msg)
                };
                return GenericEvalStep::Done((smallvec![ctx.factory().error(&msg, details)], env));
            }

            // is-error - defers evaluation to trampoline
            "is-error" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "is-error requires exactly 1 argument, got {}. Usage: (is-error expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "catch requires exactly 2 arguments, got {}. Usage: (catch expr default)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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

            // eval - defers evaluation to trampoline
            "eval" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "eval requires exactly 1 argument, got {}. Usage: (eval expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                return GenericEvalStep::EvalEval {
                    arg: items[1].clone(),
                    env,
                    depth,
                };
            }

            // function - defers evaluation to trampoline
            "function" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "function requires exactly 1 argument, got {}. Usage: (function expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
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
                    let err = ctx.factory().error(
                        &format!(
                            "return requires exactly 1 argument, got {}. Usage: (return value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                        &format!(
                            "chain requires exactly 3 arguments, got {}. Usage: (chain expr $var body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "match requires 3 or 4 arguments, got {}. Usage: (match space pattern template) or (match & self pattern template)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "match requires & as first argument (legacy syntax), got: {}",
                            if let Some(atom) = items[1].as_atom() { atom.to_string() } else { "non-atom".to_string() }
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
            }

            // match-or - like match but with a default fallback when no match found
            "match-or" => {
                if items.len() != 5 {
                    let err = ctx.factory().error(
                        &format!(
                            "match-or requires exactly 4 arguments, got {}. Usage: (match-or space pattern default template)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "case requires exactly 2 arguments, got {}. Usage: (case atom ((pattern template) ...))",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 2 arguments, got {}. Usage: ({} atom cases)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "let requires exactly 3 arguments, got {}. Usage: (let pattern value body)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                        "let* requires at least 2 arguments. Usage: (let* ((pat val) ...) body)",
                        ctx.factory().sexpr(items),
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
                            "let* bindings must be a list. Usage: (let* ((pattern value) ...) body)",
                            bindings_expr.clone(),
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
                                "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                                binding.clone(),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    } else {
                        let err = ctx.factory().error(
                            "let* binding must be (pattern value) pair. Usage: (let* ((pattern value) ...) body)",
                            binding.clone(),
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
                        &format!(
                            "unify requires exactly 4 arguments, got {}. Usage: (unify pattern1 pattern2 success failure)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            ":< requires exactly 2 arguments, got {}. Usage: (:< SubType SuperType)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                // Extract sub and super type names
                let sub_name = match items[1].as_atom() {
                    Some(atom) => atom.to_string(),
                    None => {
                        let err = ctx.factory().error(
                            ":< requires atom arguments. Usage: (:< SubType SuperType)",
                            items[1].clone(),
                        );
                        return GenericEvalStep::Done((smallvec![err], env));
                    }
                };
                let super_name = match items[2].as_atom() {
                    Some(atom) => atom.to_string(),
                    None => {
                        let err = ctx.factory().error(
                            ":< requires atom arguments. Usage: (:< SubType SuperType)",
                            items[2].clone(),
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

            // Type assertion - native generic implementation (zero-conversion)
            ":" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            ": requires exactly 2 arguments, got {}. Usage: (: expr type)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                // Extract name from expression (atom or first element of sexpr)
                let name = match (items[1].as_atom(), items[1].as_sexpr()) {
                    (Some(atom), _) => atom.to_string(),
                    (_, Some(expr_items)) => match expr_items.first().and_then(|f| f.as_atom()) {
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
                    &items, ctx.factory(), &env,
                );
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }

            // get-type-space - query types in a specific space (Phase 5)
            "get-type-space" => {
                let results = crate::backend::eval::types::eval_get_type_space_generic(
                    &items, ctx.factory(), &env,
                );
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }

            // is-function - check if a type is an arrow type (Phase G, HE parity)
            "is-function" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "is-function requires exactly 1 argument, got {}. Usage: (is-function type)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "type-cast requires exactly 3 arguments, got {}. Usage: (type-cast atom type space)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                let results = crate::backend::eval::types::eval_type_cast_generic(
                    &items, ctx.factory(), &env,
                );
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }

            // metta - interpreter operation (HE stdlib parity)
            // (metta atom type space) — evaluates atom with type constraint in space
            "metta" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "metta requires exactly 3 arguments, got {}. Usage: (metta atom type space)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                        &items, ctx.factory(), &env,
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
                    let err = ctx.factory().error(
                        &format!(
                            "match-types requires 4 arguments, got {}. Usage: (match-types type1 type2 then else)",
                            items.len() - 1
                        ),
                        ctx.factory().atom("BadArity"),
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
                    let err = ctx.factory().error(
                        &format!(
                            "match-type-or requires 3 arguments, got {}. Usage: (match-type-or folded next type)",
                            items.len() - 1
                        ),
                        ctx.factory().atom("BadArity"),
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
                let folded_bool = folded.as_bool().unwrap_or_else(|| folded.as_atom() == Some("True"));
                let result = folded_bool || matched;
                return GenericEvalStep::Done((smallvec![ctx.factory().bool(result)], env));
            }

            // first-from-pair - extract first element from a pair (HE stdlib parity)
            // (first-from-pair ($first $second)) = $first
            "first-from-pair" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "first-from-pair requires 1 argument, got {}. Usage: (first-from-pair pair)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                let err = ctx.factory().error(
                    "incorrect pair format",
                    ctx.factory().sexpr(vec![
                        ctx.factory().atom("first-from-pair"),
                        pair.clone(),
                    ]),
                );
                return GenericEvalStep::Done((smallvec![err], env));
            }

            // map-atom - defers iteration to trampoline
            "map-atom" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "map-atom requires exactly 3 arguments, got {}. Usage: (map-atom list $var template)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                // Extract list elements
                let list_arg = &items[1];
                let var_arg = &items[2];
                let template = items[3].clone();

                let var_name = match extract_var_name::<C>(var_arg, "map-atom", "second argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let elements = match extract_list_elements::<C>(list_arg, "map-atom", ctx, &env) {
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

            // filter-atom - defers iteration to trampoline
            "filter-atom" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "filter-atom requires exactly 3 arguments, got {}. Usage: (filter-atom list $var predicate)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                let list_arg = &items[1];
                let var_arg = &items[2];
                let predicate = items[3].clone();

                let var_name = match extract_var_name::<C>(var_arg, "filter-atom", "second argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let elements = match extract_list_elements::<C>(list_arg, "filter-atom", ctx, &env) {
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

            // foldl-atom - defers iteration to trampoline
            "foldl-atom" => {
                if items.len() != 6 {
                    let err = ctx.factory().error(
                        &format!(
                            "foldl-atom requires exactly 5 arguments, got {}. Usage: (foldl-atom list init $acc $x op)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                let list_arg = &items[1];
                let init = items[2].clone();
                let acc_var = &items[3];
                let item_var = &items[4];
                let operation = items[5].clone();

                let acc_var_name = match extract_var_name::<C>(acc_var, "foldl-atom", "third argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let item_var_name = match extract_var_name::<C>(item_var, "foldl-atom", "fourth argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let elements = match extract_list_elements::<C>(list_arg, "foldl-atom", ctx, &env) {
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
            }

            // List operations - native generic implementations (zero conversion)
            "car-atom" => {
                let results = eval_car_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "cdr-atom" => {
                let results = eval_cdr_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "cons-atom" => {
                let results = eval_cons_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "decons-atom" => {
                let results = eval_decons_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "size-atom" => {
                let results = eval_size_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "max-atom" => {
                let results = eval_max_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "index-atom" => {
                let results = eval_index_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "min-atom" => {
                let results = eval_min_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }

            // Tuple operations - native generic implementations
            "tuple-concat" => {
                let results = eval_tuple_concat_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "tuple-count" => {
                let results = eval_tuple_count_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "without" => {
                let results = eval_without_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "element-of" => {
                let results = eval_element_of_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "range" => {
                let results = eval_range_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "reverse-atom" => {
                let results = eval_reverse_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "flatten-atom" => {
                let results = eval_flatten_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "zip-atom" => {
                let results = eval_zip_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "take-atom" => {
                let results = eval_take_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }
            "drop-atom" => {
                let results = eval_drop_atom_generic(&items, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), env));
            }

            // sort-tuple - defers iteration to trampoline
            "sort-tuple" => {
                if items.len() != 5 {
                    let err = ctx.factory().error(
                        &format!(
                            "sort-tuple requires exactly 4 arguments, got {}. Usage: (sort-tuple tuple $var1 $var2 comparator)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                let list_arg = &items[1];
                let var1_arg = &items[2];
                let var2_arg = &items[3];
                let comparator = items[4].clone();

                let var1_name = match extract_var_name::<C>(var1_arg, "sort-tuple", "second argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let var2_name = match extract_var_name::<C>(var2_arg, "sort-tuple", "third argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let elements = match extract_list_elements::<C>(list_arg, "sort-tuple", ctx, &env) {
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

            // best-candidate - defers iteration to trampoline
            "best-candidate" => {
                if items.len() != 4 {
                    let err = ctx.factory().error(
                        &format!(
                            "best-candidate requires exactly 3 arguments, got {}. Usage: (best-candidate tuple $var rank-fn)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                let list_arg = &items[1];
                let var_arg = &items[2];
                let rank_fn = items[3].clone();

                let var_name = match extract_var_name::<C>(var_arg, "best-candidate", "second argument", &env, ctx) {
                    Ok(name) => name,
                    Err(step) => return step,
                };

                let elements = match extract_list_elements::<C>(list_arg, "best-candidate", ctx, &env) {
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
                                &format!(
                                    "new-space: optional name must be a string, got {:?}. Usage: (new-space) or (new-space \"name\")",
                                    items[1]
                                ),
                                items[1].clone(),
                            );
                            return GenericEvalStep::Done((smallvec![err], env));
                        }
                    }
                };

                // Create named space via GenericEnvironment
                let mut new_env = env.clone();
                let space_id = new_env.create_named_space(&name);
                let handle = SpaceHandle::new(space_id, name);

                // Return Space value using factory
                let space_val = ctx.factory().space(handle);
                return GenericEvalStep::Done((smallvec![space_val], new_env));
            }

            "add-atom" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "add-atom requires exactly 2 arguments, got {}. Usage: (add-atom space atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "remove-atom requires exactly 2 arguments, got {}. Usage: (remove-atom space atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "collapse requires exactly 1 argument, got {}. Usage: (collapse expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "collapse-bind requires exactly 1 argument, got {}. Usage: (collapse-bind expr)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                return GenericEvalStep::StartCollapseBind {
                    expr: items[1].clone(),
                    env,
                    depth,
                };
            }

            // superpose - HE-compatible: post-evaluate each element via StartAmb
            "superpose" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "superpose requires 1 argument, got {}. Usage: (superpose list)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "guard requires exactly 1 argument, got {}. Usage: (guard condition)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "get-atoms requires exactly 1 argument, got {}. Usage: (get-atoms space)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "new-state requires exactly 1 argument, got {}. Usage: (new-state initial-value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "get-state requires exactly 1 argument, got {}. Usage: (get-state state)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "change-state! requires exactly 2 arguments, got {}. Usage: (change-state! state new-value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "new-memo requires 1-2 arguments, got {}. Usage: (new-memo name [size])",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 2 arguments, got {}. Usage: ({} memo-table expr)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "{} requires exactly 1 argument, got {}. Usage: ({} memo-table)",
                            op,
                            items.len() - 1,
                            op
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "bind! requires exactly 2 arguments, got {}. Usage: (bind! token atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                let token = match items[1].as_atom() {
                    Some(t) => t.to_string(),
                    None => {
                        let err = ctx.factory().error(
                            "bind! requires an atom as first argument",
                            ctx.factory().sexpr(items),
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

            // I/O operations
            "println!" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "println! requires exactly 1 argument, got {}. Usage: (println! atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }
                return GenericEvalStep::StartPrintln {
                    atom: items[1].clone(),
                    env,
                    depth,
                };
            }

            "trace!" => {
                if items.len() != 3 {
                    let err = ctx.factory().error(
                        &format!(
                            "trace! requires exactly 2 arguments, got {}. Usage: (trace! message value)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "repr requires exactly 1 argument, got {}. Usage: (repr atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
                    let err = ctx.factory().error(
                        &format!(
                            "format-args requires exactly 2 arguments, got {}. Usage: (format-args format-string args)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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

            "get-metatype" => {
                if items.len() != 2 {
                    let err = ctx.factory().error(
                        &format!(
                            "get-metatype requires exactly 1 argument, got {}. Usage: (get-metatype atom)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
            "import!" => {
                let (results, new_env) = eval_import_generic(items, env, ctx);
                return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
            }
            "mod-space!" => {
                let (results, new_env) = eval_mod_space_generic(items, env, ctx.factory());
                return GenericEvalStep::Done((SmallVec::from_vec(results), new_env));
            }
            "print-mods!" => {
                let (results, new_env) = eval_print_mods_generic(items, env, ctx.factory());
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
                    let err = ctx.factory().error(
                        &format!(
                            "if-equal requires exactly 4 arguments, got {}. Usage: (if-equal pred1 pred2 then else)",
                            items.len() - 1
                        ),
                        ctx.factory().sexpr(items),
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
            "unique-atom" | "union-atom" | "intersection-atom" | "subtraction-atom" => {
                return crate::backend::eval::set_ops::eval_set_op_generic(
                    items, env, ctx,
                );
            }

            // Alpha equivalence — (=alpha expr1 expr2) → Bool
            "=alpha" => {
                return crate::backend::eval::testing_ops::eval_testing_op_generic(
                    items, env, ctx,
                );
            }

            // Testing/assertion operations — multiset nondeterministic comparison
            "assertEqual" | "assertAlphaEqual"
            | "assertEqualMsg" | "assertAlphaEqualMsg"
            | "assertEqualToResult" | "assertAlphaEqualToResult"
            | "assertEqualToResultMsg" | "assertAlphaEqualToResultMsg" => {
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
                if !op_types.is_empty() && op_types.iter().all(|t| {
                    t.as_sexpr().map_or(false, |type_items|
                        type_items.first().and_then(|v| v.as_atom()) == Some("Error"))
                }) {
                    let err = ctx.factory().error(
                        &format!("All types for '{}' are errors", op),
                        ctx.factory().sexpr(items),
                    );
                    return GenericEvalStep::Done((smallvec![err], env));
                }

                // Try generic grounded operation (zero-conversion path)
                // Uses static dispatch - works with any V: MettaValueTrait
                if has_grounded_op(op) {
                    let args: Vec<MettaValue> = items[1..].to_vec();
                    // Phase 8.8: Pre-validate ground-type args against arrow signature.
                    // Returns clear type error instead of NoReduce → unreduced expression.
                    if let Some(type_error) = validate_grounded_arg_types(op, &args, ctx.factory()) {
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
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let operator = items.first().and_then(|v| v.as_atom()).unwrap_or("?").to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&ctx.factory().sexpr(items.clone())),
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
                #[cfg(feature = "eval-trace")]
                {
                    if let Some(tc) = ctx.trace_collector() {
                        let operator = items.first().and_then(|v| v.as_atom()).unwrap_or("?").to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::TreeWalker,
                            depth as u32,
                            crate::backend::trace::trace_value_generic(&ctx.factory().sexpr(items.clone())),
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
    let all_matches = crate::backend::eval::trampoline::try_match_all_rules(&resolved_sexpr, &env, *ctx.factory());

    if !all_matches.is_empty() {
        // Trace: RuleMatchSet
        #[cfg(feature = "eval-trace")]
        {
            if let Some(tc) = ctx.trace_collector() {
                let match_count = all_matches.len() as u32;
                let matches_tv: Vec<(trace_format::TraceValue, Option<trace_format::TraceSpan>)> = all_matches.iter()
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

    // Step 4: No rules matched, no pre-eval needed — data constructor / tuple path.
    // Evaluates sub-elements independently (MeTTa HE's `interpret_tuple` path).
    GenericEvalStep::EvalSExpr { items, env, depth }
}

// ============================================================================
// Helper Functions
// ============================================================================

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
                smallvec![ctx.factory().error(&msg, var_arg.clone())],
                env.clone(),
            )))
        }
        None => Err(GenericEvalStep::Done((
            smallvec![ctx.factory().error(
                &format!(
                    "{}: {} must be a variable (starting with $)",
                    op_name, arg_position
                ),
                var_arg.clone(),
            )],
            env.clone(),
        ))),
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
                &format!("{} requires a list as first argument", op_name),
                list_arg.clone(),
            );
            Err(GenericEvalStep::Done((smallvec![err], env.clone())))
        }
    }
}

/// Preprocess space references: combine `& self` into `&self`.
fn preprocess_space_refs_generic<C: EvalContext>(
    items: Vec<MettaValue>,
    ctx: &C,
) -> Vec<MettaValue>
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

        let items = vec![
            factory.atom("quote"),
            factory.atom("foo"),
        ];

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
            GenericEvalStep::EvalIfCondition { condition, then_branch, else_branch, .. } => {
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
