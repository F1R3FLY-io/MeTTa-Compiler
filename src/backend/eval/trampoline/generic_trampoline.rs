//! Generic Trampoline Engine
//!
//! This module provides a truly generic trampoline evaluation engine that works with
//! any value type implementing `MettaValueTrait`. This enables the same evaluation
//! logic to work with both heap-allocated (`MettaValue`) and arena-allocated
//! (`MettaValue`) values.
//!
//! ## Design
//!
//! The generic engine:
//! - Uses `GenericWorkItem<V, F>` and `GenericContinuation<V, F>` for work tracking
//! - Calls `eval_step_generic` for single-step evaluation
//! - Uses `GenericEnvironment<V, F>` for all environment operations
//!
//! ## Entry Points
//!
//! - `eval_trampoline_generic`: Generic evaluation for any `EvalContext`
//!
//! The generic engine is parameterized by the `EvalContext` trait, which determines
//! the value type and factory. The production implementation uses `StaticEvalContext`
//! with arena-allocated `MettaValue` values.

use std::collections::VecDeque;

use tracing::trace;

use crate::backend::grounded::{execute_generic_grounded_op, ExecError, GenericGroundedWork};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

use super::context::{ContextEnv, EvalContext};
use super::generic_engine::{
    apply_bindings_generic, eval_switch_generic, is_boolean_check_pattern, pattern_match_generic,
    try_match_all_rules_generic, GenericSwitchResult,
};
use super::generic_types::{GenericContinuation, GenericEvalResult, GenericWorkItem};
use super::super::list_ops::substitute_variable_generic;
use super::super::step::{eval_step_generic, GenericEvalStep};
use super::super::processing::{
    process_collected_sexpr_generic, GenericProcessedSExpr,
};

/// Generic trampoline evaluation entry point.
///
/// This function provides a unified evaluation engine that works with any value type
/// implementing `MettaValueTrait`. It uses:
/// - `GenericWorkItem<V, F>` for pending evaluation work
/// - `GenericContinuation<V, F>` for continuation handling
/// - `eval_step_generic` for single-step evaluation
///
/// All environment operations use `GenericEnvironment<V, F>` inherent methods.
///
/// # Type Parameters
///
/// - `C`: The evaluation context (e.g., `StaticEvalContext` or `SessionContext`)
///
/// # Arguments
///
/// - `value`: The value to evaluate
/// - `env`: The evaluation environment (`GenericEnvironment<C::Value, C::Factory>`)
/// - `ctx`: The evaluation context providing the factory
///
/// # Returns
///
/// A tuple of (results, final_environment)
pub fn eval_trampoline_generic<C: EvalContext>(
    value: C::Value,
    env: ContextEnv<C>,
    ctx: &C,
) -> GenericEvalResult<C::Value, ContextEnv<C>>
where
    C::Value: Clone,
{
    // Debug tracing controlled by environment variable
    let debug_eval = std::env::var("METTA_DEBUG_EVAL").is_ok();
    let mut eval_count: u64 = 0;

    // Initialize work stack with the initial evaluation
    let mut work_stack: Vec<GenericWorkItem<C::Value, ContextEnv<C>>> = vec![GenericWorkItem::Eval {
        value,
        env: env.clone(),
        depth: 0,
        cont_id: 0,
        is_tail_call: false,
    }];

    // Continuation storage - index 0 is always Done
    let mut continuations: Vec<GenericContinuation<C::Value, ContextEnv<C>>> = vec![GenericContinuation::Done];

    // Final result storage
    let mut final_result: Option<GenericEvalResult<C::Value, ContextEnv<C>>> = None;

    // GC hint counter: wrapping u8 overflows every 256 iterations → maybe_gc()
    let mut gc_counter: u8 = 0;

    // Main trampoline loop
    while let Some(work) = work_stack.pop() {
        // Periodic GC hint (every 256 trampoline iterations)
        gc_counter = gc_counter.wrapping_add(1);
        if gc_counter == 0 {
            ctx.maybe_gc();
        }
        match work {
            GenericWorkItem::Eval {
                value,
                env,
                depth,
                cont_id,
                is_tail_call,
            } => {
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?value, depth, cont_id, "eval work item");

                // Debug trace (zero-conversion: uses Debug trait)
                if debug_eval {
                    eval_count += 1;
                    if eval_count % 1000 == 0 || eval_count < 100 {
                        eprintln!(
                            "[EVAL#{}] depth={} work_stack={} conts={} value={:?}",
                            eval_count,
                            depth,
                            work_stack.len(),
                            continuations.len(),
                            value
                        );
                    }
                }

                // Perform one step of evaluation using generic step function
                let step_result = eval_step_generic(value, env.clone(), depth, ctx);
                let _ = is_tail_call; // Used to determine depth in push sites
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?step_result);

                // Process the step result
                match step_result {
                    // Direct result - resume continuation
                    GenericEvalStep::Done(result) => {
                        work_stack.push(GenericWorkItem::Resume { cont_id, result });
                    }

                    // Need to evaluate S-expression sub-items
                    GenericEvalStep::EvalSExpr { items, env, depth } => {
                        if items.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut items_deque: VecDeque<C::Value> = items.into_iter().collect();
                            let first = items_deque.pop_front().expect("items is non-empty");

                            let collect_cont_id = continuations.len();
                            continuations.push(GenericContinuation::CollectSExpr {
                                remaining: items_deque,
                                collected: Vec::new(),
                                original_env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: collect_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start a TCO grounded operation
                    // Uses static dispatch - works with any V: MettaValueTrait (NO conversion)
                    GenericEvalStep::StartGroundedOp { state, env, depth } => {
                        let mut state = state;
                        // Use static dispatch - monomorphized for each value type
                        // Clone op_name to avoid borrow conflict with mutable state
                        let op_name = state.op_name.clone();
                        if let Some(work) = execute_generic_grounded_op(&op_name, &mut state, ctx.factory()) {
                            match work {
                                GenericGroundedWork::Done(results) => {
                                    // Results are already in correct type V - NO conversion
                                    let values: Vec<C::Value> = results
                                        .into_iter()
                                        .map(|(v, _)| v)
                                        .collect();
                                    work_stack.push(GenericWorkItem::Resume {
                                        cont_id,
                                        result: (values, env),
                                    });
                                }
                                GenericGroundedWork::EvalArg { arg_idx, state: new_state } => {
                                    let grounded_cont_id = continuations.len();
                                    continuations.push(GenericContinuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        env: env.clone(),
                                        parent_cont: cont_id,
                                        depth,
                                    });

                                    // Arg already in correct type V - NO conversion
                                    let arg_to_eval = new_state.args[arg_idx].clone();
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth,
                                        cont_id: grounded_cont_id,
                                        is_tail_call: true,
                                    });
                                }
                                GenericGroundedWork::Error(e) => {
                                    let error_value = match e {
                                        ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                                        ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                                        ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                                        ExecError::NoReduce => ctx.factory().error("NoReduce", ctx.factory().atom("EvalError")),
                                    };
                                    work_stack.push(GenericWorkItem::Resume {
                                        cont_id,
                                        result: (vec![error_value], env),
                                    });
                                }
                            }
                        } else {
                            // Operation not in generic registry - report error
                            // All 14 standard TCO operations (arithmetic, comparison, logical) are in the
                            // generic registry. Custom operations should be added there, not to the
                            // legacy registry.
                            let error_value = ctx.factory().error(
                                &format!("Grounded operation '{}' not found in generic registry", op_name),
                                ctx.factory().atom("OperationNotFoundError"),
                            );
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    // Start let binding
                    GenericEvalStep::StartLetBinding { pattern, value_expr, body, env, depth } => {
                        let let_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessLet {
                            pending_values: None,
                            pattern,
                            body,
                            results: Vec::new(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1,
                            cont_id: let_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate if branch (TCO)
                    GenericEvalStep::EvalIfBranch { branch, env, depth } => {
                        work_stack.push(GenericWorkItem::Eval {
                            value: branch,
                            env,
                            depth,
                            cont_id,
                            is_tail_call: true,
                        });
                    }

                    // Evaluate rule matches with unevaluated arguments (lazy evaluation)
                    // Note: matches are now in generic type (V, GenericBindings<V>)
                    GenericEvalStep::EvalRuleMatchesLazy { matches, env, depth } => {
                        if matches.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![], env),
                            });
                        } else {
                            let mut matches_deque: VecDeque<_> = matches.into_iter().collect();
                            let (rhs, bindings) = matches_deque.pop_front().expect("matches is non-empty");

                            let match_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Apply generic bindings to RHS - NO CONVERSION needed!
                            // Both rhs and bindings are already in generic type V
                            let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                cont_id: match_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }

                    // Evaluate grounded arguments
                    GenericEvalStep::EvalGroundedArgs { items, grounded_indices, env, depth } => {
                        if grounded_indices.is_empty() {
                            work_stack.push(GenericWorkItem::Eval {
                                value: ctx.factory().sexpr(items),
                                env,
                                depth,
                                cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            let first_idx = grounded_indices[0];
                            let arg_to_eval = items[first_idx].clone();

                            let grounded_cont_id = continuations.len();
                            continuations.push(GenericContinuation::CollectGroundedArg {
                                items,
                                grounded_indices,
                                current_idx: 0,
                                evaluated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: arg_to_eval,
                                env,
                                depth: depth + 1,
                                cont_id: grounded_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start map-atom
                    GenericEvalStep::StartMapAtom { elements, var_name, template, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let map_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessMapAtom {
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                template: template.clone(),
                                collected_results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Substitute variable and evaluate - NO CONVERSION NEEDED
                            let instantiated = substitute_variable_generic(
                                &template, &var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: map_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start filter-atom
                    GenericEvalStep::StartFilterAtom { elements, var_name, predicate, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![ctx.factory().sexpr(vec![])], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let filter_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessFilterAtom {
                                current_element: Some(first.clone()),
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                predicate: predicate.clone(),
                                filtered_results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // NO CONVERSION NEEDED - use generic substitute
                            let instantiated = substitute_variable_generic(
                                &predicate, &var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: filter_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start foldl-atom
                    GenericEvalStep::StartFoldlAtom { elements, init, acc_var_name, item_var_name, operation, env, depth } => {
                        if elements.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![init], env),
                            });
                        } else {
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let fold_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessFoldlAtom {
                                remaining_elements: remaining,
                                acc_var_name: acc_var_name.clone(),
                                item_var_name: item_var_name.clone(),
                                operation: operation.clone(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // NO CONVERSION NEEDED - use generic substitute for both variables
                            let instantiated = substitute_variable_generic(
                                &operation, &acc_var_name, &init, ctx.factory(),
                            );
                            let instantiated = substitute_variable_generic(
                                &instantiated, &item_var_name, &first, ctx.factory(),
                            );

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: fold_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Evaluate if condition
                    GenericEvalStep::EvalIfCondition { condition, then_branch, else_branch, env, depth } => {
                        let if_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessIfCondition {
                            then_branch,
                            else_branch,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            cont_id: if_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate case atom
                    GenericEvalStep::EvalCaseAtom { atom, cases, env, depth } => {
                        let case_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessCaseAtom {
                            cases,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: case_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Switch: pattern match WITHOUT evaluating atom
                    GenericEvalStep::SwitchAtom { atom, cases, env, depth } => {
                        // Switch does NOT evaluate atom - pattern match directly
                        match eval_switch_generic(&atom, &cases, ctx.factory()) {
                            GenericSwitchResult::Match(template, _bindings) => {
                                // Template needs evaluation
                                work_stack.push(GenericWorkItem::Eval {
                                    value: template,
                                    env,
                                    depth,
                                    cont_id,
                                    is_tail_call: true,
                                });
                            }
                            GenericSwitchResult::Error(err) => {
                                work_stack.push(GenericWorkItem::Resume {
                                    cont_id,
                                    result: (vec![err], env),
                                });
                            }
                            GenericSwitchResult::NoMatch => {
                                // No case matched - return NotReducible
                                work_stack.push(GenericWorkItem::Resume {
                                    cont_id,
                                    result: (vec![ctx.factory().atom("NotReducible")], env),
                                });
                            }
                        }
                    }

                    // Evaluate switch result (TCO)
                    GenericEvalStep::EvalSwitchResult { template, env, depth } => {
                        work_stack.push(GenericWorkItem::Eval {
                            value: template,
                            env,
                            depth,
                            cont_id,
                            is_tail_call: true,
                        });
                    }

                    // Evaluate eval
                    GenericEvalStep::EvalEval { arg, env, depth } => {
                        let eval_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: arg,
                            env,
                            depth: depth + 1,
                            cont_id: eval_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate return
                    GenericEvalStep::EvalReturn { value, env, depth } => {
                        let return_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessReturn {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value,
                            env,
                            depth: depth + 1,
                            cont_id: return_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start chain
                    GenericEvalStep::StartChain { expr, var, body, env, depth } => {
                        let chain_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessChainExpr {
                            var,
                            body,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: chain_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start function
                    GenericEvalStep::StartFunction { expr, env, depth } => {
                        let func_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessFunction {
                            iteration_count: 1,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: func_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate is-error
                    GenericEvalStep::EvalIsError { expr, env, depth } => {
                        let is_error_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessIsError {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: is_error_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start catch
                    GenericEvalStep::StartCatch { expr, default, env, depth } => {
                        let catch_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessCatch {
                            default,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: catch_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start conjunction
                    GenericEvalStep::StartConjunction { goals, env, depth } => {
                        if goals.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![ctx.factory().unit()], env),
                            });
                        } else if goals.len() == 1 {
                            work_stack.push(GenericWorkItem::Eval {
                                value: goals.into_iter().next().expect("goals.len() == 1"),
                                env,
                                depth,
                                cont_id,
                                is_tail_call: true,
                            });
                        } else {
                            let mut remaining = VecDeque::from(goals);
                            let first_goal = remaining.pop_front().expect("non-empty");

                            let conj_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessConjunction {
                                remaining_goals: remaining,
                                accumulated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first_goal,
                                env,
                                depth: depth + 1,
                                cont_id: conj_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start unify
                    GenericEvalStep::StartUnify { pattern1, pattern2, success_body, failure_body, env, depth } => {
                        let unify_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessUnifyPattern1 {
                            pattern2,
                            success_body,
                            failure_body,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: pattern1,
                            env,
                            depth: depth + 1,
                            cont_id: unify_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start collapse
                    GenericEvalStep::StartCollapse { expr, env, depth } => {
                        let collapse_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessCollapse {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: collapse_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start collapse-bind
                    GenericEvalStep::StartCollapseBind { expr, env, depth } => {
                        let collapse_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessCollapseBind {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: collapse_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start amb
                    GenericEvalStep::StartAmb { alternatives, env, depth } => {
                        if alternatives.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id,
                                result: (vec![], env),
                            });
                        } else {
                            let mut alts_deque: VecDeque<_> = alternatives.into_iter().collect();
                            let first = alts_deque.pop_front().expect("alternatives is non-empty");

                            let amb_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessAmb {
                                remaining_alts: alts_deque,
                                results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: amb_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start guard
                    GenericEvalStep::StartGuard { condition, env, depth } => {
                        let guard_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessGuard {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            cont_id: guard_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-atoms
                    GenericEvalStep::StartGetAtoms { space_ref, env, depth } => {
                        let get_atoms_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessGetAtoms {
                            space_ref: space_ref.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: get_atoms_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start memo
                    GenericEvalStep::StartMemo { memo_ref, expr, first_only, env, depth } => {
                        let memo_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessMemoTable {
                            memo_ref: memo_ref.clone(),
                            expr,
                            first_only,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            cont_id: memo_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start new-memo
                    GenericEvalStep::StartNewMemo { name_arg, size_arg, env, depth } => {
                        let new_memo_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessNewMemoName {
                            name_arg: name_arg.clone(),
                            size_arg,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: name_arg,
                            env,
                            depth: depth + 1,
                            cont_id: new_memo_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start memo operation
                    GenericEvalStep::StartMemoOp { memo_ref, op_type, env, depth } => {
                        let memo_op_cont_id = continuations.len();
                        let is_clear = matches!(op_type, super::super::step::MemoOpType::Clear);
                        continuations.push(GenericContinuation::ProcessMemoOp {
                            memo_ref: memo_ref.clone(),
                            is_clear,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            cont_id: memo_op_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start match
                    GenericEvalStep::StartMatch { space_arg, pattern, template, env, depth } => {
                        let match_space_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessMatchSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            template,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            cont_id: match_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start add-atom
                    GenericEvalStep::StartAddAtom { space_ref, atom, env, depth } => {
                        let add_space_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessAddAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: add_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start remove-atom
                    GenericEvalStep::StartRemoveAtom { space_ref, atom, env, depth } => {
                        let remove_space_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessRemoveAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: remove_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start new-state
                    GenericEvalStep::StartNewState { initial_value, env, depth } => {
                        let new_state_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessNewState {
                            initial_value: initial_value.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: initial_value,
                            env,
                            depth: depth + 1,
                            cont_id: new_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-state
                    GenericEvalStep::StartGetState { state_ref, env, depth } => {
                        let get_state_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessGetState {
                            state_ref: state_ref.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            cont_id: get_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start change-state
                    GenericEvalStep::StartChangeState { state_ref, new_value, env, depth } => {
                        let change_state_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessChangeStateRef {
                            state_ref: state_ref.clone(),
                            new_value,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            cont_id: change_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start repr
                    GenericEvalStep::StartRepr { atom, env, depth } => {
                        let repr_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessRepr {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: repr_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start format-args
                    GenericEvalStep::StartFormatArgs { format_arg, args_arg, env, depth } => {
                        let format_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessFormatArgsString {
                            format_arg: format_arg.clone(),
                            args_arg,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: format_arg,
                            env,
                            depth: depth + 1,
                            cont_id: format_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start println
                    GenericEvalStep::StartPrintln { atom, env, depth } => {
                        let println_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessPrintln {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: println_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start trace
                    GenericEvalStep::StartTrace { message, value_expr, env, depth } => {
                        let trace_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessTraceMessage {
                            message: message.clone(),
                            value_expr,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: message,
                            env,
                            depth: depth + 1,
                            cont_id: trace_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-metatype
                    GenericEvalStep::StartGetMetatype { atom, env, depth } => {
                        let metatype_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessGetMetatype {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: metatype_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start bind
                    GenericEvalStep::StartBind { token, atom_expr, env, depth } => {
                        let bind_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessBind {
                            token,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: atom_expr,
                            env,
                            depth: depth + 1,
                            cont_id: bind_cont_id,
                            is_tail_call: false,
                        });
                    }
                }
            }

            GenericWorkItem::Resume { cont_id, result } => {
                // Take ownership of continuation for processing
                let cont = std::mem::replace(&mut continuations[cont_id], GenericContinuation::Done);
                trace!(target: "mettatron::backend::eval::eval_trampoline_generic", ?cont, result_values = ?result.0, "resume work item");

                // Process continuation - delegate to continuation handler
                process_continuation_generic(
                    cont,
                    cont_id,
                    result,
                    &mut work_stack,
                    &mut continuations,
                    &mut final_result,
                    ctx,
                );
            }
        }
    }

    // Return final result
    final_result.unwrap_or_else(|| (vec![], env))
}

/// Process a continuation with generic value types.
///
/// This function handles all continuation types, converting at boundaries
/// where necessary to interact with heap-based infrastructure (rules, environment).
fn process_continuation_generic<C: EvalContext>(
    cont: GenericContinuation<C::Value, ContextEnv<C>>,
    cont_id: usize,
    result: GenericEvalResult<C::Value, ContextEnv<C>>,
    work_stack: &mut Vec<GenericWorkItem<C::Value, ContextEnv<C>>>,
    continuations: &mut Vec<GenericContinuation<C::Value, ContextEnv<C>>>,
    final_result: &mut Option<GenericEvalResult<C::Value, ContextEnv<C>>>,
    ctx: &C,
) where
    C::Value: Clone,
{
    match cont {
        GenericContinuation::Done => {
            *final_result = Some(result);
        }

        GenericContinuation::CollectSExpr {
            mut remaining,
            mut collected,
            original_env,
            depth,
            parent_cont,
        } => {
            collected.push(result);

            if remaining.is_empty() {
                // All items evaluated, process collected results
                // Use generic version - zero conversion needed!
                let processed = process_collected_sexpr_generic(collected, original_env.clone(), depth, ctx.factory());

                match processed {
                    GenericProcessedSExpr::Done((results, env)) => {
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (results, env),
                        });
                    }
                    GenericProcessedSExpr::EvalRuleMatches { matches, env, depth, base_results } => {
                        if matches.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id: parent_cont,
                                result: (base_results, env),
                            });
                        } else {
                            // Already generic types - no conversion needed!
                            let mut matches_deque = matches;
                            let (rhs, bindings) = matches_deque.pop_front().expect("matches is non-empty");

                            let match_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: base_results,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            });

                            // Apply generic bindings - no conversion needed
                            let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                cont_id: match_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }
                    GenericProcessedSExpr::EvalCombinations { combinations, env, depth } => {
                        let combo_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessCombinations {
                            combinations,
                            results: vec![],
                            pending_rule_matches: VecDeque::new(),
                            env: env.clone(),
                            depth,
                            parent_cont,
                        });

                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: combo_cont_id,
                            result: (vec![], env),
                        });
                    }
                    GenericProcessedSExpr::RedispatchSExpr { items, env, depth: redispatch_depth } => {
                        let sexpr = ctx.factory().sexpr(items);
                        work_stack.push(GenericWorkItem::Eval {
                            value: sexpr,
                            env,
                            depth: redispatch_depth,
                            cont_id: parent_cont,
                            is_tail_call: false,
                        });
                    }
                }
            } else {
                let next = remaining.pop_front().expect("remaining is non-empty");

                continuations[cont_id] = GenericContinuation::CollectSExpr {
                    remaining,
                    collected,
                    original_env: original_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: next,
                    env: original_env,
                    depth: depth + 1,
                    cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessRuleMatches {
            mut remaining_matches,
            mut results,
            env: _,
            depth,
            parent_cont,
        } => {
            results.extend(result.0);
            let env = result.1;

            if remaining_matches.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, env),
                });
            } else {
                // remaining_matches is already in generic type (V, GenericBindings<V>)
                let (rhs, bindings) = remaining_matches.pop_front().expect("remaining_matches is non-empty");

                continuations[cont_id] = GenericContinuation::ProcessRuleMatches {
                    remaining_matches,
                    results,
                    env: env.clone(),
                    depth,
                    parent_cont,
                };

                // Apply generic bindings - no conversion needed
                let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated_rhs,
                    env,
                    depth,
                    cont_id,
                    is_tail_call: true,
                });
            }
        }

        GenericContinuation::ProcessGroundedOp {
            mut state,
            env: _,
            parent_cont,
            depth,
        } => {
            let (result_values, result_env) = result;

            // Set evaluated arg - NO conversion needed (V matches)
            // The arg_idx is (step - 1) because step was incremented before EvalArg
            let arg_idx = state.step.checked_sub(1).expect("BUG: state.step underflow in ProcessGroundedOp");
            state.set_arg(arg_idx, result_values);

            // Try static dispatch first - works with generic type V (NO conversion)
            let op_name = state.op_name.clone();
            if let Some(work) = execute_generic_grounded_op(&op_name, &mut state, ctx.factory()) {
                match work {
                    GenericGroundedWork::Done(results) => {
                        // Results are already in correct type V - NO conversion
                        let values: Vec<C::Value> = results
                            .into_iter()
                            .map(|(v, _)| v)
                            .collect();
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (values, result_env),
                        });
                    }
                    GenericGroundedWork::EvalArg { arg_idx, state: new_state } => {
                        let grounded_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessGroundedOp {
                            state: new_state.clone(),
                            env: result_env.clone(),
                            parent_cont,
                            depth,
                        });

                        // Arg already in correct type V - NO conversion
                        let arg_to_eval = new_state.args[arg_idx].clone();
                        work_stack.push(GenericWorkItem::Eval {
                            value: arg_to_eval,
                            env: result_env,
                            depth,
                            cont_id: grounded_cont_id,
                            is_tail_call: true,
                        });
                    }
                    GenericGroundedWork::Error(e) => {
                        let error_value = match e {
                            ExecError::Runtime(msg) => ctx.factory().error(&msg, ctx.factory().atom("TypeError")),
                            ExecError::Arithmetic(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArithmeticError")),
                            ExecError::IncorrectArgument(msg) => ctx.factory().error(&msg, ctx.factory().atom("ArityError")),
                            ExecError::NoReduce => ctx.factory().error("NoReduce", ctx.factory().atom("EvalError")),
                        };
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![error_value], result_env),
                        });
                    }
                }
            } else {
                // Operation not in generic registry - report error
                // All 14 standard TCO operations are in the generic registry.
                let error_value = ctx.factory().error(
                    &format!("Grounded operation '{}' not found in generic registry", state.op_name),
                    ctx.factory().atom("OperationNotFoundError"),
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![error_value], result_env),
                });
            }
        }

        GenericContinuation::ProcessCombinations {
            mut combinations,
            mut results,
            mut pending_rule_matches,
            env,
            depth,
            parent_cont,
        } => {
            let (combo_results, result_env) = result;
            results.extend(combo_results);

            // Process pending rule matches first
            // pending_rule_matches is already in generic type (V, GenericBindings<V>)
            if let Some((rhs, bindings)) = pending_rule_matches.pop_front() {
                continuations[cont_id] = GenericContinuation::ProcessCombinations {
                    combinations,
                    results,
                    pending_rule_matches,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                // Apply generic bindings - no conversion needed
                let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated_rhs,
                    env: result_env,
                    depth,
                    cont_id,
                    is_tail_call: true,
                });
                return;
            }

            // Get next combination
            if let Some(combo) = combinations.next() {
                // Generic iterator yields SmallVec<[V; 8]> - create generic sexpr
                let generic_sexpr = ctx.factory().sexpr(combo.to_vec());

                // Try to match rules using generic version - no conversion needed!
                let all_matches = try_match_all_rules_generic(&generic_sexpr, &result_env, *ctx.factory());

                if all_matches.is_empty() {
                    // No rule matches - expression is data
                    results.push(generic_sexpr);

                    continuations[cont_id] = GenericContinuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches,
                        env: result_env.clone(),
                        depth,
                        parent_cont,
                    };

                    work_stack.push(GenericWorkItem::Resume {
                        cont_id,
                        result: (vec![], result_env),
                    });
                } else {
                    // Rules matched - evaluate them
                    // Already generic type - no conversion needed!
                    let mut matches_deque: VecDeque<_> = all_matches.into_iter().collect();
                    let (rhs, bindings) = matches_deque.pop_front().expect("non-empty");

                    continuations[cont_id] = GenericContinuation::ProcessCombinations {
                        combinations,
                        results,
                        pending_rule_matches: matches_deque,
                        env: result_env.clone(),
                        depth,
                        parent_cont,
                    };

                    // Apply generic bindings - no conversion needed
                    let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated_rhs,
                        env: result_env,
                        depth,
                        cont_id,
                        is_tail_call: true,
                    });
                }
            } else {
                // All combinations processed - results already contains generic values
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, env),
                });
            }
        }

        GenericContinuation::ProcessLet {
            pending_values,
            pattern,
            body,
            mut results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (result_values, result_env) = result;

            match pending_values {
                None => {
                    // First resumption: result_values are values to pattern match
                    let mut values: VecDeque<C::Value> = result_values.into_iter().collect();

                    // Try to find a matching value
                    loop {
                        match values.pop_front() {
                            Some(value) => {
                                // Use generic pattern matching - NO conversion needed
                                if let Some(bindings) = pattern_match_generic(&pattern, &value) {
                                    // Pattern matches - instantiate body and evaluate
                                    let instantiated_body = apply_bindings_generic(&body, &bindings, ctx.factory());

                                    // Restore continuation for collecting more results
                                    continuations[cont_id] = GenericContinuation::ProcessLet {
                                        pending_values: Some(values),
                                        pattern,
                                        body,
                                        results,
                                        env: result_env.clone(),
                                        depth,
                                        parent_cont,
                                    };

                                    // Push body evaluation - THIS IS TAIL CALL (TCO)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: instantiated_body,
                                        env: result_env,
                                        depth, // TCO: reuse depth for body eval
                                        cont_id,
                                        is_tail_call: true,
                                    });
                                    return;
                                }
                                // Pattern doesn't match - continue to next value
                            }
                            None => {
                                // No pattern matched - return results to parent
                                work_stack.push(GenericWorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (results, result_env),
                                });
                                return;
                            }
                        }
                    }
                }
                Some(mut remaining_values) => {
                    // Subsequent resumption: result_values are body evaluation results
                    // Add body results to collected results
                    results.extend(result_values);

                    // Try next value
                    loop {
                        match remaining_values.pop_front() {
                            Some(value) => {
                                if let Some(bindings) = pattern_match_generic(&pattern, &value) {
                                    // Pattern matches - evaluate body with bindings
                                    let instantiated_body = apply_bindings_generic(&body, &bindings, ctx.factory());

                                    // Restore continuation for collecting more results
                                    continuations[cont_id] = GenericContinuation::ProcessLet {
                                        pending_values: Some(remaining_values),
                                        pattern,
                                        body,
                                        results,
                                        env: result_env.clone(),
                                        depth,
                                        parent_cont,
                                    };

                                    // Push body evaluation - THIS IS TAIL CALL (TCO)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: instantiated_body,
                                        env: result_env,
                                        depth, // TCO: reuse depth for body eval
                                        cont_id,
                                        is_tail_call: true,
                                    });
                                    return;
                                }
                                // Pattern doesn't match - continue to next value
                            }
                            None => {
                                // All values processed - return results to parent
                                work_stack.push(GenericWorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (results, result_env),
                                });
                                return;
                            }
                        }
                    }
                }
            }
        }

        GenericContinuation::CollectGroundedArg {
            mut items,
            grounded_indices,
            current_idx,
            mut evaluated_results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (result_values, result_env) = result;

            // Take first result from evaluation
            if let Some(first_result) = result_values.into_iter().next() {
                evaluated_results.push(first_result);
            }

            let next_idx = current_idx + 1;
            if next_idx < grounded_indices.len() {
                // More grounded args to evaluate
                let arg_idx = grounded_indices[next_idx];
                let arg_to_eval = items[arg_idx].clone();

                continuations[cont_id] = GenericContinuation::CollectGroundedArg {
                    items,
                    grounded_indices,
                    current_idx: next_idx,
                    evaluated_results,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: arg_to_eval,
                    env: result_env,
                    depth: depth + 1,
                    cont_id,
                    is_tail_call: false,
                });
            } else {
                // All grounded args evaluated - substitute results back into items
                for (i, grounded_idx) in grounded_indices.iter().enumerate() {
                    if i < evaluated_results.len() {
                        items[*grounded_idx] = evaluated_results[i].clone();
                    }
                }

                // Use items directly without token resolution.
                // Not required with GenericEnvironment - tokens are already resolved.
                // For now, skip resolution - tokens are already resolved in most cases.
                let resolved_sexpr = ctx.factory().sexpr(items.clone());
                let all_matches = try_match_all_rules_generic(&resolved_sexpr, &result_env, *ctx.factory());

                if !all_matches.is_empty() {
                    // Rules matched - evaluate them
                    // Already generic types - no conversion needed!
                    let mut matches_deque: VecDeque<_> = all_matches.into_iter().collect();
                    let (rhs, bindings) = matches_deque.pop_front().unwrap();

                    let match_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessRuleMatches {
                        remaining_matches: matches_deque,
                        results: vec![],
                        env: result_env.clone(),
                        depth,
                        parent_cont,
                    });

                    // Apply generic bindings - no conversion needed
                    let instantiated_rhs = apply_bindings_generic(&rhs, &bindings, ctx.factory());

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated_rhs,
                        env: result_env,
                        depth,
                        cont_id: match_cont_id,
                        is_tail_call: true,
                    });
                } else {
                    // No rules matched - continue evaluating sub-items
                    let sexpr = ctx.factory().sexpr(items);
                    work_stack.push(GenericWorkItem::Eval {
                        value: sexpr,
                        env: result_env,
                        depth,
                        cont_id: parent_cont,
                        is_tail_call: false,
                    });
                }
            }
        }

        GenericContinuation::ProcessMapAtom {
            mut remaining_elements,
            var_name,
            template,
            mut collected_results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (mut result_values, result_env) = result;

            // Add first result from evaluation
            if result_values.is_empty() {
                collected_results.push(ctx.factory().unit());
            } else {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![first_result], result_env),
                    });
                    return;
                }
                collected_results.push(first_result);
            }

            if remaining_elements.is_empty() {
                // All elements processed - return result list
                let result_list = ctx.factory().sexpr(collected_results);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![result_list], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &template, &var_name, &next_element, ctx.factory(),
                );

                continuations[cont_id] = GenericContinuation::ProcessMapAtom {
                    remaining_elements,
                    var_name,
                    template,
                    collected_results,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessFilterAtom {
            current_element,
            mut remaining_elements,
            var_name,
            predicate,
            mut filtered_results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (mut result_values, result_env) = result;

            // Check predicate result and optionally include current element
            if !result_values.is_empty() {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![first_result], result_env),
                    });
                    return;
                }

                let should_include = if let Some(b) = first_result.as_bool() {
                    b
                } else {
                    !first_result.is_unit()
                };

                if should_include {
                    if let Some(elem) = current_element {
                        filtered_results.push(elem);
                    }
                }
            }

            if remaining_elements.is_empty() {
                // All elements processed - return filtered list
                let result_list = ctx.factory().sexpr(filtered_results);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![result_list], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &predicate, &var_name, &next_element, ctx.factory(),
                );

                continuations[cont_id] = GenericContinuation::ProcessFilterAtom {
                    current_element: Some(next_element),
                    remaining_elements,
                    var_name,
                    predicate,
                    filtered_results,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessFoldlAtom {
            mut remaining_elements,
            acc_var_name,
            item_var_name,
            operation,
            env: _,
            depth,
            parent_cont,
        } => {
            let (mut result_values, result_env) = result;

            // Get the new accumulator value from the result
            let accumulator = if result_values.is_empty() {
                ctx.factory().unit()
            } else {
                let first_result = result_values.swap_remove(0);

                // Check for error propagation
                if first_result.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![first_result], result_env),
                    });
                    return;
                }
                first_result
            };

            if remaining_elements.is_empty() {
                // All elements processed - return final accumulator
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![accumulator], result_env),
                });
            } else {
                // More elements to process
                let next_element = remaining_elements.pop_front().expect("remaining_elements is non-empty");

                // Use generic substitute - NO conversion needed
                let instantiated = substitute_variable_generic(
                    &operation, &acc_var_name, &accumulator, ctx.factory(),
                );
                let instantiated = substitute_variable_generic(
                    &instantiated, &item_var_name, &next_element, ctx.factory(),
                );

                continuations[cont_id] = GenericContinuation::ProcessFoldlAtom {
                    remaining_elements,
                    acc_var_name,
                    item_var_name,
                    operation,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessIfCondition {
            then_branch,
            else_branch,
            env: _,
            depth,
            parent_cont,
        } => {
            let (cond_results, env_after_cond) = result;

            if let Some(first) = cond_results.first() {
                // Check for error in condition
                if first.is_error() {
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![first.clone()], env_after_cond),
                    });
                    return;
                }

                // Check if condition is true
                let is_true = if let Some(b) = first.as_bool() {
                    b
                } else if first.is_unit() {
                    false
                } else {
                    true
                };

                // Evaluate the selected branch - TCO
                let branch = if is_true { then_branch } else { else_branch };
                work_stack.push(GenericWorkItem::Eval {
                    value: branch,
                    env: env_after_cond,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else {
                // No result from condition - treat as false
                work_stack.push(GenericWorkItem::Eval {
                    value: else_branch,
                    env: env_after_cond,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            }
        }

        GenericContinuation::ProcessCaseAtom {
            cases,
            env: _,
            depth,
            parent_cont,
        } => {
            let (atom_results, atom_env) = result;

            // Filter out Empty sentinels
            let filtered_results: Vec<_> = atom_results
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect();

            // Handle case when evaluation returns no results
            if filtered_results.is_empty() {
                // Match Empty against cases - NO conversion needed
                let empty_atom = ctx.factory().atom("Empty");
                match eval_switch_generic(&empty_atom, &cases, ctx.factory()) {
                    GenericSwitchResult::Match(template, _bindings) => {
                        // Template needs evaluation
                        work_stack.push(GenericWorkItem::Eval {
                            value: template,
                            env: atom_env,
                            depth,
                            cont_id: parent_cont,
                            is_tail_call: true,
                        });
                    }
                    GenericSwitchResult::Error(err) => {
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![err], atom_env),
                        });
                    }
                    GenericSwitchResult::NoMatch => {
                        // No case matched - return NotReducible (matches heap engine behavior)
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![ctx.factory().atom("NotReducible")], atom_env),
                        });
                    }
                }
                return;
            }

            // Process first atom
            let mut remaining_atoms: VecDeque<C::Value> = filtered_results.into_iter().collect();

            if let Some(first_atom) = remaining_atoms.pop_front() {
                // Check if atom is empty (Nil or empty SExpr) - use trait methods, NO conversion
                let is_empty_atom = first_atom.is_empty()
                    || first_atom.as_sexpr().map_or(false, |items| items.is_empty());
                let switch_atom = if is_empty_atom {
                    ctx.factory().atom("Empty")
                } else {
                    first_atom
                };

                // Use generic switch - NO conversion needed
                match eval_switch_generic(&switch_atom, &cases, ctx.factory()) {
                    GenericSwitchResult::Match(template, _bindings) => {
                        if remaining_atoms.is_empty() {
                            work_stack.push(GenericWorkItem::Eval {
                                value: template,
                                env: atom_env,
                                depth,
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            let multi_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessCaseMultiResults {
                                remaining_atoms,
                                cases,
                                collected: vec![],
                                env: atom_env.clone(),
                                depth,
                                parent_cont,
                            });

                            work_stack.push(GenericWorkItem::Eval {
                                value: template,
                                env: atom_env,
                                depth,
                                cont_id: multi_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }
                    GenericSwitchResult::Error(err) => {
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![err], atom_env),
                        });
                    }
                    GenericSwitchResult::NoMatch => {
                        // No case matched - return NotReducible (matches heap engine behavior)
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![ctx.factory().atom("NotReducible")], atom_env),
                        });
                    }
                }
            }
        }

        GenericContinuation::ProcessCaseMultiResults {
            mut remaining_atoms,
            cases,
            mut collected,
            env,
            depth,
            parent_cont,
        } => {
            let (results, _result_env) = result;
            collected.extend(results);

            if let Some(next_atom) = remaining_atoms.pop_front() {
                // Check if atom is empty - use trait methods, NO conversion
                let is_empty_atom = next_atom.is_empty()
                    || next_atom.as_sexpr().map_or(false, |items| items.is_empty());
                let switch_atom = if is_empty_atom {
                    ctx.factory().atom("Empty")
                } else {
                    next_atom
                };

                // Use generic switch - NO conversion needed
                match eval_switch_generic(&switch_atom, &cases, ctx.factory()) {
                    GenericSwitchResult::Match(template, _bindings) => {
                        continuations[cont_id] = GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            parent_cont,
                        };

                        work_stack.push(GenericWorkItem::Eval {
                            value: template,
                            env,
                            depth,
                            cont_id,
                            is_tail_call: true,
                        });
                    }
                    GenericSwitchResult::Error(err) => {
                        // Collect error and continue
                        collected.push(err);

                        continuations[cont_id] = GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            parent_cont,
                        };

                        work_stack.push(GenericWorkItem::Resume {
                            cont_id,
                            result: (vec![], env),
                        });
                    }
                    GenericSwitchResult::NoMatch => {
                        // No match - continue to next atom
                        continuations[cont_id] = GenericContinuation::ProcessCaseMultiResults {
                            remaining_atoms,
                            cases,
                            collected,
                            env: env.clone(),
                            depth,
                            parent_cont,
                        };

                        work_stack.push(GenericWorkItem::Resume {
                            cont_id,
                            result: (vec![], env),
                        });
                    }
                }
            } else {
                // All atoms processed
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (collected, env),
                });
            }
        }

        GenericContinuation::ProcessEvalEval {
            env: _,
            depth,
            parent_cont,
        } => {
            let (eval_results, result_env) = result;

            if eval_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![], result_env),
                });
            } else if eval_results.len() == 1 {
                // Single result - evaluate it (TCO)
                // Unwrap Quoted values: (eval (quote X)) → evaluate X.
                // Quoted is self-evaluating, so without this unwrap we'd loop.
                let mut value = eval_results.into_iter().next().unwrap();
                if let Some(inner) = value.as_quoted() {
                    value = inner;
                }
                work_stack.push(GenericWorkItem::Eval {
                    value,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else {
                // Multiple results - evaluate each (unwrap Quoted values)
                let mut results_deque: VecDeque<_> = eval_results.into_iter().map(|v| {
                    if let Some(inner) = v.as_quoted() { inner } else { v }
                }).collect();
                let first = results_deque.pop_front().unwrap();

                let eval_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessAmb {
                    remaining_alts: results_deque,
                    results: vec![],
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: first,
                    env: result_env,
                    depth,
                    cont_id: eval_cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessReturn {
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (arg_results, arg_env) = result;

            // Check for errors first - pass through without wrapping
            if let Some(err) = arg_results.iter().find(|r| r.is_error()) {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err.clone()], arg_env),
                });
            } else {
                // Wrap results in return structure: (return value)
                let return_results: Vec<C::Value> = arg_results
                    .into_iter()
                    .map(|r| {
                        ctx.factory().sexpr(vec![
                            ctx.factory().atom("return"),
                            r,
                        ])
                    })
                    .collect();
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (return_results, arg_env),
                });
            }
        }

        GenericContinuation::ProcessChainExpr {
            var,
            body,
            env: _,
            depth,
            parent_cont,
        } => {
            let (expr_results, result_env) = result;

            if expr_results.is_empty() {
                // Empty result - evaluate body with unbound variable
                work_stack.push(GenericWorkItem::Eval {
                    value: body,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else if expr_results.len() == 1 {
                // Single result - substitute and evaluate body - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &expr_results[0],
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else {
                // Multiple results - chain evaluates each
                let mut results_deque: VecDeque<_> = expr_results.into_iter().collect();
                let first = results_deque.pop_front().unwrap();

                let chain_body_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessChainBody {
                    remaining_values: results_deque,
                    var: var.clone(),
                    body: body.clone(),
                    results: vec![],
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                });

                // Substitute variable generically - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &first,
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env: result_env,
                    depth,
                    cont_id: chain_body_cont_id,
                    is_tail_call: false,
                });
            }
        }

        GenericContinuation::ProcessChainBody {
            mut remaining_values,
            var,
            body,
            mut results,
            env,
            depth,
            parent_cont,
        } => {
            let (body_results, _result_env) = result;
            results.extend(body_results);

            if let Some(next_value) = remaining_values.pop_front() {
                continuations[cont_id] = GenericContinuation::ProcessChainBody {
                    remaining_values,
                    var: var.clone(),
                    body: body.clone(),
                    results,
                    env: env.clone(),
                    depth,
                    parent_cont,
                };

                // Substitute variable generically - NO conversion needed
                let var_name = var.as_atom().unwrap_or("");
                let instantiated = substitute_variable_generic(
                    &body,
                    var_name,
                    &next_value,
                    ctx.factory(),
                );

                work_stack.push(GenericWorkItem::Eval {
                    value: instantiated,
                    env,
                    depth,
                    cont_id,
                    is_tail_call: false,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, env),
                });
            }
        }

        GenericContinuation::ProcessFunction {
            iteration_count,
            env: _,
            depth,
            parent_cont,
        } => {
            const MAX_ITERATIONS: usize = 1000;
            let (eval_results, current_env) = result;

            if eval_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().unit()], current_env),
                });
            } else {
                // Helper: check if a value is a (return ...) expression
                fn is_return_expr<V: MettaValueTrait>(v: &V) -> bool {
                    if let Some(items) = v.as_sexpr() {
                        if items.len() == 2 {
                            if let Some(s) = items[0].as_atom() {
                                return s == "return";
                            }
                        }
                    }
                    false
                }

                // Partition into return values and continue expressions
                let (final_results, continue_exprs): (Vec<_>, Vec<_>) =
                    eval_results.into_iter().partition(|r| is_return_expr(r));

                if !final_results.is_empty() {
                    // Extract return values - unwrap (return value) to just value
                    let returns: Vec<C::Value> = final_results
                        .into_iter()
                        .map(|r| {
                            if let Some(items) = r.as_sexpr() {
                                items[1].clone()
                            } else {
                                r // shouldn't happen, but be safe
                            }
                        })
                        .collect();
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (returns, current_env),
                    });
                } else if continue_exprs.is_empty() {
                    // Nothing to continue
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![ctx.factory().unit()], current_env),
                    });
                } else if iteration_count >= MAX_ITERATIONS {
                    // Hit iteration limit
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (continue_exprs, current_env),
                    });
                } else {
                    // Continue evaluating
                    if continue_exprs.len() == 1 {
                        let next_expr = continue_exprs.into_iter().next().unwrap();
                        let func_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessFunction {
                            iteration_count: iteration_count + 1,
                            env: current_env.clone(),
                            depth,
                            parent_cont,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: next_expr,
                            env: current_env,
                            depth, // TCO: reuse depth for iteration
                            cont_id: func_cont_id,
                            is_tail_call: false,
                        });
                    } else {
                        // Multiple continue expressions - just return them
                        // (more complex handling would evaluate each, but this matches heap engine)
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (continue_exprs, current_env),
                        });
                    }
                }
            }
        }

        GenericContinuation::ProcessIsError {
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (expr_results, result_env) = result;

            let is_error = expr_results.iter().any(|v| v.is_error());
            let result_value = ctx.factory().bool(is_error);

            work_stack.push(GenericWorkItem::Resume {
                cont_id: parent_cont,
                result: (vec![result_value], result_env),
            });
        }

        GenericContinuation::ProcessCatch {
            default,
            env: _,
            depth,
            parent_cont,
        } => {
            let (expr_results, result_env) = result;

            // Check if any result is an error
            let has_error = expr_results.iter().any(|v| v.is_error());

            if has_error {
                // Evaluate default value
                work_stack.push(GenericWorkItem::Eval {
                    value: default,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else {
                // No error - return original results
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (expr_results, result_env),
                });
            }
        }

        GenericContinuation::ProcessConjunction {
            mut remaining_goals,
            mut accumulated_results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (goal_results, result_env) = result;

            // Check for error or empty result
            if goal_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![], result_env),
                });
                return;
            }

            if goal_results.iter().any(|v| v.is_error()) {
                let error = goal_results.into_iter().find(|v| v.is_error()).unwrap();
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![error], result_env),
                });
                return;
            }

            accumulated_results.extend(goal_results);

            if let Some(next_goal) = remaining_goals.pop_front() {
                continuations[cont_id] = GenericContinuation::ProcessConjunction {
                    remaining_goals,
                    accumulated_results,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: next_goal,
                    env: result_env,
                    depth: depth + 1,
                    cont_id,
                    is_tail_call: false,
                });
            } else {
                // All goals evaluated - return last result
                let final_result = accumulated_results.pop().unwrap_or_else(|| ctx.factory().unit());
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![final_result], result_env),
                });
            }
        }

        GenericContinuation::ProcessUnifyPattern1 {
            pattern2,
            success_body,
            failure_body,
            env: _,
            depth,
            parent_cont,
        } => {
            let (pattern1_results, result_env) = result;

            if pattern1_results.is_empty() {
                // Empty - evaluate failure body
                work_stack.push(GenericWorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else if pattern1_results.len() == 1 {
                // Single result - check if it's a Space (special handling)
                let val1 = pattern1_results.into_iter().next().unwrap();

                if let Some(handle) = val1.as_space() {
                    // Space unification - match pattern2 against space atoms
                    // Check if this is a simple boolean check using generic trait methods
                    // (NO heap conversion needed for this check)
                    let is_boolean_check = is_boolean_check_pattern(&success_body, &failure_body);

                    if is_boolean_check {
                        // Simple existence check - use generic method (no call-site conversion)
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's generic match_space_exists
                            result_env.match_space_exists(&pattern2)
                        } else {
                            // Non-module spaces use SpaceHandle's generic collapse
                            let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                pattern_match_generic(&pattern2, atom).is_some()
                                    || pattern_match_generic(atom, &pattern2).is_some()
                            })
                        };
                        let result_value = ctx.factory().bool(exists);
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![result_value], result_env),
                        });
                    } else {
                        // Full space matching with body evaluation
                        if handle.is_module_space() || handle.name == "self" {
                            // Module/self spaces use Environment's match_space
                            let matches: Vec<(C::Value, usize)> =
                                result_env.match_space(&pattern2, &pattern2)
                                    .into_iter()
                                    .map(|m| (m.value, m.count))
                                    .collect();

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(GenericWorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    cont_id: parent_cont,
                                    is_tail_call: true,
                                });
                            } else {
                                // Build bodies to evaluate for each match - values already generic
                                let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                                let mut found_match = false;
                                for (generic_value, count) in &matches {
                                    if let Some(bindings) = pattern_match_generic(&pattern2, generic_value) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..*count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    } else if let Some(bindings) = pattern_match_generic(generic_value, &pattern2) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..*count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push_back(failure_body.clone());
                                }

                                if let Some(first_body) = bodies_to_eval.pop_front() {
                                    if bodies_to_eval.is_empty() {
                                        // Single body - tail call directly
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            cont_id: parent_cont,
                                            is_tail_call: true,
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        let bodies_cont_id = continuations.len();
                                        continuations.push(GenericContinuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_to_eval,
                                            results: vec![],
                                            env: result_env.clone(),
                                            depth,
                                            parent_cont,
                                        });
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            cont_id: bodies_cont_id,
                                            is_tail_call: false,
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        cont_id: parent_cont,
                                        is_tail_call: true,
                                    });
                                }
                            }
                        } else {
                            // GENERIC: Non-module spaces - use generic collapse_with_multiplicity
                            use crate::backend::models::GenericMultiplicityMatch;
                            let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                                handle.collapse_with_multiplicity_generic(ctx.factory());

                            if matches.is_empty() {
                                // No matches - evaluate failure body
                                work_stack.push(GenericWorkItem::Eval {
                                    value: failure_body,
                                    env: result_env,
                                    depth,
                                    cont_id: parent_cont,
                                    is_tail_call: true,
                                });
                            } else {
                                // Build bodies to evaluate for each match - NO conversion needed
                                let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                                let mut found_match = false;
                                for m in &matches {
                                    if let Some(bindings) = pattern_match_generic(&pattern2, &m.value) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..m.count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern2) {
                                        found_match = true;
                                        let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                        for _ in 0..m.count {
                                            bodies_to_eval.push_back(generic_body.clone());
                                        }
                                    }
                                }

                                // If no pattern matched, evaluate failure body
                                if !found_match {
                                    bodies_to_eval.push_back(failure_body.clone());
                                }

                                if let Some(first_body) = bodies_to_eval.pop_front() {
                                    if bodies_to_eval.is_empty() {
                                        // Single body - tail call directly
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth,
                                            cont_id: parent_cont,
                                            is_tail_call: true,
                                        });
                                    } else {
                                        // Multiple bodies - use ProcessUnifyBodies
                                        let bodies_cont_id = continuations.len();
                                        continuations.push(GenericContinuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_to_eval,
                                            results: vec![],
                                            env: result_env.clone(),
                                            depth,
                                            parent_cont,
                                        });
                                        work_stack.push(GenericWorkItem::Eval {
                                            value: first_body,
                                            env: result_env,
                                            depth: depth + 1,
                                            cont_id: bodies_cont_id,
                                            is_tail_call: false,
                                        });
                                    }
                                } else {
                                    // No bodies at all (shouldn't happen, but handle gracefully)
                                    work_stack.push(GenericWorkItem::Eval {
                                        value: failure_body,
                                        env: result_env,
                                        depth,
                                        cont_id: parent_cont,
                                        is_tail_call: true,
                                    });
                                }
                            }
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    let unify_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: result_env.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: result_env,
                        depth: depth + 1,
                        cont_id: unify_cont_id,
                        is_tail_call: false,
                    });
                }
            } else {
                // Multiple results - iterate over them
                let mut remaining: VecDeque<_> = pattern1_results.into_iter().collect();
                let first = remaining.pop_front().unwrap();

                let iter_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results: remaining,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results: vec![],
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                });

                // Check if first is a Space
                if let Some(handle) = first.as_space() {
                    // Space unification for first value
                    if handle.is_module_space() || handle.name == "self" {
                        // Module/self spaces use Environment's match_space
                        let matches: Vec<(C::Value, usize)> =
                            result_env.match_space(&pattern2, &pattern2)
                                .into_iter()
                                .map(|m| (m.value, m.count))
                                .collect();

                        // Build bodies for matches - values already generic
                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for (generic_value, count) in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern2, generic_value) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..*count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(generic_value, &pattern2) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..*count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            let bodies_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: vec![],
                                env: result_env.clone(),
                                depth,
                                parent_cont: iter_cont_id,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                cont_id: bodies_cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id: iter_cont_id,
                                result: (vec![], result_env),
                            });
                        }
                    } else {
                        // GENERIC: Non-module spaces - use generic collapse_with_multiplicity
                        use crate::backend::models::GenericMultiplicityMatch;
                        let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                            handle.collapse_with_multiplicity_generic(ctx.factory());

                        // Build bodies for matches - NO conversion needed
                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern2, &m.value) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern2) {
                                found_match = true;
                                let generic_body = apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(generic_body.clone());
                                }
                            }
                        }

                        // If no pattern matched, include failure body
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            let bodies_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: vec![],
                                env: result_env.clone(),
                                depth,
                                parent_cont: iter_cont_id,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: result_env,
                                depth: depth + 1,
                                cont_id: bodies_cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            // No bodies at all - send empty to iterator
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id: iter_cont_id,
                                result: (vec![], result_env),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    let unify_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1: first,
                        pattern2: pattern2.clone(),
                        success_body: ctx.factory().atom("__unify_success__"),
                        failure_body: ctx.factory().atom("__unify_failure__"),
                        env: result_env.clone(),
                        depth,
                        parent_cont: iter_cont_id,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: result_env,
                        depth: depth + 1,
                        cont_id: unify_cont_id,
                        is_tail_call: false,
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyPattern1Iter {
            mut remaining_pattern1_results,
            pattern2,
            success_body,
            failure_body,
            mut all_results,
            env: _iter_env,
            depth,
            parent_cont,
        } => {
            let (body_results, env_after) = result;

            // Accumulate results from the pattern1 value we just processed
            all_results.extend(body_results);

            // Get next pattern1 value to process
            if let Some(val1) = remaining_pattern1_results.pop_front() {
                // Create new iterator continuation for the REMAINING values
                // (after this one we're about to process)
                let iter_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessUnifyPattern1Iter {
                    remaining_pattern1_results,
                    pattern2: pattern2.clone(),
                    success_body: success_body.clone(),
                    failure_body: failure_body.clone(),
                    all_results,
                    env: env_after.clone(),
                    depth,
                    parent_cont,
                });

                // Process this pattern1 value
                if let Some(handle) = val1.as_space() {
                    let pattern = pattern2.clone();
                    // Boolean check optimization: if success is True and failure is False
                    let is_boolean_check = {
                        let success_true = success_body.as_bool() == Some(true)
                            || success_body.as_atom() == Some("True");
                        let failure_false = failure_body.as_bool() == Some(false)
                            || failure_body.as_atom() == Some("False");
                        success_true && failure_false
                    };

                    if is_boolean_check {
                        // Optimized exists check - no need to collect all matches
                        let exists = if handle.is_module_space() || handle.name == "self" {
                            env_after.match_space_exists(&pattern)
                        } else {
                            let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                            atoms.iter().any(|atom| {
                                pattern_match_generic(&pattern, atom).is_some()
                                    || pattern_match_generic(atom, &pattern).is_some()
                            })
                        };
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: iter_cont_id,
                            result: (vec![ctx.factory().bool(exists)], env_after),
                        });
                    } else {
                        // Full match: get matches from appropriate source
                        use crate::backend::models::GenericMultiplicityMatch;
                        let matches: Vec<GenericMultiplicityMatch<C::Value>> =
                            if handle.is_module_space() || handle.name == "self" {
                                // Module/self spaces - use match_space
                                env_after
                                    .match_space(&pattern, &pattern)
                                    .into_iter()
                                    .map(|m| GenericMultiplicityMatch { value: m.value, count: m.count })
                                    .collect()
                            } else {
                                // Non-module spaces - use collapse_with_multiplicity_generic
                                handle.collapse_with_multiplicity_generic(ctx.factory())
                            };

                        let mut bodies_to_eval: VecDeque<C::Value> = VecDeque::new();
                        let mut found_match = false;
                        for m in &matches {
                            if let Some(bindings) = pattern_match_generic(&pattern, &m.value) {
                                found_match = true;
                                let instantiated =
                                    apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(instantiated.clone());
                                }
                            } else if let Some(bindings) = pattern_match_generic(&m.value, &pattern)
                            {
                                found_match = true;
                                let instantiated =
                                    apply_bindings_generic(&success_body, &bindings, ctx.factory());
                                for _ in 0..m.count {
                                    bodies_to_eval.push_back(instantiated.clone());
                                }
                            }
                        }
                        if !found_match {
                            bodies_to_eval.push_back(failure_body.clone());
                        }

                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            let bodies_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                results: Vec::new(),
                                env: env_after.clone(),
                                depth,
                                parent_cont: iter_cont_id,
                            });
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_body,
                                env: env_after,
                                depth: depth + 1,
                                cont_id: bodies_cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id: iter_cont_id,
                                result: (vec![], env_after),
                            });
                        }
                    }
                } else {
                    // Non-space: evaluate pattern2
                    let p2_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessUnifyPattern2 {
                        val1,
                        pattern2: pattern2.clone(),
                        success_body,
                        failure_body,
                        env: env_after.clone(),
                        depth,
                        parent_cont: iter_cont_id,
                    });
                    work_stack.push(GenericWorkItem::Eval {
                        value: pattern2,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: p2_cont_id,
                        is_tail_call: false,
                    });
                }
            } else {
                // No more pattern1 values - return all accumulated results
                if all_results.is_empty() {
                    work_stack.push(GenericWorkItem::Eval {
                        value: failure_body,
                        env: env_after,
                        depth,
                        cont_id: parent_cont,
                        is_tail_call: true,
                    });
                } else {
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (all_results, env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyPattern2 {
            val1,
            pattern2: _,
            success_body,
            failure_body,
            env: _,
            depth,
            parent_cont,
        } => {
            let (pattern2_results, result_env) = result;

            if pattern2_results.is_empty() {
                work_stack.push(GenericWorkItem::Eval {
                    value: failure_body,
                    env: result_env,
                    depth,
                    cont_id: parent_cont,
                    is_tail_call: true,
                });
            } else {
                // Try to unify with each pattern2 result - NO conversion needed
                let mut all_bindings = Vec::new();
                for p2_result in &pattern2_results {
                    if let Some(bindings) = pattern_match_generic(&val1, p2_result) {
                        all_bindings.push(bindings);
                    }
                }

                if all_bindings.is_empty() {
                    work_stack.push(GenericWorkItem::Eval {
                        value: failure_body,
                        env: result_env,
                        depth,
                        cont_id: parent_cont,
                        is_tail_call: true,
                    });
                } else if all_bindings.len() == 1 {
                    // Apply bindings generically - NO conversion needed
                    let instantiated = apply_bindings_generic(&success_body, &all_bindings[0], ctx.factory());

                    work_stack.push(GenericWorkItem::Eval {
                        value: instantiated,
                        env: result_env,
                        depth,
                        cont_id: parent_cont,
                        is_tail_call: true,
                    });
                } else {
                    // Multiple bindings - pre-instantiate all bodies generically
                    let mut bodies_to_eval: VecDeque<C::Value> = all_bindings.iter()
                        .map(|bindings| {
                            apply_bindings_generic(&success_body, bindings, ctx.factory())
                        })
                        .collect();

                    let first_body = bodies_to_eval.pop_front().unwrap();

                    let bodies_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessUnifyBodies {
                        remaining_bodies: bodies_to_eval,
                        results: vec![],
                        env: result_env.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: first_body,
                        env: result_env,
                        depth,
                        cont_id: bodies_cont_id,
                        is_tail_call: false,
                    });
                }
            }
        }

        GenericContinuation::ProcessUnifyBodies {
            mut remaining_bodies,
            mut results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (body_results, env_after_body) = result;
            results.extend(body_results);

            if let Some(next_body) = remaining_bodies.pop_front() {
                let next_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessUnifyBodies {
                    remaining_bodies,
                    results,
                    env: env_after_body.clone(),
                    depth,
                    parent_cont,
                });
                work_stack.push(GenericWorkItem::Eval {
                    value: next_body,
                    env: env_after_body,
                    depth,
                    cont_id: next_cont_id,
                    is_tail_call: false,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, env_after_body),
                });
            }
        }

        GenericContinuation::ProcessCollapse {
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (expr_results, result_env) = result;

            // Collapse collects all results into a list
            let result_list = ctx.factory().sexpr(expr_results);
            work_stack.push(GenericWorkItem::Resume {
                cont_id: parent_cont,
                result: (vec![result_list], result_env),
            });
        }

        GenericContinuation::ProcessCollapseBind {
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (expr_results, result_env) = result;

            // Collapse-bind also collects results into a list
            let result_list = ctx.factory().sexpr(expr_results);
            work_stack.push(GenericWorkItem::Resume {
                cont_id: parent_cont,
                result: (vec![result_list], result_env),
            });
        }

        GenericContinuation::ProcessAmb {
            mut remaining_alts,
            mut results,
            env: _,
            depth,
            parent_cont,
        } => {
            let (alt_results, result_env) = result;
            results.extend(alt_results);

            if let Some(next_alt) = remaining_alts.pop_front() {
                continuations[cont_id] = GenericContinuation::ProcessAmb {
                    remaining_alts,
                    results,
                    env: result_env.clone(),
                    depth,
                    parent_cont,
                };

                work_stack.push(GenericWorkItem::Eval {
                    value: next_alt,
                    env: result_env,
                    depth: depth + 1,
                    cont_id,
                    is_tail_call: false,
                });
            } else {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, result_env),
                });
            }
        }

        GenericContinuation::ProcessGuard {
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (cond_results, result_env) = result;

            match cond_results.first() {
                Some(v) if v.as_bool() == Some(true) => {
                    // Guard passes - return Unit
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![ctx.factory().unit()], result_env),
                    });
                }
                Some(v) if v.as_bool() == Some(false) => {
                    // Guard fails - return empty (nondeterministic failure)
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![], result_env),
                    });
                }
                Some(v) if v.is_error() => {
                    // Error propagates
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![v.clone()], result_env),
                    });
                }
                Some(v) => {
                    // Type error - condition must be Bool
                    let err = ctx.factory().error(
                        &format!(
                            "guard: condition must evaluate to Bool, got {}",
                            v.friendly_repr()
                        ),
                        v.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], result_env),
                    });
                }
                None => {
                    // Empty results - guard fails
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![], result_env),
                    });
                }
            }
        }

        GenericContinuation::ProcessGetAtoms {
            space_ref,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (space_results, result_env) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "get-atoms: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], result_env),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    // GENERIC: Use collapse_generic to avoid heap conversion
                    let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                    if atoms.is_empty() {
                        // Empty space returns empty results
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![], result_env),
                        });
                    } else {
                        // Return all atoms as separate results (superposition)
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (atoms, result_env),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!("get-atoms: first argument must be a space, got {}", first.friendly_repr()),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], result_env),
                    });
                }
            }
        }

        GenericContinuation::ProcessMatchSpace {
            space_arg,
            pattern,
            template,
            env,
            depth,
            parent_cont,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "match: space evaluated to empty",
                    space_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    if handle.is_module_space() || handle.name == "self" {
                        // Use match_space which handles serialization internally
                        let matches = env.match_space(&pattern, &template);
                        // Expand multiplicities into flat list
                        let generic_results: Vec<C::Value> = matches
                            .into_iter()
                            .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                            .collect();
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (generic_results, env_after),
                        });
                    } else {
                        // Owned space - match against atoms in SpaceHandle
                        // GENERIC: Use collapse_generic and pattern_match_generic to avoid heap conversions
                        let atoms: Vec<C::Value> = handle.collapse_generic(ctx.factory());
                        let mut instantiated_templates: Vec<C::Value> = Vec::new();

                        for atom in &atoms {
                            if let Some(bindings) = pattern_match_generic(&pattern, atom) {
                                let instantiated = apply_bindings_generic(&template, &bindings, ctx.factory());
                                instantiated_templates.push(instantiated);
                            }
                        }

                        if instantiated_templates.is_empty() {
                            work_stack.push(GenericWorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![], env_after),
                            });
                        } else if instantiated_templates.len() == 1 {
                            work_stack.push(GenericWorkItem::Eval {
                                value: instantiated_templates.into_iter().next().unwrap(),
                                env: env_after,
                                depth,
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            // Multiple matches - queue template evaluations
                            let mut generic_templates: VecDeque<C::Value> = instantiated_templates
                                .into_iter()
                                .collect();
                            let first_template = generic_templates.pop_front().unwrap();

                            let templates_cont_id = continuations.len();
                            continuations.push(GenericContinuation::ProcessMatchTemplates {
                                remaining_templates: generic_templates,
                                results: vec![],
                                env: env_after.clone(),
                                depth,
                                parent_cont,
                            });

                            let forked_env = env_after.fork_for_nondeterminism();
                            work_stack.push(GenericWorkItem::Eval {
                                value: first_template,
                                env: forked_env,
                                depth,
                                cont_id: templates_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessMatchTemplates {
            mut remaining_templates,
            mut results,
            env,
            depth,
            parent_cont,
        } => {
            let (template_results, _env_after) = result;
            results.extend(template_results);

            if remaining_templates.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (results, env),
                });
            } else {
                let next_template = remaining_templates.pop_front().unwrap();

                let templates_cont_id = continuations.len();
                continuations.push(GenericContinuation::ProcessMatchTemplates {
                    remaining_templates,
                    results,
                    env: env.clone(),
                    depth,
                    parent_cont,
                });

                work_stack.push(GenericWorkItem::Eval {
                    value: next_template,
                    env: env.fork_for_nondeterminism(),
                    depth,
                    cont_id: templates_cont_id,
                    is_tail_call: true,
                });
            }
        }

        GenericContinuation::ProcessAddAtomSpace {
            space_ref,
            atom,
            env: _,
            depth,
            parent_cont,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "add-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    let add_atom_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessAddAtomAtom {
                        space_handle: handle.clone(),
                        atom: atom.clone(),
                        env: env_after.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: atom,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: add_atom_cont_id,
                        is_tail_call: false,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessAddAtomAtom {
            space_handle,
            atom,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    "add-atom: atom evaluated to empty",
                    atom,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // GENERIC: Use add_atom_generic to avoid heap conversion
                space_handle.add_atom_generic(&atom_results[0]);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().unit()], env_after),
                });
            }
        }

        GenericContinuation::ProcessRemoveAtomSpace {
            space_ref,
            atom,
            env: _,
            depth,
            parent_cont,
        } => {
            let (space_results, env_after) = result;

            if space_results.is_empty() {
                let err = ctx.factory().error(
                    "remove-atom: space evaluated to empty",
                    space_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &space_results[0];
                if let Some(handle) = first.as_space() {
                    let remove_atom_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessRemoveAtomAtom {
                        space_handle: handle.clone(),
                        atom: atom.clone(),
                        env: env_after.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: atom,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: remove_atom_cont_id,
                        is_tail_call: false,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessRemoveAtomAtom {
            space_handle,
            atom,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    "remove-atom: atom evaluated to empty",
                    atom,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // Remove the atom from the space
                // NOTE: Arena engine returns Unit() regardless of whether removal succeeded
                // GENERIC: Use remove_atom_generic to avoid heap conversion
                space_handle.remove_atom_generic(&atom_results[0]);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().unit()], env_after),
                });
            }
        }

        GenericContinuation::ProcessNewState {
            initial_value,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (init_results, mut env_after) = result;

            if init_results.is_empty() {
                let err = ctx.factory().error(
                    "new-state: initial value evaluated to empty",
                    initial_value,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // Use create_state directly - values are already V
                let state_id = env_after.create_state(&init_results[0]);
                let state_value = ctx.factory().state(state_id);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![state_value], env_after),
                });
            }
        }

        GenericContinuation::ProcessGetState {
            state_ref,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    "get-state: state reference evaluated to empty",
                    state_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &state_results[0];
                if let Some(state_id) = first.as_state() {
                    // Use get_state directly - returns V
                    if let Some(generic_value) = env_after.get_state(state_id) {
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![generic_value], env_after),
                        });
                    } else {
                        let err = ctx.factory().error(
                            &format!("get-state: state {} not found", state_id),
                            first.clone(),
                        );
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![err], env_after),
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "get-state: argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessChangeStateRef {
            state_ref,
            new_value,
            env: _,
            depth,
            parent_cont,
        } => {
            let (state_results, env_after) = result;

            if state_results.is_empty() {
                let err = ctx.factory().error(
                    "change-state!: state reference evaluated to empty",
                    state_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &state_results[0];
                if first.as_state().is_some() {
                    let change_value_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessChangeStateValue {
                        state_value: first.clone(),
                        new_value: new_value.clone(),
                        env: env_after.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: new_value,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: change_value_cont_id,
                        is_tail_call: false,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "change-state!: first argument must be a state reference, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessChangeStateValue {
            state_value,
            new_value,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (value_results, mut env_after) = result;

            if value_results.is_empty() {
                let err = ctx.factory().error(
                    "change-state!: new value evaluated to empty",
                    new_value,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // Get the state ID from state_value
                if let Some(state_id) = state_value.as_state() {
                    // Use change_state directly - values are already V
                    env_after.change_state(state_id, &value_results[0]);
                    let result_state = ctx.factory().state(state_id);
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![result_state], env_after),
                    });
                } else {
                    let err = ctx.factory().error(
                        "change-state!: expected state value",
                        state_value,
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessRepr {
            atom: _,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().string("")], env_after),
                });
            } else {
                let repr = atom_results[0].friendly_repr();
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().string(&repr)], env_after),
                });
            }
        }

        GenericContinuation::ProcessFormatArgsString {
            format_arg,
            args_arg,
            env: _,
            depth,
            parent_cont,
        } => {
            let (format_results, env_after) = result;

            if format_results.is_empty() {
                let err = ctx.factory().error(
                    "format-args: format string evaluated to empty",
                    format_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &format_results[0];
                if let Some(format_str) = first.as_string() {
                    let format_args_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessFormatArgsArgs {
                        format_str: format_str.to_string(),
                        args_arg: args_arg.clone(),
                        env: env_after.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: args_arg,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: format_args_cont_id,
                        is_tail_call: false,
                    });
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "format-args: first argument must be a string, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessFormatArgsArgs {
            format_str,
            args_arg,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (args_results, env_after) = result;

            if args_results.is_empty() {
                let err = ctx.factory().error(
                    "format-args: args evaluated to empty",
                    args_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // Get args as a list - use native generic values directly
                let args_list: Vec<&C::Value> = if let Some(items) = args_results[0].as_sexpr() {
                    items.iter().collect()
                } else {
                    args_results.iter().collect()
                };

                // Simple format string substitution using friendly_repr
                let mut result_str = format_str.clone();
                for (i, arg) in args_list.iter().enumerate() {
                    let placeholder = format!("{{{}}}", i);
                    let repr = arg.friendly_repr();
                    result_str = result_str.replace(&placeholder, &repr);
                }

                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().string(&result_str)], env_after),
                });
            }
        }

        GenericContinuation::ProcessPrintln {
            atom: _,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, env_after) = result;

            for atom_result in &atom_results {
                // Use to_display_string() - prints strings without quotes
                println!("{}", atom_result.to_display_string());
            }

            work_stack.push(GenericWorkItem::Resume {
                cont_id: parent_cont,
                result: (vec![ctx.factory().unit()], env_after),
            });
        }

        GenericContinuation::ProcessTraceMessage {
            message: _,
            value_expr,
            env: _,
            depth,
            parent_cont,
        } => {
            let (msg_results, env_after) = result;

            // Get message string
            let message_str = if let Some(first) = msg_results.first() {
                first.friendly_repr()
            } else {
                "<empty>".to_string()
            };

            // Print the message prefix
            eprint!("[TRACE] {}: ", message_str);

            // Now evaluate the value
            let trace_value_cont_id = continuations.len();
            continuations.push(GenericContinuation::ProcessTraceValue {
                message_str,
                value_expr: value_expr.clone(),
                env: env_after.clone(),
                depth,
                parent_cont,
            });

            work_stack.push(GenericWorkItem::Eval {
                value: value_expr,
                env: env_after,
                depth: depth + 1,
                cont_id: trace_value_cont_id,
                is_tail_call: false,
            });
        }

        GenericContinuation::ProcessTraceValue {
            message_str: _,
            value_expr,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (value_results, env_after) = result;

            if value_results.is_empty() {
                let err = ctx.factory().error("trace!: value evaluated to empty", value_expr);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                // Print the value
                for value in &value_results {
                    eprintln!("{}", value.friendly_repr());
                }

                // Return the evaluated value
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (value_results, env_after),
                });
            }
        }

        GenericContinuation::ProcessGetMetatype {
            atom: _,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, env_after) = result;

            if atom_results.is_empty() {
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().atom("Undefined")], env_after),
                });
            } else {
                let first = &atom_results[0];
                // Quoted is transparent to get-metatype: returns "Expression"
                let metatype = if first.is_quoted() {
                    "Expression"
                } else if first.as_atom().is_some() {
                    "Symbol"
                } else if first.as_sexpr().is_some() {
                    "Expression"
                } else if first.is_bool() {
                    "Grounded"
                } else if first.is_long() {
                    "Grounded"
                } else if first.is_float() {
                    "Grounded"
                } else if first.is_string() {
                    "Grounded"
                } else if first.is_error() {
                    "Error"
                } else {
                    "Undefined"
                };

                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().atom(metatype)], env_after),
                });
            }
        }

        GenericContinuation::ProcessBind {
            token,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (atom_results, mut env_after) = result;

            if atom_results.is_empty() {
                let err = ctx.factory().error(
                    "bind!: atom evaluated to empty",
                    ctx.factory().atom(&token),
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                env_after.register_token(&token, atom_results[0].clone());
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![ctx.factory().unit()], env_after),
                });
            }
        }

        // Memo-related continuations - delegate to heap conversion for now
        GenericContinuation::ProcessMemoTable {
            memo_ref,
            expr,
            first_only,
            env: _,
            depth,
            parent_cont,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let err = ctx.factory().error(
                    "memo/memo!: memo reference evaluated to empty",
                    memo_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    // Check if already cached - use generic lookup
                    if let Some(cached) = memo_handle.lookup_generic(&expr, ctx.factory()) {
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (cached, env_after),
                        });
                    } else {
                        // Not cached - evaluate and cache result
                        let memo_expr_cont_id = continuations.len();
                        continuations.push(GenericContinuation::ProcessMemoExpr {
                            memo_handle: memo_handle.clone(),
                            expr: expr.clone(),
                            first_only,
                            env: env_after.clone(),
                            depth,
                            parent_cont,
                        });

                        work_stack.push(GenericWorkItem::Eval {
                            value: expr,
                            env: env_after,
                            depth: depth + 1,
                            cont_id: memo_expr_cont_id,
                            is_tail_call: false,
                        });
                    }
                } else {
                    let err = ctx.factory().error(
                        &format!(
                            "memo/memo!: first argument must be a memo table, got {}",
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessMemoExpr {
            memo_handle,
            expr,
            first_only,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (expr_results, env_after) = result;

            // Cache the result using generic store
            if first_only && !expr_results.is_empty() {
                memo_handle.store_generic(&expr, &expr_results[..1]);
            } else {
                memo_handle.store_generic(&expr, &expr_results);
            }

            work_stack.push(GenericWorkItem::Resume {
                cont_id: parent_cont,
                result: (expr_results, env_after),
            });
        }

        GenericContinuation::ProcessNewMemoName {
            name_arg,
            size_arg,
            env: _,
            depth,
            parent_cont,
        } => {
            let (name_results, env_after) = result;

            if name_results.is_empty() {
                let err = ctx.factory().error(
                    "new-memo: name evaluated to empty",
                    name_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &name_results[0];
                let name = if let Some(s) = first.as_string() {
                    s.to_string()
                } else if let Some(a) = first.as_atom() {
                    a.to_string()
                } else {
                    first.friendly_repr()
                };

                if let Some(size_value) = size_arg {
                    let new_memo_size_cont_id = continuations.len();
                    continuations.push(GenericContinuation::ProcessNewMemoSize {
                        name,
                        size_arg: size_value.clone(),
                        env: env_after.clone(),
                        depth,
                        parent_cont,
                    });

                    work_stack.push(GenericWorkItem::Eval {
                        value: size_value,
                        env: env_after,
                        depth: depth + 1,
                        cont_id: new_memo_size_cont_id,
                        is_tail_call: false,
                    });
                } else {
                    // No size argument - create memo with default size (no limit)
                    let memo_handle = crate::backend::models::MemoHandle::new(name);
                    let memo_value = ctx.factory().memo(memo_handle);
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![memo_value], env_after),
                    });
                }
            }
        }

        GenericContinuation::ProcessNewMemoSize {
            name,
            size_arg,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (size_results, env_after) = result;

            if size_results.is_empty() {
                let err = ctx.factory().error(
                    "new-memo: size evaluated to empty",
                    size_arg,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let size = size_results[0].as_long().unwrap_or(1000) as usize;
                let memo_handle = crate::backend::models::MemoHandle::with_max_size(name, size);
                let memo_value = ctx.factory().memo(memo_handle);
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![memo_value], env_after),
                });
            }
        }

        GenericContinuation::ProcessMemoOp {
            memo_ref,
            is_clear,
            env: _,
            depth: _,
            parent_cont,
        } => {
            let (memo_results, env_after) = result;

            if memo_results.is_empty() {
                let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                let err = ctx.factory().error(
                    &format!("{}: memo reference evaluated to empty", op_name),
                    memo_ref,
                );
                work_stack.push(GenericWorkItem::Resume {
                    cont_id: parent_cont,
                    result: (vec![err], env_after),
                });
            } else {
                let first = &memo_results[0];
                if let Some(memo_handle) = first.as_memo() {
                    if is_clear {
                        memo_handle.clear();
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![ctx.factory().unit()], env_after),
                        });
                    } else {
                        let stats = memo_handle.stats();
                        let stats_sexpr = ctx.factory().sexpr(vec![
                            ctx.factory().atom("hits"),
                            ctx.factory().long(stats.0 as i64),
                            ctx.factory().atom("misses"),
                            ctx.factory().long(stats.1 as i64),
                            ctx.factory().atom("size"),
                            ctx.factory().long(stats.2 as i64),
                        ]);
                        work_stack.push(GenericWorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![stats_sexpr], env_after),
                        });
                    }
                } else {
                    let op_name = if is_clear { "clear-memo!" } else { "memo-stats" };
                    let err = ctx.factory().error(
                        &format!(
                            "{}: argument must be a memo table, got {}",
                            op_name,
                            first.friendly_repr()
                        ),
                        first.clone(),
                    );
                    work_stack.push(GenericWorkItem::Resume {
                        cont_id: parent_cont,
                        result: (vec![err], env_after),
                    });
                }
            }
        }
    }
}
