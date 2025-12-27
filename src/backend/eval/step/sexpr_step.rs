//! S-Expression Step Evaluation
//!
//! This module handles the evaluation step for S-expressions, including
//! special forms dispatch and rule matching.

use std::sync::Arc;

use tracing::trace;

use crate::backend::environment::Environment;
use crate::backend::grounded::{ExecError, GroundedState};
use crate::backend::models::MettaValue;

use super::super::{
    bindings, control_flow, errors, eval, evaluation, expression, io, list_ops, modules,
    mork_forms, preprocess_space_refs, quoting, resolve_tokens_shallow, space, strings,
    try_match_all_rules, types, utilities,
};
use super::grounded::evaluate_grounded_args;
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
    if let Some(MettaValue::Atom(op)) = items.first() {
        match op.as_str() {
            "=" => return EvalStep::Done(space::eval_add(items, env)),
            "!" => return EvalStep::Done(evaluation::force_eval(items, env)),
            "quote" => return EvalStep::Done(quoting::eval_quote(items, env)),
            "if" => return control_flow::eval_if_step(items, env, depth),
            "error" => return EvalStep::Done(errors::eval_error(items, env)),
            // HE compatibility: (Error details msg) -> adapt to MeTTaTron's (error msg details)
            "Error" => return EvalStep::Done(errors::eval_error_he(items, env)),
            "is-error" => return EvalStep::Done(errors::eval_if_error(items, env)),
            "catch" => return EvalStep::Done(errors::eval_catch(items, env)),
            "eval" => return EvalStep::Done(evaluation::eval_eval(items, env)),
            "function" => return EvalStep::Done(evaluation::eval_function(items, env)),
            "return" => return EvalStep::Done(evaluation::eval_return(items, env)),
            "chain" => return EvalStep::Done(evaluation::eval_chain(items, env)),
            "match" => return EvalStep::Done(space::eval_match(items, env)),
            "case" => return EvalStep::Done(control_flow::eval_case(items, env)),
            "switch" => return EvalStep::Done(control_flow::eval_switch(items, env)),
            "switch-minimal" => {
                return EvalStep::Done(control_flow::eval_switch_minimal_handler(items, env))
            }
            "switch-internal" => {
                return EvalStep::Done(control_flow::eval_switch_internal_handler(items, env))
            }
            "let" => return bindings::eval_let_step(items, env, depth),
            "let*" => return EvalStep::Done(bindings::eval_let_star(items, env)),
            "unify" => return EvalStep::Done(bindings::eval_unify(items, env)),
            "sealed" => return EvalStep::Done(bindings::eval_sealed(items, env)),
            "atom-subst" => return EvalStep::Done(bindings::eval_atom_subst(items, env)),
            ":" => return EvalStep::Done(types::eval_type_assertion(items, env)),
            "get-type" => return EvalStep::Done(types::eval_get_type(items, env)),
            "check-type" => return EvalStep::Done(types::eval_check_type(items, env)),
            "map-atom" => return EvalStep::Done(list_ops::eval_map_atom(items, env)),
            "filter-atom" => return EvalStep::Done(list_ops::eval_filter_atom(items, env)),
            "foldl-atom" => return EvalStep::Done(list_ops::eval_foldl_atom(items, env)),
            "car-atom" => return EvalStep::Done(list_ops::eval_car_atom(items, env)),
            "cdr-atom" => return EvalStep::Done(list_ops::eval_cdr_atom(items, env)),
            "cons-atom" => return EvalStep::Done(list_ops::eval_cons_atom(items, env)),
            "decons-atom" => return EvalStep::Done(list_ops::eval_decons_atom(items, env)),
            "size-atom" => return EvalStep::Done(list_ops::eval_size_atom(items, env)),
            "max-atom" => return EvalStep::Done(list_ops::eval_max_atom(items, env)),
            // Additional expression operations from main
            "index-atom" => return EvalStep::Done(expression::eval_index_atom(items, env)),
            "min-atom" => return EvalStep::Done(expression::eval_min_atom(items, env)),
            // Space Operations
            "new-space" => return EvalStep::Done(space::eval_new_space(items, env)),
            "add-atom" => return EvalStep::Done(space::eval_add_atom(items, env)),
            "remove-atom" => return EvalStep::Done(space::eval_remove_atom(items, env)),
            "collapse" => return EvalStep::Done(space::eval_collapse(items, env)),
            "collapse-bind" => return EvalStep::Done(space::eval_collapse_bind(items, env)),
            "superpose" => return EvalStep::Done(space::eval_superpose(items, env)),
            // Advanced Nondeterminism (Phase G)
            "amb" => return EvalStep::Done(space::eval_amb(items, env)),
            "guard" => return EvalStep::Done(space::eval_guard(items, env)),
            "commit" => return EvalStep::Done(space::eval_commit(items, env)),
            "backtrack" => return EvalStep::Done(space::eval_backtrack(items, env)),
            "get-atoms" => return EvalStep::Done(space::eval_get_atoms(items, env)),
            // State Operations
            "new-state" => return EvalStep::Done(space::eval_new_state(items, env)),
            "get-state" => return EvalStep::Done(space::eval_get_state(items, env)),
            "change-state!" => return EvalStep::Done(space::eval_change_state(items, env)),
            // Memoization Operations
            "new-memo" => return EvalStep::Done(space::eval_new_memo(items, env)),
            "memo" => return EvalStep::Done(space::eval_memo(items, env)),
            "memo-first" => return EvalStep::Done(space::eval_memo_first(items, env)),
            "clear-memo!" => return EvalStep::Done(space::eval_clear_memo(items, env)),
            "memo-stats" => return EvalStep::Done(space::eval_memo_stats(items, env)),
            // Token Binding (HE-compatible tokenizer-based bind!)
            "bind!" => return EvalStep::Done(modules::eval_bind(items, env)),
            // I/O Operations
            "println!" => return EvalStep::Done(io::eval_println(items, env)),
            "trace!" => return EvalStep::Done(io::eval_trace(items, env)),
            "nop" => return EvalStep::Done(io::eval_nop(items, env)),
            // String Operations
            "repr" => return EvalStep::Done(strings::eval_repr(items, env)),
            "format-args" => return EvalStep::Done(strings::eval_format_args(items, env)),
            // Utility Operations
            "empty" => return EvalStep::Done(utilities::eval_empty(items, env)),
            "get-metatype" => return EvalStep::Done(utilities::eval_get_metatype(items, env)),
            // Module Operations
            "include" => return EvalStep::Done(modules::eval_include(items, env)),
            "import!" => return EvalStep::Done(modules::eval_import(items, env)),
            "mod-space!" => return EvalStep::Done(modules::eval_mod_space(items, env)),
            "print-mods!" => return EvalStep::Done(modules::eval_print_mods(items, env)),
            // MORK Special Forms
            "exec" => return EvalStep::Done(mork_forms::eval_exec(items, env)),
            "coalg" => return EvalStep::Done(mork_forms::eval_coalg(items, env)),
            "lookup" => return EvalStep::Done(mork_forms::eval_lookup(items, env)),
            "rulify" => return EvalStep::Done(mork_forms::eval_rulify(items, env)),
            _ => {}
        }
    }

    // HE-compatible lazy evaluation: try grounded operations and rules with UNEVALUATED args first
    let sexpr = MettaValue::SExpr(items.clone());

    // Step 1: Try grounded operations with RAW (unevaluated) arguments
    // First try TCO-enabled operations (which use trampoline for deep recursion),
    // then fall back to legacy operations if no TCO version exists.
    if let Some(MettaValue::Atom(op)) = items.first() {
        // Try TCO operation first - these don't call eval() internally and are
        // safe for arbitrarily deep recursion
        if env.get_grounded_operation_tco(op).is_some() {
            // Create initial state for the grounded operation
            let state = GroundedState::new(op.clone(), items[1..].to_vec());
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
                            Arc::new(MettaValue::Atom("TypeError".to_string())),
                        )],
                        env,
                    ));
                }
                Err(ExecError::IncorrectArgument(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            Arc::new(MettaValue::Atom("ArityError".to_string())),
                        )],
                        env,
                    ));
                }
                Err(ExecError::Arithmetic(msg)) => {
                    return EvalStep::Done((
                        vec![MettaValue::Error(
                            msg,
                            Arc::new(MettaValue::Atom("ArithmeticError".to_string())),
                        )],
                        env,
                    ));
                }
            }
        }
    }

    // Step 2: Evaluate grounded sub-expressions BEFORE rule matching (hybrid lazy/eager)
    // This ensures that arithmetic operations like (- 3 1) are evaluated to values
    // before being used in pattern matching, while keeping user-defined expressions lazy.
    //
    // WHY: Pure lazy evaluation causes infinite loops with recursive rules like:
    //   (= (countdown $n) (countdown (- $n 1)))
    // Because `$n` binds to `(- 3 1)` instead of `2`, the expression grows infinitely.
    //
    // SOLUTION: Evaluate arguments that are GROUNDED operations (like +, -, *, /)
    // but keep user-defined expressions unevaluated (for lazy pattern matching).
    let items_with_grounded_evaluated = evaluate_grounded_args(&items, &env);

    // Step 3: Try user rule matching with partially-evaluated arguments
    // Grounded operations in arguments are now values, user-defined expressions are still lazy.
    let resolved_items = resolve_tokens_shallow(&items_with_grounded_evaluated, &env);
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
    EvalStep::EvalSExpr {
        items: items_with_grounded_evaluated,
        env,
        depth,
    }
}
