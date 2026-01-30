//! S-Expression Step Evaluation
//!
//! This module handles the evaluation step for S-expressions, including
//! special forms dispatch and rule matching.

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::grounded::{ExecError, GroundedState};
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::{
    bindings, control_flow, errors, eval, evaluation, expression, io, list_ops, modules,
    mork_forms, preprocess_space_refs, quoting, resolve_tokens_shallow, space, strings,
    try_match_all_rules, types, utilities,
};
use super::grounded::find_grounded_arg_indices;
use super::types::EvalStep;

/// Evaluate an S-expression step - handles special forms and delegates to iterative collection
pub fn eval_sexpr_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    trace!(target: "mettatron::backend::eval::eval_sexpr_step",?items, depth);

    // Preprocess to combine `& self` into `&self` for HE-compatible space references
    let items = preprocess_space_refs(items);

    if items.is_empty() {
        // HE-compatible: empty SExpr () evaluates to itself, not Nil
        // This is important for collapse semantics: collapse of one () result is (())
        return EvalStep::Done((vec![MettaValue::SExpr(vec![])], env));
    }

    // Check for special forms - these are handled directly (they manage their own recursion)
    if let Some(op) = items.first().and_then(|v| match v.inner() {
        MettaValueInner::Atom(s) => Some(s.as_str()),
        _ => None,
    }) {
        match op {
            "=" => return EvalStep::Done(space::eval_add(items, env)),
            "!" => {
                // Force evaluation operator - defer to trampoline for TCO
                if items.len() != 2 {
                    let err = MettaValue::Error(
                        format!(
                            "! requires exactly 1 argument, got {}. Usage: (! expr)",
                            items.len() - 1
                        ),
                        MettaValue::SExpr(items),
                    );
                    return EvalStep::Done((vec![err], env));
                }
                // Defer evaluation to trampoline - this IS a tail call (TCO)
                // Reusing EvalIfBranch since it has identical semantics:
                // evaluate an expression and return result to parent continuation
                return EvalStep::EvalIfBranch {
                    branch: items[1].clone(),
                    env,
                    depth,
                };
            }
            "quote" => return EvalStep::Done(quoting::eval_quote(items, env)),
            "if" => return control_flow::eval_if_step(items, env, depth),
            "error" => return EvalStep::Done(errors::eval_error(items, env)),
            // HE compatibility: (Error details msg) -> adapt to MeTTaTron's (error msg details)
            "Error" => return EvalStep::Done(errors::eval_error_he(items, env)),
            "is-error" => return errors::eval_if_error_step(items, env, depth),
            "catch" => return errors::eval_catch_step(items, env, depth),
            "eval" => return evaluation::eval_eval_step(items, env, depth),
            "function" => return evaluation::eval_function_step(items, env, depth),
            "return" => return evaluation::eval_return_step(items, env, depth),
            "chain" => return evaluation::eval_chain_step(items, env, depth),
            "match" => return space::eval_match_step(items, env, depth),
            "case" => return control_flow::eval_case_step(items, env, depth),
            "switch" => return control_flow::eval_switch_step(items, env, depth),
            "switch-minimal" => return control_flow::eval_switch_minimal_step(items, env, depth),
            "switch-internal" => return control_flow::eval_switch_internal_step(items, env, depth),
            "let" => return bindings::eval_let_step(items, env, depth),
            "let*" => return bindings::eval_let_star_step(items, env, depth),
            "unify" => return bindings::eval_unify_step(items, env, depth),
            "sealed" => return EvalStep::Done(bindings::eval_sealed(items, env)),
            "atom-subst" => return EvalStep::Done(bindings::eval_atom_subst(items, env)),
            ":" => return EvalStep::Done(types::eval_type_assertion(items, env)),
            "get-type" => return EvalStep::Done(types::eval_get_type(items, env)),
            "check-type" => return EvalStep::Done(types::eval_check_type(items, env)),
            "map-atom" => return list_ops::eval_map_atom_step(items, env, depth),
            "filter-atom" => return list_ops::eval_filter_atom_step(items, env, depth),
            "foldl-atom" => return list_ops::eval_foldl_atom_step(items, env, depth),
            "car-atom" => return list_ops::eval_car_atom_step(items, env, depth),
            "cdr-atom" => return list_ops::eval_cdr_atom_step(items, env, depth),
            "cons-atom" => return list_ops::eval_cons_atom_step(items, env, depth),
            "decons-atom" => return list_ops::eval_decons_atom_step(items, env, depth),
            "size-atom" => return list_ops::eval_size_atom_step(items, env, depth),
            "max-atom" => return list_ops::eval_max_atom_step(items, env, depth),
            // Additional expression operations from main
            "index-atom" => return EvalStep::Done(expression::eval_index_atom(items, env)),
            "min-atom" => return EvalStep::Done(expression::eval_min_atom(items, env)),
            // Space Operations
            "new-space" => return EvalStep::Done(space::eval_new_space(items, env)),
            "add-atom" => return space::eval_add_atom_step(items, env, depth),
            "remove-atom" => return space::eval_remove_atom_step(items, env, depth),
            "collapse" => return space::eval_collapse_step(items, env, depth),
            "collapse-bind" => return space::eval_collapse_bind_step(items, env, depth),
            "superpose" => return EvalStep::Done(space::eval_superpose(items, env)),
            // Advanced Nondeterminism (Phase G)
            "amb" => return space::eval_amb_step(items, env, depth),
            "guard" => return space::eval_guard_step(items, env, depth),
            "commit" => return EvalStep::Done(space::eval_commit(items, env)),
            "backtrack" => return EvalStep::Done(space::eval_backtrack(items, env)),
            "get-atoms" => return space::eval_get_atoms_step(items, env, depth),
            // State Operations
            "new-state" => return space::eval_new_state_step(items, env, depth),
            "get-state" => return space::eval_get_state_step(items, env, depth),
            "change-state!" => return space::eval_change_state_step(items, env, depth),
            // Memoization Operations
            "new-memo" => return space::eval_new_memo_step(items, env, depth),
            "memo" => return space::eval_memo_step(items, env, depth),
            "memo-first" => return space::eval_memo_first_step(items, env, depth),
            "clear-memo!" => return space::eval_clear_memo_step(items, env, depth),
            "memo-stats" => return space::eval_memo_stats_step(items, env, depth),
            // Token Binding (HE-compatible tokenizer-based bind!)
            "bind!" => return modules::eval_bind_step(items, env, depth),
            // I/O Operations
            "println!" => return io::eval_println_step(items, env, depth),
            "trace!" => return io::eval_trace_step(items, env, depth),
            "nop" => return EvalStep::Done(io::eval_nop(items, env)),
            // String Operations
            "repr" => return strings::eval_repr_step(items, env, depth),
            "format-args" => return strings::eval_format_args_step(items, env, depth),
            // Utility Operations
            "empty" => return EvalStep::Done(utilities::eval_empty(items, env)),
            "get-metatype" => return utilities::eval_get_metatype_step(items, env, depth),
            // Structural comparison (non-evaluating) - matches HE semantics
            "==" => return utilities::eval_eq_step(items, env, depth),
            "!=" => return utilities::eval_neq_step(items, env, depth),
            // Module Operations
            "include" => return modules::eval_include_step(items, env, depth),
            "import!" => return modules::eval_import_step(items, env, depth),
            "mod-space!" => return EvalStep::Done(modules::eval_mod_space(items, env)),
            "print-mods!" => return EvalStep::Done(modules::eval_print_mods(items, env)),
            // MORK Special Forms
            "exec" => return EvalStep::Done(mork_forms::eval_exec(items, env)),
            "coalg" => return EvalStep::Done(mork_forms::eval_coalg(items, env)),
            "lookup" => return mork_forms::eval_lookup_step(items, env, depth),
            "rulify" => return EvalStep::Done(mork_forms::eval_rulify(items, env)),
            _ => {}
        }
    }

    // HE-compatible lazy evaluation: try grounded operations and rules with UNEVALUATED args first
    // Step 1: Try grounded operations with RAW (unevaluated) arguments
    // First try TCO-enabled operations (which use trampoline for deep recursion),
    // then fall back to legacy operations if no TCO version exists.
    if let Some(op) = items.first().and_then(|v| match v.inner() {
        MettaValueInner::Atom(s) => Some(s.as_str()),
        _ => None,
    }) {
        // Try TCO operation first - these don't call eval() internally and are
        // safe for arbitrarily deep recursion
        if env.get_grounded_operation_tco(op).is_some() {
            // Create initial state for the grounded operation
            let state = GroundedState::new(op.to_string(), items[1..].to_vec());
            return EvalStep::StartGroundedOp { state, env, depth };
        }

        // Fall back to legacy grounded operations (non-TCO)
        // These call eval() internally and may overflow the Rust stack on deep recursion
        if let Some(grounded_op) = env.get_grounded_operation(op) {
            // Create an eval function closure for grounded operations to use
            let eval_fn = |value: MettaValue,
                           env_inner: Environment|
             -> (Vec<MettaValue>, Environment) { eval(value, env_inner) };

            match grounded_op.execute_raw(&items[1..], &env, &eval_fn) {
                Ok(results) => {
                    // Grounded operation succeeded
                    let values: Vec<MettaValue> = results.into_iter().map(|(v, _)| v).collect();
                    return EvalStep::Done((values, env));
                }
                Err(ExecError::NoReduce) => {
                    // Not applicable - fall through to rule matching
                }
                Err(ExecError::Runtime(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            MettaValue::Atom("TypeError".to_string()),
                        )],
                        env,
                    ));
                }
                Err(ExecError::IncorrectArgument(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            MettaValue::Atom("ArityError".to_string()),
                        )],
                        env,
                    ));
                }
                Err(ExecError::Arithmetic(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            MettaValue::Atom("ArithmeticError".to_string()),
                        )],
                        env,
                    ));
                }
            }
        }
    }

    // Step 2: Check for grounded args that need evaluation BEFORE rule matching
    // This defers grounded arg evaluation to the trampoline to prevent stack overflow.
    //
    // WHY: Pure lazy evaluation causes infinite loops with recursive rules like:
    //   (= (countdown $n) (countdown (- $n 1)))
    // Because `$n` binds to `(- 3 1)` instead of `2`, the expression grows infinitely.
    //
    // SOLUTION: Evaluate arguments that are GROUNDED operations (like +, -, *, /)
    // but keep user-defined expressions unevaluated (for lazy pattern matching).
    // The key change: evaluation is DEFERRED to the trampoline, not done synchronously.
    let grounded_indices = find_grounded_arg_indices(&items, &env);
    if !grounded_indices.is_empty() {
        // Defer evaluation to trampoline - this returns immediately without calling eval()
        return EvalStep::EvalGroundedArgs {
            items,
            grounded_indices,
            env,
            depth,
        };
    }

    // Step 3: No grounded args - proceed with rule matching directly
    let resolved_items = resolve_tokens_shallow(&items, &env);
    let resolved_sexpr = MettaValue::SExpr(resolved_items.clone());
    let all_matches = try_match_all_rules(&resolved_sexpr, &env);

    if !all_matches.is_empty() {
        // User rules matched - evaluate RHS with bindings from pattern match
        return EvalStep::EvalRuleMatchesLazy {
            matches: all_matches,
            env,
            depth,
        };
    }

    // Step 4: No lazy rules matched - expression is irreducible (data constructor).
    // In HE semantics, if no rule matches the unevaluated expression, it's a data constructor.
    // We still evaluate arguments (for grounded operations within them) but don't retry rule matching.
    EvalStep::EvalSExpr { items, env, depth }
}
