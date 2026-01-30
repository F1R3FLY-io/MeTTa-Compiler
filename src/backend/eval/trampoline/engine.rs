//! Trampoline Engine - Iterative Evaluation
//!
//! This module contains the main `eval_trampoline` function that implements
//! iterative evaluation using an explicit work stack instead of recursive
//! function calls. This prevents stack overflow for deeply nested expressions.

use std::collections::VecDeque;
use std::sync::Arc;

use tracing::trace;

use crate::backend::environment::{Environment, MultiplicityMatch};
use crate::backend::grounded::{ExecError, GroundedWork};
use crate::backend::models::{EvalResult, MemoHandle, MettaValue, MettaValueInner};

use super::super::{
    apply_bindings, eval_step, friendly_value_repr, pattern_match, process_collected_sexpr,
    EvalStep, MemoOpType, ProcessedSExpr,
};
use super::types::{Continuation, WorkItem};

/// Iterative evaluation using a trampoline pattern with explicit work stack.
/// This prevents stack overflow by using heap-allocated work items instead of
/// recursive function calls.
pub fn eval_trampoline(value: MettaValue, env: Environment) -> EvalResult {
    // Debug tracing controlled by environment variable
    let debug_eval = std::env::var("METTA_DEBUG_EVAL").is_ok();
    let mut eval_count: u64 = 0;

    // Initialize work stack with the initial evaluation
    let mut work_stack: Vec<WorkItem> = vec![WorkItem::Eval {
        value,
        env: env.clone(),
        depth: 0,
        cont_id: 0,          // Done continuation
        is_tail_call: false, // Initial evaluation is not a tail call
    }];

    // Continuation storage - index 0 is always Done
    let mut continuations: Vec<Continuation> = vec![Continuation::Done];

    // Final result storage
    let mut final_result: Option<EvalResult> = None;

    // Main trampoline loop
    while let Some(work) = work_stack.pop() {
        match work {
            WorkItem::Eval {
                value,
                env,
                depth,
                cont_id,
                is_tail_call,
            } => {
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?value, depth, cont_id, "eval work item");

                // Debug trace
                if debug_eval {
                    eval_count += 1;
                    if eval_count % 1000 == 0 || eval_count < 100 {
                        eprintln!(
                            "[EVAL#{}] depth={} work_stack={} conts={} value={}",
                            eval_count,
                            depth,
                            work_stack.len(),
                            continuations.len(),
                            friendly_value_repr(&value)
                        );
                    }
                }

                // Perform one step of evaluation
                // For tail calls, we don't increment depth - this enables TCO
                let step_result = eval_step(value, env.clone(), depth);
                let _ = is_tail_call; // Used to determine depth in push sites
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?step_result);

                match step_result {
                    // Direct result - resume continuation
                    EvalStep::Done(result) => {
                        work_stack.push(WorkItem::Resume { cont_id, result });
                    }

                    // Need to evaluate S-expression sub-items
                    EvalStep::EvalSExpr { items, env, depth } => {
                        if items.is_empty() {
                            // HE-compatible: empty SExpr () evaluates to itself, not Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::SExpr(vec![])], env),
                            });
                        } else {
                            // Convert to VecDeque ONCE (O(n)) and pop front (O(1))
                            // This avoids O(n) slice copy + O(n) remove(0) = O(n²) total
                            let mut items_deque: VecDeque<MettaValue> = items.into_iter().collect();
                            let first = items_deque.pop_front().unwrap();

                            // Create continuation to collect results
                            let collect_cont_id = continuations.len();
                            continuations.push(Continuation::CollectSExpr {
                                remaining: items_deque, // Already a VecDeque, no copy needed
                                collected: Vec::new(),
                                original_env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate first item (moved, not cloned)
                            // NOT a tail call - more items to process after this
                            work_stack.push(WorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: collect_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start a TCO grounded operation (e.g., +, -, and, or)
                    EvalStep::StartGroundedOp { state, env, depth } => {
                        // Look up the TCO operation
                        if let Some(grounded_op) = env.get_grounded_operation_tco(&state.op_name) {
                            let mut state = state;
                            match grounded_op.execute_step(&mut state) {
                                GroundedWork::Done(results) => {
                                    // Operation completed immediately (rare: all args already evaluated)
                                    let values: Vec<MettaValue> =
                                        results.into_iter().map(|(v, _)| v).collect();
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (values, env),
                                    });
                                }
                                GroundedWork::EvalArg {
                                    arg_idx,
                                    state: new_state,
                                } => {
                                    // Need to evaluate an argument first
                                    let grounded_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        env: env.clone(),
                                        parent_cont: cont_id,
                                        depth,
                                    });

                                    // Get the argument to evaluate
                                    let arg_to_eval = new_state.args[arg_idx].clone();

                                    // Push eval work item - TCO: don't increment depth
                                    work_stack.push(WorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth, // TCO: reuse depth for grounded arg eval
                                        cont_id: grounded_cont_id,
                                        is_tail_call: true,
                                    });
                                }
                                GroundedWork::Error(e) => {
                                    // Operation failed
                                    let error_value = match e {
                                        ExecError::Runtime(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("TypeError".to_string()),
                                        ),
                                        ExecError::Arithmetic(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("ArithmeticError".to_string()),
                                        ),
                                        ExecError::IncorrectArgument(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("ArityError".to_string()),
                                        ),
                                        ExecError::NoReduce => MettaValue::Error(
                                            "NoReduce".to_string(),
                                            MettaValue::Atom("EvalError".to_string()),
                                        ),
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (vec![error_value], env),
                                    });
                                }
                            }
                        } else {
                            // TCO operation not found - shouldn't happen if we check first
                            let error_value = MettaValue::Error(
                                format!("TCO operation '{}' not found", state.op_name),
                                MettaValue::Atom("InternalError".to_string()),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    // Start let binding evaluation - evaluate value expression first
                    EvalStep::StartLetBinding {
                        pattern,
                        value_expr,
                        body,
                        env,
                        depth,
                    } => {
                        // Create ProcessLet continuation to handle value results
                        let let_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessLet {
                            pending_values: None, // Will be filled when value eval completes
                            pattern,
                            body,
                            results: Vec::new(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Push value expression evaluation
                        work_stack.push(WorkItem::Eval {
                            value: value_expr,
                            env,
                            depth: depth + 1, // Value is not tail position
                            cont_id: let_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate if branch - condition already evaluated, now evaluate selected branch
                    EvalStep::EvalIfBranch { branch, env, depth } => {
                        // Push branch evaluation - THIS IS TAIL CALL (TCO)
                        // The branch inherits the continuation from the if expression
                        work_stack.push(WorkItem::Eval {
                            value: branch,
                            env,
                            depth, // TCO: reuse depth for branch eval
                            cont_id,
                            is_tail_call: true,
                        });
                    }

                    // Evaluate rule matches with UNEVALUATED arguments (lazy evaluation)
                    // This is used when user-defined rules match before argument evaluation.
                    EvalStep::EvalRuleMatchesLazy {
                        matches,
                        env,
                        depth,
                    } => {
                        if matches.is_empty() {
                            // No rule matches - shouldn't happen (this variant is only used when matches exist)
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![], env),
                            });
                        } else {
                            // Convert to VecDeque ONCE and pop front (O(n) + O(1) vs O(n²))
                            let mut matches_deque: VecDeque<_> = matches.into_iter().collect();
                            let (rhs, bindings) = matches_deque.pop_front().unwrap();

                            // Create continuation to process remaining rule matches
                            let match_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessRuleMatches {
                                remaining_matches: matches_deque,
                                results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate first rule RHS with bindings - THIS IS A TAIL CALL
                            // The bindings contain UNEVALUATED expressions from pattern match
                            // TCO: Don't increment depth for tail calls
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth, // TCO: reuse depth
                                cont_id: match_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }

                    // Evaluate grounded arguments before rule matching.
                    // This defers grounded arg evaluation to the trampoline, preventing stack overflow.
                    EvalStep::EvalGroundedArgs {
                        items,
                        grounded_indices,
                        env,
                        depth,
                    } => {
                        if grounded_indices.is_empty() {
                            // No grounded args - shouldn't happen, but handle gracefully.
                            // Continue to rule matching by creating EvalSExpr step.
                            work_stack.push(WorkItem::Eval {
                                value: MettaValue::SExpr(items),
                                env,
                                depth,
                                cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            // Evaluate first grounded arg
                            let first_idx = grounded_indices[0];
                            let arg_to_eval = items[first_idx].clone();

                            // Create continuation to collect result
                            let grounded_cont_id = continuations.len();
                            continuations.push(Continuation::CollectGroundedArg {
                                items,
                                grounded_indices,
                                current_idx: 0,
                                evaluated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Push evaluation of the grounded arg
                            work_stack.push(WorkItem::Eval {
                                value: arg_to_eval,
                                env,
                                depth: depth + 1,
                                cont_id: grounded_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start map-atom operation - iterates lazily via trampoline
                    EvalStep::StartMapAtom {
                        elements,
                        var_name,
                        template,
                        env,
                        depth,
                    } => {
                        if elements.is_empty() {
                            // HE-compatible: empty SExpr () evaluates to itself, not Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::SExpr(vec![])], env),
                            });
                        } else {
                            // Create continuation for remaining elements
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let map_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessMapAtom {
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                template: template.clone(),
                                collected_results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate first element's template
                            let instantiated = super::super::list_ops::helpers::substitute_variable(
                                &template, &var_name, &first,
                            );
                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: map_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start filter-atom operation - iterates lazily via trampoline
                    EvalStep::StartFilterAtom {
                        elements,
                        var_name,
                        predicate,
                        env,
                        depth,
                    } => {
                        if elements.is_empty() {
                            // HE-compatible: empty SExpr () evaluates to itself, not Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::SExpr(vec![])], env),
                            });
                        } else {
                            // Create continuation for processing elements
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let filter_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessFilterAtom {
                                current_element: Some(first.clone()),
                                remaining_elements: remaining,
                                var_name: var_name.clone(),
                                predicate: predicate.clone(),
                                filtered_results: vec![],
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate predicate for first element
                            let instantiated = super::super::list_ops::helpers::substitute_variable(
                                &predicate, &var_name, &first,
                            );
                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: filter_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start foldl-atom operation - iterates lazily via trampoline
                    EvalStep::StartFoldlAtom {
                        elements,
                        init,
                        acc_var_name,
                        item_var_name,
                        operation,
                        env,
                        depth,
                    } => {
                        if elements.is_empty() {
                            // Empty list -> return initial value
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![init], env),
                            });
                        } else {
                            // Create continuation for processing elements
                            let mut remaining: VecDeque<_> = elements.into_iter().collect();
                            let first = remaining.pop_front().expect("elements is non-empty");

                            let fold_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessFoldlAtom {
                                remaining_elements: remaining,
                                acc_var_name: acc_var_name.clone(),
                                item_var_name: item_var_name.clone(),
                                operation: operation.clone(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            // Evaluate operation with init as accumulator and first element
                            let mut instantiated =
                                super::super::list_ops::helpers::substitute_variable(
                                    &operation,
                                    &acc_var_name,
                                    &init,
                                );
                            instantiated = super::super::list_ops::helpers::substitute_variable(
                                &instantiated,
                                &item_var_name,
                                &first,
                            );
                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env,
                                depth: depth + 1,
                                cont_id: fold_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Evaluate if condition - defers condition evaluation to trampoline
                    EvalStep::EvalIfCondition {
                        condition,
                        then_branch,
                        else_branch,
                        env,
                        depth,
                    } => {
                        // Create continuation to track then/else branches while awaiting condition
                        let if_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessIfCondition {
                            then_branch,
                            else_branch,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Push condition evaluation
                        work_stack.push(WorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1, // Condition is not tail position
                            cont_id: if_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate case atom - defers atom evaluation to trampoline
                    EvalStep::EvalCaseAtom {
                        atom,
                        cases,
                        env,
                        depth,
                    } => {
                        // Create continuation to track cases while awaiting atom evaluation
                        let case_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessCaseAtom {
                            cases,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Push atom evaluation
                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1, // Atom is not tail position
                            cont_id: case_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate switch result - defers template evaluation to trampoline
                    EvalStep::EvalSwitchResult {
                        template,
                        env,
                        depth,
                    } => {
                        // Push template evaluation - THIS IS TAIL CALL (TCO)
                        // The template inherits the continuation from the switch expression
                        work_stack.push(WorkItem::Eval {
                            value: template,
                            env,
                            depth, // TCO: reuse depth for template eval
                            cont_id,
                            is_tail_call: true,
                        });
                    }

                    // Evaluate (eval expr) - first evaluate argument, then result
                    EvalStep::EvalEval { arg, env, depth } => {
                        // Create continuation to process argument result
                        let eval_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessEvalEval {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate argument first
                        work_stack.push(WorkItem::Eval {
                            value: arg,
                            env,
                            depth: depth + 1,
                            cont_id: eval_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate (return value) - evaluate argument, wrap in return
                    EvalStep::EvalReturn { value, env, depth } => {
                        // Create continuation to wrap result
                        let return_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessReturn {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate value first
                        work_stack.push(WorkItem::Eval {
                            value,
                            env,
                            depth: depth + 1,
                            cont_id: return_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start (chain expr $var body) - evaluate expr first
                    EvalStep::StartChain {
                        expr,
                        var,
                        body,
                        env,
                        depth,
                    } => {
                        // Create continuation to process expr results
                        let chain_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessChainExpr {
                            var,
                            body,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate expression first
                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: chain_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start (function expr) - begin evaluation loop
                    EvalStep::StartFunction { expr, env, depth } => {
                        // Create continuation to process loop iteration
                        let func_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessFunction {
                            iteration_count: 1,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate expression
                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: func_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Evaluate (is-error expr) - check if expression result is error
                    EvalStep::EvalIsError { expr, env, depth } => {
                        // Create continuation to check result
                        let is_error_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessIsError {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate expression
                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: is_error_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start (catch expr default) - evaluate expression first
                    EvalStep::StartCatch {
                        expr,
                        default,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle result
                        let catch_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessCatch {
                            default,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate expression
                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: catch_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start conjunction (, goal1 goal2 ...) - evaluate first goal
                    EvalStep::StartConjunction { goals, env, depth } => {
                        if goals.is_empty() {
                            // Empty conjunction (,) succeeds with Nil
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![MettaValue::Nil()], env),
                            });
                        } else if goals.len() == 1 {
                            // Unary conjunction: just evaluate the single goal (tail call)
                            work_stack.push(WorkItem::Eval {
                                value: goals[0].clone(),
                                env,
                                depth,
                                cont_id,
                                is_tail_call: true,
                            });
                        } else {
                            // N-ary conjunction: evaluate first goal
                            let mut remaining = VecDeque::from(goals);
                            let first_goal = remaining.pop_front().expect("non-empty");

                            let conj_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessConjunction {
                                remaining_goals: remaining,
                                accumulated_results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: first_goal,
                                env,
                                depth: depth + 1,
                                cont_id: conj_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start unify (unify pattern1 pattern2 success failure)
                    EvalStep::StartUnify {
                        pattern1,
                        pattern2,
                        success_body,
                        failure_body,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle pattern1 results
                        let unify_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessUnifyPattern1 {
                            pattern2,
                            success_body,
                            failure_body,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate pattern1 first
                        work_stack.push(WorkItem::Eval {
                            value: pattern1,
                            env,
                            depth: depth + 1,
                            cont_id: unify_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start collapse (collapse expr) - evaluate expr, collect into list
                    EvalStep::StartCollapse { expr, env, depth } => {
                        let collapse_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessCollapse {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: collapse_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start collapse-bind (collapse-bind expr) - evaluate expr, collect ALL
                    EvalStep::StartCollapseBind { expr, env, depth } => {
                        let collapse_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessCollapseBind {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: expr,
                            env,
                            depth: depth + 1,
                            cont_id: collapse_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start amb (amb alt1 alt2 ...) - evaluate each alternative
                    EvalStep::StartAmb {
                        alternatives,
                        env,
                        depth,
                    } => {
                        if alternatives.is_empty() {
                            // Empty amb returns empty (nondeterministic failure)
                            work_stack.push(WorkItem::Resume {
                                cont_id,
                                result: (vec![], env),
                            });
                        } else {
                            let mut alts_deque: VecDeque<MettaValue> =
                                alternatives.into_iter().collect();
                            let first = alts_deque.pop_front().unwrap();

                            let amb_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessAmb {
                                remaining_alts: alts_deque,
                                results: Vec::new(),
                                env: env.clone(),
                                depth,
                                parent_cont: cont_id,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: first,
                                env,
                                depth: depth + 1,
                                cont_id: amb_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Start guard (guard condition) - evaluate condition
                    EvalStep::StartGuard {
                        condition,
                        env,
                        depth,
                    } => {
                        let guard_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessGuard {
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: condition,
                            env,
                            depth: depth + 1,
                            cont_id: guard_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-atoms (get-atoms space) - evaluate space ref
                    EvalStep::StartGetAtoms {
                        space_ref,
                        env,
                        depth,
                    } => {
                        let get_atoms_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessGetAtoms {
                            space_ref: space_ref.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: get_atoms_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start memo (memo memo-table expr) or memo-first
                    EvalStep::StartMemo {
                        memo_ref,
                        expr,
                        first_only,
                        env,
                        depth,
                    } => {
                        let memo_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessMemoTable {
                            memo_ref: memo_ref.clone(),
                            expr,
                            first_only,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            cont_id: memo_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start new-memo (new-memo name [size])
                    EvalStep::StartNewMemo {
                        name_arg,
                        size_arg,
                        env,
                        depth,
                    } => {
                        let new_memo_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessNewMemoName {
                            name_arg: name_arg.clone(),
                            size_arg,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: name_arg,
                            env,
                            depth: depth + 1,
                            cont_id: new_memo_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start memo operation (clear-memo! or memo-stats)
                    EvalStep::StartMemoOp {
                        memo_ref,
                        op_type,
                        env,
                        depth,
                    } => {
                        let memo_op_cont_id = continuations.len();
                        let is_clear = matches!(op_type, MemoOpType::Clear);
                        continuations.push(Continuation::ProcessMemoOp {
                            memo_ref: memo_ref.clone(),
                            is_clear,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: memo_ref,
                            env,
                            depth: depth + 1,
                            cont_id: memo_op_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start match evaluation (3-arg syntax)
                    EvalStep::StartMatch {
                        space_arg,
                        pattern,
                        template,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle space evaluation result
                        let match_space_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessMatchSpace {
                            space_arg: space_arg.clone(),
                            pattern,
                            template,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate space argument
                        work_stack.push(WorkItem::Eval {
                            value: space_arg,
                            env,
                            depth: depth + 1,
                            cont_id: match_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start add-atom evaluation
                    EvalStep::StartAddAtom {
                        space_ref,
                        atom,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle space evaluation result
                        let add_space_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessAddAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate space reference
                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: add_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start remove-atom evaluation
                    EvalStep::StartRemoveAtom {
                        space_ref,
                        atom,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle space evaluation result
                        let remove_space_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessRemoveAtomSpace {
                            space_ref: space_ref.clone(),
                            atom,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate space reference
                        work_stack.push(WorkItem::Eval {
                            value: space_ref,
                            env,
                            depth: depth + 1,
                            cont_id: remove_space_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start new-state evaluation
                    EvalStep::StartNewState {
                        initial_value,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle initial value result
                        let new_state_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessNewState {
                            initial_value: initial_value.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate initial value
                        work_stack.push(WorkItem::Eval {
                            value: initial_value,
                            env,
                            depth: depth + 1,
                            cont_id: new_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-state evaluation
                    EvalStep::StartGetState {
                        state_ref,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle state reference result
                        let get_state_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessGetState {
                            state_ref: state_ref.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate state reference
                        work_stack.push(WorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            cont_id: get_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start change-state! evaluation
                    EvalStep::StartChangeState {
                        state_ref,
                        new_value,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle state reference result
                        let change_state_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessChangeStateRef {
                            state_ref: state_ref.clone(),
                            new_value,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate state reference first
                        work_stack.push(WorkItem::Eval {
                            value: state_ref,
                            env,
                            depth: depth + 1,
                            cont_id: change_state_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start repr evaluation - evaluates atom then converts to string
                    EvalStep::StartRepr { atom, env, depth } => {
                        // Create continuation to handle atom result
                        let repr_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessRepr {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate atom first
                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: repr_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start format-args evaluation - evaluates format string then args
                    EvalStep::StartFormatArgs {
                        format_arg,
                        args_arg,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle format string result
                        let format_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessFormatArgsString {
                            format_arg: format_arg.clone(),
                            args_arg,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate format string first
                        work_stack.push(WorkItem::Eval {
                            value: format_arg,
                            env,
                            depth: depth + 1,
                            cont_id: format_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start println! evaluation - evaluates atom then prints it
                    EvalStep::StartPrintln { atom, env, depth } => {
                        // Create continuation to handle atom result
                        let println_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessPrintln {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate atom first
                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: println_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start trace! evaluation - evaluates message then value
                    EvalStep::StartTrace {
                        message,
                        value_expr,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle message result
                        let trace_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessTraceMessage {
                            message: message.clone(),
                            value_expr,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate message first
                        work_stack.push(WorkItem::Eval {
                            value: message,
                            env,
                            depth: depth + 1,
                            cont_id: trace_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start get-metatype evaluation - evaluates atom then returns its meta-type
                    EvalStep::StartGetMetatype { atom, env, depth } => {
                        // Create continuation to handle atom result
                        let metatype_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessGetMetatype {
                            atom: atom.clone(),
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate atom first
                        work_stack.push(WorkItem::Eval {
                            value: atom,
                            env,
                            depth: depth + 1,
                            cont_id: metatype_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Start bind! evaluation - evaluates atom expression then registers token
                    EvalStep::StartBind {
                        token,
                        atom_expr,
                        env,
                        depth,
                    } => {
                        // Create continuation to handle atom expression result
                        let bind_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessBind {
                            token,
                            env: env.clone(),
                            depth,
                            parent_cont: cont_id,
                        });

                        // Evaluate atom expression first
                        work_stack.push(WorkItem::Eval {
                            value: atom_expr,
                            env,
                            depth: depth + 1,
                            cont_id: bind_cont_id,
                            is_tail_call: false,
                        });
                    }
                }
            }

            WorkItem::Resume { cont_id, result } => {
                // Take ownership of continuation for processing
                let cont = std::mem::replace(&mut continuations[cont_id], Continuation::Done);
                trace!(target: "mettatron::backend::eval::eval_trampoline", ?cont, result_values = ?result.0, "resume work item");

                match cont {
                    Continuation::Done => {
                        // Final result
                        final_result = Some(result);
                        trace!(target: "mettatron::backend::eval::eval_trampoline", ?final_result);
                    }

                    Continuation::CollectSExpr {
                        mut remaining,
                        mut collected,
                        original_env,
                        depth,
                        parent_cont,
                    } => {
                        // Add result to collected
                        collected.push(result);

                        if remaining.is_empty() {
                            // All items evaluated, process collected results
                            let processed = process_collected_sexpr(collected, original_env, depth);
                            trace!(target: "mettatron::backend::eval::eval_trampoline", processed_sexpr=?processed);

                            match processed {
                                ProcessedSExpr::Done(result) => {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result,
                                    });
                                }
                                ProcessedSExpr::EvalRuleMatches {
                                    matches,
                                    env,
                                    depth,
                                    base_results,
                                } => {
                                    if matches.is_empty() {
                                        // No rule matches, return base results
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (base_results, env),
                                        });
                                    } else {
                                        // Convert to VecDeque ONCE and pop front (O(n) + O(1) vs O(n²))
                                        let mut matches_deque: VecDeque<_> =
                                            matches.into_iter().collect();
                                        let (rhs, bindings) = matches_deque.pop_front().unwrap();

                                        // Create continuation to process remaining rule matches
                                        let match_cont_id = continuations.len();
                                        continuations.push(Continuation::ProcessRuleMatches {
                                            remaining_matches: matches_deque,
                                            results: base_results,
                                            env: env.clone(),
                                            depth,
                                            parent_cont,
                                        });

                                        // Evaluate first rule RHS - THIS IS A TAIL CALL
                                        // TCO: Don't increment depth for tail calls
                                        let instantiated_rhs =
                                            apply_bindings(&rhs, &bindings).into_owned();
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_rhs,
                                            env,
                                            depth, // TCO: reuse depth
                                            cont_id: match_cont_id,
                                            is_tail_call: true,
                                        });
                                    }
                                }
                                ProcessedSExpr::EvalCombinations {
                                    combinations,
                                    env,
                                    depth,
                                } => {
                                    // Create continuation to process combinations lazily
                                    let combo_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCombinations {
                                        combinations,
                                        results: vec![],
                                        pending_rule_matches: VecDeque::new(),
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    // Resume to process first combination
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: combo_cont_id,
                                        result: (vec![], env),
                                    });
                                }
                            }
                        } else {
                            // More items to evaluate - O(1) pop from VecDeque front
                            let next = remaining.pop_front().unwrap();

                            // Put continuation back (modified)
                            continuations[cont_id] = Continuation::CollectSExpr {
                                remaining,
                                collected,
                                original_env: original_env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate next item
                            // NOT a tail call - collecting results for S-expr
                            work_stack.push(WorkItem::Eval {
                                value: next,
                                env: original_env,
                                depth: depth + 1,
                                cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    Continuation::ProcessRuleMatches {
                        mut remaining_matches,
                        mut results,
                        env: _,
                        depth,
                        parent_cont,
                    } => {
                        // Add results from this rule evaluation
                        results.extend(result.0);
                        // IMPORTANT: Propagate environment changes (including state mutations)
                        // from rule evaluation to ensure side effects like change-state! are visible
                        let env = result.1;

                        if remaining_matches.is_empty() {
                            // All rules evaluated
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, env),
                            });
                        } else {
                            // More rules to evaluate - O(1) pop from VecDeque front
                            let (rhs, bindings) = remaining_matches.pop_front().unwrap();

                            // Put continuation back (modified)
                            continuations[cont_id] = Continuation::ProcessRuleMatches {
                                remaining_matches,
                                results,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate next rule RHS - THIS IS A TAIL CALL
                            // TCO: Don't increment depth for tail calls
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth, // TCO: reuse depth
                                cont_id,
                                is_tail_call: true,
                            });
                        }
                    }

                    Continuation::ProcessGroundedOp {
                        mut state,
                        env,
                        parent_cont,
                        depth,
                    } => {
                        // Add evaluation results to state
                        // The arg_idx is (step - 1) because step was incremented before EvalArg
                        //
                        // DEFENSIVE ASSERTION: Catch underflow that could cause memory corruption.
                        // The 7.3 exabyte allocation bug (0x6573fb666f6f6468 = "hdoof" + 0xfb + "se")
                        // suggests string data being read as a size - likely from HashMap corruption
                        // caused by inserting at usize::MAX when step == 0.
                        debug_assert!(
                            state.step > 0,
                            "BUG: ProcessGroundedOp resumed with step=0! op_name={}, args={:?}, \
                             evaluated_args={:?}. This would cause underflow to usize::MAX.",
                            state.op_name,
                            state.args,
                            state.evaluated_args
                        );
                        let arg_idx = state.step.checked_sub(1).unwrap_or_else(|| {
                            panic!(
                                "BUG: state.step underflow in ProcessGroundedOp! \
                                 op_name={}, step={}, args={:?}, evaluated_args={:?}",
                                state.op_name, state.step, state.args, state.evaluated_args
                            )
                        });
                        state.set_arg(arg_idx, result.0);

                        // Look up the TCO operation and continue
                        if let Some(grounded_op) = env.get_grounded_operation_tco(&state.op_name) {
                            match grounded_op.execute_step(&mut state) {
                                GroundedWork::Done(results) => {
                                    // Operation complete
                                    let values: Vec<MettaValue> =
                                        results.into_iter().map(|(v, _)| v).collect();
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (values, env),
                                    });
                                }
                                GroundedWork::EvalArg {
                                    arg_idx: next_arg_idx,
                                    state: new_state,
                                } => {
                                    // Need to evaluate another argument
                                    continuations[cont_id] = Continuation::ProcessGroundedOp {
                                        state: new_state.clone(),
                                        env: env.clone(),
                                        parent_cont,
                                        depth,
                                    };

                                    // Get the argument to evaluate
                                    let arg_to_eval = new_state.args[next_arg_idx].clone();

                                    // Push eval work item - TCO: don't increment depth
                                    work_stack.push(WorkItem::Eval {
                                        value: arg_to_eval,
                                        env,
                                        depth, // TCO: reuse depth for grounded arg eval
                                        cont_id,
                                        is_tail_call: true,
                                    });
                                }
                                GroundedWork::Error(e) => {
                                    // Operation failed
                                    let error_value = match e {
                                        ExecError::Runtime(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("TypeError".to_string()),
                                        ),
                                        ExecError::Arithmetic(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("ArithmeticError".to_string()),
                                        ),
                                        ExecError::IncorrectArgument(msg) => MettaValue::Error(
                                            msg,
                                            MettaValue::Atom("ArityError".to_string()),
                                        ),
                                        ExecError::NoReduce => MettaValue::Error(
                                            "NoReduce".to_string(),
                                            MettaValue::Atom("EvalError".to_string()),
                                        ),
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![error_value], env),
                                    });
                                }
                            }
                        } else {
                            // Operation not found - shouldn't happen
                            let error_value = MettaValue::Error(
                                format!("TCO operation '{}' not found", state.op_name),
                                MettaValue::Atom("InternalError".to_string()),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![error_value], env),
                            });
                        }
                    }

                    Continuation::ProcessCombinations {
                        mut combinations,
                        mut results,
                        mut pending_rule_matches,
                        mut env,
                        depth,
                        parent_cont,
                    } => {
                        // First, add any results from rule evaluation
                        results.extend(result.0);
                        // IMPORTANT: Propagate environment changes (including state mutations)
                        // from rule evaluation to ensure side effects like change-state! are visible
                        env = result.1;

                        // If we have pending rule matches, process the next one
                        if !pending_rule_matches.is_empty() {
                            let (rhs, bindings) = pending_rule_matches.pop_front().unwrap();

                            // Update continuation with remaining matches
                            continuations[cont_id] = Continuation::ProcessCombinations {
                                combinations,
                                results,
                                pending_rule_matches,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            };

                            // Evaluate the rule RHS - THIS IS A TAIL CALL
                            let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                            work_stack.push(WorkItem::Eval {
                                value: instantiated_rhs,
                                env,
                                depth,
                                cont_id,
                                is_tail_call: true,
                            });
                        } else {
                            // No pending rule matches - get next combination
                            match combinations.next() {
                                None => {
                                    // All combinations processed, return results
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (results, env),
                                    });
                                }
                                Some(evaled_items) => {
                                    // Process this combination
                                    // Check if this is a grounded operation
                                    if let Some(first) = evaled_items.first() {
                                        if let MettaValueInner::Atom(op) = first.inner() {
                                            if let Some(builtin_result) =
                                                super::super::try_eval_builtin(
                                                    op,
                                                    &evaled_items[1..],
                                                )
                                            {
                                                results.push(builtin_result);

                                                // Continue to next combination
                                                continuations[cont_id] =
                                                    Continuation::ProcessCombinations {
                                                        combinations,
                                                        results,
                                                        pending_rule_matches: VecDeque::new(),
                                                        env: env.clone(),
                                                        depth,
                                                        parent_cont,
                                                    };

                                                // Resume to process next combination
                                                work_stack.push(WorkItem::Resume {
                                                    cont_id,
                                                    result: (vec![], env),
                                                });
                                                continue;
                                            }
                                        }
                                    }

                                    // MeTTa HE semantics: After argument evaluation, try rule matching AGAIN.
                                    // The evaluated arguments may now match rules that didn't match before.
                                    // Example: (intensity (color)) → (intensity red) → 100
                                    let evaled_vec: Vec<MettaValue> = evaled_items.into_vec();
                                    let sexpr = MettaValue::SExpr(evaled_vec.clone());
                                    let all_matches =
                                        super::super::try_match_all_rules(&sexpr, &env);

                                    if !all_matches.is_empty() {
                                        // Rules match! Queue them for evaluation
                                        pending_rule_matches = all_matches.into_iter().collect();

                                        // Take the first match and evaluate it
                                        let (rhs, bindings) =
                                            pending_rule_matches.pop_front().unwrap();

                                        // Update continuation with pending matches
                                        continuations[cont_id] =
                                            Continuation::ProcessCombinations {
                                                combinations,
                                                results,
                                                pending_rule_matches,
                                                env: env.clone(),
                                                depth,
                                                parent_cont,
                                            };

                                        // Evaluate the rule RHS
                                        let instantiated_rhs =
                                            apply_bindings(&rhs, &bindings).into_owned();
                                        work_stack.push(WorkItem::Eval {
                                            value: instantiated_rhs,
                                            env,
                                            depth,
                                            cont_id,
                                            is_tail_call: true,
                                        });
                                        continue;
                                    }

                                    // No rules matched even with evaluated arguments - data constructor
                                    let result_value = super::super::handle_no_rule_match(
                                        evaled_vec, &sexpr, &mut env,
                                    );
                                    results.push(result_value);

                                    // Continue to next combination
                                    continuations[cont_id] = Continuation::ProcessCombinations {
                                        combinations,
                                        results,
                                        pending_rule_matches: VecDeque::new(),
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    };

                                    // Resume to process next combination
                                    work_stack.push(WorkItem::Resume {
                                        cont_id,
                                        result: (vec![], env),
                                    });
                                }
                            }
                        }
                    }

                    // Handle let binding continuation
                    Continuation::ProcessLet {
                        pending_values,
                        pattern,
                        body,
                        mut results,
                        env: _let_env, // Unused - we use result_env from the resumed result
                        depth,
                        parent_cont,
                    } => {
                        let (result_values, result_env) = result;

                        match pending_values {
                            None => {
                                // First resumption: received value evaluation results
                                // Now process each value, trying to match pattern
                                let mut values = VecDeque::from(result_values);

                                // Try to find a matching value - use explicit loop for ownership clarity
                                loop {
                                    match values.pop_front() {
                                        Some(value) => {
                                            if let Some(bindings) =
                                                super::super::pattern_match(&pattern, &value)
                                            {
                                                // Pattern matches - evaluate body with bindings
                                                let instantiated_body =
                                                    apply_bindings(&body, &bindings).into_owned();

                                                // Restore continuation for collecting more results
                                                continuations[cont_id] = Continuation::ProcessLet {
                                                    pending_values: Some(values),
                                                    pattern,
                                                    body,
                                                    results,
                                                    env: result_env.clone(),
                                                    depth,
                                                    parent_cont,
                                                };

                                                // Push body evaluation - THIS IS TAIL CALL (TCO)
                                                work_stack.push(WorkItem::Eval {
                                                    value: instantiated_body,
                                                    env: result_env,
                                                    depth, // TCO: reuse depth for body eval
                                                    cont_id,
                                                    is_tail_call: true,
                                                });
                                                break; // Exit loop, work is pushed
                                            }
                                            // Pattern doesn't match - continue to next value
                                        }
                                        None => {
                                            // No pattern matched - return results to parent
                                            work_stack.push(WorkItem::Resume {
                                                cont_id: parent_cont,
                                                result: (results, result_env),
                                            });
                                            break; // Exit loop
                                        }
                                    }
                                }
                            }

                            Some(mut remaining_values) => {
                                // Subsequent resumption: received body evaluation results
                                // Add body results to collected results
                                results.extend(result_values);

                                // Try next value - use explicit loop for ownership clarity
                                loop {
                                    match remaining_values.pop_front() {
                                        Some(value) => {
                                            if let Some(bindings) =
                                                super::super::pattern_match(&pattern, &value)
                                            {
                                                // Pattern matches - evaluate body with bindings
                                                let instantiated_body =
                                                    apply_bindings(&body, &bindings).into_owned();

                                                // Restore continuation for collecting more results
                                                continuations[cont_id] = Continuation::ProcessLet {
                                                    pending_values: Some(remaining_values),
                                                    pattern,
                                                    body,
                                                    results,
                                                    env: result_env.clone(),
                                                    depth,
                                                    parent_cont,
                                                };

                                                // Push body evaluation - THIS IS TAIL CALL (TCO)
                                                work_stack.push(WorkItem::Eval {
                                                    value: instantiated_body,
                                                    env: result_env,
                                                    depth, // TCO: reuse depth for body eval
                                                    cont_id,
                                                    is_tail_call: true,
                                                });
                                                break; // Exit loop, work is pushed
                                            }
                                            // Pattern doesn't match - continue to next value
                                        }
                                        None => {
                                            // All values processed - return results to parent
                                            work_stack.push(WorkItem::Resume {
                                                cont_id: parent_cont,
                                                result: (results, result_env),
                                            });
                                            break; // Exit loop
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Handle grounded arg collection continuation
                    Continuation::CollectGroundedArg {
                        mut items,
                        grounded_indices,
                        current_idx,
                        mut evaluated_results,
                        env: _grounded_env, // Unused - we use result_env from the resumed result
                        depth,
                        parent_cont,
                    } => {
                        // Take first result from evaluation (deterministic for grounded ops)
                        let (result_values, result_env) = result;
                        if let Some(first_result) = result_values.into_iter().next() {
                            evaluated_results.push(first_result);
                        }

                        let next_idx = current_idx + 1;
                        if next_idx < grounded_indices.len() {
                            // More grounded args to evaluate
                            let arg_idx = grounded_indices[next_idx];
                            let arg_to_eval = items[arg_idx].clone();

                            continuations[cont_id] = Continuation::CollectGroundedArg {
                                items,
                                grounded_indices,
                                current_idx: next_idx,
                                evaluated_results,
                                env: result_env.clone(),
                                depth,
                                parent_cont,
                            };

                            work_stack.push(WorkItem::Eval {
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

                            // Continue with rule matching using the updated items
                            let resolved_items =
                                super::super::resolve_tokens_shallow(&items, &result_env);
                            let resolved_sexpr = MettaValue::SExpr(resolved_items.clone());
                            let all_matches =
                                super::super::try_match_all_rules(&resolved_sexpr, &result_env);

                            if !all_matches.is_empty() {
                                // Rules matched - evaluate them via EvalRuleMatchesLazy
                                let mut matches_deque: VecDeque<_> =
                                    all_matches.into_iter().collect();
                                let (rhs, bindings) = matches_deque.pop_front().unwrap();

                                // Create continuation to process remaining rule matches
                                let match_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessRuleMatches {
                                    remaining_matches: matches_deque,
                                    results: vec![],
                                    env: result_env.clone(),
                                    depth,
                                    parent_cont,
                                });

                                // Evaluate first rule RHS with bindings
                                let instantiated_rhs = apply_bindings(&rhs, &bindings).into_owned();
                                work_stack.push(WorkItem::Eval {
                                    value: instantiated_rhs,
                                    env: result_env,
                                    depth, // TCO: reuse depth
                                    cont_id: match_cont_id,
                                    is_tail_call: true,
                                });
                            } else {
                                // No rules matched - expression is irreducible (data constructor)
                                // Continue evaluating sub-items via EvalSExpr
                                work_stack.push(WorkItem::Eval {
                                    value: MettaValue::SExpr(items),
                                    env: result_env,
                                    depth,
                                    cont_id: parent_cont,
                                    is_tail_call: false,
                                });
                            }
                        }
                    }

                    // Handle map-atom iteration continuation
                    Continuation::ProcessMapAtom {
                        mut remaining_elements,
                        var_name,
                        template,
                        mut collected_results,
                        env: _map_env,
                        depth,
                        parent_cont,
                    } => {
                        let (mut result_values, result_env) = result;

                        // Add first result from evaluation (move instead of clone)
                        if result_values.is_empty() {
                            collected_results.push(MettaValue::Nil());
                        } else {
                            // Move the first result out of the vector
                            let first_result = result_values.swap_remove(0);

                            // Check for error propagation
                            if matches!(first_result.inner(), MettaValueInner::Error(_, _)) {
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![first_result], result_env),
                                });
                                continue;
                            }
                            collected_results.push(first_result);
                        }

                        if remaining_elements.is_empty() {
                            // All elements processed - return result list
                            // HE-compatible: empty lists are SExpr([]), not Nil
                            let result_list = MettaValue::SExpr(collected_results);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![result_list], result_env),
                            });
                        } else {
                            // More elements to process
                            let next_element = remaining_elements
                                .pop_front()
                                .expect("remaining_elements is non-empty");

                            // Evaluate template for next element
                            // Note: substitute_variable takes references, no clone needed
                            let instantiated = super::super::list_ops::helpers::substitute_variable(
                                &template,
                                &var_name,
                                &next_element,
                            );

                            // Update continuation in-place - move template and var_name
                            // instead of cloning to avoid allocation overhead
                            continuations[cont_id] = Continuation::ProcessMapAtom {
                                remaining_elements,
                                var_name, // move, not clone
                                template, // move, not clone
                                collected_results,
                                env: result_env.clone(),
                                depth,
                                parent_cont,
                            };

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env: result_env,
                                depth, // TCO: reuse depth for iteration
                                cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Handle filter-atom iteration continuation
                    Continuation::ProcessFilterAtom {
                        current_element,
                        mut remaining_elements,
                        var_name,
                        predicate,
                        mut filtered_results,
                        env: _filter_env,
                        depth,
                        parent_cont,
                    } => {
                        let (mut result_values, result_env) = result;

                        // Check predicate result and optionally include current element
                        if !result_values.is_empty() {
                            // Move the first result out of the vector
                            let first_result = result_values.swap_remove(0);

                            // Check for error propagation
                            if matches!(first_result.inner(), MettaValueInner::Error(_, _)) {
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![first_result], result_env),
                                });
                                continue;
                            }

                            let should_include = match first_result.inner() {
                                MettaValueInner::Bool(true) => true,
                                MettaValueInner::Bool(false) => false,
                                _ => !matches!(first_result.inner(), MettaValueInner::Nil),
                            };

                            if should_include {
                                if let Some(elem) = current_element {
                                    filtered_results.push(elem);
                                }
                            }
                        }

                        if remaining_elements.is_empty() {
                            // All elements processed - return filtered list
                            // HE-compatible: empty lists are SExpr([]), not Nil
                            let result_list = MettaValue::SExpr(filtered_results);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![result_list], result_env),
                            });
                        } else {
                            // More elements to process
                            let next_element = remaining_elements
                                .pop_front()
                                .expect("remaining_elements is non-empty");

                            // Evaluate predicate for next element
                            // Note: substitute_variable takes references, no clone needed
                            let instantiated = super::super::list_ops::helpers::substitute_variable(
                                &predicate,
                                &var_name,
                                &next_element,
                            );

                            // Update continuation - move var_name, predicate, and next_element
                            // instead of cloning to avoid allocation overhead
                            continuations[cont_id] = Continuation::ProcessFilterAtom {
                                current_element: Some(next_element), // move, not clone
                                remaining_elements,
                                var_name,  // move, not clone
                                predicate, // move, not clone
                                filtered_results,
                                env: result_env.clone(),
                                depth,
                                parent_cont,
                            };

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env: result_env,
                                depth, // TCO: reuse depth for iteration
                                cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Handle foldl-atom iteration continuation
                    Continuation::ProcessFoldlAtom {
                        mut remaining_elements,
                        acc_var_name,
                        item_var_name,
                        operation,
                        env: _fold_env,
                        depth,
                        parent_cont,
                    } => {
                        let (mut result_values, result_env) = result;

                        // Get the new accumulator value from the result (move instead of clone)
                        let accumulator = if result_values.is_empty() {
                            MettaValue::Nil()
                        } else {
                            // Move the first result out of the vector
                            let first_result = result_values.swap_remove(0);

                            // Check for error propagation
                            if matches!(first_result.inner(), MettaValueInner::Error(_, _)) {
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![first_result], result_env),
                                });
                                continue;
                            }
                            first_result
                        };

                        if remaining_elements.is_empty() {
                            // All elements processed - return final accumulator
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![accumulator], result_env),
                            });
                        } else {
                            // More elements to process
                            let next_element = remaining_elements
                                .pop_front()
                                .expect("remaining_elements is non-empty");

                            // Evaluate operation with current accumulator and next element
                            // Note: substitute_variable takes references, no clone needed
                            let mut instantiated =
                                super::super::list_ops::helpers::substitute_variable(
                                    &operation,
                                    &acc_var_name,
                                    &accumulator,
                                );
                            instantiated = super::super::list_ops::helpers::substitute_variable(
                                &instantiated,
                                &item_var_name,
                                &next_element,
                            );

                            // Update continuation - move var names and operation
                            // instead of cloning to avoid allocation overhead
                            continuations[cont_id] = Continuation::ProcessFoldlAtom {
                                remaining_elements,
                                acc_var_name,  // move, not clone
                                item_var_name, // move, not clone
                                operation,     // move, not clone
                                env: result_env.clone(),
                                depth,
                                parent_cont,
                            };

                            work_stack.push(WorkItem::Eval {
                                value: instantiated,
                                env: result_env,
                                depth, // TCO: reuse depth for iteration
                                cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Handle if condition continuation
                    Continuation::ProcessIfCondition {
                        then_branch,
                        else_branch,
                        env: _if_env,
                        depth,
                        parent_cont,
                    } => {
                        let (cond_results, env_after_cond) = result;

                        // Check for error in condition
                        if let Some(first) = cond_results.first() {
                            if matches!(first.inner(), MettaValueInner::Error(_, _)) {
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![first.clone()], env_after_cond),
                                });
                                continue;
                            }

                            // Check if condition is true
                            let is_true = match first.inner() {
                                MettaValueInner::Bool(true) => true,
                                MettaValueInner::Bool(false) => false,
                                // Non-boolean values: treat as true if not Nil
                                MettaValueInner::Nil => false,
                                _ => true,
                            };

                            // Evaluate the selected branch - THIS IS TAIL CALL (TCO)
                            let branch = if is_true { then_branch } else { else_branch };
                            work_stack.push(WorkItem::Eval {
                                value: branch,
                                env: env_after_cond,
                                depth, // TCO: reuse depth for branch eval
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            // No result from condition - treat as false
                            work_stack.push(WorkItem::Eval {
                                value: else_branch,
                                env: env_after_cond,
                                depth, // TCO: reuse depth for branch eval
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        }
                    }

                    // Handle case atom continuation
                    Continuation::ProcessCaseAtom {
                        cases,
                        env: _case_env,
                        depth,
                        parent_cont,
                    } => {
                        let (atom_results, atom_env) = result;

                        // Filter out Empty sentinels - they represent "no result to report"
                        let filtered_results: Vec<_> = atom_results
                            .into_iter()
                            .filter(|v| !matches!(v.inner(), MettaValueInner::Empty))
                            .collect();

                        // Handle case when evaluation returns no results (empty) - treat as Empty
                        if filtered_results.is_empty() {
                            // Match Empty against cases using switch-minimal logic
                            let switch_result = super::super::eval_switch_minimal_trampoline(
                                MettaValue::Atom("Empty".to_string()),
                                cases,
                                atom_env.clone(),
                                depth,
                            );
                            match switch_result {
                                EvalStep::Done(result) => {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result,
                                    });
                                }
                                EvalStep::EvalSwitchResult {
                                    template,
                                    env,
                                    depth,
                                } => {
                                    work_stack.push(WorkItem::Eval {
                                        value: template,
                                        env,
                                        depth,
                                        cont_id: parent_cont,
                                        is_tail_call: true,
                                    });
                                }
                                _ => {
                                    // Unexpected step type - shouldn't happen
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![], atom_env),
                                    });
                                }
                            }
                            continue;
                        }

                        // Convert to VecDeque for continuation-based processing
                        let mut remaining_atoms: VecDeque<MettaValue> =
                            filtered_results.into_iter().collect();

                        // Process first atom, queue rest for continuation
                        if let Some(first_atom) = remaining_atoms.pop_front() {
                            let is_empty = match first_atom.inner() {
                                MettaValueInner::Nil => true,
                                MettaValueInner::SExpr(items) if items.is_empty() => true,
                                _ => false,
                            };
                            let switch_atom = if is_empty {
                                MettaValue::Atom("Empty".to_string())
                            } else {
                                first_atom
                            };

                            let switch_result = super::super::eval_switch_minimal_trampoline(
                                switch_atom,
                                cases.clone(),
                                atom_env.clone(),
                                depth,
                            );

                            match switch_result {
                                EvalStep::Done((results, _)) => {
                                    // Create continuation to process remaining atoms
                                    let multi_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms,
                                        cases,
                                        collected: results,
                                        env: atom_env.clone(),
                                        depth,
                                        parent_cont,
                                    });
                                    // Resume immediately with empty result to trigger continuation
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: multi_cont_id,
                                        result: (vec![], atom_env),
                                    });
                                }
                                EvalStep::EvalSwitchResult {
                                    template,
                                    env: switch_env,
                                    depth: switch_depth,
                                } => {
                                    // Create continuation FIRST, then push eval
                                    let multi_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms,
                                        cases,
                                        collected: vec![],
                                        env: atom_env,
                                        depth,
                                        parent_cont,
                                    });
                                    // Evaluate template - continuation will collect result
                                    work_stack.push(WorkItem::Eval {
                                        value: template,
                                        env: switch_env,
                                        depth: switch_depth,
                                        cont_id: multi_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                                _ => {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![], atom_env),
                                    });
                                }
                            }
                        } else {
                            // No atoms to process
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![], atom_env),
                            });
                        }
                    }

                    // Handle multi-result case continuation
                    Continuation::ProcessCaseMultiResults {
                        mut remaining_atoms,
                        cases,
                        mut collected,
                        env,
                        depth,
                        parent_cont,
                    } => {
                        // Collect results from previous evaluation
                        let (results, _result_env) = result;
                        collected.extend(results);

                        // Process next atom if any
                        if let Some(next_atom) = remaining_atoms.pop_front() {
                            let is_empty = match next_atom.inner() {
                                MettaValueInner::Nil => true,
                                MettaValueInner::SExpr(items) if items.is_empty() => true,
                                _ => false,
                            };
                            let switch_atom = if is_empty {
                                MettaValue::Atom("Empty".to_string())
                            } else {
                                next_atom
                            };

                            let switch_result = super::super::eval_switch_minimal_trampoline(
                                switch_atom,
                                cases.clone(),
                                env.clone(),
                                depth,
                            );

                            match switch_result {
                                EvalStep::Done((results, _)) => {
                                    collected.extend(results);
                                    // Create new continuation for remaining
                                    let multi_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms,
                                        cases,
                                        collected,
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    });
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: multi_cont_id,
                                        result: (vec![], env),
                                    });
                                }
                                EvalStep::EvalSwitchResult {
                                    template,
                                    env: switch_env,
                                    depth: switch_depth,
                                } => {
                                    let multi_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms,
                                        cases,
                                        collected,
                                        env,
                                        depth,
                                        parent_cont,
                                    });
                                    work_stack.push(WorkItem::Eval {
                                        value: template,
                                        env: switch_env,
                                        depth: switch_depth,
                                        cont_id: multi_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                                _ => {
                                    // Continue with remaining
                                    let multi_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessCaseMultiResults {
                                        remaining_atoms,
                                        cases,
                                        collected,
                                        env: env.clone(),
                                        depth,
                                        parent_cont,
                                    });
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: multi_cont_id,
                                        result: (vec![], env),
                                    });
                                }
                            }
                        } else {
                            // All atoms processed - return collected results
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (collected, env),
                            });
                        }
                    }

                    // Handle (eval expr) continuation - evaluate result of argument
                    Continuation::ProcessEvalEval {
                        env: _eval_env,
                        depth,
                        parent_cont,
                    } => {
                        let (arg_results, arg_env) = result;
                        if let Some(expr) = arg_results.first() {
                            // Evaluate the result - THIS IS A TAIL CALL
                            work_stack.push(WorkItem::Eval {
                                value: expr.clone(),
                                env: arg_env,
                                depth, // TCO: reuse depth
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            // No result from argument evaluation
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Nil()], arg_env),
                            });
                        }
                    }

                    // Handle (return value) continuation - wrap result in return structure
                    Continuation::ProcessReturn {
                        env: _return_env,
                        depth: _,
                        parent_cont,
                    } => {
                        let (arg_results, arg_env) = result;

                        // Check for errors first
                        if let Some(err) = arg_results
                            .iter()
                            .find(|r| matches!(r.inner(), MettaValueInner::Error(_, _)))
                        {
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err.clone()], arg_env),
                            });
                        } else {
                            // Wrap results in return structure
                            let return_results: Vec<_> = arg_results
                                .into_iter()
                                .map(|r| {
                                    MettaValue::SExpr(vec![
                                        MettaValue::Atom("return".to_string()),
                                        r,
                                    ])
                                })
                                .collect();

                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (return_results, arg_env),
                            });
                        }
                    }

                    // Handle chain expression continuation - process expr results
                    Continuation::ProcessChainExpr {
                        var,
                        body,
                        env: _chain_env,
                        depth,
                        parent_cont,
                    } => {
                        let (expr_results, current_env) = result;

                        // Check for errors first
                        if let Some(err) = expr_results
                            .iter()
                            .find(|r| matches!(r.inner(), MettaValueInner::Error(_, _)))
                        {
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err.clone()], current_env),
                            });
                        } else if expr_results.is_empty() {
                            // No results, return empty
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![], current_env),
                            });
                        } else {
                            // Convert to VecDeque for processing
                            let mut remaining: VecDeque<_> = expr_results.into_iter().collect();
                            let first_value = remaining.pop_front().unwrap();

                            // Try to match and evaluate first body
                            if let Some(bindings) = super::super::pattern_match(&var, &first_value)
                            {
                                let instantiated_body =
                                    super::super::apply_bindings(&body, &bindings).into_owned();

                                // Create continuation to process remaining values
                                let chain_body_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessChainBody {
                                    remaining_values: remaining,
                                    var,
                                    body,
                                    results: vec![],
                                    env: current_env.clone(),
                                    depth,
                                    parent_cont,
                                });

                                // Evaluate first body
                                work_stack.push(WorkItem::Eval {
                                    value: instantiated_body,
                                    env: current_env,
                                    depth: depth + 1,
                                    cont_id: chain_body_cont_id,
                                    is_tail_call: false,
                                });
                            } else {
                                // No match, continue with remaining
                                if remaining.is_empty() {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![], current_env),
                                    });
                                } else {
                                    // Process remaining values
                                    let chain_body_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessChainBody {
                                        remaining_values: remaining,
                                        var,
                                        body,
                                        results: vec![],
                                        env: current_env.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    // Resume immediately to process next value
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: chain_body_cont_id,
                                        result: (vec![], current_env),
                                    });
                                }
                            }
                        }
                    }

                    // Handle chain body continuation - process body result, continue with remaining
                    Continuation::ProcessChainBody {
                        mut remaining_values,
                        var,
                        body,
                        mut results,
                        env: _,
                        depth,
                        parent_cont,
                    } => {
                        let (body_results, current_env) = result;

                        // Accumulate results
                        results.extend(body_results);

                        if remaining_values.is_empty() {
                            // All values processed
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, current_env),
                            });
                        } else {
                            // Process next value
                            let next_value = remaining_values.pop_front().unwrap();

                            if let Some(bindings) = super::super::pattern_match(&var, &next_value) {
                                let instantiated_body =
                                    super::super::apply_bindings(&body, &bindings).into_owned();

                                // Create new continuation for remaining values
                                let next_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessChainBody {
                                    remaining_values,
                                    var,
                                    body,
                                    results,
                                    env: current_env.clone(),
                                    depth,
                                    parent_cont,
                                });

                                // Evaluate body
                                work_stack.push(WorkItem::Eval {
                                    value: instantiated_body,
                                    env: current_env,
                                    depth, // TCO: reuse depth for iteration
                                    cont_id: next_cont_id,
                                    is_tail_call: false,
                                });
                            } else {
                                // No match, skip to next value
                                let next_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessChainBody {
                                    remaining_values,
                                    var,
                                    body,
                                    results,
                                    env: current_env.clone(),
                                    depth,
                                    parent_cont,
                                });

                                work_stack.push(WorkItem::Resume {
                                    cont_id: next_cont_id,
                                    result: (vec![], current_env),
                                });
                            }
                        }
                    }

                    // Handle function continuation - loop until return
                    Continuation::ProcessFunction {
                        iteration_count,
                        env: _func_env,
                        depth,
                        parent_cont,
                    } => {
                        const MAX_ITERATIONS: usize = 1000;

                        let (eval_results, current_env) = result;

                        if eval_results.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Nil()], current_env),
                            });
                            continue;
                        }

                        // Partition into return values and continue expressions
                        let (final_results, continue_exprs): (Vec<_>, Vec<_>) =
                            eval_results.into_iter().partition(|r| {
                                matches!(
                                    r.inner(),
                                    MettaValueInner::SExpr(items)
                                    if items.len() == 2
                                        && items[0] == MettaValue::Atom("return".to_string())
                                )
                            });

                        if !final_results.is_empty() {
                            // Extract return values
                            let returns: Vec<_> = final_results
                                .into_iter()
                                .map(|r| match r.inner() {
                                    MettaValueInner::SExpr(items) => items[1].clone(),
                                    _ => unreachable!("partition guarantees return expressions"),
                                })
                                .collect();
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (returns, current_env),
                            });
                            continue;
                        }

                        if continue_exprs.is_empty() {
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Nil()], current_env),
                            });
                            continue;
                        }

                        // Check for fixed point (expression didn't change)
                        // Note: We can't easily check this without storing previous expr
                        // For now, just continue iterating

                        if iteration_count >= MAX_ITERATIONS {
                            let err = MettaValue::Error(
                                format!(
                                    "function exceeded maximum iterations ({})",
                                    MAX_ITERATIONS
                                ),
                                continue_exprs[0].clone(),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], current_env),
                            });
                            continue;
                        }

                        // Continue with first expression
                        let next_expr = continue_exprs[0].clone();

                        let next_cont_id = continuations.len();
                        continuations.push(Continuation::ProcessFunction {
                            iteration_count: iteration_count + 1,
                            env: current_env.clone(),
                            depth,
                            parent_cont,
                        });

                        work_stack.push(WorkItem::Eval {
                            value: next_expr,
                            env: current_env,
                            depth, // TCO: reuse depth for iteration
                            cont_id: next_cont_id,
                            is_tail_call: false,
                        });
                    }

                    // Handle (is-error expr) continuation - check if result is error
                    Continuation::ProcessIsError {
                        env: _is_error_env,
                        depth: _,
                        parent_cont,
                    } => {
                        let (results, new_env) = result;
                        let is_err = results
                            .first()
                            .map_or(false, |r| matches!(r.inner(), MettaValueInner::Error(_, _)));
                        work_stack.push(WorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![MettaValue::Bool(is_err)], new_env),
                        });
                    }

                    // Handle (catch expr default) continuation - check for errors
                    Continuation::ProcessCatch {
                        default,
                        env: _catch_env,
                        depth,
                        parent_cont,
                    } => {
                        let (results, env_after_eval) = result;

                        // Partition results into errors and non-errors
                        let (_errors, non_errors): (Vec<_>, Vec<_>) = results
                            .into_iter()
                            .partition(|r| matches!(r.inner(), MettaValueInner::Error(_, _)));

                        if non_errors.is_empty() {
                            // All results were errors - evaluate default
                            // This is a TAIL CALL
                            work_stack.push(WorkItem::Eval {
                                value: default,
                                env: env_after_eval,
                                depth, // TCO: reuse depth
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            // Some non-error results - return only those
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (non_errors, env_after_eval),
                            });
                        }
                    }

                    // Handle conjunction goal evaluation - collect results and continue
                    Continuation::ProcessConjunction {
                        mut remaining_goals,
                        mut accumulated_results,
                        env: _conj_env,
                        depth,
                        parent_cont,
                    } => {
                        let (goal_results, env_after_goal) = result;

                        // Extend accumulated results with this goal's results
                        // (for intermediate goals, we thread results through)
                        // Filter out errors from propagation (errors stop the goal chain)
                        let mut next_results = Vec::new();
                        for r in goal_results {
                            if matches!(r.inner(), MettaValueInner::Error(_, _)) {
                                // Error stops this branch of conjunction
                                next_results.push(r);
                            } else {
                                next_results.push(r);
                            }
                        }

                        // If we're at the last goal, we're done - return all accumulated results
                        if remaining_goals.is_empty() {
                            // All goals completed - return the final results
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (next_results, env_after_goal),
                            });
                        } else {
                            // More goals to evaluate
                            // For conjunction semantics: evaluate next goal for each current result
                            let next_goal = remaining_goals.pop_front().expect("non-empty");

                            // Check if any results - if none or all errors, might fail early
                            if next_results.is_empty() {
                                // No results - conjunction failed
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![], env_after_goal),
                                });
                            } else {
                                // Create continuation for next goal
                                // Note: We move next_results directly into the continuation
                                // to avoid an unnecessary clone
                                let next_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessConjunction {
                                    remaining_goals,
                                    accumulated_results: next_results,
                                    env: env_after_goal.clone(),
                                    depth,
                                    parent_cont,
                                });

                                // Evaluate next goal
                                work_stack.push(WorkItem::Eval {
                                    value: next_goal,
                                    env: env_after_goal,
                                    depth, // TCO: reuse depth for iteration
                                    cont_id: next_cont_id,
                                    is_tail_call: false,
                                });
                            }
                        }
                    }

                    // Handle unify pattern1 evaluation - process results and queue body evals
                    Continuation::ProcessUnifyPattern1 {
                        pattern2,
                        success_body,
                        failure_body,
                        env: _unify_env,
                        depth,
                        parent_cont,
                    } => {
                        let (results1, env_after_p1) = result;

                        if results1.is_empty() {
                            // No pattern1 results - evaluate failure_body (tail call)
                            work_stack.push(WorkItem::Eval {
                                value: failure_body,
                                env: env_after_p1,
                                depth,
                                cont_id: parent_cont,
                                is_tail_call: true,
                            });
                        } else {
                            // Process pattern1 results - convert to VecDeque for iteration
                            let mut remaining = VecDeque::from(results1);
                            let first_val = remaining.pop_front().expect("non-empty");

                            // Check if first value is a Space
                            if let MettaValueInner::Space(ref handle) = first_val.inner() {
                                // Space unification - compute matches synchronously
                                let pattern = pattern2.clone();

                                // Check for boolean optimization
                                let is_boolean_check =
                                    match (success_body.inner(), failure_body.inner()) {
                                        (
                                            MettaValueInner::Bool(true),
                                            MettaValueInner::Bool(false),
                                        ) => true,
                                        (MettaValueInner::Atom(s), MettaValueInner::Atom(f))
                                            if s == "True" && f == "False" =>
                                        {
                                            true
                                        }
                                        _ => false,
                                    };

                                if is_boolean_check {
                                    // Boolean check - compute result synchronously
                                    let exists =
                                        if handle.is_module_space() || handle.name == "self" {
                                            env_after_p1.match_space_exists(&pattern)
                                        } else {
                                            let atoms = handle.collapse();
                                            atoms.iter().any(|atom| {
                                                pattern_match(&pattern, atom).is_some()
                                                    || pattern_match(atom, &pattern).is_some()
                                            })
                                        };

                                    let bool_result = MettaValue::Bool(exists);

                                    // Continue with remaining pattern1 results
                                    if remaining.is_empty() {
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![bool_result], env_after_p1),
                                        });
                                    } else {
                                        // More pattern1 results to process
                                        let next_cont_id = continuations.len();
                                        continuations.push(Continuation::ProcessUnifyBodies {
                                            remaining_bodies: VecDeque::new(),
                                            remaining_pattern1_results: remaining,
                                            pattern2,
                                            success_body,
                                            failure_body,
                                            all_results: vec![bool_result],
                                            env: env_after_p1.clone(),
                                            depth,
                                            parent_cont,
                                        });
                                        // Resume to process next pattern1 result
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: next_cont_id,
                                            result: (vec![], env_after_p1),
                                        });
                                    }
                                } else {
                                    // Non-boolean space match - collect bodies to evaluate
                                    // Get matches with multiplicity tracking for correct result counts
                                    let matches: Vec<MultiplicityMatch> =
                                        if handle.is_module_space() || handle.name == "self" {
                                            env_after_p1.match_space(&pattern, &pattern)
                                        } else {
                                            handle.collapse_with_multiplicity()
                                        };

                                    let mut bodies_to_eval: VecDeque<MettaValue> = VecDeque::new();
                                    let mut found_match = false;

                                    // Pattern match once per unique atom, expand by multiplicity
                                    // MettaValue clone is O(1) since it uses Arc internally
                                    for m in &matches {
                                        if let Some(bindings) = pattern_match(&pattern, &m.value) {
                                            found_match = true;
                                            // MettaValue clone is O(1) (just Arc reference count increment)
                                            let instantiated =
                                                apply_bindings(&success_body, &bindings)
                                                    .into_owned();
                                            // O(1) MettaValue clones for multiplicity expansion
                                            for _ in 0..m.count {
                                                bodies_to_eval.push_back(instantiated.clone());
                                            }
                                        } else if let Some(bindings) =
                                            pattern_match(&m.value, &pattern)
                                        {
                                            found_match = true;
                                            let instantiated =
                                                apply_bindings(&success_body, &bindings)
                                                    .into_owned();
                                            for _ in 0..m.count {
                                                bodies_to_eval.push_back(instantiated.clone());
                                            }
                                        }
                                    }

                                    if !found_match {
                                        bodies_to_eval.push_back(failure_body.clone());
                                    }

                                    // Start evaluating bodies
                                    if let Some(first_body) = bodies_to_eval.pop_front() {
                                        let bodies_cont_id = continuations.len();
                                        continuations.push(Continuation::ProcessUnifyBodies {
                                            remaining_bodies: bodies_to_eval,
                                            remaining_pattern1_results: remaining,
                                            pattern2,
                                            success_body,
                                            failure_body,
                                            all_results: Vec::new(),
                                            env: env_after_p1.clone(),
                                            depth,
                                            parent_cont,
                                        });

                                        work_stack.push(WorkItem::Eval {
                                            value: first_body,
                                            env: env_after_p1,
                                            depth: depth + 1,
                                            cont_id: bodies_cont_id,
                                            is_tail_call: false,
                                        });
                                    } else {
                                        // No bodies - continue with remaining pattern1 results
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![], env_after_p1),
                                        });
                                    }
                                }
                            } else {
                                // Non-space value - need to evaluate pattern2
                                let p2_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessUnifyPattern2 {
                                    val1: first_val,
                                    remaining_pattern1_results: remaining,
                                    pattern2: pattern2.clone(),
                                    success_body,
                                    failure_body,
                                    all_results: Vec::new(),
                                    env: env_after_p1.clone(),
                                    depth,
                                    parent_cont,
                                });

                                work_stack.push(WorkItem::Eval {
                                    value: pattern2,
                                    env: env_after_p1,
                                    depth: depth + 1,
                                    cont_id: p2_cont_id,
                                    is_tail_call: false,
                                });
                            }
                        }
                    }

                    // Handle unify pattern2 evaluation (for non-space case)
                    Continuation::ProcessUnifyPattern2 {
                        val1,
                        remaining_pattern1_results,
                        pattern2,
                        success_body,
                        failure_body,
                        mut all_results,
                        env: _p2_env,
                        depth,
                        parent_cont,
                    } => {
                        let (results2, env_after_p2) = result;

                        // Perform unification for each pattern2 result
                        // MettaValue clone is O(1) since it uses Arc internally
                        let mut bodies_to_eval: VecDeque<MettaValue> = VecDeque::new();

                        for val2 in results2 {
                            if let Some(bindings) = pattern_match(&val1, &val2) {
                                let instantiated =
                                    apply_bindings(&success_body, &bindings).into_owned();
                                bodies_to_eval.push_back(instantiated);
                            } else if let Some(bindings) = pattern_match(&val2, &val1) {
                                let instantiated =
                                    apply_bindings(&success_body, &bindings).into_owned();
                                bodies_to_eval.push_back(instantiated);
                            } else {
                                bodies_to_eval.push_back(failure_body.clone());
                            }
                        }

                        // Start evaluating bodies
                        if let Some(first_body) = bodies_to_eval.pop_front() {
                            let bodies_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessUnifyBodies {
                                remaining_bodies: bodies_to_eval,
                                remaining_pattern1_results,
                                pattern2: pattern2.clone(),
                                success_body,
                                failure_body,
                                all_results,
                                env: env_after_p2.clone(),
                                depth,
                                parent_cont,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: first_body,
                                env: env_after_p2,
                                depth: depth + 1,
                                cont_id: bodies_cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            // No bodies to evaluate - continue with remaining pattern1 results
                            if remaining_pattern1_results.is_empty() {
                                if all_results.is_empty() {
                                    // No results at all - evaluate failure body
                                    work_stack.push(WorkItem::Eval {
                                        value: failure_body,
                                        env: env_after_p2,
                                        depth,
                                        cont_id: parent_cont,
                                        is_tail_call: true,
                                    });
                                } else {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (all_results, env_after_p2),
                                    });
                                }
                            } else {
                                // More pattern1 results - create new ProcessUnifyPattern1
                                // to handle them (simplified - just process next one)
                                let mut remaining = remaining_pattern1_results;
                                let next_val = remaining.pop_front().expect("non-empty");

                                if let MettaValueInner::Space(_) = next_val.inner() {
                                    // Space - would need complex handling, for now just return
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (all_results, env_after_p2),
                                    });
                                } else {
                                    // Evaluate pattern2 again for next val1
                                    let p2_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessUnifyPattern2 {
                                        val1: next_val,
                                        remaining_pattern1_results: remaining,
                                        pattern2: pattern2.clone(),
                                        success_body,
                                        failure_body,
                                        all_results,
                                        env: env_after_p2.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    work_stack.push(WorkItem::Eval {
                                        value: pattern2,
                                        env: env_after_p2,
                                        depth, // TCO: reuse depth for iteration
                                        cont_id: p2_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                            }
                        }
                    }

                    // Handle unify body evaluations - accumulate and continue
                    Continuation::ProcessUnifyBodies {
                        mut remaining_bodies,
                        remaining_pattern1_results,
                        pattern2,
                        success_body,
                        failure_body,
                        mut all_results,
                        env: _bodies_env,
                        depth,
                        parent_cont,
                    } => {
                        let (body_results, env_after_body) = result;

                        // Accumulate results
                        all_results.extend(body_results);

                        // More bodies to evaluate?
                        if let Some(next_body) = remaining_bodies.pop_front() {
                            let next_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessUnifyBodies {
                                remaining_bodies,
                                remaining_pattern1_results,
                                pattern2,
                                success_body,
                                failure_body,
                                all_results,
                                env: env_after_body.clone(),
                                depth,
                                parent_cont,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: next_body,
                                env: env_after_body,
                                depth, // TCO: reuse depth for iteration
                                cont_id: next_cont_id,
                                is_tail_call: false,
                            });
                        } else if !remaining_pattern1_results.is_empty() {
                            // Process next pattern1 result
                            let mut remaining = remaining_pattern1_results;
                            let next_val = remaining.pop_front().expect("non-empty");

                            if let MettaValueInner::Space(ref handle) = next_val.inner() {
                                // Handle space - compute bodies synchronously
                                // Get matches with multiplicity tracking for correct result counts
                                let pattern = pattern2.clone();
                                let matches: Vec<MultiplicityMatch> =
                                    if handle.is_module_space() || handle.name == "self" {
                                        env_after_body.match_space(&pattern, &pattern)
                                    } else {
                                        handle.collapse_with_multiplicity()
                                    };

                                let mut new_bodies: VecDeque<MettaValue> = VecDeque::new();
                                let mut found_match = false;

                                // Pattern match once per unique atom, expand by multiplicity
                                // MettaValue clone is O(1) since it uses Arc internally
                                for m in &matches {
                                    if let Some(bindings) = pattern_match(&pattern, &m.value) {
                                        found_match = true;
                                        let instantiated =
                                            apply_bindings(&success_body, &bindings).into_owned();
                                        // O(1) MettaValue clones for multiplicity expansion
                                        for _ in 0..m.count {
                                            new_bodies.push_back(instantiated.clone());
                                        }
                                    } else if let Some(bindings) = pattern_match(&m.value, &pattern)
                                    {
                                        found_match = true;
                                        let instantiated =
                                            apply_bindings(&success_body, &bindings).into_owned();
                                        for _ in 0..m.count {
                                            new_bodies.push_back(instantiated.clone());
                                        }
                                    }
                                }

                                if !found_match {
                                    new_bodies.push_back(failure_body.clone());
                                }

                                if let Some(first_body) = new_bodies.pop_front() {
                                    let next_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessUnifyBodies {
                                        remaining_bodies: new_bodies,
                                        remaining_pattern1_results: remaining,
                                        pattern2,
                                        success_body,
                                        failure_body,
                                        all_results,
                                        env: env_after_body.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    work_stack.push(WorkItem::Eval {
                                        value: first_body,
                                        env: env_after_body,
                                        depth, // TCO: reuse depth for iteration
                                        cont_id: next_cont_id,
                                        is_tail_call: false,
                                    });
                                } else {
                                    // No bodies - continue with remaining
                                    let next_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessUnifyBodies {
                                        remaining_bodies: VecDeque::new(),
                                        remaining_pattern1_results: remaining,
                                        pattern2,
                                        success_body,
                                        failure_body,
                                        all_results,
                                        env: env_after_body.clone(),
                                        depth,
                                        parent_cont,
                                    });
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: next_cont_id,
                                        result: (vec![], env_after_body),
                                    });
                                }
                            } else {
                                // Non-space - evaluate pattern2
                                let p2_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessUnifyPattern2 {
                                    val1: next_val,
                                    remaining_pattern1_results: remaining,
                                    pattern2: pattern2.clone(),
                                    success_body,
                                    failure_body,
                                    all_results,
                                    env: env_after_body.clone(),
                                    depth,
                                    parent_cont,
                                });

                                work_stack.push(WorkItem::Eval {
                                    value: pattern2,
                                    env: env_after_body,
                                    depth, // TCO: reuse depth for iteration
                                    cont_id: p2_cont_id,
                                    is_tail_call: false,
                                });
                            }
                        } else {
                            // All done - return accumulated results
                            if all_results.is_empty() {
                                // No results - evaluate failure body
                                work_stack.push(WorkItem::Eval {
                                    value: failure_body,
                                    env: env_after_body,
                                    depth,
                                    cont_id: parent_cont,
                                    is_tail_call: true,
                                });
                            } else {
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (all_results, env_after_body),
                                });
                            }
                        }
                    }

                    // Process collapse - collect results into list
                    Continuation::ProcessCollapse {
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;

                        if results.is_empty() {
                            // Empty superposition returns Unit () (HE-compatible)
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Unit()], env_after),
                            });
                        } else {
                            // Filter out Empty sentinels and Nil values
                            let filtered: Vec<MettaValue> = results
                                .into_iter()
                                .filter(|v| {
                                    !matches!(
                                        v.inner(),
                                        MettaValueInner::Empty | MettaValueInner::Nil
                                    )
                                })
                                .collect();

                            if filtered.is_empty() {
                                // All results were Empty/Nil → return Unit
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![MettaValue::Unit()], env_after),
                                });
                            } else if filtered.len() == 1 {
                                // Check if single result is a space
                                if let MettaValueInner::Space(handle) = filtered[0].inner() {
                                    let atoms = handle.collapse();
                                    let result_val = if atoms.is_empty() {
                                        MettaValue::Unit()
                                    } else {
                                        MettaValue::SExpr(atoms)
                                    };
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![result_val], env_after),
                                    });
                                } else {
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![MettaValue::SExpr(filtered)], env_after),
                                    });
                                }
                            } else {
                                // Gather all results into a list
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![MettaValue::SExpr(filtered)], env_after),
                                });
                            }
                        }
                    }

                    // Process collapse-bind - collect ALL results into list (no filtering)
                    Continuation::ProcessCollapseBind {
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;
                        // Unlike collapse, do NOT filter - return all results as list
                        work_stack.push(WorkItem::Resume {
                            cont_id: parent_cont,
                            result: (vec![MettaValue::SExpr(results)], env_after),
                        });
                    }

                    // Process amb - collect results and evaluate next alternative
                    Continuation::ProcessAmb {
                        mut remaining_alts,
                        mut results,
                        env,
                        depth,
                        parent_cont,
                    } => {
                        let (alt_results, env_after) = result;
                        results.extend(alt_results);

                        if let Some(next_alt) = remaining_alts.pop_front() {
                            // More alternatives to evaluate
                            let amb_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessAmb {
                                remaining_alts,
                                results,
                                env: env_after.clone(),
                                depth,
                                parent_cont,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: next_alt,
                                env: env_after,
                                depth, // TCO: reuse depth for iteration
                                cont_id: amb_cont_id,
                                is_tail_call: false,
                            });
                        } else {
                            // All alternatives evaluated
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, env_after),
                            });
                        }
                    }

                    // Process guard condition result
                    Continuation::ProcessGuard {
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (cond_results, env_after) = result;

                        match cond_results.first().map(|v| v.inner()) {
                            Some(MettaValueInner::Bool(true)) => {
                                // Guard passes - return Unit
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![MettaValue::Unit()], env_after),
                                });
                            }
                            Some(MettaValueInner::Bool(false)) => {
                                // Guard fails - return empty (nondeterministic failure)
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![], env_after),
                                });
                            }
                            Some(MettaValueInner::Error(msg, details)) => {
                                // Error propagates
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (
                                        vec![MettaValue::Error(msg.clone(), details.clone())],
                                        env_after,
                                    ),
                                });
                            }
                            Some(_) => {
                                // Type error
                                let other = cond_results.first().unwrap();
                                let err = MettaValue::Error(
                                    format!(
                                        "guard: condition must evaluate to Bool, got {}",
                                        friendly_value_repr(other)
                                    ),
                                    other.clone(),
                                );
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![err], env_after),
                                });
                            }
                            None => {
                                // Empty - treat as guard failure
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![], env_after),
                                });
                            }
                        }
                    }

                    // Process get-atoms space reference result
                    Continuation::ProcessGetAtoms {
                        space_ref,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (space_results, env_after) = result;

                        if space_results.is_empty() {
                            let err = MettaValue::Error(
                                "get-atoms: space evaluated to empty".to_string(),
                                space_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match space_results[0].inner() {
                                MettaValueInner::Space(handle) => {
                                    let atoms = handle.collapse();
                                    if atoms.is_empty() {
                                        // Empty space returns empty results
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![], env_after),
                                        });
                                    } else {
                                        // Return all atoms as separate results (superposition)
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (atoms, env_after),
                                        });
                                    }
                                }
                                _ => {
                                    let other = &space_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "get-atoms: argument must be a space, got {}. Usage: (get-atoms space)",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process memo table reference result - phase 1
                    Continuation::ProcessMemoTable {
                        memo_ref,
                        expr,
                        first_only,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (memo_results, env_after) = result;

                        if memo_results.is_empty() {
                            let err = MettaValue::Error(
                                "memo: memo-table evaluated to empty".to_string(),
                                memo_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match memo_results[0].inner() {
                                MettaValueInner::Memo(handle) => {
                                    // Check cache first
                                    if let Some(cached) = handle.lookup(&expr) {
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (cached, env_after),
                                        });
                                    } else {
                                        // Not cached - evaluate expr via trampoline
                                        let memo_expr_cont_id = continuations.len();
                                        continuations.push(Continuation::ProcessMemoExpr {
                                            memo_handle: handle.clone(),
                                            expr: expr.clone(),
                                            first_only,
                                            env: env_after.clone(),
                                            depth,
                                            parent_cont,
                                        });

                                        work_stack.push(WorkItem::Eval {
                                            value: expr,
                                            env: env_after,
                                            depth: depth + 1,
                                            cont_id: memo_expr_cont_id,
                                            is_tail_call: false,
                                        });
                                    }
                                }
                                _ => {
                                    let other = &memo_results[0];
                                    let op_name = if first_only { "memo-first" } else { "memo" };
                                    let err = MettaValue::Error(
                                        format!(
                                            "{}: first argument must be a memo table, got {}. Usage: ({} memo-table expr)",
                                            op_name,
                                            friendly_value_repr(other),
                                            op_name
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process memo expression result - phase 2 (after cache miss)
                    Continuation::ProcessMemoExpr {
                        memo_handle,
                        expr,
                        first_only,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;

                        // Store in cache (only if non-empty)
                        if !results.is_empty() {
                            if first_only {
                                memo_handle.store(&expr, vec![results[0].clone()], true);
                            } else {
                                memo_handle.store(&expr, results.clone(), false);
                            }
                        }

                        work_stack.push(WorkItem::Resume {
                            cont_id: parent_cont,
                            result: (results, env_after),
                        });
                    }

                    // Process new-memo name result - phase 1
                    Continuation::ProcessNewMemoName {
                        name_arg,
                        size_arg,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (name_results, env_after) = result;

                        if name_results.is_empty() {
                            let err = MettaValue::Error(
                                "new-memo: name evaluated to empty".to_string(),
                                name_arg,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Extract string name
                            let name = match name_results[0].inner() {
                                MettaValueInner::String(s) => s.clone(),
                                MettaValueInner::Atom(s) => s.clone(),
                                _ => {
                                    let other = &name_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "new-memo: name must be a string or atom, got {}",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                    continue;
                                }
                            };

                            // Check for optional size argument
                            if let Some(size_expr) = size_arg {
                                let size_cont_id = continuations.len();
                                continuations.push(Continuation::ProcessNewMemoSize {
                                    name,
                                    size_arg: size_expr.clone(),
                                    env: env_after.clone(),
                                    depth,
                                    parent_cont,
                                });

                                work_stack.push(WorkItem::Eval {
                                    value: size_expr,
                                    env: env_after,
                                    depth: depth + 1,
                                    cont_id: size_cont_id,
                                    is_tail_call: false,
                                });
                            } else {
                                // No max-size - create unlimited cache
                                let memo = MemoHandle::new(name);
                                work_stack.push(WorkItem::Resume {
                                    cont_id: parent_cont,
                                    result: (vec![MettaValue::Memo(memo)], env_after),
                                });
                            }
                        }
                    }

                    // Process new-memo size result - phase 2
                    Continuation::ProcessNewMemoSize {
                        name,
                        size_arg,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (size_results, env_after) = result;

                        if size_results.is_empty() {
                            let err = MettaValue::Error(
                                "new-memo: max-size evaluated to empty".to_string(),
                                size_arg,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match size_results[0].inner() {
                                MettaValueInner::Long(n) if *n > 0 => {
                                    let memo = MemoHandle::with_max_size(name, *n as usize);
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![MettaValue::Memo(memo)], env_after),
                                    });
                                }
                                _ => {
                                    let other = &size_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "new-memo: max-size must be a positive integer, got {}",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process clear-memo! or memo-stats result
                    Continuation::ProcessMemoOp {
                        memo_ref,
                        is_clear,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (memo_results, env_after) = result;

                        if memo_results.is_empty() {
                            let op_name = if is_clear {
                                "clear-memo!"
                            } else {
                                "memo-stats"
                            };
                            let err = MettaValue::Error(
                                format!("{}: memo-table evaluated to empty", op_name),
                                memo_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match memo_results[0].inner() {
                                MettaValueInner::Memo(handle) => {
                                    if is_clear {
                                        handle.clear();
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (
                                                vec![MettaValue::Memo(handle.clone())],
                                                env_after,
                                            ),
                                        });
                                    } else {
                                        // memo-stats
                                        let (hits, misses, size, max_size) = handle.stats();
                                        let hit_rate = handle.hit_rate();
                                        let stats = MettaValue::SExpr(vec![
                                            MettaValue::Atom("stats".to_string()),
                                            MettaValue::Long(hits as i64),
                                            MettaValue::Long(misses as i64),
                                            MettaValue::Long(size as i64),
                                            MettaValue::Long(max_size as i64),
                                            MettaValue::Float(hit_rate),
                                        ]);
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![stats], env_after),
                                        });
                                    }
                                }
                                _ => {
                                    let other = &memo_results[0];
                                    let op_name = if is_clear {
                                        "clear-memo!"
                                    } else {
                                        "memo-stats"
                                    };
                                    let err = MettaValue::Error(
                                        format!(
                                            "{}: argument must be a memo table, got {}. Usage: ({} memo-table)",
                                            op_name,
                                            friendly_value_repr(other),
                                            op_name
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process match space evaluation result
                    Continuation::ProcessMatchSpace {
                        space_arg,
                        pattern,
                        template,
                        env,
                        depth,
                        parent_cont,
                    } => {
                        let (space_results, env_after) = result;

                        if space_results.is_empty() {
                            let err = MettaValue::Error(
                                "match: space evaluated to empty".to_string(),
                                space_arg,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match space_results[0].inner() {
                                MettaValueInner::Space(handle) => {
                                    // For module-backed spaces or the global "self" space,
                                    // use Environment's MORK-based matching directly (no eval needed)
                                    if handle.is_module_space() || handle.name == "self" {
                                        // Always use match_space for now - query_multi returns Some([])
                                        // instead of None when no matches found, preventing proper fallback.
                                        // The query_multi optimization can be investigated separately.
                                        let results: Vec<MettaValue> = env
                                            .match_space(&pattern, &template)
                                            .into_iter()
                                            .flat_map(|m| m.expand())
                                            .collect();
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (results, env_after),
                                        });
                                    } else {
                                        // Owned space - match against atoms stored in SpaceHandle
                                        let atoms = handle.collapse();

                                        // Collect all matching atoms and create instantiated templates
                                        let mut instantiated_templates: Vec<MettaValue> =
                                            Vec::new();
                                        for atom in &atoms {
                                            if let Some(bindings) = pattern_match(&pattern, atom) {
                                                let instantiated =
                                                    apply_bindings(&template, &bindings)
                                                        .into_owned();
                                                instantiated_templates.push(instantiated);
                                            }
                                        }

                                        if instantiated_templates.is_empty() {
                                            // No matches - return empty
                                            work_stack.push(WorkItem::Resume {
                                                cont_id: parent_cont,
                                                result: (vec![], env_after),
                                            });
                                        } else if instantiated_templates.len() == 1 {
                                            // Single match - evaluate directly (optimization)
                                            work_stack.push(WorkItem::Eval {
                                                value: instantiated_templates.pop().unwrap(),
                                                env: env_after,
                                                depth, // TCO: reuse depth
                                                cont_id: parent_cont,
                                                is_tail_call: true,
                                            });
                                        } else {
                                            // Multiple matches - queue template evaluations
                                            let mut templates_deque: VecDeque<MettaValue> =
                                                instantiated_templates.into_iter().collect();
                                            let first_template =
                                                templates_deque.pop_front().unwrap();

                                            // Create continuation to collect template results
                                            let templates_cont_id = continuations.len();
                                            continuations.push(
                                                Continuation::ProcessMatchTemplates {
                                                    remaining_templates: templates_deque,
                                                    results: vec![],
                                                    env: env_after.clone(),
                                                    depth,
                                                    parent_cont,
                                                },
                                            );

                                            // Fork environment for first evaluation
                                            // (isolation for nondeterministic branches)
                                            let forked_env = env_after.fork_for_nondeterminism();
                                            work_stack.push(WorkItem::Eval {
                                                value: first_template,
                                                env: forked_env,
                                                depth, // TCO: reuse depth
                                                cont_id: templates_cont_id,
                                                is_tail_call: true,
                                            });
                                        }
                                    }
                                }
                                _ => {
                                    let other = &space_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "match: first argument must be a space, got {}. Usage: (match space pattern template)",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process match template evaluations
                    Continuation::ProcessMatchTemplates {
                        mut remaining_templates,
                        mut results,
                        env,
                        depth,
                        parent_cont,
                    } => {
                        let (template_results, _env_after) = result;
                        results.extend(template_results);

                        if remaining_templates.is_empty() {
                            // All templates evaluated - return collected results
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, env),
                            });
                        } else {
                            // More templates to evaluate
                            let next_template = remaining_templates.pop_front().unwrap();

                            // Put continuation back (modified)
                            let templates_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessMatchTemplates {
                                remaining_templates,
                                results,
                                env: env.clone(),
                                depth,
                                parent_cont,
                            });

                            // Fork environment for this evaluation (isolation)
                            let forked_env = env.fork_for_nondeterminism();
                            work_stack.push(WorkItem::Eval {
                                value: next_template,
                                env: forked_env,
                                depth, // TCO: reuse depth
                                cont_id: templates_cont_id,
                                is_tail_call: true,
                            });
                        }
                    }

                    // Process add-atom space evaluation result (phase 1)
                    Continuation::ProcessAddAtomSpace {
                        space_ref,
                        atom,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (space_results, env_after) = result;

                        if space_results.is_empty() {
                            let err = MettaValue::Error(
                                "add-atom: space evaluated to empty".to_string(),
                                space_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match space_results[0].inner() {
                                MettaValueInner::Space(handle) => {
                                    // Space evaluated - now evaluate the atom
                                    let add_atom_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessAddAtomAtom {
                                        space_handle: handle.clone(),
                                        atom: atom.clone(),
                                        env: env_after.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    work_stack.push(WorkItem::Eval {
                                        value: atom,
                                        env: env_after,
                                        depth: depth + 1,
                                        cont_id: add_atom_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                                _ => {
                                    let other = &space_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "add-atom: first argument must be a space reference, got {}. Usage: (add-atom space atom)",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process add-atom atom evaluation result (phase 2)
                    Continuation::ProcessAddAtomAtom {
                        space_handle,
                        atom,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (atom_results, env_after) = result;

                        if atom_results.is_empty() {
                            let err = MettaValue::Error(
                                "add-atom: atom evaluated to empty".to_string(),
                                atom,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Add the atom to the space
                            space_handle.add_atom(atom_results[0].clone());
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Unit()], env_after),
                            });
                        }
                    }

                    // Process remove-atom space evaluation result (phase 1)
                    Continuation::ProcessRemoveAtomSpace {
                        space_ref,
                        atom,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (space_results, env_after) = result;

                        if space_results.is_empty() {
                            let err = MettaValue::Error(
                                "remove-atom: space evaluated to empty".to_string(),
                                space_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match space_results[0].inner() {
                                MettaValueInner::Space(handle) => {
                                    // Space evaluated - now evaluate the atom
                                    let remove_atom_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessRemoveAtomAtom {
                                        space_handle: handle.clone(),
                                        atom: atom.clone(),
                                        env: env_after.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    work_stack.push(WorkItem::Eval {
                                        value: atom,
                                        env: env_after,
                                        depth: depth + 1,
                                        cont_id: remove_atom_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                                _ => {
                                    let other = &space_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "remove-atom: first argument must be a space reference, got {}. Usage: (remove-atom space atom)",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process remove-atom atom evaluation result (phase 2)
                    Continuation::ProcessRemoveAtomAtom {
                        space_handle,
                        atom,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (atom_results, env_after) = result;

                        if atom_results.is_empty() {
                            let err = MettaValue::Error(
                                "remove-atom: atom evaluated to empty".to_string(),
                                atom,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Remove the atom from the space
                            space_handle.remove_atom(&atom_results[0]);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Unit()], env_after),
                            });
                        }
                    }

                    // Process new-state initial value result
                    Continuation::ProcessNewState {
                        initial_value,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (value_results, mut env_after) = result;

                        if value_results.is_empty() {
                            let err = MettaValue::Error(
                                "new-state: initial value evaluated to empty".to_string(),
                                initial_value,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            let value = value_results[0].clone();
                            let state_id = env_after.create_state(value);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::State(state_id)], env_after),
                            });
                        }
                    }

                    // Process get-state state reference result
                    Continuation::ProcessGetState {
                        state_ref,
                        env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (state_results, env_after) = result;

                        if state_results.is_empty() {
                            let err = MettaValue::Error(
                                "get-state: state evaluated to empty".to_string(),
                                state_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            match state_results[0].inner() {
                                MettaValueInner::State(state_id) => {
                                    if let Some(value) = env.get_state(*state_id) {
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![value], env_after),
                                        });
                                    } else {
                                        let err = MettaValue::Error(
                                            format!("get-state: state {} not found", state_id),
                                            state_results[0].clone(),
                                        );
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![err], env_after),
                                        });
                                    }
                                }
                                _ => {
                                    let other = &state_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "get-state: argument must be a state reference, got {}. Usage: (get-state state)",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process change-state! state reference result (phase 1)
                    Continuation::ProcessChangeStateRef {
                        state_ref,
                        new_value,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (state_results, env_after) = result;

                        if state_results.is_empty() {
                            let err = MettaValue::Error(
                                "change-state!: state evaluated to empty".to_string(),
                                state_ref,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Now evaluate the new value
                            let change_value_cont_id = continuations.len();
                            continuations.push(Continuation::ProcessChangeStateValue {
                                state_value: state_results[0].clone(),
                                new_value: new_value.clone(),
                                env: env_after.clone(),
                                depth,
                                parent_cont,
                            });

                            work_stack.push(WorkItem::Eval {
                                value: new_value,
                                env: env_after,
                                depth: depth + 1,
                                cont_id: change_value_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Process change-state! new value result (phase 2)
                    Continuation::ProcessChangeStateValue {
                        state_value,
                        new_value,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (value_results, mut env_after) = result;

                        if value_results.is_empty() {
                            let err = MettaValue::Error(
                                "change-state!: new value evaluated to empty".to_string(),
                                new_value,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            let value = value_results[0].clone();

                            match state_value.inner() {
                                MettaValueInner::State(state_id) => {
                                    if env_after.change_state(*state_id, value) {
                                        // Return the state reference for chaining
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![state_value], env_after),
                                        });
                                    } else {
                                        let err = MettaValue::Error(
                                            format!("change-state!: state {} not found", state_id),
                                            state_value,
                                        );
                                        work_stack.push(WorkItem::Resume {
                                            cont_id: parent_cont,
                                            result: (vec![err], env_after),
                                        });
                                    }
                                }
                                _ => {
                                    let err = MettaValue::Error(
                                        format!(
                                            "change-state!: first argument must be a state reference, got {}. Usage: (change-state! state new-value)",
                                            friendly_value_repr(&state_value)
                                        ),
                                        state_value,
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process repr atom result
                    Continuation::ProcessRepr {
                        atom,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;

                        if results.is_empty() {
                            let err = MettaValue::Error(
                                "repr: argument evaluated to empty".to_string(),
                                atom,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Convert the first result to its string representation
                            let value = &results[0];
                            let repr = atom_repr(value);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::String(repr)], env_after),
                            });
                        }
                    }

                    // Process format-args format string result (phase 1)
                    Continuation::ProcessFormatArgsString {
                        format_arg,
                        args_arg,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (format_results, env_after) = result;

                        if format_results.is_empty() {
                            let err = MettaValue::Error(
                                "format-args: format string evaluated to empty".to_string(),
                                format_arg,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Get the format string
                            match format_results[0].inner() {
                                MettaValueInner::String(s) => {
                                    // Create continuation for args evaluation
                                    let args_cont_id = continuations.len();
                                    continuations.push(Continuation::ProcessFormatArgsArgs {
                                        format_str: s.clone(),
                                        args_arg: args_arg.clone(),
                                        env: env_after.clone(),
                                        depth,
                                        parent_cont,
                                    });

                                    // Evaluate args expression
                                    work_stack.push(WorkItem::Eval {
                                        value: args_arg,
                                        env: env_after,
                                        depth: depth + 1,
                                        cont_id: args_cont_id,
                                        is_tail_call: false,
                                    });
                                }
                                _ => {
                                    let other = &format_results[0];
                                    let err = MettaValue::Error(
                                        format!(
                                            "format-args: first argument must be a string, got {}",
                                            friendly_value_repr(other)
                                        ),
                                        other.clone(),
                                    );
                                    work_stack.push(WorkItem::Resume {
                                        cont_id: parent_cont,
                                        result: (vec![err], env_after),
                                    });
                                }
                            }
                        }
                    }

                    // Process format-args args result (phase 2)
                    Continuation::ProcessFormatArgsArgs {
                        format_str,
                        args_arg,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (args_results, env_after) = result;

                        if args_results.is_empty() {
                            let err = MettaValue::Error(
                                "format-args: args evaluated to empty".to_string(),
                                args_arg,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Get the args as a list of values
                            let args: Vec<&MettaValue> = match args_results[0].inner() {
                                MettaValueInner::SExpr(items) => items.iter().collect(),
                                _ => vec![&args_results[0]],
                            };

                            // Perform the formatting
                            let formatted = format_string(&format_str, &args);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::String(formatted)], env_after),
                            });
                        }
                    }

                    // Process println! atom result
                    Continuation::ProcessPrintln {
                        atom,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;

                        if results.is_empty() {
                            let err = MettaValue::Error(
                                "println!: argument evaluated to empty".to_string(),
                                atom,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Print the first result to stdout
                            let value = &results[0];
                            println!("{}", atom_to_string(value));

                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Unit()], env_after),
                            });
                        }
                    }

                    // Process trace! message result (phase 1)
                    Continuation::ProcessTraceMessage {
                        message,
                        value_expr,
                        env: _env,
                        depth,
                        parent_cont,
                    } => {
                        let (msg_results, env_after) = result;

                        if msg_results.is_empty() {
                            let err = MettaValue::Error(
                                "trace!: message evaluated to empty".to_string(),
                                message,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Create continuation for value evaluation
                            let value_cont_id = continuations.len();
                            let message_str = atom_to_string(&msg_results[0]);
                            continuations.push(Continuation::ProcessTraceValue {
                                message_str,
                                value_expr: value_expr.clone(),
                                env: env_after.clone(),
                                depth,
                                parent_cont,
                            });

                            // Evaluate value expression
                            work_stack.push(WorkItem::Eval {
                                value: value_expr,
                                env: env_after,
                                depth: depth + 1,
                                cont_id: value_cont_id,
                                is_tail_call: false,
                            });
                        }
                    }

                    // Process trace! value result (phase 2)
                    Continuation::ProcessTraceValue {
                        message_str,
                        value_expr,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (value_results, env_after) = result;

                        if value_results.is_empty() {
                            let err = MettaValue::Error(
                                "trace!: value evaluated to empty".to_string(),
                                value_expr,
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else {
                            // Print message to stderr
                            eprintln!("{}", message_str);

                            // Return the value (first result)
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![value_results[0].clone()], env_after),
                            });
                        }
                    }

                    // Process get-metatype atom result
                    Continuation::ProcessGetMetatype {
                        atom: _atom,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, env_after) = result;

                        if results.is_empty() {
                            // If evaluation returns empty, that's valid - return empty
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![], env_after),
                            });
                        } else {
                            // Get the meta-type of the first result
                            let value = &results[0];
                            let meta_type = get_metatype_util(value);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Atom(meta_type.to_string())], env_after),
                            });
                        }
                    }

                    // Process bind! atom expression result
                    Continuation::ProcessBind {
                        token,
                        env: _env,
                        depth: _depth,
                        parent_cont,
                    } => {
                        let (results, mut env_after) = result;

                        if results.is_empty() {
                            // Atom evaluated to empty - return error
                            let err = MettaValue::Error(
                                "bind!: atom evaluated to empty".to_string(),
                                MettaValue::Atom(token),
                            );
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![err], env_after),
                            });
                        } else if let MettaValueInner::Error(_, _) = results[0].inner() {
                            // Error in evaluation - propagate it
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (results, env_after),
                            });
                        } else {
                            // Register the token in the tokenizer
                            let atom = results[0].clone();
                            env_after.register_token(&token, atom);
                            work_stack.push(WorkItem::Resume {
                                cont_id: parent_cont,
                                result: (vec![MettaValue::Unit()], env_after),
                            });
                        }
                    }
                }
            }
        }
    }

    final_result.unwrap_or_else(|| (vec![], env))
}

/// Convert a MettaValue to its repr string (MeTTa representation)
fn atom_repr(value: &MettaValue) -> String {
    match value.inner() {
        MettaValueInner::Long(n) => n.to_string(),
        MettaValueInner::Float(f) => f.to_string(),
        MettaValueInner::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValueInner::String(s) => format!("\"{}\"", s), // Include quotes for string repr
        MettaValueInner::Atom(a) => a.clone(),
        MettaValueInner::Nil => "Nil".to_string(),
        MettaValueInner::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_repr).collect();
            format!("({})", inner.join(" "))
        }
        MettaValueInner::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValueInner::Type(t) => format!("(: {})", atom_repr(t)),
        MettaValueInner::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_repr).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValueInner::State(id) => format!("(State {})", id),
        MettaValueInner::Unit => "()".to_string(),
        MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValueInner::Empty => "Empty".to_string(),
    }
}

/// Convert a MettaValue to a string for formatting (without quotes)
fn atom_to_string(value: &MettaValue) -> String {
    match value.inner() {
        MettaValueInner::Long(n) => n.to_string(),
        MettaValueInner::Float(f) => f.to_string(),
        MettaValueInner::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        MettaValueInner::String(s) => s.clone(), // No quotes for formatting
        MettaValueInner::Atom(a) => a.clone(),
        MettaValueInner::Nil => "Nil".to_string(),
        MettaValueInner::SExpr(items) => {
            let inner: Vec<String> = items.iter().map(atom_to_string).collect();
            format!("({})", inner.join(" "))
        }
        MettaValueInner::Error(msg, _) => format!("(Error \"{}\")", msg),
        MettaValueInner::Type(t) => format!("(: {})", atom_to_string(t)),
        MettaValueInner::Conjunction(goals) => {
            let inner: Vec<String> = goals.iter().map(atom_to_string).collect();
            format!("(, {})", inner.join(" "))
        }
        MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
        MettaValueInner::State(id) => format!("(State {})", id),
        MettaValueInner::Unit => "()".to_string(),
        MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
        MettaValueInner::Empty => "Empty".to_string(),
    }
}

/// Format a string by replacing {} placeholders with argument values
fn format_string(format_str: &str, args: &[&MettaValue]) -> String {
    let mut result = String::with_capacity(format_str.len() * 2);
    let mut chars = format_str.chars().peekable();
    let mut arg_index = 0;

    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'}') {
                chars.next(); // consume '}'
                if arg_index < args.len() {
                    // Use atom_to_string (without quotes for strings)
                    result.push_str(&atom_to_string(args[arg_index]));
                    arg_index += 1;
                } else {
                    // Not enough arguments, keep the placeholder
                    result.push_str("{}");
                }
            } else if chars.peek() == Some(&'{') {
                // Escaped {{ -> {
                chars.next();
                result.push('{');
            } else {
                result.push(c);
            }
        } else if c == '}' && chars.peek() == Some(&'}') {
            // Escaped }} -> }
            chars.next();
            result.push('}');
        } else {
            result.push(c);
        }
    }

    result
}

/// Get the meta-type of a MettaValue
fn get_metatype_util(value: &MettaValue) -> &'static str {
    match value.inner() {
        // Atoms (symbols) are the basic named entities
        MettaValueInner::Atom(s) => {
            if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                "Variable"
            } else {
                "Symbol"
            }
        }
        // S-expressions are compound expressions
        MettaValueInner::SExpr(_) => "Expression",
        // All grounded values (numbers, strings, bools, etc.)
        MettaValueInner::Long(_)
        | MettaValueInner::Float(_)
        | MettaValueInner::Bool(_)
        | MettaValueInner::String(_) => "Grounded",
        // Special types
        MettaValueInner::Nil => "Symbol",
        MettaValueInner::Unit => "Expression", // () is an empty expression
        MettaValueInner::Type(_) => "Expression",
        MettaValueInner::Conjunction(_) => "Expression",
        MettaValueInner::Space(_) => "Grounded",
        MettaValueInner::State(_) => "Grounded",
        MettaValueInner::Error(_, _) => "Expression",
        MettaValueInner::Memo(_) => "Grounded",
        MettaValueInner::Empty => "Symbol", // Empty is treated as a symbol for meta-type purposes
    }
}
